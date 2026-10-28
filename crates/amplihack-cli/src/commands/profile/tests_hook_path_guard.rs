//! Issue #1699: the hook path guard. Every test passes its own temp dir as
//! `tmp_dir`, so none depends on `$TMPDIR` or touches a real profile.
//!
//! Layout in each test: `base` is a fresh canonical tempdir. `base/tmp` plays
//! the system temp dir; everything else under `base` is "not temporary".

use super::hook_path_guard::{check_hook_binary, check_hook_path, strip_unsafe_hooks_with};
use super::settings_refresh::{Refresh, refresh_in_guarded};
use super::store::ProfileStore;
use serde_json::{Map, Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

struct Layout {
    _d: tempfile::TempDir,
    base: PathBuf,
    tmp: PathBuf,
}

fn layout() -> Layout {
    let d = tempfile::tempdir().unwrap();
    let base = d.path().canonicalize().unwrap();
    let tmp = base.join("tmp");
    fs::create_dir_all(&tmp).unwrap();
    Layout { _d: d, base, tmp }
}

/// Writes an `amplihack-hooks` stub in `dir` with `mode`.
fn stub(dir: &Path, mode: u32) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let bin = dir.join("amplihack-hooks");
    fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(mode)).unwrap();
    bin
}

fn dir(path: PathBuf) -> PathBuf {
    fs::create_dir_all(&path).unwrap();
    path
}

// ---- check_hook_binary (R3) -------------------------------------------------

#[test]
fn missing_binary_is_refused_and_named() {
    let l = layout();
    let bin = l.base.join("bin/amplihack-hooks");
    let err = check_hook_binary(&bin).unwrap_err();
    assert_eq!(err, format!("hooks binary {bin:?} does not exist"));
}

#[test]
fn relative_binary_is_refused_before_anything_else() {
    // SEC-1 / V-1: relative wins over "does not exist".
    let bin = Path::new("bin/amplihack-hooks");
    let err = check_hook_binary(bin).unwrap_err();
    assert_eq!(err, format!("hooks binary {bin:?} is not an absolute path"));
}

#[test]
fn a_directory_is_not_a_binary() {
    let l = layout();
    let bin = dir(l.base.join("bin/amplihack-hooks"));
    let err = check_hook_binary(&bin).unwrap_err();
    assert_eq!(err, format!("hooks binary {bin:?} does not exist"));
}

#[test]
fn non_executable_binary_is_refused() {
    let l = layout();
    let bin = stub(&l.base.join("bin"), 0o644);
    let err = check_hook_binary(&bin).unwrap_err();
    assert_eq!(err, format!("hooks binary {bin:?} is not executable"));
}

#[test]
fn group_or_world_writable_binary_is_refused() {
    // SEC-3 / AZ-3.
    for mode in [0o777, 0o757, 0o775] {
        let l = layout();
        let bin = stub(&l.base.join("bin"), mode);
        let err = check_hook_binary(&bin).unwrap_err();
        assert_eq!(
            err,
            format!("hooks binary {bin:?} is writable by group or others"),
            "mode {mode:o}"
        );
    }
}

#[test]
fn a_private_executable_owned_by_this_user_passes() {
    let l = layout();
    for mode in [0o755, 0o700, 0o555] {
        let bin = stub(&l.base.join(format!("bin{mode:o}")), mode);
        assert_eq!(check_hook_binary(&bin), Ok(()), "mode {mode:o}");
    }
}

#[test]
fn a_root_owned_system_binary_passes() {
    // Owned by uid 0 and mode 0755 on every Linux/macOS host.
    let sh = Path::new("/bin/sh");
    if !sh.exists() {
        return;
    }
    assert_eq!(check_hook_binary(sh), Ok(()));
}

#[test]
fn a_symlink_to_a_good_binary_passes() {
    let l = layout();
    let real = stub(&l.base.join("real"), 0o755);
    let link = l.base.join("link-amplihack-hooks");
    symlink(&real, &link).unwrap();
    assert_eq!(check_hook_binary(&link), Ok(()));
}

#[test]
fn a_dangling_symlink_does_not_exist() {
    let l = layout();
    let link = l.base.join("amplihack-hooks");
    symlink(l.base.join("gone"), &link).unwrap();
    let err = check_hook_binary(&link).unwrap_err();
    assert_eq!(err, format!("hooks binary {link:?} does not exist"));
}

// ---- check_hook_path (R3 + R4) ----------------------------------------------

