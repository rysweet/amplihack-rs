//! Issue #1661: `amplihack install` refreshes the hooks in each profile's
//! existing settings.json. Every test uses a temp profile root.

use super::settings_refresh::{Refresh, refresh_in};
use super::store::ProfileStore;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

fn store() -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("profiles");
    fs::create_dir_all(&root).unwrap();
    (d, ProfileStore::new(root))
}

fn profile(store: &ProfileStore, name: &str, settings: Option<&str>) {
    let dir = store.root().join(name);
    fs::create_dir_all(&dir).unwrap();
    if let Some(text) = settings {
        fs::write(dir.join("settings.json"), text).unwrap();
        fs::set_permissions(dir.join("settings.json"), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn read(store: &ProfileStore, name: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(store.root().join(name).join("settings.json")).unwrap(),
    )
    .unwrap()
}

/// Stands in for install's hook update: sets one hook event.
fn add_stop_failure(settings: &mut Value) {
    settings["hooks"]["StopFailure"] = json!([{"matcher": "rate_limit"}]);
}

fn result<'a>(results: &'a [(String, Refresh)], name: &str) -> &'a Refresh {
    &results.iter().find(|(n, _)| n == name).unwrap().1
}

#[test]
fn hooks_are_updated_and_other_settings_kept() {
    let (_d, store) = store();
    profile(
        &store,
        "work",
        Some(r#"{"hooks":{"Stop":[{"x":1}]},"language":"en"}"#),
    );
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "work"), &Refresh::Updated);
    let s = read(&store, "work");
    assert_eq!(s["language"], json!("en"));
    assert_eq!(s["hooks"]["Stop"], json!([{"x": 1}]));
    assert_eq!(
        s["hooks"]["StopFailure"],
        json!([{"matcher": "rate_limit"}])
    );
    let mode = fs::metadata(store.root().join("work/settings.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_missing_settings_file_is_not_created() {
    let (_d, store) = store();
    profile(&store, "bare", None);
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "bare"), &Refresh::Missing);
    assert!(!store.root().join("bare/settings.json").exists());
}

#[test]
fn an_up_to_date_file_is_not_rewritten() {
    let (_d, store) = store();
    profile(
        &store,
        "done",
        Some(r#"{"hooks":{"StopFailure":[{"matcher":"rate_limit"}]}}"#),
    );
    let path = store.root().join("done/settings.json");
    let before = fs::read_to_string(&path).unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "done"), &Refresh::Unchanged);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "byte-for-byte untouched"
    );
}

#[test]
fn a_symlinked_settings_file_is_skipped_and_its_target_untouched() {
    let (d, store) = store();
    profile(&store, "linked", None);
    let target = d.path().join("elsewhere.json");
    fs::write(&target, "{}").unwrap();
    symlink(&target, store.root().join("linked/settings.json")).unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    assert!(matches!(result(&results, "linked"), Refresh::Skipped(_)));
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
}

#[test]
fn invalid_or_non_object_json_is_skipped_unchanged() {
    let (_d, store) = store();
    profile(&store, "broken", Some("{not json"));
    profile(&store, "list", Some("[1,2]"));
    let results = refresh_in(&store, &add_stop_failure);
    assert!(matches!(result(&results, "broken"), Refresh::Skipped(_)));
    assert!(matches!(result(&results, "list"), Refresh::Skipped(_)));
    assert_eq!(
        fs::read_to_string(store.root().join("broken/settings.json")).unwrap(),
        "{not json"
    );
}

