//! Real native registration acceptance; no bootstrap PATH adapter or model call.
//! Run explicitly with CODEX_TEST_BINARY=/absolute/path/to/codex and --ignored.
use super::helpers::*;
use super::*;
use crate::test_support::{EnvGuard, home_env_lock};
use std::fs;
use std::process::Command;

// Query the installed CLI's discovery protocol, independently of Rust loaders.
fn native_inventory(binary: &str, cwd: &Path) -> serde_json::Value {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(binary)
        .process_group(0)
        .args(["app-server", "--stdio"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let requests = [
            serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"amplihack-acceptance","version":"1"},"capabilities":{"experimentalApi":true}}}),
            serde_json::json!({"id":2,"method":"skills/list","params":{"cwds":[cwd],"forceReload":true}}),
            serde_json::json!({"id":3,"method":"hooks/list","params":{"cwds":[cwd]}}),
        ];
        let mut inventory = serde_json::json!({});
        for request in requests {
            writeln!(stdin, "{request}")?;
            stdin.flush()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let line = rx.recv_timeout(
                    deadline.saturating_duration_since(std::time::Instant::now()),
                )??;
                let response: serde_json::Value = serde_json::from_str(&line)?;
                if response["id"] == request["id"] {
                    anyhow::ensure!(
                        response.get("error").is_none(),
                        "native inventory error: {response}"
                    );
                    inventory[request["method"].as_str().unwrap()] = response["result"].clone();
                    break;
                }
            }
            if request["id"] == 1 {
                writeln!(stdin, "{{\"method\":\"initialized\",\"params\":{{}}}}")?;
                stdin.flush()?;
            }
        }
        Ok(inventory)
    })();
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    child.wait().unwrap();
    drop(rx);
    reader.join().unwrap();
    result.unwrap()
}

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
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap().flatten() {
            let destination = to.join(entry.file_name());
            if entry.file_type().unwrap().is_symlink() {
                std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), destination)
                    .unwrap();
            } else if entry.path().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
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
    fn find_package(root: &Path) -> Option<std::path::PathBuf> {
        for entry in fs::read_dir(root).ok()?.flatten() {
            let path = entry.path();
            if entry.file_type().ok()?.is_dir() {
                if path.join("plugin.json").is_file() && path.join("skills").is_dir() {
                    return Some(path);
                }
                if let Some(found) = find_package(&path) {
                    return Some(found);
                }
            }
        }
        None
    }
    let package = find_package(&home.join(".amplihack")).expect("owned portable package");
    fn find_resource(root: &Path) -> bool {
        fs::read_dir(root).unwrap().flatten().any(|entry| {
            if entry.path().is_dir() {
                find_resource(&entry.path())
            } else {
                entry.file_name() == "guide.md"
                    && fs::read_to_string(entry.path()).unwrap()
                        == "provider-neutral nested resource\n"
            }
        })
    }
    assert!(find_resource(&package.join("skills")));
    fn skill_roots(root: &Path, found: &mut Vec<std::path::PathBuf>) {
        if root.join("SKILL.md").is_file() {
            found.push(root.to_path_buf());
        }
        for entry in fs::read_dir(root).unwrap().flatten() {
            if entry.path().is_dir() {
                skill_roots(&entry.path(), found);
            }
        }
    }
    fn assert_resource_tree(source: &Path, staged: &Path) {
        for entry in fs::read_dir(source).unwrap().flatten() {
            let destination = staged.join(entry.file_name());
            if entry.file_type().unwrap().is_symlink() && !entry.path().exists() {
                assert_eq!(
                    fs::read_link(entry.path()).unwrap(),
                    fs::read_link(destination).unwrap()
                );
            } else if entry.path().is_dir() {
                assert_resource_tree(&entry.path(), &destination);
            } else {
                assert_eq!(
                    fs::read(entry.path()).unwrap(),
                    fs::read(destination).unwrap()
                );
            }
        }
    }
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