#[test]
fn temp_binary_into_a_profile_outside_temp_is_refused() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let profile = dir(l.base.join("real-root/work"));
    let err = check_hook_path(&bin, &profile, &l.tmp).unwrap_err();
    assert_eq!(
        err,
        format!(
            "hooks binary {bin:?} is under temporary dir {:?} but profile {profile:?} is not",
            l.tmp
        )
    );
}

#[test]
fn temp_binary_into_a_temp_profile_is_allowed() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let profile = dir(l.tmp.join("profiles/work"));
    assert_eq!(check_hook_path(&bin, &profile, &l.tmp), Ok(()));
}

#[test]
fn non_temp_binary_into_a_profile_outside_temp_is_allowed() {
    let l = layout();
    let bin = stub(&l.base.join("home/.local/bin"), 0o755);
    let profile = dir(l.base.join("data/profiles/work"));
    assert_eq!(check_hook_path(&bin, &profile, &l.tmp), Ok(()));
}

#[test]
fn non_temp_binary_into_a_temp_profile_is_allowed() {
    let l = layout();
    let bin = stub(&l.base.join("home/.local/bin"), 0o755);
    let profile = dir(l.tmp.join("profiles/work"));
    assert_eq!(check_hook_path(&bin, &profile, &l.tmp), Ok(()));
}

#[test]
fn r3_runs_before_r4() {
    // A missing temp binary reports "does not exist", not the temp rule.
    let l = layout();
    let bin = l.tmp.join("home/.local/bin/amplihack-hooks");
    let profile = dir(l.base.join("real-root/work"));
    let err = check_hook_path(&bin, &profile, &l.tmp).unwrap_err();
    assert_eq!(err, format!("hooks binary {bin:?} does not exist"));
}

#[test]
fn prefix_match_is_by_component_not_by_string() {
    // V-7: `<base>/tmpfoo` is not under `<base>/tmp`.
    let l = layout();
    let bin = stub(&l.base.join("tmpfoo/bin"), 0o755);
    let profile = dir(l.base.join("real-root/work"));
    assert_eq!(check_hook_path(&bin, &profile, &l.tmp), Ok(()));
}

#[test]
fn a_symlinked_temp_dir_is_resolved_before_comparing() {
    // macOS: /var/folders/... vs /private/var/folders/...
    let l = layout();
    let tmp_link = l.base.join("tmp-link");
    symlink(&l.tmp, &tmp_link).unwrap();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let outside = dir(l.base.join("real-root/work"));
    assert!(
        check_hook_path(&bin, &outside, &tmp_link).is_err(),
        "binary under the real target of a symlinked temp dir is temporary"
    );
    let inside_via_link = tmp_link.join("profiles/work");
    fs::create_dir_all(&inside_via_link).unwrap();
    assert_eq!(check_hook_path(&bin, &inside_via_link, &l.tmp), Ok(()));
}

#[test]
fn an_uncanonicalisable_profile_counts_as_not_temporary() {
    // SEC-2 / V-3: `<tmp>/../outside` starts with `<tmp>` as text but does not
    // exist, so it cannot be resolved; fail closed.
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let target = l.tmp.join("../outside-1699-missing");
    assert!(!target.exists());
    let err = check_hook_path(&bin, &target, &l.tmp).unwrap_err();
    assert!(err.contains("is under temporary dir"), "{err}");
}

#[test]
fn a_symlink_outside_temp_to_a_temp_binary_is_temporary() {
    // SEC-6: the link's location does not matter, its target does.
    let l = layout();
    let real = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let link_dir = dir(l.base.join("home/.local/bin"));
    let link = link_dir.join("amplihack-hooks");
    symlink(&real, &link).unwrap();
    let profile = dir(l.base.join("real-root/work"));
    let err = check_hook_path(&link, &profile, &l.tmp).unwrap_err();
    assert!(err.contains("is under temporary dir"), "{err}");
}

#[test]
fn a_missing_non_temp_profile_with_a_non_temp_binary_is_allowed() {
    let l = layout();
    let bin = stub(&l.base.join("home/.local/bin"), 0o755);
    let target = l.base.join("real-root/not-yet");
    assert_eq!(check_hook_path(&bin, &target, &l.tmp), Ok(()));
}

// ---- strip_unsafe_hooks_with (writer b) -------------------------------------

fn quoted(bin: &Path) -> String {
    format!("\"{}\"", bin.display())
}

fn handler(command: &str) -> Value {
    json!({"type": "command", "command": command})
}

fn settings_with(event_handlers: Vec<Value>) -> Map<String, Value> {
    let Value::Object(map) = json!({
        "statusLine": {"type": "command", "command": "amplihack profile statusline -- ccstatusline"},
        "model": "opus",
        "hooks": {"Stop": [{"hooks": event_handlers}]}
    }) else {
        unreachable!()
    };
    map
}

