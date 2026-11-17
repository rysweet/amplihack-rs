//! Issue #2279 (C5, D4): the launch repair heals any drift, not only a file
//! with no amplihack hook at all.
//!
//! * A file holding only some amplihack hooks is repaired at launch.
//! * A stale `StopFailure` with a current Claude is current and untouched; a
//!   current file is not rewritten and gets no backup.
//! * The target is `$CLAUDE_CONFIG_DIR/settings.json` when that variable is
//!   set, non-empty and absolute, else `~/.claude/settings.json`.
//! * An unparseable file gets one warning and no write.
//!
//! Every path is under a temp HOME through `IsolatedHome`.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::IsolatedHome;
use super::*;
use hook_block::HookTarget;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    _tmp: tempfile::TempDir,
    _bin_dir: tempfile::TempDir,
    home: PathBuf,
    hooks_bin: PathBuf,
    env: IsolatedHome,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let hooks_bin = create_exe_stub(&bin_dir.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::new(&home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_bin);
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
    env.unset("CLAUDE_CONFIG_DIR");
    create_minimal_staged_assets(&home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    Fixture {
        _tmp: tmp,
        _bin_dir: bin_dir,
        home,
        hooks_bin,
        env,
    }
}

impl Fixture {
    fn target(&self) -> HookTarget {
        HookTarget {
            bin: self.hooks_bin.clone(),
            limit_resume: true,
        }
    }

    fn write(&self, path: &Path, settings: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(settings).unwrap()).unwrap();
    }

    fn read(&self, path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn launch(&self) -> String {
        let mut notice = Vec::new();
        crate::freshness::with_provider("claude", || {
            ensure_framework_installed_with(&mut notice).expect("the launch repair must succeed")
        });
        String::from_utf8_lossy(&notice).into_owned()
    }

    fn home_settings(&self) -> PathBuf {
        self.home.join(".claude/settings.json")
    }
}

fn backups(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("settings.json.backup."))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_file_with_only_some_amplihack_hooks_is_repaired_at_launch() {
    let f = fixture();
    let mut partial = json!({"model": "opus"});
    hook_block::merge(&mut partial, &f.target());
    for event in ["SessionEnd", "PreCompact", "StopFailure"] {
        partial["hooks"].as_object_mut().unwrap().remove(event);
    }
    let path = f.home_settings();
    f.write(&path, &partial);

    f.launch();

    let after = f.read(&path);
    let drift = hook_block::drift(&after, &f.target());
    assert!(drift.is_clean(), "{drift:?}");
    assert_eq!(after["model"], "opus");
}

#[test]
fn a_file_with_a_differing_timeout_is_repaired_at_launch() {
    let f = fixture();
    let mut s = json!({});
    hook_block::merge(&mut s, &f.target());
    for w in s["hooks"]["Stop"].as_array_mut().unwrap() {
        w["hooks"][0]["timeout"] = json!(1);
    }
    let path = f.home_settings();
    f.write(&path, &s);
    f.launch();
    assert!(hook_block::drift(&f.read(&path), &f.target()).is_clean());
}

#[test]
fn a_current_file_is_not_rewritten_and_gets_no_backup() {
    let f = fixture();
    // Issue #2589: current means current for both passes a launch makes --
    // the hooks block and the deny entries -- so the file starts with the
    // entries the policy would add, and neither pass has anything to write.
    let mut s = json!({"model": "opus", "permissions": {"deny": []}});
    crate::policy::claude::ensure_deny(&mut s, &[]);
    hook_block::merge(&mut s, &f.target());
    let path = f.home_settings();
    f.write(&path, &s);
    let before = fs::read(&path).unwrap();
    f.launch();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(backups(path.parent().unwrap()).is_empty());
}

#[test]
fn a_custom_name_override_is_repaired_once_and_passes_final_verification() {
    let mut f = fixture();
    f.hooks_bin = create_exe_stub(f._bin_dir.path(), "amplihack-hooks-v2");
    f.env
        .set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &f.hooks_bin);
    let path = f.home_settings();
    f.write(&path, &json!({"hooks": {}}));
    f.launch();
    assert!(hook_block::drift(&f.read(&path), &f.target()).is_clean());
    let before = fs::read(&path).unwrap();
    let saved = backups(path.parent().unwrap());
    f.launch();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(backups(path.parent().unwrap()), saved);
}

#[test]
fn an_absent_home_settings_file_is_created_at_launch() {
    let f = fixture();
    let path = f.home_settings();
    assert!(!path.exists());
    f.launch();
    assert!(hook_block::drift(&f.read(&path), &f.target()).is_clean());
}

#[test]
fn an_absent_claude_config_dir_settings_file_is_left_absent() {
    let mut f = fixture();
    let cfg = f.home.join("profile-cfg");
    f.env.set("CLAUDE_CONFIG_DIR", &cfg);
    f.launch();
    assert!(!cfg.join("settings.json").exists());
    assert!(!f.home_settings().exists());
}

#[test]
fn the_repair_backs_up_only_when_it_changes_the_file() {
    let f = fixture();
    let path = f.home_settings();
    f.write(&path, &json!({"hooks": {}}));
    f.launch();
    assert_eq!(backups(path.parent().unwrap()).len(), 1);
    f.launch();
    assert_eq!(
        backups(path.parent().unwrap()).len(),
        1,
        "a second launch on a current file adds no backup"
    );
}

