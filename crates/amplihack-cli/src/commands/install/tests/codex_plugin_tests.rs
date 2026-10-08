//! Real native registration acceptance; no bootstrap PATH adapter or model call.
//! Run explicitly with CODEX_TEST_BINARY=/absolute/path/to/codex and --ignored.
use super::helpers::*;
use super::*;
use crate::test_support::{EnvGuard, home_env_lock};
use std::fs;
use std::process::Command;

#[path = "codex_plugin_tests/native_inventory.rs"]
mod native_inventory;
#[path = "codex_plugin_tests/resources.rs"]
mod resources;
use native_inventory::native_inventory;
use resources::*;

#[test]
#[ignore = "requires real installed Codex; set CODEX_TEST_BINARY and run --ignored"]
fn codex_plugin_install_update_uninstall_preserves_user_configuration_and_resources() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let codex =
        std::env::var("CODEX_TEST_BINARY").expect("absolute real Codex executable required");
    assert!(Path::new(&codex).is_absolute());
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    create_source_repo(&source);
    // Feed the production installer the complete canonical skill tree, not a
    // second provider-specific copy. Assert package bytes below.
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
    copy_tree(&canonical, &source.join("amplifier-bundle/skills"));

    let skill = source.join("amplifier-bundle/skills/nested/fixture");
    fs::create_dir_all(skill.join("references")).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: fixture\ndescription: native discovery fixture\n---\nRead references/guide.md\n").unwrap();
    fs::write(
        skill.join("references/guide.md"),
        "provider-neutral nested resource\n",
    )
    .unwrap();
    let commands = source.join("docs/claude/commands/amplihack");
    fs::create_dir_all(&commands).unwrap();
    fs::write(
        commands.join("fixture-command.md"),
        "# Fixture command\nExplain the fixture.\n",
    )
    .unwrap();
    fs::write(
        source.join("amplifier-bundle/agents/fixture-persona.md"),
        "# Fixture persona\nAct as the fixture reviewer.\n",
    )
    .unwrap();
    let home = temp.path().join("home");
    let codex_home = home.join("custom-codex");
    fs::create_dir_all(&codex_home).unwrap();
    let original =
        "# user preferences\napproval_policy = \"on-request\"\nsandbox_mode = \"read-only\"\n";
    fs::write(codex_home.join("config.toml"), original).unwrap();
    let bin = home.join("bin");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    std::os::unix::fs::symlink(&codex, bin.join("codex")).unwrap();
    let runner = create_exe_stub(&bin, "recipe-runner-rs");
    fs::write(&runner, "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'\n").unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = EnvGuard::set([
        ("HOME", home.to_str().unwrap()),
        ("CODEX_HOME", codex_home.to_str().unwrap()),
        ("AMPLIHACK_HOME", home.join(".amplihack").to_str().unwrap()),
        ("PATH", &path),
        ("AMPLIHACK_CODEX_BINARY_PATH", &codex),
        ("RECIPE_RUNNER_RS_PATH", runner.to_str().unwrap()),
        (
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
            hooks.to_str().unwrap(),
        ),
        ("AMPLIHACK_SKIP_MMDC", "1"),
        ("AMPLIHACK_SKIP_UPDATE_CHECK", "1"),
    ]);
    local_install(&source, None).unwrap();
    let list = || {
        let result = Command::new(&codex)
            .args(["plugin", "list", "--json"])
            .output()
            .unwrap();
        assert!(result.status.success());
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()
    };
    let registered = list();
    assert!(
        registered["installed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pluginId"] == "amplihack@amplihack-local"
                && p["installed"] == true
                && p["enabled"] == true)
    );
    let inventory = native_inventory(&codex, &source);
    if let Ok(evidence) = std::env::var("CODEX_TEST_EVIDENCE_DIR") {
        fs::create_dir_all(&evidence).unwrap();
        fs::write(
            Path::new(&evidence).join("native-inventory.json"),
            serde_json::to_vec_pretty(&inventory).unwrap(),
        )
        .unwrap();
    }
    let native_skills = inventory["skills/list"]["data"][0]["skills"]
        .as_array()
        .unwrap();
    assert!(
        inventory["skills/list"]["data"][0]["errors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let hooks = inventory["hooks/list"]["data"][0]["hooks"]
        .as_array()
        .unwrap();
    assert_eq!(hooks.len(), 7);
    assert!(
        hooks
            .iter()
            .all(|h| h["trustStatus"] == "untrusted" && h["source"] == "user")
    );
    // Discover package by its manifest rather than guessing native cache paths.
    let package = find_package(&home.join(".amplihack")).expect("owned portable package");
    assert!(find_resource(&package.join("skills")));
    let mut canonical_roots = Vec::new();
    let mut packaged_roots = Vec::new();
    skill_roots(&canonical, &mut canonical_roots);
    skill_roots(&package.join("skills"), &mut packaged_roots);
    assert!(canonical_roots.len() >= 130, "canonical all130 fixture");
    for original in canonical_roots {
        let body = fs::read_to_string(original.join("SKILL.md")).unwrap();
        let name = body
            .lines()
            .find_map(|line| line.strip_prefix("name:"))
            .unwrap()
            .trim()
            .trim_matches(['\'', '"']);
        assert!(
            native_skills
                .iter()
                .any(|s| s["name"] == format!("amplihack:{name}") && s["enabled"] == true),
            "native canonical discovery {name}"
        );
        let staged = packaged_roots
            .iter()
            .find(|path| path.file_name() == original.file_name())
            .expect("every canonical skill must be packaged under its existing name");
        assert_resource_tree(&original, staged);
    }

    for name in [
        "amplihack-command-fixture-command",
        "amplihack-persona-fixture-persona",
    ] {
        assert!(
            package.join("skills").join(name).join("SKILL.md").is_file(),
            "generated instruction skill {name}"
        );
    }
    assert!(package.join("hooks/hooks.json").is_file());
    fs::write(
        skill.join("references/guide.md"),
        "updated native resource\n",
    )
    .unwrap();
    local_install(&source, None).unwrap();
    let refreshed = native_inventory(&codex, &source);
    let fixture = refreshed["skills/list"]["data"][0]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "amplihack:fixture")
        .unwrap();
    let loaded = Path::new(fixture["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("references/guide.md");
    assert_eq!(
        fs::read_to_string(loaded).unwrap(),
        "updated native resource\n"
    );
    assert!(
        list()["installed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pluginId"] == "amplihack@amplihack-local" && p["installed"] == true)
    );
    // Native registration may add plugin keys, but must preserve these user values/comments.
    let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert!(config.contains(original));
    assert!(!config.contains("dangerously-bypass-hook-trust"));
    super::super::uninstall::run_uninstall().unwrap();
    assert!(
        !list()["installed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pluginId"] == "amplihack@amplihack-local")
    );
    assert!(
        fs::read_to_string(codex_home.join("config.toml"))
            .unwrap()
            .contains(original)
    );
}
