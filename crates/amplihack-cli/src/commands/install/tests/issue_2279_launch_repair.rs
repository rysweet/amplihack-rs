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
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let bin_dir = crate::test_support::tempdir_outside_framework_roots();
    let home = tmp.path().to_path_buf();
    let hooks_bin = create_exe_stub(&bin_dir.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::installed(&home);
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
    let mut s = json!({"model": "opus"});
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
