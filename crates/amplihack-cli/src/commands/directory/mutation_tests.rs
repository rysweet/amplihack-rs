use super::*;
use crate::{
    ConfigCommands,
    commands::config::run_config,
    test_support,
    user_config::{self, edit},
};
use std::{collections::BTreeMap, fs, path::PathBuf};

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn fixture(home: &Path) -> PathBuf {
    let staged = home.join(".amplihack/.claude");
    for name in ["pdf", "extra", "tla-plus-expert"] {
        write(
            &staged,
            &format!("skills/{name}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: example\n---\n{name}\n"),
        );
    }
    for name in [
        "core/builder",
        "guide",
        "core/guide",
        "specialized/tla-plus-expert",
    ] {
        write(
            &staged,
            &format!("agents/{name}.md"),
            &format!(
                "---\nname: {}\ndescription: example\n---\n{name}\n",
                name.rsplit('/').next().unwrap()
            ),
        );
    }
    index::write_index(&staged).unwrap();
    edit::write_atomic(
        &home.join(".amplihack/config.toml"),
        b"# keep\n[recipe]\nverbose = true\n",
    )
    .unwrap();
    crate::commands::install::version_stamp::write_installed_version(crate::VERSION).unwrap();
    staged
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !dir.exists() {
            return;
        }
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn change(name: &str, enable: bool) -> Result<String> {
    run_for_test(if enable {
        DirectoryCommands::Enable { name: name.into() }
    } else {
        DirectoryCommands::Disable { name: name.into() }
    })
}

fn set(key: &str, value: &str) {
    run_config(ConfigCommands::Set {
        key: key.into(),
        value: value.into(),
        project: false,
    })
    .unwrap();
}

#[test]
fn commands_publish_extensions_immediately_and_restore_values_and_tool_bytes() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home = test_support::HomeGuard::set(home.path());
    let staged = fixture(home.path());
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    write(
        home.path(),
        ".copilot/config.json",
        "{\"plugins\":[{\"name\":\"amplihack\"}]}\n",
    );
    crate::copilot_setup::ensure_copilot_home_staged_with(
        None,
        crate::copilot_setup::UserConfig::KeepExisting,
    )
    .unwrap();
    let config = home.path().join(".amplihack/config.toml");
    let original_config = user_config::load_user_only();
    let original_index = fs::read(index::index_path(&staged)).unwrap();
    let claude = home.path().join(".claude/skills");
    let copilot = home.path().join(".copilot");
    let original_claude = snapshot(&claude);
    let original_copilot = snapshot(&copilot);
    for name in [
        "amplihack:extra",
        "amplihack:core:guide",
        "amplihack:amplihack:extra",
        "amplihack:amplihack:core:guide",
    ] {
        change(name, true).unwrap();
        let first_config = fs::read(&config).unwrap();
        let first_claude = snapshot(&claude);
        let first_copilot = snapshot(&copilot);
        change(name, true).unwrap();
        assert_eq!(fs::read(&config).unwrap(), first_config);
        assert_eq!(snapshot(&claude), first_claude);
        assert_eq!(snapshot(&copilot), first_copilot);
        if name.ends_with("extra") {
            assert!(claude.join("extra/SKILL.md").is_file());
            assert!(copilot.join("skills/extra/SKILL.md").is_file());
        } else {
            assert!(claude.join("amplihack/agents/core/guide.md").is_file());
            assert_eq!(
                fs::read(copilot.join("agents/amplihack/guide.md")).unwrap(),
                fs::read(staged.join("agents/core/guide.md")).unwrap()
            );
        }
        change(name, false).unwrap();
        change(name, false).unwrap();
        let restored = user_config::load_user_only();
        for key in ["lists.enabled", "lists.set", "recipe.verbose"] {
            assert_eq!(restored.entry(key).value, original_config.entry(key).value);
        }
        assert_eq!(snapshot(&claude), original_claude);
        assert_eq!(snapshot(&copilot), original_copilot);
    }
    assert_eq!(
        fs::read(index::index_path(&staged)).unwrap(),
        original_index
    );
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn disabling_the_last_extension_removes_empty_configuration_entries() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for original in [
        "",
        "[lists]\nenabled = []\n",
        "lists = { enabled = [] }\n",
        "[lists]\nset = 'all'\n",
        "[lists]\nenabled = ['agent:core/guide']\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        let _home = test_support::HomeGuard::set(home.path());
        fixture(home.path());
        let config = home.path().join(".amplihack/config.toml");
        fs::write(&config, original).unwrap();
        let before = user_config::load_user_only();
        change("skill:extra", true).unwrap();
        change("skill:extra", false).unwrap();
        let restored = user_config::load_user_only();
        for key in ["lists.enabled", "lists.set"] {
            assert_eq!(restored.entry(key).value, before.entry(key).value);
        }
        let doc = fs::read_to_string(&config)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        if original.contains("agent:core/guide") {
            assert!(doc["lists"]["enabled"].is_array());
        } else if original.contains("set =") {
            assert_eq!(doc["lists"]["set"].as_str(), Some("all"));
            assert!(doc["lists"].get("enabled").is_none());
        } else {
            assert!(doc.get("lists").is_none());
        }
    }
}

