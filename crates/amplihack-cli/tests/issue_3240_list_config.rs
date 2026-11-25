//! Public configuration writes validate selection before changing user bytes.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;

struct Install {
    home: tempfile::TempDir,
}

impl Install {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join(".amplihack")).unwrap();
        let install = Self { home };
        fs::write(install.config(), "# keep\n[recipe]\nverbose = true\n").unwrap();
        fs::set_permissions(install.config(), fs::Permissions::from_mode(0o600)).unwrap();
        install
    }

    fn config(&self) -> PathBuf {
        self.home.path().join(".amplihack/config.toml")
    }

    fn index(&self) -> PathBuf {
        self.home
            .path()
            .join(".amplihack/.claude/install/skills-and-agents-index.tsv")
    }

    fn write_index(&self) {
        let staged = self.home.path().join(".amplihack/.claude");
        fs::create_dir_all(self.index().parent().unwrap()).unwrap();
        fs::write(self.index(), format!(
            "skill\tcode-visualizer\t{}/skills/code-visualizer/SKILL.md\tDraw code\nagent\tguide\t{}/agents/core/guide.md\tGuide\n",
            staged.display(), staged.display()
        )).unwrap();
        fs::write(
            self.home.path().join(".amplihack/.installed-version"),
            amplihack_cli::VERSION,
        )
        .unwrap();
    }

    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut command =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        command
            .args(args)
            .current_dir(self.home.path())
            .env("HOME", self.home.path())
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn register_claude(&self) {
        self.write_index();
        let staged = self.home.path().join(".amplihack/.claude");
        for name in std::iter::once("pdf".to_owned())
            .chain(std::iter::once("code-visualizer".to_owned()))
            .chain((0..10).map(|number| format!("extra-{number}")))
        {
            let directory = staged.join("skills").join(&name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("SKILL.md"), format!("{name}\n")).unwrap();
        }
        fs::create_dir_all(staged.join("agents")).unwrap();
        fs::write(
            staged.join("install/claude-skills-manifest.json"),
            r#"{"version":2,"skills":[]}"#,
        )
        .unwrap();
        let result = self.run(&["config", "set", "lists.set", "all"]);
        assert!(result.status.success(), "{result:?}");
    }
}

#[test]
fn edited_skills_are_named_and_preserved_without_failing_list_changes() {
    let install = Install::new();
    install.register_claude();
    let skills = install.home.path().join(".claude/skills");
    let edited: Vec<_> = std::iter::once("code-visualizer".to_owned())
        .chain((0..9).map(|number| format!("extra-{number}")))
        .collect();
    for name in &edited {
        fs::write(skills.join(name).join("SKILL.md"), "personal changes\n").unwrap();
    }
    for args in [
        vec!["directory", "enable", "skill:code-visualizer"],
        vec!["config", "set", "lists.set", "default"],
        vec!["directory", "disable", "skill:code-visualizer"],
        vec!["config", "set", "lists.set", "all"],
    ] {
        let result = install.run(&args);
        assert!(result.status.success(), "{args:?}: {result:?}");
        let warning = String::from_utf8_lossy(&result.stderr);
        for name in &edited {
            // Once narrowing drops an edited skill's ownership record, later
            // publication only reports it when that skill is selected again.
            if args[1] != "disable" || name == "code-visualizer" {
                assert!(
                    warning.contains(name),
                    "{args:?}: missing {name}: {warning}"
                );
            }
            assert_eq!(
                fs::read(skills.join(name).join("SKILL.md")).unwrap(),
                b"personal changes\n"
            );
        }
        assert_eq!(fs::read(skills.join("pdf/SKILL.md")).unwrap(), b"pdf\n");
        if args[1] == "enable" || args.last() == Some(&"all") {
            assert_eq!(
                fs::read(skills.join("extra-9/SKILL.md")).unwrap(),
                b"extra-9\n"
            );
        } else {
            assert!(!skills.join("extra-9").exists());
        }
    }
}

