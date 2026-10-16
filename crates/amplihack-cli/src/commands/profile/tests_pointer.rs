use super::name::ProfileName;
use super::{ProfileError, pointer};
use std::fs;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn root_with(profiles: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for p in profiles {
        fs::create_dir(d.path().join(p)).unwrap();
    }
    d
}

#[test]
fn missing_pointer_is_none() {
    let d = root_with(&[]);
    assert_eq!(pointer::read(d.path()).unwrap(), None);
}

#[test]
fn write_then_read_roundtrip_with_bare_name() {
    let d = root_with(&["work"]);
    pointer::write(d.path(), &n("work")).unwrap();
    assert_eq!(
        fs::read_to_string(d.path().join(".default"))
            .unwrap()
            .trim_end(),
        "work"
    );
    assert_eq!(pointer::read(d.path()).unwrap(), Some(n("work")));
}

#[test]
fn read_trims_one_trailing_newline() {
    let d = root_with(&["work"]);
    fs::write(d.path().join(".default"), "work\n").unwrap();
    assert_eq!(pointer::read(d.path()).unwrap(), Some(n("work")));
}

#[test]
fn dangling_pointer_errors_without_fallback() {
    let d = root_with(&[]);
    fs::write(d.path().join(".default"), "gone").unwrap();
    assert!(matches!(
        pointer::read(d.path()),
        Err(ProfileError::PointerDangling(_))
    ));
}

#[test]
fn malicious_pointer_content_is_corrupt() {
    for bad in ["../etc", "a/b", "", "..", "a\0b", "two\nlines"] {
        let d = root_with(&[]);
        fs::write(d.path().join(".default"), bad).unwrap();
        assert!(
            matches!(
                pointer::read(d.path()),
                Err(ProfileError::PointerCorrupt(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn oversized_pointer_is_corrupt() {
    let d = root_with(&[]);
    fs::write(d.path().join(".default"), "a".repeat(257)).unwrap();
    assert!(matches!(
        pointer::read(d.path()),
        Err(ProfileError::PointerCorrupt(_))
    ));
}

#[test]
fn clear_removes_pointer_and_is_idempotent() {
    let d = root_with(&["work"]);
    pointer::write(d.path(), &n("work")).unwrap();
    pointer::clear(d.path()).unwrap();
    pointer::clear(d.path()).unwrap();
    assert_eq!(pointer::read(d.path()).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn pointer_is_0600_and_write_does_not_follow_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let d = root_with(&["work"]);
    let victim = d.path().join("victim.txt");
    fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, d.path().join(".default")).unwrap();
    pointer::write(d.path(), &n("work")).unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    let meta = fs::symlink_metadata(d.path().join(".default")).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn write_leaves_no_temp_files() {
    let d = root_with(&["work"]);
    pointer::write(d.path(), &n("work")).unwrap();
    let mut names: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec![".default", "work"]);
}
