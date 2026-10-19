//! The login fingerprint (issue #1618): which directory is watched, the
//! three parts (ledger account, account identity, credentials present), and
//! what must not count as a change, chiefly an OAuth token refresh that
//! rewrites `.credentials.json`.
//!
//! `Fingerprint` has no `Debug`, so comparisons use `assert!(a == b)`.
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_fingerprint;`.

use super::super::name::ProfileName;
use super::super::store::ProfileStore;
use super::fingerprint::{Fingerprint, Watch};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

struct Env {
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
    home: PathBuf,
    store: ProfileStore,
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn account(uuid: Option<&str>, email: &str, pad: usize) -> String {
    let mut v = serde_json::json!({
        "oauthAccount": {"emailAddress": email},
        "projects": {"pad": "p".repeat(pad)},
    });
    if let Some(u) = uuid {
        v["oauthAccount"]["accountUuid"] = u.into();
    }
    v.to_string()
}

/// Profiles `a` and `b` with logins; `~/.claude` with a login.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_path_buf();
    let home = base.join("home");
    let store = ProfileStore::new(base.join("root"));
    for n in ["a", "b"] {
        store.add(&ProfileName::parse(n).unwrap()).unwrap();
        let dir = store.root().join(n);
        private(
            &dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"t1"}}"#,
        );
        private(
            &dir.join(".claude.json"),
            &account(Some(&format!("uuid-{n}")), "x@example.invalid", 0),
        );
    }
    private(&home.join(".claude/.credentials.json"), "{}");
    private(
        &home.join(".claude.json"),
        &account(Some("uuid-home"), "me@example.invalid", 0),
    );
    Env {
        _tmp: tmp,
        tmp: base,
        home,
        store,
    }
}

impl Env {
    fn profile(&self, n: &str) -> Watch {
        let name = ProfileName::parse(n).unwrap();
        Watch::new(Some(&self.store), Some(&name), None, Some(&self.home))
    }

    fn home_watch(&self) -> Watch {
        Watch::new(Some(&self.store), None, None, Some(&self.home))
    }

    fn dir(&self, n: &str) -> PathBuf {
        self.store.root().join(n)
    }

    fn rotator(&self, live: &str) {
        let dir = self.store.root().join(".rotator");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(
            &dir.join("ledger.json"),
            &format!(
                r#"{{"duplicate":null,"live":"{live}","live_hash":"h","live_week_reset":0,"since":1}}"#
            ),
        );
    }

    fn auto_switch_ledger(&self, dir: &str, live: &str) {
        let state = self.store.root().join(".auto-switch.d");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        private(
            &state.join("ledger.json"),
            &format!(
                r#"{{"version":1,"hosts":{{"{dir}":{{"live":"{live}","swapped_at":1,"pre_swap_fingerprint":null}}}}}}"#
            ),
        );
    }
}

fn probe(w: &Watch) -> Fingerprint {
    w.probe().expect("a watched directory gives a fingerprint")
}

#[test]
fn the_watched_directory_follows_the_documented_order() {
    let e = env();
    let a = ProfileName::parse("a").unwrap();
    let cfg = e.tmp.join("cfg");
    assert_eq!(
        Watch::new(Some(&e.store), Some(&a), Some(&cfg), Some(&e.home)).dir(),
        Some(e.dir("a").as_path()),
        "an inherited profile comes first"
    );
    assert_eq!(
        Watch::new(Some(&e.store), None, Some(&cfg), Some(&e.home)).dir(),
        Some(cfg.as_path()),
        "then CLAUDE_CONFIG_DIR"
    );
    assert_eq!(
        Watch::new(None, None, None, Some(&e.home)).dir(),
        Some(e.home.join(".claude").as_path()),
        "then $HOME/.claude"
    );
    let none = Watch::new(None, None, None, None);
    assert_eq!(none.dir(), None);
    assert!(none.probe().is_none());
}

#[test]
fn an_unchanged_login_gives_equal_fingerprints() {
    let e = env();
    for w in [e.profile("a"), e.home_watch()] {
        assert!(probe(&w) == probe(&w));
    }
}

#[test]
fn a_token_refresh_rewriting_the_credentials_is_not_a_change() {
    let e = env();
    let w = e.profile("a");
    let before = probe(&w);
    let creds = e.dir("a").join(".credentials.json");
    std::thread::sleep(std::time::Duration::from_millis(20));
    private(
        &creds,
        r#"{"claudeAiOauth":{"accessToken":"t2-much-longer-refreshed-token","refreshToken":"r2"}}"#,
    );
    assert!(before == probe(&w), "a refresh is not a new account");
}

#[test]
fn credentials_appearing_or_disappearing_is_a_change() {
    let e = env();
    let w = e.profile("a");
    let with = probe(&w);
    fs::remove_file(e.dir("a").join(".credentials.json")).unwrap();
    let without = probe(&w);
    assert!(with != without, "removed");
    private(&e.dir("a").join(".credentials.json"), "{}");
    assert!(without != probe(&w), "appeared");
}

#[test]
fn a_new_account_uuid_is_a_change() {
    let e = env();
    let w = e.profile("a");
    let before = probe(&w);
    private(
        &e.dir("a").join(".claude.json"),
        &account(Some("uuid-other"), "x@example.invalid", 0),
    );
    assert!(before != probe(&w));
}

#[test]
fn the_email_is_used_when_there_is_no_uuid() {
    let e = env();
    let w = e.profile("a");
    private(
        &e.dir("a").join(".claude.json"),
        &account(None, "one@example.invalid", 0),
    );
    let before = probe(&w);
    private(
        &e.dir("a").join(".claude.json"),
        &account(None, "two@example.invalid", 0),
    );
    assert!(before != probe(&w));
}

#[test]
fn other_keys_of_claude_json_do_not_count() {
    let e = env();
    let w = e.profile("a");
    let before = probe(&w);
    private(
        &e.dir("a").join(".claude.json"),
        &account(Some("uuid-a"), "x@example.invalid", 500),
    );
    assert!(before == probe(&w), "only the account identity is read");
}

#[test]
fn a_claude_json_larger_than_64_kib_is_still_read() {
    let e = env();
    let w = e.profile("a");
    let big = 200 * 1024;
    private(
        &e.dir("a").join(".claude.json"),
        &account(Some("uuid-1"), "x@example.invalid", big),
    );
    let before = probe(&w);
    private(
        &e.dir("a").join(".claude.json"),
        &account(Some("uuid-2"), "x@example.invalid", big),
    );
    assert!(
        before != probe(&w),
        "a 64 KiB cap would read both as absent"
    );
}

#[test]
fn an_untrusted_claude_json_counts_as_absent() {
    let e = env();
    let w = e.profile("a");
    let path = e.dir("a").join(".claude.json");
    fs::remove_file(&path).unwrap();
    let absent = probe(&w);
    private(&path, &account(Some("uuid-a"), "x@example.invalid", 0));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(absent == probe(&w), "others-writable");
    fs::remove_file(&path).unwrap();
    let target = e.tmp.join("elsewhere.json");
    private(&target, &account(Some("uuid-a"), "x@example.invalid", 0));
    symlink(&target, &path).unwrap();
    assert!(absent == probe(&w), "symlink");
    fs::remove_file(&path).unwrap();
    private(&path, "not json");
    assert!(absent == probe(&w), "unparseable");
}

#[test]
fn home_reads_home_claude_json_not_one_inside_dot_claude() {
    let e = env();
    let w = e.home_watch();
    let before = probe(&w);
    private(
        &e.home.join(".claude/.claude.json"),
        &account(Some("uuid-inner"), "i@example.invalid", 0),
    );
    assert!(
        before == probe(&w),
        "$HOME/.claude/.claude.json is not read"
    );
    private(
        &e.home.join(".claude.json"),
        &account(Some("uuid-new"), "me@example.invalid", 0),
    );
    assert!(before != probe(&w), "$HOME/.claude.json is read");
}

#[test]
fn a_rotation_into_home_claude_is_a_change_for_home() {
    let e = env();
    e.rotator("a");
    let w = e.home_watch();
    let before = probe(&w);
    e.rotator("b");
    assert!(before != probe(&w));
}

#[test]
fn a_rotation_into_home_claude_is_ignored_for_an_inherited_profile() {
    let e = env();
    e.rotator("a");
    let w = e.profile("a");
    let before = probe(&w);
    e.rotator("b");
    assert!(before == probe(&w));
}

#[test]
fn an_untrusted_rotator_ledger_counts_as_absent() {
    let e = env();
    let w = e.home_watch();
    let absent = probe(&w);
    e.rotator("a");
    fs::set_permissions(
        e.store.root().join(".rotator/ledger.json"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(absent == probe(&w));
}

#[test]
fn a_swap_recorded_in_the_auto_switch_ledger_is_a_change_for_that_profile() {
    let e = env();
    e.auto_switch_ledger("a", "a");
    let w = e.profile("a");
    let before = probe(&w);
    e.auto_switch_ledger("a", "b");
    assert!(before != probe(&w));
}
