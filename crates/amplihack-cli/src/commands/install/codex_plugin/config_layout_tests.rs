//! Issue #3423: native Codex lays out config.toml differently from the
//! installer's `toml_edit` prediction. Layout-only differences must not
//! refuse; any change in content still refuses and says what differs.
use super::recovery_test_support::Fixture;
use super::*;
use crate::test_support::write_executable;

/// The top-level keys run straight into a table, with no blank line.
const CONFIG: &str =
    "review_model = \"a\"\napproval_policy = \"never\"\n[tui]\nnotifications = true\n";

/// A stand-in Codex that writes the marketplace table the way the reported
/// host's Codex did: after the top-level keys, behind a blank line, and
/// before the first existing table. `$HOME/fail-add` makes the next
/// `plugin add` fail once.
const LAYOUT_CODEX: &str = r#"#!/bin/sh
set -e
cfg="$CODEX_HOME/config.toml"
case "$1 $2 $3" in
 '--version  ') echo 'codex-cli 0.166.0';;
 'plugin list --json')
  if [ -f "$HOME/registered" ]; then
   printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"enabled":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
  else echo '{"installed":[]}'; fi;;
 'plugin marketplace add')
  if ! grep -Fqs '[marketplaces.amplihack-local]' "$cfg"; then
   [ -e "$cfg" ] || : > "$cfg"
   awk -v src="$4" '
    function table() { print ""; print "[marketplaces.amplihack-local]"; print "source_type = \"local\""; print "source = \"" src "\""; print ""; done = 1 }
    !done && /^\[/ { table() }
    { print }
    END { if (!done) table() }' "$cfg" > "$cfg.new"
   mv "$cfg.new" "$cfg"
  fi
  echo '{}';;
 'plugin add amplihack@amplihack-local')
  if [ -f "$HOME/fail-add" ]; then rm -f "$HOME/fail-add"; echo 'add failed' >&2; exit 1; fi
  if ! grep -Fq '[plugins."amplihack@amplihack-local"]' "$cfg"; then
   printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$cfg"
  fi
  : > "$HOME/registered"; echo '{}';;
 'plugin remove amplihack@amplihack-local')
  sed '/^\[plugins\."amplihack@amplihack-local"\]/,$d' "$cfg" > "$cfg.new"
  sed '${/^$/d;}' "$cfg.new" > "$cfg"
  rm -f "$cfg.new" "$HOME/registered"; echo '{}';;
 *) echo '{}';;
esac
"#;

fn fixture() -> Fixture {
    let f = Fixture::new();
    write_executable(&f.binary, LAYOUT_CODEX);
    fs::write(f.codex.join("config.toml"), CONFIG).unwrap();
    f
}

fn config(f: &Fixture) -> String {
    fs::read_to_string(f.codex.join("config.toml")).unwrap()
}

fn assert_registered(f: &Fixture) {
    let table: toml::Table = config(f).parse().unwrap();
    assert_eq!(
        table["marketplaces"]["amplihack-local"]["source_type"].as_str(),
        Some("local")
    );
    assert_eq!(
        table["plugins"]["amplihack@amplihack-local"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(table["review_model"].as_str(), Some("a"));
}

#[test]
fn native_layout_install_completes_and_later_rollback_accepts_it() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    let f = fixture();
    assert!(f.install().unwrap());
    assert!(!f.root.join("pending.json").exists());
    assert_registered(&f);

    f.upgrade(b"second");
    let before = fs::read(f.codex.join("config.toml")).unwrap();
    fs::write(f.home.join("fail-add"), b"once").unwrap();
    f.install()
        .expect_err("the failed native add fails the install");
    assert_eq!(fs::read(f.codex.join("config.toml")).unwrap(), before);
    assert!(!f.root.join("pending.json").exists());
    assert!(f.archives().is_empty());

    assert!(f.install().unwrap());
    f.assert_progress();
    assert_registered(&f);
}

#[test]
fn interrupted_native_layout_write_recovers_on_next_install() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    let f = fixture();
    f.plan();
    let market = f.root.join("market");
    let status = std::process::Command::new(&f.binary)
        .args(["plugin", "marketplace", "add"])
        .arg(&market)
        .status()
        .unwrap();
    assert!(status.success());
    assert_ne!(config(&f), CONFIG);

    assert!(f.install().unwrap());
    assert!(f.archives().is_empty());
    f.assert_progress();
    assert_registered(&f);
}

#[test]
fn whitespace_only_edit_of_interrupted_journal_rolls_back_and_installs() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    let f = fixture();
    assert!(f.install().unwrap());
    f.upgrade(b"interrupted");
    f.plan();
    let spaced = config(&f).replacen('\n', "\n\n", 1).replace(" = ", "=");
    fs::write(f.codex.join("config.toml"), spaced).unwrap();

    assert!(f.install().unwrap());
    assert!(f.archives().is_empty());
    f.assert_progress();
    assert_registered(&f);
}

fn refused_edit(edit: impl FnOnce(String) -> String, names: &str) {
    let f = fixture();
    assert!(f.install().unwrap());
    f.upgrade(b"interrupted");
    let journal = f.plan();
    let edited = edit(config(&f));
    fs::write(f.codex.join("config.toml"), &edited).unwrap();

    let error = f.assert_retained(&journal);
    assert_eq!(config(&f), edited);
    let path = f.root.join("pending.json");
    assert!(error.contains(&path.display().to_string()), "{error}");
    assert!(error.contains(names), "{error}");
}

#[test]
fn changed_value_still_refuses_and_names_journal_and_key() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    refused_edit(
        |text| text.replace("review_model = \"a\"", "review_model = \"b\""),
        "review_model",
    );
}

#[test]
fn added_table_still_refuses_and_names_journal_and_table() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    refused_edit(
        |text| format!("{text}\n[shell_environment_policy]\ninherit = \"core\"\n"),
        "shell_environment_policy",
    );
}