#[test]
fn configuration_commands_restore_full_lists_and_keep_all_mode_extensions() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home = test_support::HomeGuard::set(home.path());
    fixture(home.path());
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    let skills = home.path().join(".claude/skills");
    set("lists.set", "all");
    assert!(skills.join("extra/SKILL.md").is_file());
    change("extra", true).unwrap();
    set("lists.set", "default");
    assert!(skills.join("extra/SKILL.md").is_file());
    change("extra", false).unwrap();
    assert!(!skills.join("extra").exists());
    change("extra", true).unwrap();
    assert!(skills.join("extra/SKILL.md").is_file());
    run_config(ConfigCommands::Unset {
        key: "lists.enabled".into(),
        project: false,
    })
    .unwrap();
    assert!(skills.join("extra/SKILL.md").is_file());
    // Setting the same value applies deferred configuration changes too.
    set("lists.set", "default");
    assert!(!skills.join("extra").exists());
    assert!(!home.path().join(".copilot").exists());
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn rejected_names_change_neither_configuration_nor_index() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home = test_support::HomeGuard::set(home.path());
    let staged = fixture(home.path());
    let before = snapshot(home.path());
    for name in ["tla-plus-expert", "guide", "unknown", "amplihack:"] {
        assert!(change(name, true).is_err());
        assert!(change(name, false).is_err());
        assert_eq!(snapshot(home.path()), before);
    }
    assert!(change("skill:pdf", false).is_err());
    change("skill:pdf", true).unwrap();
    change("agent:core/builder", true).unwrap();
    change("amplihack:amplihack:core:builder", true).unwrap();
    change("skill:extra", false).unwrap();
    assert_eq!(snapshot(home.path()), before);
    assert!(index::index_path(&staged).exists());
}

#[test]
fn repeating_enable_publishes_a_previously_skipped_skill_when_available() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home = test_support::HomeGuard::set(home.path());
    fixture(home.path());
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    write(
        home.path(),
        ".claude/skills/extra/SKILL.md",
        "independent skill",
    );
    change("extra", true).unwrap();
    assert!(crate::user_config::list_selection().includes_skill("extra"));
    assert_eq!(
        fs::read(home.path().join(".claude/skills/extra/SKILL.md")).unwrap(),
        b"independent skill"
    );
    fs::rename(
        home.path().join(".claude/skills/extra"),
        home.path().join("saved-extra"),
    )
    .unwrap();
    change("extra", true).unwrap();
    assert_ne!(
        fs::read(home.path().join(".claude/skills/extra/SKILL.md")).unwrap(),
        b"independent skill"
    );
    assert_eq!(
        fs::read(home.path().join("saved-extra/SKILL.md")).unwrap(),
        b"independent skill"
    );
}

#[test]
fn stale_installations_reject_list_changes_without_mutation() {
    let _lock = test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let _home = test_support::HomeGuard::set(home.path());
    let staged = fixture(home.path());
    write(
        &staged,
        "skills/code-visualizer/SKILL.md",
        "---\nname: code-visualizer\ndescription: old definition\n---\nold body\n",
    );
    index::write_index(&staged).unwrap();
    crate::claude_plugin::ensure_claude_plugin_installed().unwrap();
    for version in ["0.0.1", "9999.0.0"] {
        crate::commands::install::version_stamp::write_installed_version(version).unwrap();
        let before = snapshot(home.path());
        run_for_test(DirectoryCommands::Find {
            words: vec!["code-visualizer".into()],
        })
        .unwrap();
        for enable in [true, false] {
            let error = change("skill:code-visualizer", enable).unwrap_err();
            assert!(error.to_string().contains("amplihack install"));
            assert_eq!(snapshot(home.path()), before);
        }
        assert!(
            run_config(ConfigCommands::Set {
                key: "lists.set".into(),
                value: "all".into(),
                project: false,
            })
            .is_err()
        );
        assert_eq!(snapshot(home.path()), before);
    }
}