#[test]
fn claude_config_dir_is_the_target_when_set_and_absolute() {
    let mut f = fixture();
    let cfg = f.home.join("profile-cfg");
    let cfg_settings = cfg.join("settings.json");
    f.write(&cfg_settings, &json!({"hooks": {}}));
    let home_settings = f.home_settings();
    f.write(&home_settings, &json!({"hooks": {}}));
    let home_before = fs::read(&home_settings).unwrap();
    f.env.set("CLAUDE_CONFIG_DIR", &cfg);

    f.launch();

    assert!(hook_block::drift(&f.read(&cfg_settings), &f.target()).is_clean());
    assert_eq!(
        fs::read(&home_settings).unwrap(),
        home_before,
        "the home file is not the target when CLAUDE_CONFIG_DIR is set"
    );
}

#[test]
fn a_relative_claude_config_dir_is_ignored() {
    let mut f = fixture();
    let home_settings = f.home_settings();
    f.write(&home_settings, &json!({"hooks": {}}));
    f.env.set("CLAUDE_CONFIG_DIR", "relative/cfg");
    f.launch();
    assert!(hook_block::drift(&f.read(&home_settings), &f.target()).is_clean());
}

#[test]
fn an_unparseable_file_gets_one_warning_and_no_write() {
    let f = fixture();
    let path = f.home_settings();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        b"{ \"SENTINEL_SECRET\": \"sk-test-do-not-print\", oops",
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let mut notice = Vec::new();
    crate::freshness::with_provider("claude", || {
        ensure_framework_installed_with(&mut notice).expect("no error on an unparseable file")
    });
    assert_eq!(fs::read(&path).unwrap(), before);
    let notice = String::from_utf8_lossy(&notice);
    assert!(!notice.contains("sk-test-do-not-print"), "{notice}");
    assert!(backups(path.parent().unwrap()).is_empty());
}

// --- issue #2589: the deny entries are refreshed on every launch -----------
//
// `repair_settings_hooks` returns when amplihack's hooks block is clean, so
// the settings writer -- and with it `ensure_permissions` -- is never
// reached on an ordinary launch. `refresh_deny_entries` runs beside it for
// exactly that case. Before it existed, a user who added a
// `policy.deny_commands` entry and then only ever launched kept the old
// entries until the next explicit install or update.

/// A settings file whose hooks block is current, so the hook repair returns
/// before it writes anything.
fn clean_hooks_settings(f: &Fixture, extra: Value) -> PathBuf {
    let mut settings = extra;
    hook_block::merge(&mut settings, &f.target());
    let path = f.home_settings();
    f.write(&path, &settings);
    assert_eq!(
        hook_block::drift_file(&path, &f.target()),
        hook_block::FileDrift::Clean,
        "the fixture must start with a clean hooks block"
    );
    path
}

fn deny_entries(settings: &Value) -> Vec<String> {
    settings["permissions"]["deny"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Writes `[policy] deny_commands = [...]` into `<home>/.amplihack/config.toml`.
fn instance_config(home: &Path, entries: &[&str]) {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    let list = entries
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        dir.join("config.toml"),
        format!("[policy]\ndeny_commands = [{list}]\n"),
    )
    .unwrap();
}

#[test]
fn a_launch_with_clean_hooks_still_writes_the_default_deny_entries() {
    let f = fixture();
    let path = clean_hooks_settings(&f, json!({"model": "opus"}));

    f.launch();

    let after = f.read(&path);
    let entries = deny_entries(&after);
    for expected in crate::policy::claude::deny_entries(&[]) {
        assert!(
            entries.contains(&expected),
            "{expected} missing: {entries:?}"
        );
    }
    assert_eq!(after["model"], "opus", "the rest of the file is kept");
}

#[test]
fn a_launch_with_clean_hooks_picks_up_an_instance_addition() {
    let f = fixture();
    instance_config(&f.home, &["terraform apply"]);
    let path = clean_hooks_settings(&f, json!({}));

    f.launch();

    assert!(
        deny_entries(&f.read(&path)).contains(&"Bash(terraform apply:*)".to_string()),
        "{:?}",
        deny_entries(&f.read(&path))
    );
}

#[test]
fn a_user_entry_already_in_the_file_is_kept_and_a_second_launch_rewrites_nothing() {
    let f = fixture();
    let path = clean_hooks_settings(
        &f,
        json!({"permissions": {"deny": ["Bash(my-own-command:*)"]}}),
    );

    f.launch();
    assert_eq!(
        deny_entries(&f.read(&path)).first().map(String::as_str),
        Some("Bash(my-own-command:*)"),
        "the user's own entry keeps its place"
    );

    let before = fs::metadata(&path).unwrap();
    let bytes = fs::read(&path).unwrap();
    f.launch();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap()),
        std::os::unix::fs::MetadataExt::ino(&before),
        "a current file is not rewritten"
    );
}

#[test]
fn no_claude_settings_file_is_created_by_the_deny_refresh() {
    let mut f = fixture();
    // `CLAUDE_CONFIG_DIR` is the target the launch repair never creates, so
    // a launch there leaves no settings file for the refresh to write into.
    let cfg = f.home.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    f.env.set("CLAUDE_CONFIG_DIR", &cfg);

    f.launch();

    assert!(!cfg.join("settings.json").exists());
}

#[test]
fn an_unmergeable_permissions_value_is_reported_and_never_replaced() {
    let f = fixture();
    let path = clean_hooks_settings(&f, json!({"permissions": "SENTINEL_SECRET_VALUE"}));
    let before = fs::read(&path).unwrap();

    let notice = f.launch();

    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "the file is left as it was"
    );
    assert!(
        notice.contains("permissions is not a JSON object"),
        "{notice}"
    );
    assert!(!notice.contains("SENTINEL_SECRET_VALUE"), "{notice}");
}
