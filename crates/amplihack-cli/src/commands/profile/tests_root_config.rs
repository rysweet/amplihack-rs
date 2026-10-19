//! Issue #1592: where the profile root comes from (the variable, then
//! `~/.amplihack/profile-root`, then the default) and how the saved file is
//! read. The environment is passed in through a closure and HOME
//! is a temporary directory, so nothing here reads the real environment or
//! touches the real `~/.amplihack`.

use super::ProfileError;
use super::root_config::{
    PROFILE_ROOT_FILE, ROTATE_ROOT_FILE, RootSource, config_dir, read_saved, resolve_with,
};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

fn env_none(_: &str) -> Option<OsString> {
    None
}

fn env_root(value: &str) -> impl Fn(&str) -> Option<OsString> + '_ {
    move |k| (k == super::ROOT_ENV).then(|| OsString::from(value))
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn saved_file(home: &Path) -> PathBuf {
    config_dir(home).join(PROFILE_ROOT_FILE)
}

/// Writes `bytes` to `~/.amplihack/profile-root` with mode 0600.
fn plant(home: &Path, bytes: &[u8]) -> PathBuf {
    let file = saved_file(home);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    file
}

fn saved_reason(r: Result<impl std::fmt::Debug, ProfileError>) -> String {
    match r {
        Err(ProfileError::InvalidSavedRoot { reason, .. }) => reason,
        other => panic!("expected InvalidSavedRoot, got {other:?}"),
    }
}

#[test]
fn file_names_and_source_words() {
    assert_eq!(PROFILE_ROOT_FILE, "profile-root");
    assert_eq!(ROTATE_ROOT_FILE, "rotate-root");
    assert_eq!(config_dir(Path::new("/h")), Path::new("/h/.amplihack"));
    assert_eq!(RootSource::Env.to_string(), "env");
    assert_eq!(RootSource::Config.to_string(), "config");
    assert_eq!(RootSource::Default.to_string(), "default");
}

#[test]
fn default_root_when_nothing_is_set() {
    let h = home();
    let got = resolve_with(env_none, Some(h.path())).unwrap();
    assert_eq!(
        got,
        (
            h.path().join(".amplihack").join("claude-profiles"),
            RootSource::Default
        )
    );
}

#[test]
fn saved_root_is_used_and_trimmed() {
    let h = home();
    plant(h.path(), b"  /srv/profiles \n");
    let got = resolve_with(env_none, Some(h.path())).unwrap();
    assert_eq!(got, (PathBuf::from("/srv/profiles"), RootSource::Config));
}

#[test]
fn variable_wins_over_the_saved_root() {
    let h = home();
    plant(h.path(), b"/srv/profiles\n");
    let got = resolve_with(env_root("/tmp/scratch"), Some(h.path())).unwrap();
    assert_eq!(got, (PathBuf::from("/tmp/scratch"), RootSource::Env));
}

/// A non-empty variable never opens the saved file, so a damaged one cannot
/// block a command run with the variable set.
#[cfg(unix)]
#[test]
fn variable_wins_without_reading_a_planted_symlink() {
    let h = home();
    fs::create_dir_all(config_dir(h.path())).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", saved_file(h.path())).unwrap();
    let got = resolve_with(env_root("/tmp/scratch"), Some(h.path())).unwrap();
    assert_eq!(got.1, RootSource::Env);
}

#[test]
fn empty_variable_counts_as_unset() {
    let h = home();
    let got = resolve_with(env_root(""), Some(h.path())).unwrap();
    assert_eq!(got.1, RootSource::Default);
    plant(h.path(), b"/srv/profiles\n");
    let got = resolve_with(env_root(""), Some(h.path())).unwrap();
    assert_eq!(got, (PathBuf::from("/srv/profiles"), RootSource::Config));
}

