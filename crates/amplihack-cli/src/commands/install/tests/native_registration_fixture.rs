//! Models native add re-enabling an owned entry; installation is separate from preference.
use super::helpers::create_exe_stub;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn executable(bin: &Path) -> PathBuf {
    let binary = create_exe_stub(bin, "codex");
    let script = fs::read_to_string(&binary).unwrap();
    let add = r#"'plugin add amplihack@amplihack-local')
    printf 'add\n' >> "$0.calls"
    if grep -Fq '[plugins."amplihack@amplihack-local"]' "$CODEX_HOME/config.toml" 2>/dev/null; then
      awk '
        /^\[/ { owned = ($0 == "[plugins.\"amplihack@amplihack-local\"]") }
        owned { sub(/^enabled = false$/, "enabled = true") }
        { print }
      ' "$CODEX_HOME/config.toml" > "$CODEX_HOME/config.new"
      mv "$CODEX_HOME/config.new" "$CODEX_HOME/config.toml"
    fi
    if [ -f "$0.foreign-on-add" ]; then
      printf '\n# foreign during native add\nsandbox_mode = "danger-full-access"\n' >> "$CODEX_HOME/config.toml"
    fi
"#;
    let list = r#"'plugin list --json')
    if [ -f "$0.installed" ]; then
      enabled=$(awk '
        /^\[/ { owned = ($0 == "[plugins.\"amplihack@amplihack-local\"]") }
        owned && /^enabled = / { print $3 }
      ' "$CODEX_HOME/config.toml")
      printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"enabled":%s,"source":{"source":"local","path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$enabled" "$HOME"
    else printf '%s\n' '{"installed":[]}'; fi;;
  *) exit 2;;"#;
    let start = script.find("'plugin list --json')").unwrap();
    let end = script[start..].find("  *) exit 2;;").unwrap() + start + "  *) exit 2;;".len();
    let script = format!("{}{}{}", &script[..start], list, &script[end..]);
    fs::write(
        &binary,
        script.replace("'plugin add amplihack@amplihack-local')\n", add),
    )
    .unwrap();
    binary
}

fn native_command(binary: &Path, home: &Path, codex_home: &Path) -> Command {
    let mut command = Command::new(binary);
    // Other tests temporarily replace the process PATH. Keep this fixture's
    // bash interpreter and POSIX utilities independent of that ambient state.
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("CODEX_HOME", codex_home);
    command
}

pub(super) fn inventory(binary: &Path, home: &Path, codex_home: &Path) -> Value {
    let output = native_command(binary, home, codex_home)
        .args(["plugin", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

pub(super) fn fail_upgrade(binary: &Path) {
    let script = fs::read_to_string(binary).unwrap();
    let branch = "'plugin marketplace add')";
    assert!(script.contains(branch));
    fs::write(
        binary,
        script.replace(branch, "'plugin marketplace add') exit 17;;\n  'never')"),
    )
    .unwrap();
}

#[test]
fn fixture_add_reenables_owned_disabled_entry_and_reports_preference_separately() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let codex_home = home.join(".codex");
    fs::create_dir(&codex_home).unwrap();
    let binary = executable(&home.join("bin"));
    let original = "# preference\n[plugins.\"other@foreign\"]\nenabled = false\n\n[plugins.\"amplihack@amplihack-local\"]\nenabled = false\n";
    fs::write(codex_home.join("config.toml"), original).unwrap();
    fs::write(binary.with_extension("installed"), b"").unwrap();
    let before = inventory(&binary, home, &codex_home);
    assert_eq!(before["installed"][0]["installed"], true);
    assert_eq!(before["installed"][0]["enabled"], false);
    let output = native_command(&binary, home, &codex_home)
        .args(["plugin", "add", "amplihack@amplihack-local", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let after = inventory(&binary, home, &codex_home);
    assert_eq!(after["installed"][0]["installed"], true);
    assert_eq!(after["installed"][0]["enabled"], true);
    assert_eq!(
        fs::read_to_string(codex_home.join("config.toml")).unwrap(),
        original.replacen(
            "[plugins.\"amplihack@amplihack-local\"]\nenabled = false",
            "[plugins.\"amplihack@amplihack-local\"]\nenabled = true",
            1
        )
    );
}

#[test]
fn native_fixture_works_without_an_ambient_shell_path() {
    let isolated = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "commands::install::tests::native_registration_fixture::fixture_add_reenables_owned_disabled_entry_and_reports_preference_separately",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", isolated.path())
        .env("HOME", isolated.path())
        .env("TMPDIR", isolated.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "fixture failed without an ambient shell PATH:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed;"),
        "fixture did not run: {stdout}"
    );
}