#[test]
fn wrapper_and_returned_publication_errors_still_fail_list_changes() {
    let install = Install::new();
    install.register_claude();
    let wrapper = install
        .home
        .path()
        .join(".claude/skills/amplihack/personal.md");
    fs::write(&wrapper, "personal wrapper changes\n").unwrap();
    let result = install.run(&["config", "set", "lists.set", "default"]);
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(fs::read(&wrapper).unwrap(), b"personal wrapper changes\n");
    let manifest = install
        .home
        .path()
        .join(".amplihack/.claude/install/claude-skills-manifest.json");
    fs::write(&manifest, "invalid manifest\n").unwrap();
    let result = install.run(&["directory", "enable", "skill:code-visualizer"]);
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(fs::read(&manifest).unwrap(), b"invalid manifest\n");
    assert_eq!(fs::read(&wrapper).unwrap(), b"personal wrapper changes\n");
}

#[test]
fn list_settings_report_defaults_and_read_typed_extensions() {
    let install = Install::new();
    let default = install.run(&["config", "get", "lists.set"]);
    assert!(default.status.success(), "{default:?}");
    assert_eq!(default.stdout, b"default\n");
    let enabled = install.run(&["config", "get", "lists.enabled"]);
    assert!(enabled.status.success());
    assert_eq!(enabled.stdout, b"\n");
    install.write_index();
    let index = fs::read(install.index()).unwrap();
    fs::write(
        install.config(),
        "[lists]\nenabled = ['skill:code-visualizer', 'agent:core/guide']\n",
    )
    .unwrap();
    let result = install.run(&["config", "get", "lists.enabled"]);
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("agent:core/guide"));
    assert!(output.contains("skill:code-visualizer"));
    assert_eq!(fs::read(install.index()).unwrap(), index);
}

#[test]
fn invalid_or_unknown_entries_leave_configuration_unchanged() {
    let install = Install::new();
    install.write_index();
    let original = fs::read(install.config()).unwrap();
    for raw in [
        r#"["skill:code-visualizer"]"#,
        "[]",
        r#"["skill:no-such-entry"]"#,
        r#"["agent:guide"]"#,
        r#"["guide"]"#,
        "[1]",
        "true",
    ] {
        let result = install.run(&["config", "set", "lists.enabled", raw]);
        assert!(!result.status.success(), "{raw}: {result:?}");
        assert_eq!(fs::read(install.config()).unwrap(), original);
    }
    let result = install.run(&["config", "set", "lists.set", "everything"]);
    assert!(!result.status.success());
    assert_eq!(fs::read(install.config()).unwrap(), original);
}

#[test]
fn missing_index_rejects_extensions_without_changing_configuration() {
    let install = Install::new();
    let original = fs::read(install.config()).unwrap();
    let result = install.run(&[
        "config",
        "set",
        "lists.enabled",
        r#"["skill:code-visualizer"]"#,
    ]);
    assert!(!result.status.success());
    assert_eq!(fs::read(install.config()).unwrap(), original);
    assert!(!install.index().exists());
}

#[test]
fn all_mode_can_be_saved_before_installation_without_an_index() {
    let install = Install::new();
    let result = install.run(&["config", "set", "lists.set", "all"]);
    assert!(result.status.success(), "{result:?}");
    let document = fs::read_to_string(install.config())
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(document["lists"]["set"].as_str(), Some("all"));
    assert!(document["lists"].get("enabled").is_none());
    assert!(!install.index().exists());
}

#[test]
fn trailing_colon_discovery_has_no_matches_and_changes_no_files() {
    let install = Install::new();
    install.write_index();
    let config = fs::read(install.config()).unwrap();
    let index = fs::read(install.index()).unwrap();
    for word in ["amplihack:", "amplihack:core:"] {
        let result = install.run(&["directory", "find", word]);
        assert!(result.status.success(), "{result:?}");
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
        assert_eq!(fs::read(install.config()).unwrap(), config);
        assert_eq!(fs::read(install.index()).unwrap(), index);
    }
}

