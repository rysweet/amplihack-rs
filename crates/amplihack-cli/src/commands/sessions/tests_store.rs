//! `sessions::store` (issue #1746): DEST, its lock, its manifest.
//!
//! Contract:
//! - `contains_or_overlaps(dest: &Path, projects_root: &Path) -> bool`: true
//!   when DEST equals, lies inside, or contains the projects root, compared
//!   through the canonical form of each path's deepest existing ancestor.
//!   Never creates anything.
//! - `Dest::new(root: PathBuf)`; `ensure()` creates it 0700;
//!   `check_private() -> io::Result<bool>` is true for a directory owned by
//!   the euid with no group/other write bit (a symlinked DEST is checked at
//!   its target); `lock() -> Result<Lock, LockError>` with
//!   `LockError::Held` when another descriptor holds it.
//! - `Entry { time, session_id, project, bytes, archive, parts }` (serde,
//!   unknown fields allowed); `append(&Entry)` writes one 0600 line with
//!   O_APPEND; `lookup(id) -> Result<Option<Entry>, LookupError>` skips
//!   unparseable lines, returns the last match, and refuses (`TooLarge`) a
//!   manifest over 64 MiB without reading it.
//! - `clean_tmp()` removes regular files named `.<id>.tar.gz.tmp-<digits>`
//!   one level below DEST, in real directories only.

use super::store::{Dest, Entry, LockError, LookupError, contains_or_overlaps};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

fn entry(id: &str, archive: &str) -> Entry {
    Entry {
        time: "2026-10-29T12:00:00Z".into(),
        session_id: id.into(),
        project: "-p".into(),
        bytes: 123,
        archive: archive.into(),
        parts: 5,
    }
}

fn private_dest(base: &Path) -> Dest {
    let d = Dest::new(base.join("dest"));
    d.ensure().unwrap();
    d
}

#[test]
fn overlap_equal_inside_and_ancestor() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("home/.claude/projects");
    fs::create_dir_all(&root).unwrap();
    assert!(contains_or_overlaps(&root, &root));
    assert!(
        contains_or_overlaps(&root.join("a/b"), &root),
        "inside, not yet created"
    );
    assert!(
        contains_or_overlaps(&t.path().join("home"), &root),
        "ancestor"
    );
    assert!(contains_or_overlaps(t.path(), &root));
    assert!(!contains_or_overlaps(&t.path().join("home/archive"), &root));
    assert!(!contains_or_overlaps(
        &t.path().join("home/.claude/projects-x"),
        &root
    ));
    assert!(!root.join("a").exists(), "the check created DEST");
}

#[test]
fn overlap_sees_through_symlinks() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("projects");
    fs::create_dir_all(&root).unwrap();
    symlink(&root, t.path().join("alias")).unwrap();
    assert!(contains_or_overlaps(&t.path().join("alias/sub"), &root));
    assert!(contains_or_overlaps(
        &t.path().join("alias/../projects/x"),
        &root
    ));
}

#[test]
fn ensure_creates_dest_private() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    let m = fs::metadata(t.path().join("dest")).unwrap();
    assert_eq!(m.mode() & 0o777, 0o700);
    assert!(d.check_private().unwrap());
}

#[test]
fn group_or_other_writable_dest_is_not_private() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    for mode in [0o770, 0o702, 0o777] {
        fs::set_permissions(t.path().join("dest"), fs::Permissions::from_mode(mode)).unwrap();
        assert!(!d.check_private().unwrap(), "{mode:o}");
    }
}

#[test]
fn symlinked_dest_is_checked_at_its_target() {
    let t = tempfile::tempdir().unwrap();
    let target = t.path().join("real");
    fs::create_dir_all(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&target, t.path().join("link")).unwrap();
    let d = Dest::new(t.path().join("link"));
    assert!(d.check_private().unwrap());
    fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!d.check_private().unwrap());
}

#[test]
fn lock_is_exclusive_and_private() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    let held = d.lock().unwrap();
    assert!(matches!(d.lock(), Err(LockError::Held(_))));
    drop(held);
    let _again = d.lock().unwrap();
    let m = fs::symlink_metadata(t.path().join("dest/.archive.lock")).unwrap();
    assert!(m.is_file());
    assert_eq!(m.mode() & 0o777, 0o600);
}

