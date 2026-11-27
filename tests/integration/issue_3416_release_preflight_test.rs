//! Outside-in acceptance for complete and incomplete local release bundles.

use super::{exec_stub, isolated_amplihack};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn local_install_accepts_release_and_preserves_it_when_input_is_incomplete() {
    let scratch = tempfile::tempdir().unwrap();
    let environment = isolated_amplihack::private_env_in(scratch.path());
    let source = scratch.path().join("source");
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = source.join("amplifier-bundle");
    copy_tree(&checkout.join("amplifier-bundle"), &bundle);

    // Ancillary executables are test doubles. The installer and release assets
    // are real, and no dependency download or client login is needed.
    let bin = scratch.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let hooks = bin.join("amplihack-hooks");
    let runner = bin.join("recipe-runner-rs");
    exec_stub::write_executable(&hooks, "#!/bin/sh\nexit 0\n", 0o755);
    exec_stub::write_executable(
        &runner,
        "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}'\n",
        0o755,
    );
    write_native_client(&bin);
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let install = || {
        environment
            .command(env!("CARGO_BIN_EXE_amplihack"))
            .args(["install", "--local"])
            .arg(&source)
            .env("PATH", &path)
            .env("CARGO_HOME", environment.root().join("cargo"))
            .env("RUSTUP_HOME", environment.root().join("rustup"))
            .env("RECIPE_RUNNER_RS_PATH", &runner)
            .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks)
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .output()
            .unwrap()
    };
    let output = install();
    assert!(
        output.status.success(),
        "complete release install failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let installed = environment.home().join(".amplihack/amplifier-bundle");
    for asset in [
        "recipes/default-workflow.yaml",
        "skills/default-workflow/SKILL.md",
    ] {
        assert_eq!(
            fs::read(installed.join(asset)).unwrap(),
            fs::read(bundle.join(asset)).unwrap()
        );
    }
    assert!(environment.home().join(".amplihack/CLAUDE.md").is_file());
    let staging = environment
        .home()
        .join(".amplihack/amplifier-bundle.staging");
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("asset"), "pending staged content\n").unwrap();
    let before = snapshot(environment.home());

    let recipes = bundle.join("recipes");
    fs::rename(&recipes, source.join("saved-recipes")).unwrap();
    assert!(!install().status.success());
    assert_eq!(snapshot(environment.home()), before);

    fs::rename(source.join("saved-recipes"), &recipes).unwrap();
    fs::remove_dir_all(bundle.join("skills")).unwrap();
    fs::create_dir(bundle.join("skills")).unwrap();
    fs::write(bundle.join("skills/marker.txt"), "fixture\n").unwrap();
    assert!(!install().status.success());
    assert_eq!(snapshot(environment.home()), before);
}

fn write_native_client(bin: &Path) {
    // Model native registration and inventory, including its persisted config.
    // These commands never contact a client service.
    let script = r#"#!/bin/sh
case "$1 $2 $3" in
  '--version  ') printf '%s\n' 'codex-cli 0.160.0';;
  'plugin marketplace add')
    mkdir -p "$CODEX_HOME"
    printf '[marketplaces.amplihack-local]\nsource_type = "local"\nsource = "%s"\n' "$4" >> "$CODEX_HOME/config.toml"
    printf '%s\n' '{"marketplaceName":"amplihack-local"}';;
  'plugin add amplihack@amplihack-local')
    printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$CODEX_HOME/config.toml"
    touch "$CODEX_HOME/fixture-installed"
    printf '%s\n' '{"pluginId":"amplihack@amplihack-local"}';;
  'plugin list --json')
    if [ -f "$CODEX_HOME/fixture-installed" ]; then
      printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"enabled":true,"source":{"source":"local","path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
    else printf '%s\n' '{"installed":[]}'; fi;;
  *) exit 2;;
esac
"#;
    exec_stub::write_executable(&bin.join("codex"), script, 0o755);
    exec_stub::write_executable(
        &bin.join("codex-code-mode-host"),
        format!("#!/bin/sh\nexit 0\n#{}\n", "fixture ".repeat(150)),
        0o755,
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u8, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let value = if metadata.file_type().is_symlink() {
                (
                    b'l',
                    fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_bytes()
                        .to_vec(),
                )
            } else if metadata.is_dir() {
                pending.push(path.clone());
                (b'd', Vec::new())
            } else {
                (b'f', fs::read(&path).unwrap())
            };
            entries.insert(path.strip_prefix(root).unwrap().to_path_buf(), value);
        }
    }
    entries
}
