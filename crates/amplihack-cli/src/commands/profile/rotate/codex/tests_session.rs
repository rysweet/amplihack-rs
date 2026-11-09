//! `codex::session` (issue #2005): session ids, the `--events` file and the
//! rollout link.
//!
//! Contract:
//! - `valid_session_id(&str) -> bool` for `^[A-Za-z0-9][A-Za-z0-9-]{0,63}$`.
//! - `read_events(path: &Path, refused_under: &[&Path]) ->
//!   Result<EventsScan, ProfileError>`, `EventsScan { session:
//!   Option<String>, limit_hit: bool }`. A non-regular file, a file named
//!   `auth.json` or one under a refused directory is an error, at once.
//! - `ensure_rollout(src_home, dst_home, id) -> io::Result<Rollout>`,
//!   `Rollout::{Linked(PathBuf), Present}` (the path is relative to
//!   `dst_home`); no rollout under `src_home/sessions` is `NotFound`.

use super::session::{Rollout, ensure_rollout, read_events, valid_session_id};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ID: &str = "0199a213-81c0-7800-8aa1-bbab2a035a53";

#[test]
fn session_ids_follow_the_documented_pattern() {
    for ok in [ID, "a", "A1-b2", &"a".repeat(64)] {
        assert!(valid_session_id(ok), "{ok:?}");
    }
    for bad in [
        "",
        "-x",
        "--yolo",
        "a/b",
        "..",
        ".a",
        "a b",
        "a_b",
        "é",
        &"a".repeat(65),
        "a\n",
    ] {
        assert!(!valid_session_id(bad), "{bad:?}");
    }
}

fn write(p: &Path, body: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn read_events_finds_the_first_id_and_the_limit() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ev.jsonl");
    write(
        &p,
        &format!(
            "{{\"type\":\"thread.started\",\"thread_id\":\"{ID}\"}}\n\
             {{\"type\":\"thread.started\",\"thread_id\":\"second\"}}\n\
             not json at all\n\
             {{\"type\":\"error\",\"message\":\"You've hit your usage limit\"}}\n"
        ),
    );
    let s = read_events(&p, &[]).unwrap();
    assert_eq!(s.session.as_deref(), Some(ID));
    assert!(s.limit_hit);
}

#[test]
fn read_events_takes_session_configured_too() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ev.jsonl");
    write(
        &p,
        &format!(
            "{{\"id\":\"0\",\"msg\":{{\"type\":\"session_configured\",\"session_id\":\"{ID}\"}}}}\n"
        ),
    );
    let s = read_events(&p, &[]).unwrap();
    assert_eq!(s.session.as_deref(), Some(ID));
    assert!(!s.limit_hit);
}

#[test]
fn read_events_refuses_fifos_auth_json_and_refused_dirs() {
    let d = tempfile::tempdir().unwrap();
    let fifo = d.path().join("ev.fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(read_events(&fifo, &[]).is_err(), "a FIFO must fail at once");

    let auth = d.path().join("auth.json");
    write(
        &auth,
        &format!("{{\"type\":\"thread.started\",\"thread_id\":\"{ID}\"}}\n"),
    );
    assert!(read_events(&auth, &[]).is_err());

    let root = d.path().join("profiles");
    let under = root.join("a").join("ev.jsonl");
    write(
        &under,
        &format!("{{\"type\":\"thread.started\",\"thread_id\":\"{ID}\"}}\n"),
    );
    assert!(read_events(&under, &[&root]).is_err());
    assert!(read_events(&d.path().join("missing"), &[]).is_err());
    assert!(read_events(d.path(), &[]).is_err(), "a directory");
}

fn rel() -> PathBuf {
    PathBuf::from(format!(
        "sessions/2026/11/08/rollout-2026-11-08T10-00-00-{ID}.jsonl"
    ))
}

#[test]
fn ensure_rollout_links_without_opening_the_source() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("a"), d.path().join("b"));
    write(&src.join(rel()), "secret");
    fs::set_permissions(src.join(rel()), fs::Permissions::from_mode(0o000)).unwrap();
    fs::create_dir_all(&dst).unwrap();
    assert_eq!(
        ensure_rollout(&src, &dst, ID).unwrap(),
        Rollout::Linked(rel())
    );
    let link = dst.join(rel());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(src.join(rel())).unwrap()
    );
    let parent = fs::metadata(link.parent().unwrap()).unwrap();
    assert_eq!(parent.permissions().mode() & 0o777, 0o700);
    // A second call finds it present.
    assert_eq!(ensure_rollout(&src, &dst, ID).unwrap(), Rollout::Present);
}

#[test]
fn ensure_rollout_reports_a_missing_rollout() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("a"), d.path().join("b"));
    fs::create_dir_all(src.join("sessions")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    let e = ensure_rollout(&src, &dst, ID).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    assert!(!dst.join("sessions").exists(), "nothing created");
}

#[test]
fn ensure_rollout_matches_the_id_suffix_exactly() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("a"), d.path().join("b"));
    write(&src.join(format!("sessions/x/rollout-1-x{ID}.jsonl")), "");
    fs::create_dir_all(&dst).unwrap();
    assert_eq!(
        ensure_rollout(&src, &dst, ID).unwrap_err().kind(),
        std::io::ErrorKind::NotFound,
        "`x<id>` is not `-<id>`"
    );
}

#[test]
fn a_source_rollout_that_is_a_planted_symlink_is_not_found() {
    // FS-6 / S2: `x-<id>.jsonl -> ../../auth.json` is never linked.
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("a"), d.path().join("b"));
    write(&src.join("auth.json"), "{}");
    fs::create_dir_all(src.join("sessions")).unwrap();
    std::os::unix::fs::symlink("../auth.json", src.join(format!("sessions/x-{ID}.jsonl"))).unwrap();
    std::os::unix::fs::symlink("../nowhere", src.join(format!("sessions/y-{ID}.jsonl"))).unwrap();
    fs::create_dir_all(&dst).unwrap();
    assert_eq!(
        ensure_rollout(&src, &dst, ID).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(!dst.join("sessions").exists(), "nothing created");
}

#[test]
fn a_real_rollout_beside_a_planted_symlink_is_still_found() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("a"), d.path().join("b"));
    write(&src.join("auth.json"), "{}");
    fs::create_dir_all(src.join("sessions")).unwrap();
    // Sorts before the real one, so it is seen first and skipped.
    std::os::unix::fs::symlink("../auth.json", src.join(format!("sessions/a-{ID}.jsonl"))).unwrap();
    write(&src.join(rel()), "");
    fs::create_dir_all(&dst).unwrap();
    assert_eq!(
        ensure_rollout(&src, &dst, ID).unwrap(),
        Rollout::Linked(rel())
    );
}

#[test]
fn a_rollout_linked_by_an_earlier_resume_can_be_resumed_again() {
    // a -> b -> c: b's rollout is the symlink the first resume made, and it
    // resolves inside another account's `sessions`.
    let d = tempfile::tempdir().unwrap();
    let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
    write(&a.join(rel()), "");
    fs::create_dir_all(&b).unwrap();
    fs::create_dir_all(&c).unwrap();
    assert_eq!(ensure_rollout(&a, &b, ID).unwrap(), Rollout::Linked(rel()));
    assert_eq!(ensure_rollout(&b, &c, ID).unwrap(), Rollout::Linked(rel()));
    assert_eq!(
        fs::canonicalize(c.join(rel())).unwrap(),
        fs::canonicalize(a.join(rel())).unwrap()
    );
}
