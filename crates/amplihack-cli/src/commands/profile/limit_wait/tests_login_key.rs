//! Issue #2731: the **login key**, [`Watch::login`]. It names the login a
//! `$HOME/.claude` step would run on now, so a step can tell one wait from
//! the next and the recipe log can say which account is waiting.
//!
//! It is not the fingerprint. The fingerprint says when a wait ends and has
//! no readable form; the key is a profile name read from the files the
//! rotator writes under its own lock: the rotate ledger's `live`, else the
//! swap record's `live_login`. It is never read from `.claude.json`, whose
//! account field may hold an e-mail address or a UUID.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_login_key;`.

use super::fingerprint::Watch;
use super::tests_fingerprint::{Env, env, private, rotate_ledger};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A swap record naming `live` as the login now live, written as the rotator
/// writes it: private, in `<root>/.auto-switch.d/rotate/swap.json`.
fn swap_record(root: &Path, live: &str, from: &str) {
    let now = super::super::fsx::now();
    private(
        &root.join(".auto-switch.d").join("rotate").join("swap.json"),
        &format!(
            r#"{{"seq":3,"live_login":"{live}","from_login":"{from}","swapped_at":{now},
               "reading":{{"session":12,"week":40,"measured_at":{now}}}}}"#
        ),
    );
}

fn home(e: &Env) -> Watch {
    e.home_watch()
}

// --- which file the key comes from ---------------------------------------

#[test]
fn the_rotate_ledgers_live_profile_is_the_key() {
    let e = env();
    e.rotator("a");
    assert_eq!(home(&e).login().as_deref(), Some("a"));
    e.rotator("b");
    assert_eq!(
        home(&e).login().as_deref(),
        Some("b"),
        "the key is read now, never cached"
    );
}

#[test]
fn the_swap_record_is_the_fallback_when_the_ledger_names_no_live_profile() {
    let e = env();
    swap_record(e.store.root(), "b", "a");
    assert_eq!(
        home(&e).login().as_deref(),
        Some("b"),
        "no ledger at all: the swap record's live login"
    );
    e.rotator("a");
    assert_eq!(
        home(&e).login().as_deref(),
        Some("a"),
        "the ledger outranks the swap record"
    );
}

#[test]
fn an_unreadable_ledger_falls_back_to_the_swap_record() {
    let e = env();
    e.rotator("a");
    // World-writable: `read_ledger` refuses it, as the limit-resume hook does.
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o622),
    )
    .unwrap();
    swap_record(e.store.root(), "b", "a");
    assert_eq!(home(&e).login().as_deref(), Some("b"));
}

// --- when there is no key ------------------------------------------------

#[test]
fn a_step_with_no_rotator_files_has_no_key() {
    let e = env();
    assert_eq!(
        home(&e).login(),
        None,
        "nothing the rotator writes exists yet"
    );
}

#[test]
fn only_a_home_step_has_a_key() {
    let e = env();
    e.rotator("a");
    swap_record(e.store.root(), "a", "b");
    assert_eq!(
        e.profile("a").login(),
        None,
        "an inherited profile is the login the launch chose"
    );
    let config_dir = e.tmp.join("elsewhere");
    fs::create_dir_all(&config_dir).unwrap();
    let w = Watch::new(Some(&e.store), None, Some(&config_dir), Some(&e.home));
    assert!(!w.is_home());
    assert_eq!(w.login(), None, "an explicit CLAUDE_CONFIG_DIR has no key");
}

#[test]
fn a_home_step_with_no_profile_store_has_no_key() {
    let e = env();
    e.rotator("a");
    let w = Watch::new(None, None, None, Some(&e.home));
    assert!(w.is_home(), "it is still a $HOME/.claude step");
    assert_eq!(
        w.login(),
        None,
        "without a store there is no root to read the rotator's files under"
    );
}

#[test]
fn a_name_the_rotator_would_not_write_is_not_a_key() {
    let e = env();
    // `valid_live_name` refuses it, so the ledger reader drops `live`.
    private(
        &rotate_ledger(e.store.root()),
        r#"{"version":1,"live":"../escape","alert":null,"last_cycle":{"at":1,"outcome":"stayed"}}"#,
    );
    assert_eq!(home(&e).login(), None);
}

// --- what the key must never be ------------------------------------------

#[test]
fn the_key_never_comes_from_claude_json() {
    let e = env();
    // `~/.claude.json` holds an e-mail address and a UUID; neither may leak
    // into the key, which reaches the recipe log as `account=`.
    assert_eq!(home(&e).login(), None);
    e.rotator("a");
    let key = home(&e).login().unwrap();
    assert_eq!(key, "a");
    for forbidden in ["@", "uuid-", "example.invalid"] {
        assert!(!key.contains(forbidden), "{forbidden:?} in {key:?}");
    }
}

#[test]
fn the_key_is_a_bare_name_with_no_path_and_no_separator() {
    let e = env();
    e.rotator("a");
    let key = home(&e).login().unwrap();
    assert!(
        !key.is_empty() && key.len() <= 64,
        "the rotator's own name form: {key:?}"
    );
    assert!(
        key.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
        "{key:?} must need no quoting in a log line"
    );
    assert!(!key.contains('/') && !key.contains('='), "{key:?}");
}
