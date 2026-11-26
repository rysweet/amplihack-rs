//! A login-less profile whose identity record cannot be read refuses the
//! swap instead of counting as "no match": it may be the profile that holds
//! the live account, and adopting a second profile for it is not safe.

use super::identify::{Account, LiveId, live};
use super::ledger::RotateState;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn work() -> ProfileName {
    ProfileName::parse("work").unwrap()
}

/// Profile `work` holds no login and is recorded for u-work; `~/.claude`
/// holds u-work's login.
fn env() -> (tempfile::TempDir, PathBuf, ProfileStore, RotateState) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("root");
    let store = ProfileStore::new(root.clone());
    store.add(&work()).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let account = serde_json::json!({"oauthAccount": {
        "accountUuid": "u-work", "emailAddress": "work@example.com"}})
    .to_string();
    private(&root.join("work/.claude.json"), &account);
    private(&home.join(".claude.json"), &account);
    let creds = home.join(".claude/.credentials.json");
    private(&creds, &super::tests_fixture_identity::body(&creds));
    fs::set_permissions(creds.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    let state = State::new(&root).unwrap();
    let a = Account::of_dir(&root.join("work")).unwrap();
    crate::commands::profile::expected::record(&state, &work(), &a).unwrap();
    (tmp, home, store, rs)
}

#[test]
fn readable_record_names_the_login_less_profile() {
    let (_tmp, home, store, rs) = env();
    assert_eq!(
        live(&store, &rs, &home).unwrap(),
        (LiveId::Known(work()), None)
    );
}

#[test]
fn unreadable_record_refuses_instead_of_adopting() {
    let (_tmp, home, store, rs) = env();
    let record = store.root().join(".auto-switch.d/expected/work.json");
    assert!(
        record.exists(),
        "fixture record not at {}",
        record.display()
    );
    private(&record, r#"{"account": "u-work"}"#);
    let error = live(&store, &rs, &home).unwrap_err();
    assert!(matches!(error, ProfileError::RotateRefused(_)), "{error}");
    assert!(!store.root().join("work/.credentials.json").exists());
}
