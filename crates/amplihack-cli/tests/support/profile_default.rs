//! Sets `<root>/.default` directly, as `profile switch NAME` did before
//! issue #1691. Since #1691 `switch NAME` also moves the live login and pins
//! rotation, so a test that only needs a default pointer writes it here.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Writes `<name>\n` to `<root>/.default`, mode 0600 on Unix, the way
/// `pointer::write` does. The profile directory must already exist.
pub fn set_default(root: &Path, name: &str) {
    assert!(
        root.join(name).is_dir(),
        "profile {name} must exist before it is made the default"
    );
    let path = root.join(".default");
    fs::write(&path, format!("{name}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// Gives `profile switch NAME` (issue #1691) a login to move: `live`'s
/// login in `<home>/.claude` and a stored login in each `stored` profile.
/// Each profile's `.claude.json`, and `~/.claude.json` for `live`, name the
/// profile's own account, so the rotator identifies the live login. Login
/// files hold a placeholder, never a token. Every profile must exist.
#[cfg(unix)]
pub fn give_logins(home: &Path, root: &Path, live: &str, stored: &[&str]) {
    use std::os::unix::fs::PermissionsExt;
    let private = |p: &Path, body: &str| {
        fs::write(p, body).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    };
    let account = |name: &str| {
        format!(
            r#"{{"oauthAccount":{{"accountUuid":"uuid-{name}","emailAddress":"{name}@example.com"}}}}"#
        )
    };
    // An OAuth login by key name (issue #1700): the launch guard and the
    // swap check read a login file's top-level keys for its kind.
    const LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"PLACEHOLDER"}}"#;
    let dot_claude = home.join(".claude");
    fs::create_dir_all(&dot_claude).unwrap();
    fs::set_permissions(&dot_claude, fs::Permissions::from_mode(0o700)).unwrap();
    private(&dot_claude.join(".credentials.json"), LOGIN);
    private(&home.join(".claude.json"), &account(live));
    for name in std::iter::once(live).chain(stored.iter().copied()) {
        assert!(root.join(name).is_dir(), "profile {name} must exist");
        private(&root.join(name).join(".claude.json"), &account(name));
    }
    for name in stored {
        private(&root.join(name).join(".credentials.json"), LOGIN);
    }
}