fn stop_commands(settings: &Map<String, Value>) -> Vec<String> {
    settings
        .get("hooks")
        .and_then(|h| h.get("Stop"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| w.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|h| h.get("command").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

#[test]
fn seeding_drops_a_missing_amplihack_hook_and_keeps_everything_else() {
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let missing = l.base.join("gone/amplihack-hooks");
    let user = "echo user-hook";
    let mut s = settings_with(vec![
        handler(&format!("{} stop", quoted(&missing))),
        handler(user),
    ]);
    let status_before = s["statusLine"].clone();
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed, vec![missing.display().to_string()]);
    assert_eq!(stop_commands(&s), vec![user.to_string()]);
    assert_eq!(s["statusLine"], status_before);
    assert_eq!(s["model"], "opus");
}

#[test]
fn seeding_drops_a_temp_amplihack_hook_for_a_profile_outside_temp() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let profile = dir(l.base.join("real-root/work"));
    let mut s = settings_with(vec![
        handler(&format!("{} stop", quoted(&bin))),
        handler("echo keep"),
    ]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed, vec![bin.display().to_string()]);
    assert_eq!(stop_commands(&s), vec!["echo keep".to_string()]);
}

#[test]
fn seeding_keeps_a_temp_amplihack_hook_for_a_temp_profile() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let profile = dir(l.tmp.join("profiles/work"));
    let mut s = settings_with(vec![handler(&format!("{} stop", quoted(&bin)))]);
    let before = s.clone();
    assert!(strip_unsafe_hooks_with(&mut s, &profile, &l.tmp).is_empty());
    assert_eq!(s, before);
}

#[test]
fn seeding_keeps_a_good_quoted_path_with_spaces() {
    let l = layout();
    let bin = stub(&l.base.join("tool dir/bin"), 0o755);
    let profile = dir(l.base.join("real-root/work"));
    let mut s = settings_with(vec![handler(&format!("{} stop", quoted(&bin)))]);
    let before = s.clone();
    assert!(strip_unsafe_hooks_with(&mut s, &profile, &l.tmp).is_empty());
    assert_eq!(s, before);
}

#[test]
fn seeding_drops_a_missing_quoted_path_with_spaces() {
    let l = layout();
    let missing = l.base.join("gone dir/amplihack-hooks");
    let profile = dir(l.base.join("real-root/work"));
    let mut s = settings_with(vec![handler(&format!("{} stop", quoted(&missing)))]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed, vec![missing.display().to_string()]);
}

#[test]
fn unquoted_absolute_paths_are_recognised_too() {
    let l = layout();
    let missing = l.base.join("gone/amplihack-hooks");
    let profile = dir(l.base.join("real-root/work"));
    let mut s = settings_with(vec![handler(&format!("{} stop", missing.display()))]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed, vec![missing.display().to_string()]);
}

#[test]
fn bare_and_look_alike_commands_are_kept_relative_paths_are_dropped() {
    // SEC-5 / V-2 / V-5.
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let lookalike = l.base.join("gone/my-amplihack-hooks-wrapper");
    let mut s = settings_with(vec![
        handler("amplihack-hooks stop"),
        handler(&format!("{} stop", quoted(&lookalike))),
        handler("./rel/amplihack-hooks stop"),
    ]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed, vec!["./rel/amplihack-hooks".to_string()]);
    assert_eq!(
        stop_commands(&s),
        vec![
            "amplihack-hooks stop".to_string(),
            format!("{} stop", quoted(&lookalike)),
        ]
    );
}

#[test]
fn emptied_wrappers_events_and_hooks_object_are_removed() {
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let missing = l.base.join("gone/amplihack-hooks");
    let mut s = settings_with(vec![handler(&format!("{} stop", quoted(&missing)))]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed.len(), 1);
    assert!(!s.contains_key("hooks"), "{s:?}");
    assert!(s.contains_key("statusLine"));
    assert_eq!(s["model"], "opus");
}

#[test]
fn other_events_survive_when_one_event_empties() {
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let missing = l.base.join("gone/amplihack-hooks");
    let mut s = settings_with(vec![handler(&format!("{} stop", quoted(&missing)))]);
    s["hooks"]["PreToolUse"] = json!([{"matcher": "*", "hooks": [handler("echo pre")]}]);
    strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    let hooks = s["hooks"].as_object().unwrap();
    assert!(!hooks.contains_key("Stop"), "{hooks:?}");
    assert_eq!(hooks["PreToolUse"][0]["hooks"][0]["command"], "echo pre");
}

