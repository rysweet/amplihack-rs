//! The real adoption helper must inherit env before any login is moved.
//! These checks run on Unix, including Darwin, without a swap or network call.

use super::adopt;
use crate::commands::profile::store::ProfileStore;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

fn private(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn adoption_inherits_only_template_env_and_copies_only_the_account_block() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let shared = home.join(".claude");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(home.join(".amplihack")).unwrap();
    private(
        &shared.join("settings.json"),
        r#"{"env":{"DEFAULT_ONLY":"no"},"language":"en"}"#,
    );
    let template = base.join("template.json");
    private(
        &template,
        r#"{"env":{"XPIA_ENABLED":"false","TEAM":"yes","ANTHROPIC_API_KEY":"SENTINEL"}}"#,
    );
    private(
        &home.join(".amplihack/config.toml"),
        &format!(
            "[profile]\nsettings_template = {:?}\n",
            template.to_str().unwrap()
        ),
    );
    let account =
        json!({"accountUuid": "synthetic-account", "emailAddress": "fixture@example.invalid"});
    private(
        &home.join(".claude.json"),
        &json!({"oauthAccount": account, "primaryApiKey": "SENTINEL", "numStartups": 19})
            .to_string(),
    );
    // The adoption helper leaves authentication handling to the later swap.
    let login = shared.join(".credentials.json");
    private(&login, r#"{"claudeAiOauth":{"accessToken":"SYNTHETIC"}}"#);
    let before = fs::symlink_metadata(&login).unwrap();
    let store = ProfileStore::new(base.join("profiles")).with_shared_assets(shared);

    let name = adopt(&store, &home).unwrap();

    let dir = store.resolve(&name).unwrap();
    let settings: Value =
        serde_json::from_slice(&fs::read(dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(
        settings,
        json!({"env": {"XPIA_ENABLED": "false", "TEAM": "yes"}, "language": "en"})
    );
    let adopted_account: Value =
        serde_json::from_slice(&fs::read(dir.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(adopted_account, json!({"oauthAccount": account}));
    assert!(!dir.join(".credentials.json").exists());
    assert_eq!(
        fs::metadata(dir.join("settings.json")).unwrap().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::symlink_metadata(login).unwrap().ino(), before.ino());
}
