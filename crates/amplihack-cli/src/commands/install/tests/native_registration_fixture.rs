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

pub(super) fn inventory(binary: &Path, home: &Path, codex_home: &Path) -> Value {
    let output = Command::new(binary)
        .args(["plugin", "list", "--json"])
        .env("HOME", home)
        .env("CODEX_HOME", codex_home)
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
    let output = Command::new(&binary)
        .args(["plugin", "add", "amplihack@amplihack-local", "--json"])
        .env("HOME", home)
        .env("CODEX_HOME", &codex_home)
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