#[test]
fn only_real_profile_directories_are_visited() {
    let (d, store) = store();
    profile(&store, "real", Some("{}"));
    let outside = d.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("settings.json"), "{}").unwrap();
    symlink(&outside, store.root().join("alias")).unwrap();
    fs::create_dir_all(store.root().join(".auto-switch.d")).unwrap();
    fs::write(store.root().join(".auto-switch.d/settings.json"), "{}").unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    let names: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["real"]);
    assert_eq!(
        fs::read_to_string(outside.join("settings.json")).unwrap(),
        "{}"
    );
    assert!(Path::new(&store.root().join(".auto-switch.d/settings.json")).exists());
    assert_eq!(
        fs::read_to_string(store.root().join(".auto-switch.d/settings.json")).unwrap(),
        "{}"
    );
}

#[test]
fn no_root_means_no_profiles() {
    let d = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(d.path().join("absent"));
    assert!(refresh_in(&store, &add_stop_failure).is_empty());
}

// ---------------------------------------------------------------------------
// Issue #1699: `amplihack install` run by a test under a temp HOME rewrote the
// real `~/.claude-profiles/*/settings.json` with a hooks path inside that temp
// HOME. `hook_path_refusal` decides, from its arguments alone, whether a hooks
// path may be written into the profiles under a root. These tests call it and
// `refresh_in` directly and change no process state.
// ---------------------------------------------------------------------------

