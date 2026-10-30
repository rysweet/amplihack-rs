//! `default_live::detect` (for `profile usage`) and `identify::live` (for a
//! rotate cycle) read the same rotate ledger. These fixtures pin them to the
//! same answer, so the two readers cannot drift apart unnoticed.
//!
//! Where they differ by design, the table says so: `default_live` refuses a
//! ledger profile that has its own login (and warns), while `identify::live`
//! names it and leaves the swap to refuse. Warnings are not compared.

use super::identify::{LiveId, live};
use super::ledger::RotateState;
use crate::commands::profile::default_live::detect;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const OAUTH_LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn acct(uuid: &str) -> String {
    serde_json::json!({"oauthAccount": {"accountUuid": uuid}}).to_string()
}

/// Profiles `a` (no login, the live account `u1`), `b` (no login, another
/// account) and `c` (its own login). `~/.claude` holds `u1`'s login.
fn env() -> (tempfile::TempDir, PathBuf, ProfileStore, RotateState) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("root");
    let store = ProfileStore::new(root.clone());
    for p in ["a", "b", "c"] {
        store.add(&ProfileName::parse(p).unwrap()).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(&root.join("a/.claude.json"), &acct("u1"));
    private(&root.join("b/.claude.json"), &acct("u2"));
    private(&root.join("c/.claude.json"), &acct("u3"));
    // OAuth logins (issue #1700: `{}` is an API-key profile's login file).
    private(&root.join("c/.credentials.json"), OAUTH_LOGIN);
    private(&home.join(".claude/.credentials.json"), OAUTH_LOGIN);
    private(&home.join(".claude.json"), &acct("u1"));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    (tmp, home, store, rs)
}

fn by_default_live(store: &ProfileStore, home: &Path) -> Option<String> {
    detect(store, Some(home))
        .0
        .map(|l| l.name.as_str().to_string())
}

/// A ledger `identify::live` refuses to load names nobody.
fn by_identify(store: &ProfileStore, rs: &RotateState, home: &Path) -> Option<String> {
    match live(store, rs, home) {
        Ok((LiveId::Known(n) | LiveId::HandChanged(n), _)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

#[test]
fn both_readers_name_the_same_live_profile() {
    // (rotate ledger, default_live, identify::live)
    let table: [(&str, Option<&str>, Option<&str>); 5] = [
        (
            r#"{"version":1,"alert":"no-capacity:-"}"#,
            Some("a"),
            Some("a"),
        ),
        (
            r#"{"version":1,"live":null,"account":"u1"}"#,
            Some("a"),
            Some("a"),
        ),
        (
            r#"{"version":1,"live":"b","account":"u1"}"#,
            Some("b"),
            Some("b"),
        ),
        // By design: see the module comment.
        (
            r#"{"version":1,"live":"c","account":"u1"}"#,
            None,
            Some("c"),
        ),
        (r#"{"version":1,"live":"../x","account":"u1"}"#, None, None),
    ];
    for (body, want_default, want_identify) in table {
        let (_tmp, home, store, rs) = env();
        private(
            &store.root().join(".auto-switch.d/rotate/ledger.json"),
            body,
        );
        assert_eq!(
            by_default_live(&store, &home).as_deref(),
            want_default,
            "default_live: {body}"
        );
        assert_eq!(
            by_identify(&store, &rs, &home).as_deref(),
            want_identify,
            "identify::live: {body}"
        );
    }
}