#[test]
fn directory_mutations_accept_qualified_names_without_installing_another_tool() {
    let install = Install::new();
    install.write_index();
    let config = install.run(&["config", "get", "lists.enabled"]).stdout;
    let index = fs::read(install.index()).unwrap();
    for name in ["amplihack:code-visualizer", "amplihack:core:guide"] {
        for _ in 0..2 {
            let result = install.run(&["directory", "enable", name]);
            assert!(result.status.success(), "{result:?}");
        }
        for _ in 0..2 {
            let result = install.run(&["directory", "disable", name]);
            assert!(result.status.success(), "{result:?}");
        }
        assert_eq!(
            install.run(&["config", "get", "lists.enabled"]).stdout,
            config
        );
    }
    assert_eq!(fs::read(install.index()).unwrap(), index);
    for tool in [".claude", ".codex", ".copilot"] {
        assert!(!install.home.path().join(tool).exists());
    }
    for name in ["amplihack:", "unknown", "agent:missing"] {
        let result = install.run(&["directory", "enable", name]);
        assert!(!result.status.success());
        assert_eq!(
            install.run(&["config", "get", "lists.enabled"]).stdout,
            config
        );
    }
}

#[test]
fn deferred_configuration_writes_do_not_attempt_tool_publication() {
    let install = Install::new();
    install.write_index();
    let copilot = install.home.path().join(".copilot");
    fs::create_dir(&copilot).unwrap();
    fs::write(copilot.join("config.json"), "not JSON").unwrap();
    fs::write(copilot.join("personal.md"), "unchanged").unwrap();
    fs::write(
        install.home.path().join(".amplihack/.installed-version"),
        "0.0.1",
    )
    .unwrap();
    fs::write(
        install.config(),
        "[lists]\nset = 'all'\nenabled = ['skill:code-visualizer']\n",
    )
    .unwrap();
    for args in [
        vec!["config", "unset", "lists.set"],
        vec!["config", "unset", "lists.enabled"],
        vec!["config", "set", "recipe.verbose", "false"],
    ] {
        let result = install.run(&args);
        assert!(result.status.success(), "{result:?}");
    }
    let result = install
        .command(&["config", "edit"])
        .env(
            "VISUAL",
            "printf '[lists]\\nset = \"all\"\\nenabled = [\"skill:code-visualizer\"]\\n' >",
        )
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        install.run(&["config", "get", "lists.set"]).stdout,
        b"all\n"
    );
    assert_eq!(fs::read(copilot.join("config.json")).unwrap(), b"not JSON");
    assert_eq!(fs::read(copilot.join("personal.md")).unwrap(), b"unchanged");
    assert!(!copilot.join("skills").exists());
    assert!(!install.home.path().join(".claude").exists());
    assert!(!install.home.path().join(".codex").exists());
}

#[test]
fn stale_installation_rejects_applying_commands_before_configuration_changes() {
    let install = Install::new();
    install.write_index();
    let stamp = install.home.path().join(".amplihack/.installed-version");
    fs::write(&stamp, "0.0.1").unwrap();
    let config = fs::read(install.config()).unwrap();
    let index = fs::read(install.index()).unwrap();
    for args in [
        vec!["directory", "enable", "skill:code-visualizer"],
        vec!["directory", "disable", "skill:code-visualizer"],
        vec!["config", "set", "lists.set", "all"],
    ] {
        let result = install.run(&args);
        assert!(!result.status.success(), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("amplihack install"));
        assert_eq!(fs::read(install.config()).unwrap(), config);
        assert_eq!(fs::read(install.index()).unwrap(), index);
        assert_eq!(fs::read(&stamp).unwrap(), b"0.0.1");
    }
}