mod issue_1699 {
    use super::super::settings_refresh::{Refresh, hook_path_refusal, refresh_in};
    use super::super::store::ProfileStore;
    use super::{add_stop_failure, profile, read, result};
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    /// A fixture directory, canonical, so that injected temp bases compare
    /// equal to the canonical hooks path.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().canonicalize().unwrap();
        (d, base)
    }

    /// Writes an executable stand-in for `amplihack-hooks` at `path`.
    fn hooks_file(path: &Path) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_path_buf()
    }

    fn dir(path: PathBuf) -> PathBuf {
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn missing_hooks_binary_is_refused() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let root = dir(home.join("profiles"));
        let bin = Path::new("/nonexistent/amplihack-hooks");
        let reason = hook_path_refusal(bin, &root, Some(&home), &[])
            .expect("a hooks path that does not exist must be refused");
        assert!(reason.contains("does not exist"), "{reason}");
        assert!(reason.contains("/nonexistent/amplihack-hooks"), "{reason}");
    }

    #[test]
    fn a_directory_is_not_a_hooks_binary() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let root = dir(home.join("profiles"));
        let not_a_file = dir(home.join(".local/bin/amplihack-hooks"));
        assert!(
            hook_path_refusal(&not_a_file, &root, Some(&home), &[]).is_some(),
            "only a regular file (or a symlink to one) is accepted"
        );
    }

    #[test]
    fn a_dangling_symlink_is_refused() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let root = dir(home.join("profiles"));
        let link = home.join(".local/bin/amplihack-hooks");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(base.join("gone"), &link).unwrap();
        let reason = hook_path_refusal(&link, &root, Some(&home), &[]).unwrap();
        assert!(reason.contains("does not exist"), "{reason}");
    }

    /// The incident's shape: HOME and the hooks binary are under a temporary
    /// directory, the profile root is not under HOME.
    #[test]
    fn temp_hooks_into_root_outside_home_is_refused() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let (_other, root) = fixture();
        let reason = hook_path_refusal(&bin, &root, Some(&home), std::slice::from_ref(&base))
            .expect("temp hooks path into a root outside HOME must be refused");
        assert!(reason.contains("is under temporary directory"), "{reason}");
        assert!(reason.contains("is outside HOME"), "{reason}");
        assert!(reason.contains(&bin.display().to_string()), "{reason}");
        assert!(reason.contains(&base.display().to_string()), "{reason}");
        assert!(reason.contains(&root.display().to_string()), "{reason}");
    }

    /// T1's `/tmp` variant: the hooks binary is under the temp base but not
    /// under HOME at all.
    #[test]
    fn temp_hooks_outside_home_into_real_looking_root_is_refused() {
        let (_d, base) = fixture();
        let bin = hooks_file(&base.join("build/amplihack-hooks"));
        let (_h, real) = fixture();
        let home = dir(real.join("home"));
        let root = dir(real.join("elsewhere/profiles"));
        assert!(hook_path_refusal(&bin, &root, Some(&home), std::slice::from_ref(&base)).is_some());
    }

    #[test]
    fn temp_hooks_with_home_unset_is_refused() {
        let (_d, base) = fixture();
        let bin = hooks_file(&base.join("home/.local/bin/amplihack-hooks"));
        let root = dir(base.join("home/profiles"));
        let reason = hook_path_refusal(&bin, &root, None, std::slice::from_ref(&base))
            .expect("with HOME unset a temp hooks path is refused");
        assert!(reason.contains("is under temporary directory"), "{reason}");
        assert!(reason.contains("HOME is unset"), "{reason}");
    }

    /// A symlink in a real-looking HOME that points into a temp directory is
    /// judged by its target.
    #[test]
    fn a_symlink_into_a_temp_directory_is_judged_by_its_target() {
        let (_d, tmp) = fixture();
        let target = hooks_file(&tmp.join("amplihack-hooks"));
        let (_h, real) = fixture();
        let home = dir(real.join("home"));
        let link = home.join(".local/bin/amplihack-hooks");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();
        let (_r, root) = fixture();
        assert!(
            hook_path_refusal(&link, &root, Some(&home), std::slice::from_ref(&tmp)).is_some(),
            "canonical target is under the temp base"
        );
    }

    /// A symlink to a regular file outside any temp base is accepted.
    #[test]
    fn a_symlink_to_a_real_file_is_accepted() {
        let (_d, real) = fixture();
        let home = dir(real.join("home"));
        let target = hooks_file(&real.join("opt/amplihack-hooks"));
        let link = home.join(".local/bin/amplihack-hooks");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();
        let root = dir(real.join("disk2/profiles"));
        assert_eq!(hook_path_refusal(&link, &root, Some(&home), &[]), None);
    }

    /// The stored path is the one written, so a symlink placed in a temp
    /// directory is refused even when its target lies outside every base.
    #[test]
    fn a_symlink_in_a_temp_directory_is_judged_by_where_it_sits() {
        let (_d, tmp) = fixture();
        let (_r, real) = fixture();
        let home = dir(real.join("home"));
        let target = hooks_file(&real.join("opt/amplihack-hooks"));
        let link = tmp.join("build/amplihack-hooks");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();
        let root = dir(real.join("disk2/profiles"));
        let reason = hook_path_refusal(&link, &root, Some(&home), std::slice::from_ref(&tmp))
            .expect("a hooks path written under a temp base must be refused");
        assert!(reason.contains("is under temporary directory"), "{reason}");
        assert!(reason.contains(&link.display().to_string()), "{reason}");
    }

    /// A symlink outside every temp base whose target is also outside them is
    /// accepted with the temp bases in force.
    #[test]
    fn a_symlink_outside_temp_to_a_file_outside_temp_is_accepted() {
        let (_d, tmp) = fixture();
        let (_r, real) = fixture();
        let home = dir(real.join("home"));
        let target = hooks_file(&real.join("opt/amplihack-hooks"));
        let link = home.join(".local/bin/amplihack-hooks");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();
        let root = dir(real.join("disk2/profiles"));
        assert_eq!(
            hook_path_refusal(&link, &root, Some(&home), std::slice::from_ref(&tmp)),
            None
        );
    }

    /// A normal user: real HOME, hooks binary inside it, root on another disk.
    /// #1661 behaviour is kept.
    #[test]
    fn real_home_hooks_into_root_on_another_disk_are_allowed() {
        let (_d, real) = fixture();
        let home = dir(real.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let root = dir(real.join("mnt/data/profiles"));
        assert_eq!(hook_path_refusal(&bin, &root, Some(&home), &[]), None);
    }

    /// A temp HOME whose profile root is inside it is self-contained and
    /// allowed: this is every well-isolated test fixture.
    #[test]
    fn temp_home_with_root_inside_it_is_allowed() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let root = dir(home.join("profiles"));
        assert_eq!(
            hook_path_refusal(&bin, &root, Some(&home), std::slice::from_ref(&base)),
            None
        );
    }

    /// A root that does not exist yet is compared in its lexical form.
    #[test]
    fn a_missing_root_is_compared_lexically() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let inside = home.join("not-yet/profiles");
        let (_o, other) = fixture();
        let outside = other.join("not-yet/profiles");
        let bases = std::slice::from_ref(&base);
        assert_eq!(hook_path_refusal(&bin, &inside, Some(&home), bases), None);
        assert!(hook_path_refusal(&bin, &outside, Some(&home), bases).is_some());
    }

    /// "Under" is component-wise: `/x/tmpfoo` is not under `/x/tmp`, and
    /// `/x/home2` is not under `/x/home`.
    #[test]
    fn tmp_prefix_is_component_wise() {
        let (_d, base) = fixture();
        let tmp = dir(base.join("tmp"));
        let home = dir(base.join("home"));
        let bin = hooks_file(&base.join("tmpfoo/amplihack-hooks"));
        let root = dir(base.join("elsewhere"));
        assert_eq!(
            hook_path_refusal(&bin, &root, Some(&home), std::slice::from_ref(&tmp)),
            None,
            "tmpfoo is not under tmp"
        );

        let in_tmp = hooks_file(&tmp.join("home/.local/bin/amplihack-hooks"));
        let tmp_home = dir(tmp.join("home"));
        let sibling = dir(tmp.join("home2/profiles"));
        assert!(
            hook_path_refusal(
                &in_tmp,
                &sibling,
                Some(&tmp_home),
                std::slice::from_ref(&tmp)
            )
            .is_some(),
            "home2 is not under home"
        );
    }

    /// Temp bases that do not lie over the hooks path make no difference.
    #[test]
    fn unrelated_temp_bases_are_ignored() {
        let (_d, real) = fixture();
        let home = dir(real.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let (_t, tmp) = fixture();
        let (_r, root) = fixture();
        assert_eq!(
            hook_path_refusal(
                &bin,
                &root,
                Some(&home),
                &[tmp, PathBuf::from("/nonexistent")]
            ),
            None
        );
    }

    /// T4: fixture hooks inside HOME, with temp bases that leave the fixture
    /// out, are allowed, and the refresh then updates the profile.
    #[test]
    fn fixture_hooks_inside_home_are_allowed() {
        let (_d, base) = fixture();
        let home = dir(base.join("home"));
        let bin = hooks_file(&home.join(".local/bin/amplihack-hooks"));
        let root = dir(home.join("profiles"));
        assert_eq!(hook_path_refusal(&bin, &root, Some(&home), &[]), None);

        let store = ProfileStore::new(root);
        profile(&store, "work", Some(r#"{"hooks":{}}"#));
        let results = refresh_in(&store, &add_stop_failure);
        assert_eq!(result(&results, "work"), &Refresh::Updated);
    }

    /// T3: the refresh edits `hooks` only; a profile's status-line wrapper is
    /// kept exactly.
    #[test]
    fn refresh_keeps_status_line_wrapper() {
        let (_d, base) = fixture();
        let store = ProfileStore::new(dir(base.join("profiles")));
        let status_line = json!({
            "type": "command",
            "command": "/home/someone/.claude-profiles/work/statusline-wrapper.sh",
            "padding": 0
        });
        let text = serde_json::to_string(&json!({
            "hooks": {},
            "statusLine": status_line,
        }))
        .unwrap();
        profile(&store, "work", Some(&text));
        let results = refresh_in(&store, &add_stop_failure);
        assert_eq!(result(&results, "work"), &Refresh::Updated);
        assert_eq!(read(&store, "work")["statusLine"], status_line);
    }
}