#[test]
fn malformed_shapes_never_panic_and_are_left_unchanged() {
    // SEC-4 / V-4.
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    for hooks in [
        json!("not an object"),
        json!(42),
        json!({"Stop": "not an array"}),
        json!({"Stop": [1, "two", null]}),
        json!({"Stop": [{"hooks": "nope"}]}),
        json!({"Stop": [{"hooks": [{"command": 7}]}]}),
        json!({"Stop": [{"hooks": [{"type": "command"}]}]}),
        json!({"Stop": [{"hooks": [{"command": ""}]}]}),
    ] {
        let mut s = Map::new();
        s.insert("hooks".into(), hooks.clone());
        let before = s.clone();
        let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
        assert!(removed.is_empty(), "{hooks}: {removed:?}");
        assert_eq!(s, before, "{hooks}");
    }
}

#[test]
fn an_unterminated_quote_does_not_panic() {
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let mut s = settings_with(vec![handler("\"/gone/amplihack-hooks stop")]);
    // Either outcome is acceptable; the contract is no panic and no change to
    // anything but hook entries.
    let _ = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(s["model"], "opus");
    assert!(s.contains_key("statusLine"));
}

#[test]
fn no_hooks_key_is_a_no_op() {
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let Value::Object(mut s) = json!({"statusLine": {"type": "command", "command": "x"}}) else {
        unreachable!()
    };
    let before = s.clone();
    assert!(strip_unsafe_hooks_with(&mut s, &profile, &l.tmp).is_empty());
    assert_eq!(s, before);
}

#[test]
fn removed_paths_never_carry_the_rest_of_the_command() {
    // SEC-7 / DP-1: the warning is built from the returned paths only.
    let l = layout();
    let profile = dir(l.base.join("real-root/work"));
    let missing = l.base.join("gone/amplihack-hooks");
    let mut s = settings_with(vec![handler(&format!(
        "{} stop --key sk-test-SENTINEL",
        quoted(&missing)
    ))]);
    let removed = strip_unsafe_hooks_with(&mut s, &profile, &l.tmp);
    assert_eq!(removed.len(), 1);
    assert!(!removed[0].contains("SENTINEL"), "{removed:?}");
}

// ---- refresh_in_guarded (writer a) ------------------------------------------

fn profile_settings(root: &Path, name: &str, text: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    file
}

fn touch_hooks(settings: &mut Value) {
    settings["hooks"]["StopFailure"] = json!([{"matcher": "rate_limit"}]);
}

#[test]
fn guarded_refresh_skips_and_never_writes_a_profile_outside_temp() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let root = dir(l.base.join("real-root"));
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let file = profile_settings(&root, "work", r#"{"hooks":{}}"#);
    let before = fs::read(&file).unwrap();
    let results = refresh_in_guarded(&ProfileStore::new(root), &bin, &l.tmp, &touch_hooks);
    assert_eq!(results.len(), 1);
    match &results[0].1 {
        Refresh::Skipped(reason) => {
            assert!(reason.contains(&format!("{bin:?}")), "{reason}");
            assert!(reason.contains("is under temporary dir"), "{reason}");
        }
        other => panic!("expected Skipped, got {other:?}"),
    }
    assert_eq!(fs::read(&file).unwrap(), before);
}

#[test]
fn guarded_refresh_updates_a_temp_profile_with_a_temp_binary() {
    let l = layout();
    let bin = stub(&l.tmp.join("home/.local/bin"), 0o755);
    let root = dir(l.tmp.join("profiles"));
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    profile_settings(&root, "work", r#"{"hooks":{}}"#);
    let results = refresh_in_guarded(&ProfileStore::new(root), &bin, &l.tmp, &touch_hooks);
    assert_eq!(results, vec![("work".to_string(), Refresh::Updated)]);
}

#[test]
fn guarded_refresh_skips_every_profile_when_the_binary_is_missing() {
    let l = layout();
    let bin = l.base.join("gone/amplihack-hooks");
    let root = dir(l.base.join("real-root"));
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let a = profile_settings(&root, "a", r#"{"hooks":{}}"#);
    let b = profile_settings(&root, "b", r#"{"hooks":{}}"#);
    let (ba, bb) = (fs::read(&a).unwrap(), fs::read(&b).unwrap());
    let results = refresh_in_guarded(&ProfileStore::new(root), &bin, &l.tmp, &touch_hooks);
    assert_eq!(results.len(), 2);
    for (_, r) in &results {
        assert_eq!(
            r,
            &Refresh::Skipped(format!("hooks binary {bin:?} does not exist"))
        );
    }
    assert_eq!(fs::read(&a).unwrap(), ba);
    assert_eq!(fs::read(&b).unwrap(), bb);
}
