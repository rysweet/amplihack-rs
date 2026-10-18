//! Issue #1592: writing `~/.amplihack/profile-root`, checking the
//! `amplihack profile root <dir>` argument, and the `root:` line. HOME is a
//! temporary directory; nothing here reads or writes the real `~/.amplihack`.

use super::ProfileError;
use super::root_cmd::root_line;
use super::root_config::{
    PROFILE_ROOT_FILE, RootSource, config_dir, read_saved, save, validate_arg,
};
use std::fs;
use std::path::{Path, PathBuf};

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn saved_file(home: &Path) -> PathBuf {
    config_dir(home).join(PROFILE_ROOT_FILE)
}

#[test]
fn save_round_trip_is_atomic_and_private() {
    let h = home();
    let file = saved_file(h.path());
    save(&file, Path::new("/srv/profiles")).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"/srv/profiles\n");
    assert_eq!(
        read_saved(&file).unwrap(),
        Some(PathBuf::from("/srv/profiles"))
    );
    save(&file, Path::new("/srv/other")).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"/srv/other\n");
    let names: Vec<_> = fs::read_dir(config_dir(h.path()))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [PROFILE_ROOT_FILE], "no temporary file is left");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&config_dir(h.path())), 0o700);
    }
}

#[cfg(unix)]
#[test]
fn save_keeps_the_mode_of_an_existing_amplihack_dir() {
    use std::os::unix::fs::PermissionsExt;
    let h = home();
    let dir = config_dir(h.path());
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    save(&saved_file(h.path()), Path::new("/srv/profiles")).unwrap();
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn save_refuses_a_symlink_target() {
    let h = home();
    let elsewhere = h.path().join("elsewhere");
    fs::write(&elsewhere, "untouched").unwrap();
    fs::create_dir_all(config_dir(h.path())).unwrap();
    std::os::unix::fs::symlink(&elsewhere, saved_file(h.path())).unwrap();
    assert!(save(&saved_file(h.path()), Path::new("/srv/profiles")).is_err());
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "untouched");
    let meta = fs::symlink_metadata(saved_file(h.path())).unwrap();
    assert!(meta.file_type().is_symlink());
}

/// A value the reader would refuse is never written, so a bad variable
/// recorded by the first `add` cannot leave a file that blocks every command.
#[test]
fn save_refuses_values_the_reader_would_refuse() {
    let h = home();
    let file = saved_file(h.path());
    for bad in [
        "",
        "relative",
        "/srv/a\nb",
        "/srv/\x1b",
        "/srv/a/../b",
        "/srv/a ",
        " /srv/a",
    ] {
        assert!(save(&file, Path::new(bad)).is_err(), "{bad:?}");
        assert!(!file.exists(), "{bad:?} was written");
    }
}

#[cfg(unix)]
#[test]
fn save_refuses_a_non_utf8_path() {
    use std::os::unix::ffi::OsStrExt;
    let h = home();
    let file = saved_file(h.path());
    let bad = Path::new(std::ffi::OsStr::from_bytes(b"/srv/\xff"));
    assert!(save(&file, bad).is_err());
    assert!(!file.exists());
}

#[test]
fn validate_arg_normalises_one_trailing_slash() {
    let h = home();
    assert_eq!(
        validate_arg("/srv/profiles/", h.path()).unwrap(),
        PathBuf::from("/srv/profiles")
    );
    assert_eq!(validate_arg("/", h.path()).unwrap(), PathBuf::from("/"));
    assert_eq!(
        validate_arg("/srv/profiles", h.path()).unwrap(),
        PathBuf::from("/srv/profiles")
    );
}

#[test]
fn validate_arg_refuses_bad_values_with_invalid_root() {
    let h = home();
    for bad in ["", "~/x", "a/b", "/a/../b", "/a/\x1bb"] {
        assert!(
            matches!(
                validate_arg(bad, h.path()),
                Err(ProfileError::InvalidRoot(_))
            ),
            "{bad:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn validate_arg_refuses_an_existing_symlink() {
    let h = home();
    let real = h.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = h.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(
        validate_arg(link.to_str().unwrap(), h.path()),
        Err(ProfileError::SymlinkRefused(_))
    ));
}

#[test]
fn validate_arg_refuses_a_root_inside_dot_claude() {
    let h = home();
    fs::create_dir_all(h.path().join(".claude")).unwrap();
    for inside in [h.path().join(".claude"), h.path().join(".claude/profiles")] {
        assert!(
            matches!(
                validate_arg(inside.to_str().unwrap(), h.path()),
                Err(ProfileError::SharedSourceContainsRoot(_))
            ),
            "{inside:?}"
        );
    }
}

#[test]
fn validate_arg_neither_creates_nor_requires_the_directory() {
    let h = home();
    let dir = h.path().join("not-yet");
    assert_eq!(validate_arg(dir.to_str().unwrap(), h.path()).unwrap(), dir);
    assert!(!dir.exists());
}

#[test]
fn root_line_shows_path_and_source_escaped() {
    assert_eq!(
        root_line(Path::new("/srv/profiles"), RootSource::Config),
        "root: /srv/profiles (source: config)"
    );
    let line = root_line(Path::new("/srv/\x1b[2Jx"), RootSource::Env);
    assert!(!line.contains('\x1b'), "{line:?}");
    assert!(line.ends_with("(source: env)"), "{line:?}");
}
