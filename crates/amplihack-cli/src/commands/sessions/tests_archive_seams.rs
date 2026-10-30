//! `sessions::archive` internals (issue #1746): the write and read-back
//! steps that decide whether originals may be removed. A failure here must
//! name a fixed reason and leave the session to be kept.

use super::{verify, write_tmp};
use crate::commands::sessions::scan::describe;
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const ID: &str = "abc";

struct T {
    _tmp: tempfile::TempDir,
    pdir: PathBuf,
    out: PathBuf,
}

/// `-p/abc.jsonl`, `-p/abc/sub/a`, `-p/abc/b`.
fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let pdir = tmp.path().join("-p");
    fs::create_dir_all(pdir.join("abc/sub")).unwrap();
    fs::write(pdir.join("abc.jsonl"), b"12345").unwrap();
    fs::write(pdir.join("abc/sub/a"), b"123").unwrap();
    fs::write(pdir.join("abc/b"), b"12").unwrap();
    let out = tmp.path().join(".abc.tar.gz.tmp-1");
    T {
        _tmp: tmp,
        pdir,
        out,
    }
}

fn session(pdir: &Path) -> crate::commands::sessions::scan::Session {
    describe(pdir, "-p", ID).unwrap()
}

#[test]
fn written_archive_verifies() {
    let t = setup();
    let s = session(&t.pdir);
    write_tmp(&t.out, &t.pdir, &s).unwrap();
    verify(&t.out, &s).unwrap();
}

#[test]
fn truncated_gzip_trailer_is_a_verify_mismatch() {
    let t = setup();
    let s = session(&t.pdir);
    write_tmp(&t.out, &t.pdir, &s).unwrap();
    let len = fs::metadata(&t.out).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&t.out)
        .unwrap()
        .set_len(len - 4)
        .unwrap();
    assert_eq!(verify(&t.out, &s).unwrap_err(), "verify mismatch");
}

#[test]
fn corrupt_crc_is_a_verify_mismatch() {
    let t = setup();
    let s = session(&t.pdir);
    write_tmp(&t.out, &t.pdir, &s).unwrap();
    let mut raw = fs::read(&t.out).unwrap();
    let crc = raw.len() - 8;
    raw[crc] ^= 0xff;
    fs::File::create(&t.out).unwrap().write_all(&raw).unwrap();
    assert_eq!(verify(&t.out, &s).unwrap_err(), "verify mismatch");
}

#[test]
fn plan_that_differs_from_the_archive_is_a_verify_mismatch() {
    let t = setup();
    let s = session(&t.pdir);
    write_tmp(&t.out, &t.pdir, &s).unwrap();
    let mut more = s.clone();
    more.bytes += 1;
    assert_eq!(verify(&t.out, &more).unwrap_err(), "verify mismatch");
    let mut fewer = s.clone();
    fewer.fingerprint.pop();
    fewer.parts -= 1;
    assert_eq!(verify(&t.out, &fewer).unwrap_err(), "verify mismatch");
    let mut renamed = s.clone();
    renamed.fingerprint[0].rel = PathBuf::from("other.jsonl");
    assert_eq!(verify(&t.out, &renamed).unwrap_err(), "verify mismatch");
}

#[test]
fn file_swapped_for_a_symlink_is_not_followed() {
    let t = setup();
    let s = session(&t.pdir);
    let secret = t.pdir.parent().unwrap().join("secret");
    fs::write(&secret, b"12").unwrap();
    fs::remove_file(t.pdir.join("abc/b")).unwrap();
    symlink(&secret, t.pdir.join("abc/b")).unwrap();
    assert_eq!(
        write_tmp(&t.out, &t.pdir, &s).unwrap_err(),
        "changed during archive"
    );
}

#[test]
fn file_replaced_or_resized_is_a_change() {
    let t = setup();
    let s = session(&t.pdir);
    fs::write(t.pdir.join("abc/sub/a"), b"1234").unwrap();
    assert_eq!(
        write_tmp(&t.out, &t.pdir, &s).unwrap_err(),
        "changed during archive"
    );
    let t = setup();
    let s = session(&t.pdir);
    fs::remove_file(t.pdir.join("abc/b")).unwrap();
    fs::write(t.pdir.join("abc/b"), b"12").unwrap();
    // Same size; the inode number may be reused, so give it another mtime.
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::open(t.pdir.join("abc/b"))
        .unwrap()
        .set_modified(earlier)
        .unwrap();
    let out = t.out.with_extension("2");
    assert_eq!(
        write_tmp(&out, &t.pdir, &s).unwrap_err(),
        "changed during archive"
    );
}

#[test]
fn existing_temporary_name_is_never_reused() {
    let t = setup();
    let s = session(&t.pdir);
    fs::write(&t.out, b"other").unwrap();
    assert_eq!(
        write_tmp(&t.out, &t.pdir, &s).unwrap_err(),
        "write failed (already exists)"
    );
    assert_eq!(fs::read(&t.out).unwrap(), b"other");
}