#[test]
fn lock_file_symlink_is_not_followed() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    let victim = t.path().join("victim");
    fs::write(&victim, b"keep").unwrap();
    symlink(&victim, t.path().join("dest/.archive.lock")).unwrap();
    assert!(d.lock().is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
}

#[test]
fn manifest_round_trip_last_match_wins() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    d.append(&entry("a", "-p/a.tar.gz")).unwrap();
    d.append(&entry("b", "-p/b.tar.gz")).unwrap();
    d.append(&entry("a", "-p/a.20261029T120000Z.tar.gz"))
        .unwrap();
    let m = t.path().join("dest/MANIFEST.jsonl");
    assert_eq!(fs::metadata(&m).unwrap().mode() & 0o777, 0o600);
    let raw = fs::read_to_string(&m).unwrap();
    assert_eq!(raw.lines().count(), 3);
    for l in raw.lines() {
        let v: serde_json::Value = serde_json::from_str(l).unwrap();
        for k in ["time", "session_id", "project", "bytes", "archive", "parts"] {
            assert!(v.get(k).is_some(), "{k} missing in {l}");
        }
    }
    let got = d.lookup("a").unwrap().unwrap();
    assert_eq!(got.archive, "-p/a.20261029T120000Z.tar.gz");
    assert_eq!(d.lookup("b").unwrap().unwrap().archive, "-p/b.tar.gz");
    assert!(d.lookup("c").unwrap().is_none());
}

#[test]
fn lookup_skips_corrupt_lines_and_allows_unknown_fields() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    d.append(&entry("a", "-p/a.tar.gz")).unwrap();
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(t.path().join("dest/MANIFEST.jsonl"))
        .unwrap();
    writeln!(f, "{{broken").unwrap();
    writeln!(f, "\u{0}\u{1}garbage").unwrap();
    writeln!(
        f,
        r#"{{"time":"t","session_id":"a","project":"-p","bytes":1,"archive":"-p/new.tar.gz","parts":1,"future":42}}"#
    )
    .unwrap();
    assert_eq!(d.lookup("a").unwrap().unwrap().archive, "-p/new.tar.gz");
}

#[test]
fn missing_manifest_is_none() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    assert!(d.lookup("a").unwrap().is_none());
}

#[test]
fn manifest_over_64_mib_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    d.append(&entry("a", "-p/a.tar.gz")).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(t.path().join("dest/MANIFEST.jsonl"))
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(d.lookup("a"), Err(LookupError::TooLarge)));
}

#[test]
fn clean_tmp_is_narrow() {
    let t = tempfile::tempdir().unwrap();
    let d = private_dest(t.path());
    let base = t.path().join("dest");
    let p = base.join("-p");
    fs::create_dir_all(p.join("nested")).unwrap();
    let gone = [p.join(".abc.tar.gz.tmp-123"), p.join(".x-y.tar.gz.tmp-9")];
    let kept = [
        p.join("abc.tar.gz.tmp-123"),
        p.join(".abc.tar.gz.tmp-"),
        p.join(".abc.tar.gz.tmp-12a"),
        p.join(".abc.tar.gz"),
        p.join("nested/.abc.tar.gz.tmp-1"),
        base.join(".top.tar.gz.tmp-1"),
    ];
    for f in gone.iter().chain(kept.iter()) {
        fs::write(f, b"x").unwrap();
    }
    fs::create_dir_all(p.join(".dir.tar.gz.tmp-5")).unwrap();
    // A symlinked project directory is not entered.
    let outside = t.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join(".o.tar.gz.tmp-1"), b"x").unwrap();
    symlink(&outside, base.join("-linked")).unwrap();

    d.clean_tmp();
    for f in &gone {
        assert!(!f.exists(), "{} kept", f.display());
    }
    for f in &kept {
        assert!(f.exists(), "{} removed", f.display());
    }
    assert!(p.join(".dir.tar.gz.tmp-5").is_dir());
    assert!(outside.join(".o.tar.gz.tmp-1").exists());
}
