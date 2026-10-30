//! `sessions::scan` (issue #1746): finds sessions from metadata only.
//!
//! Contract:
//! - `valid_name(&str) -> bool`: non-empty, no leading `.`, no `/`, NUL or
//!   control character.
//! - `scan(root: &Path) -> io::Result<(Vec<Session>, Vec<(String, String,
//!   Skip)>)>`: sessions and `(project, id, reason)` skips. A missing root
//!   gives two empty lists.
//! - `Session { project, id, jsonl, dir: Option<PathBuf>, bytes, parts,
//!   newest_mtime, .. }`; `Skip::{Symlink, Special, BadName, Unreadable(_)}`.

use super::scan::{Skip, list_sessions, scan, valid_name};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn set_mtime(p: &Path, secs_ago: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs_ago);
    fs::File::open(p)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(t))
        .unwrap();
}

#[test]
fn name_rule() {
    for ok in ["4f1c2a7e-9b3d", "a", "x.y", "ünï"] {
        assert!(valid_name(ok), "{ok:?}");
    }
    for bad in [
        "", ".", "..", ".hidden", "a/b", "a\0b", "a\nb", "a\u{1b}b", "a\u{7f}b",
    ] {
        assert!(!valid_name(bad), "{bad:?}");
    }
}

#[test]
fn missing_root_is_empty() {
    let t = tempfile::tempdir().unwrap();
    let (s, k) = scan(&t.path().join("nope")).unwrap();
    assert!(s.is_empty() && k.is_empty());
}

#[test]
fn session_with_folder_counts_parts_bytes_and_newest_mtime() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(p.join("abc/sub")).unwrap();
    fs::write(p.join("abc.jsonl"), b"12345").unwrap();
    fs::write(p.join("abc/sub/a"), b"123").unwrap();
    fs::write(p.join("abc/b"), b"12").unwrap();
    for f in ["abc.jsonl", "abc/sub/a", "abc/b", "abc/sub", "abc"] {
        set_mtime(&p.join(f), 1000);
    }
    set_mtime(&p.join("abc/sub/a"), 10);
    let (s, k) = scan(t.path()).unwrap();
    assert!(k.is_empty(), "{k:?}");
    assert_eq!(s.len(), 1);
    let s = &s[0];
    assert_eq!((s.project.as_str(), s.id.as_str()), ("-proj", "abc"));
    assert_eq!(s.bytes, 10);
    // .jsonl + abc/ + abc/sub/ + 2 files.
    assert_eq!(s.parts, 5);
    let newest = fs::metadata(p.join("abc/sub/a"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(s.newest_mtime, newest);
    assert!(s.dir.is_some());
}

#[test]
fn directory_without_jsonl_and_other_files_are_not_sessions() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(p.join("memory")).unwrap();
    fs::write(p.join("memory/MEMORY.md"), b"x").unwrap();
    fs::write(p.join("notes.txt"), b"x").unwrap();
    let (s, _) = scan(t.path()).unwrap();
    assert!(s.is_empty(), "{s:?}");
}

#[test]
fn jsonl_without_folder_is_one_part() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("solo.jsonl"), b"1").unwrap();
    let (s, _) = scan(t.path()).unwrap();
    assert_eq!(s[0].parts, 1);
    assert!(s[0].dir.is_none());
}

#[test]
fn symlinks_anywhere_skip_the_whole_session() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(p.join("deep/x")).unwrap();
    fs::write(p.join("deep.jsonl"), b"1").unwrap();
    symlink("/etc/passwd", p.join("deep/x/link")).unwrap();
    fs::write(t.path().join("target"), b"1").unwrap();
    symlink(t.path().join("target"), p.join("linked.jsonl")).unwrap();
    fs::write(p.join("dirlink.jsonl"), b"1").unwrap();
    symlink(t.path(), p.join("dirlink")).unwrap();
    let (s, k) = scan(t.path()).unwrap();
    assert!(s.is_empty(), "{s:?}");
    let mut ids: Vec<_> = k
        .iter()
        .filter(|(_, _, r)| matches!(r, Skip::Symlink))
        .map(|(_, id, _)| id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, ["deep", "dirlink", "linked"]);
}

#[test]
fn fifo_skips_the_session_as_special() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(p.join("f")).unwrap();
    fs::write(p.join("f.jsonl"), b"1").unwrap();
    let c = std::ffi::CString::new(p.join("f/pipe").to_str().unwrap()).unwrap();
    // SAFETY: c is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let (s, k) = scan(t.path()).unwrap();
    assert!(s.is_empty());
    assert!(matches!(k[0].2, Skip::Special), "{k:?}");
}

#[test]
fn symlinked_project_is_skipped_silently() {
    let t = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("s.jsonl"), b"1").unwrap();
    symlink(other.path(), t.path().join("-linked")).unwrap();
    let (s, k) = scan(t.path()).unwrap();
    assert!(s.is_empty() && k.is_empty(), "{s:?} {k:?}");
}

#[test]
fn bad_names_are_skipped() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("-proj");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("bad\u{1}id.jsonl"), b"1").unwrap();
    let (s, k) = scan(t.path()).unwrap();
    assert!(s.is_empty());
    assert!(matches!(k[0].2, Skip::BadName), "{k:?}");
}

#[test]
fn a_listing_error_part_way_fails_the_whole_listing() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("a.jsonl"), b"{}").unwrap();
    let entries =
        fs::read_dir(t.path())
            .unwrap()
            .chain(std::iter::once(Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))));
    let err = list_sessions(entries).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}