#[test]
fn bad_variable_is_invalid_root_without_fallback() {
    let h = home();
    plant(h.path(), b"/srv/profiles\n");
    for bad in [
        "relative/path",
        "~/x",
        "/tmp/\x1bevil",
        "/home/you/x/../profiles",
    ] {
        assert!(
            matches!(
                resolve_with(env_root(bad), Some(h.path())),
                Err(ProfileError::InvalidRoot(_))
            ),
            "{bad:?} must be InvalidRoot"
        );
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_variable_is_invalid_root() {
    use std::os::unix::ffi::OsStringExt;
    let h = home();
    let env = |k: &str| (k == super::ROOT_ENV).then(|| OsString::from_vec(b"/tmp/\xff".to_vec()));
    assert!(matches!(
        resolve_with(env, Some(h.path())),
        Err(ProfileError::InvalidRoot(_))
    ));
}

#[test]
fn no_home_is_invalid_root_unless_the_variable_is_set() {
    assert!(matches!(
        resolve_with(env_none, None),
        Err(ProfileError::InvalidRoot(_))
    ));
    let got = resolve_with(env_root("/tmp/scratch"), None).unwrap();
    assert_eq!(got.1, RootSource::Env);
}

#[test]
fn a_corrupt_saved_root_never_falls_back_to_the_default() {
    let h = home();
    plant(h.path(), b"relative/path\n");
    assert_eq!(
        saved_reason(resolve_with(env_none, Some(h.path()))),
        "not an absolute path"
    );
}

#[test]
fn saved_root_reasons() {
    let cases: &[(&[u8], &str)] = &[
        (b"", "empty"),
        (b"  \n\t\n", "empty"),
        (b"relative/path\n", "not an absolute path"),
        (b"~/profiles\n", "not an absolute path"),
        (b"/tmp/\xff\xfe\n", "not UTF-8"),
        (b"/tmp/\x1b[2Jx\n", "contains control characters"),
        (b"/srv/a/../b\n", "contains a .. component"),
    ];
    for (bytes, want) in cases {
        let h = home();
        let file = plant(h.path(), bytes);
        assert_eq!(saved_reason(read_saved(&file)), *want, "{bytes:?}");
    }
}

#[test]
fn oversized_saved_root_is_refused() {
    let h = home();
    let mut body = b"/".to_vec();
    body.extend(std::iter::repeat_n(b'a', 4999));
    let file = plant(h.path(), &body);
    assert_eq!(saved_reason(read_saved(&file)), "larger than 4096 bytes");
}

#[test]
fn missing_saved_root_is_none() {
    let h = home();
    assert_eq!(read_saved(&saved_file(h.path())).unwrap(), None);
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let h = home();
    fs::create_dir_all(saved_file(h.path())).unwrap();
    assert_eq!(
        saved_reason(read_saved(&saved_file(h.path()))),
        "not a regular file"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_saved_root_is_not_followed() {
    let h = home();
    let target = plant(h.path(), b"/srv/profiles\n").with_file_name("real");
    fs::rename(saved_file(h.path()), &target).unwrap();
    std::os::unix::fs::symlink(&target, saved_file(h.path())).unwrap();
    assert_eq!(
        saved_reason(read_saved(&saved_file(h.path()))),
        "is a symlink"
    );
}

#[cfg(unix)]
#[test]
fn a_saved_root_others_can_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let h = home();
    for mode in [0o666, 0o620] {
        let file = plant(h.path(), b"/srv/profiles\n");
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(saved_reason(read_saved(&file)), "writable by others");
    }
}

/// The message names the file and the reason, never the contents.
#[test]
fn invalid_saved_root_message_and_hint() {
    let h = home();
    let file = plant(h.path(), b"SECRET-relative\n");
    let e = read_saved(&file).unwrap_err();
    let msg = e.to_string();
    assert_eq!(
        msg,
        format!(
            "saved profile root in {:?} is invalid: not an absolute path",
            file.display().to_string()
        )
    );
    assert!(!msg.contains("SECRET"), "{msg}");
    assert_eq!(
        e.hint(),
        "fix or remove ~/.amplihack/profile-root, or run: amplihack profile root <absolute dir>"
    );
    let inherited = super::inherited_error("work", e);
    assert!(!inherited.to_string().contains("SECRET"), "{inherited}");
}

#[test]
fn invalid_root_hint_names_the_root_command() {
    assert_eq!(
        ProfileError::InvalidRoot("x".into()).hint(),
        "use an absolute path: set AMPLIHACK_CLAUDE_PROFILE_ROOT to one, or run: amplihack profile root <absolute dir>"
    );
}
