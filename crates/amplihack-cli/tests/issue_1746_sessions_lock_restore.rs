//! Issue #1746: the archive lock and `amplihack sessions restore`.
//!
//! Contract: docs/reference/sessions-command.md (Lock, Restore, Messages,
//! Exit status). Malicious archives are built here with raw tar headers, so
//! the `tar` crate's own path checks do not hide what restore must refuse.

#![cfg(unix)]

use crate::support::sessions_archive::*;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ID: &str = "4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10";

/// Holds an exclusive `flock` on `<DEST>/.archive.lock` until dropped.
struct Held(#[allow(dead_code)] fs::File);

fn hold_lock(fx: &Fx) -> Held {
    fs::create_dir_all(&fx.dest).unwrap();
    chmod(&fx.dest, 0o700);
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(fx.dest.join(".archive.lock"))
        .unwrap();
    // SAFETY: f is an open descriptor for the life of this call.
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "could not take the test lock");
    Held(f)
}

fn lock_line(fx: &Fx) -> String {
    format!(
        "another sessions archive run is in progress (lock: {})",
        fx.dest.join(".archive.lock").display()
    )
}

/// Archives one session `ID` with two extra files and returns the bodies
/// of every file, by path relative to the project directory.
fn archived(fx: &Fx) -> Vec<(String, Vec<u8>)> {
    fx.session(ID, 10, &["subagents/agent-1.jsonl", "tool-results/r.txt"]);
    let rels = [
        format!("{ID}.jsonl"),
        format!("{ID}/subagents/agent-1.jsonl"),
        format!("{ID}/tool-results/r.txt"),
    ];
    let bodies: Vec<_> = rels
        .iter()
        .map(|r| (r.clone(), fs::read(fx.project_dir().join(r)).unwrap()))
        .collect();
    ok(&fx.apply());
    assert!(!fx.project_dir().join(format!("{ID}.jsonl")).exists());
    bodies
}

fn snapshot(p: &Path) -> (Vec<u8>, i64) {
    let m = fs::metadata(p).unwrap();
    (fs::read(p).unwrap(), m.mtime())
}

// ---------------------------------------------------------------- lock

#[test]
fn apply_with_the_lock_held_changes_nothing_and_exits_1() {
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &[]);
    let _held = hold_lock(&fx);
    let out = fx.apply();
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &lock_line(&fx));
    assert!(text(&out.stdout).is_empty());
    assert!(jsonl.is_file());
    assert!(!fx.manifest().exists());
    assert!(!fx.dest.join(PROJECT).exists());
}

#[test]
fn dry_run_takes_no_lock() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let _held = hold_lock(&fx);
    ok(&fx.archive(&[]));
}

#[test]
fn restore_with_the_lock_held_exits_1() {
    let fx = Fx::new();
    archived(&fx);
    let _held = hold_lock(&fx);
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &lock_line(&fx));
    assert!(!fx.project_dir().join(format!("{ID}.jsonl")).exists());
}

#[test]
fn lock_file_is_private_and_left_on_disk() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    ok(&fx.apply());
    let lock = fx.dest.join(".archive.lock");
    assert!(lock.is_file());
    assert_eq!(mode(&lock), 0o600);
}

// ------------------------------------------------------------- restore

#[test]
fn restore_round_trip_puts_every_file_back_with_fresh_mtimes() {
    let fx = Fx::new();
    let bodies = archived(&fx);
    let tgz = fx.dest.join(PROJECT).join(format!("{ID}.tar.gz"));
    let before_archive = snapshot(&tgz);
    let before_manifest = fs::read(fx.manifest()).unwrap();
    let start = SystemTime::now() - Duration::from_secs(5);

    let out = fx.restore(ID);
    ok(&out);
    assert_stdout(
        &out,
        &format!("restored 3 files to {}", fx.project_dir().display()),
    );
    for (rel, body) in &bodies {
        let p = fx.project_dir().join(rel);
        assert_eq!(&fs::read(&p).unwrap(), body, "{rel}");
        assert_eq!(mode(&p), 0o600, "{rel}");
        assert!(
            fs::metadata(&p).unwrap().modified().unwrap() >= start,
            "{rel} kept the archived mtime"
        );
    }
    assert_eq!(mode(&fx.project_dir().join(ID)), 0o700);
    assert_eq!(snapshot(&tgz), before_archive, "archive changed");
    assert_eq!(
        fs::read(fx.manifest()).unwrap(),
        before_manifest,
        "manifest changed"
    );

    // Restored sessions are new: the next run keeps them.
    let out = fx.archive(&[]);
    assert!(
        text(&out.stdout).starts_with("would archive 0 sessions"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn archive_again_after_restore_gets_a_timestamped_name() {
    let fx = Fx::new();
    archived(&fx);
    ok(&fx.restore(ID));
    let jsonl = fx.project_dir().join(format!("{ID}.jsonl"));
    age(&jsonl, 10);
    for d in [
        fx.project_dir().join(ID).join("subagents/agent-1.jsonl"),
        fx.project_dir().join(ID).join("tool-results/r.txt"),
        fx.project_dir().join(ID).join("subagents"),
        fx.project_dir().join(ID).join("tool-results"),
        fx.project_dir().join(ID),
    ] {
        age(&d, 10);
    }
    ok(&fx.apply());
    let lines = fx.manifest_lines();
    assert_eq!(lines.len(), 2);
    let second = lines[1]["archive"].as_str().unwrap();
    assert_ne!(second, format!("{PROJECT}/{ID}.tar.gz"));
    assert!(fx.dest.join(PROJECT).join(format!("{ID}.tar.gz")).is_file());
    assert!(fx.dest.join(second).is_file());
    // Restore uses the last manifest line.
    ok(&fx.restore(ID));
}

#[test]
fn restore_never_overwrites_and_reports_each_conflict() {
    let fx = Fx::new();
    archived(&fx);
    let jsonl = fx.project_dir().join(format!("{ID}.jsonl"));
    fs::write(&jsonl, b"newer local copy").unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!("conflict: {} already exists; kept", jsonl.display()),
    );
    assert_stdout(
        &out,
        &format!(
            "restored 2 files to {}; 1 conflict kept",
            fx.project_dir().display()
        ),
    );
    assert_eq!(fs::read(&jsonl).unwrap(), b"newer local copy");
    assert!(
        fx.project_dir()
            .join(ID)
            .join("tool-results/r.txt")
            .is_file()
    );
}

#[test]
fn restore_plural_conflicts_and_non_directory_in_the_way() {
    let fx = Fx::new();
    archived(&fx);
    fs::write(fx.project_dir().join(format!("{ID}.jsonl")), b"x").unwrap();
    fs::create_dir_all(fx.project_dir().join(ID)).unwrap();
    // A file where the archive has a directory: the subtree is kept.
    fs::write(fx.project_dir().join(ID).join("subagents"), b"x").unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stdout(
        &out,
        &format!(
            "restored 1 files to {}; 2 conflicts kept",
            fx.project_dir().display()
        ),
    );
}

#[test]
fn restore_unknown_id_and_missing_manifest_say_not_found() {
    let fx = Fx::new();
    fs::create_dir_all(&fx.dest).unwrap();
    chmod(&fx.dest, 0o700);
    let line = format!("session {ID} not found in {}", fx.manifest().display());
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &line);

    archived(&fx);
    let other = "0000aaaa-0000-0000-0000-000000000000";
    let out = fx.restore(other);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!("session {other} not found in {}", fx.manifest().display()),
    );
}

#[test]
fn restore_never_creates_a_missing_dest() {
    let fx = Fx::new();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!("cannot prepare {} (not found)", fx.dest.display()),
    );
    assert!(!fx.dest.exists());
}

#[test]
fn restore_missing_archive_file() {
    let fx = Fx::new();
    archived(&fx);
    let tgz = fx.dest.join(PROJECT).join(format!("{ID}.tar.gz"));
    fs::remove_file(&tgz).unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &format!("archive {} not found", tgz.display()));
}

#[test]
fn restore_refuses_a_symlinked_project_directory() {
    let fx = Fx::new();
    archived(&fx);
    let pdir = fx.project_dir();
    fs::remove_dir_all(&pdir).unwrap();
    let elsewhere = fx.base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &pdir).unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &format!("refused {}: symlink", pdir.display()));
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn restore_rejects_bad_ids_without_reading_anything() {
    let fx = Fx::new();
    archived(&fx);
    for bad in ["../x", ".hidden", "a/b", "bad\u{1}id"] {
        let out = fx.restore(bad);
        assert_eq!(exit_code(&out), 1, "{bad:?}");
        assert!(text(&out.stdout).is_empty());
        assert!(!text(&out.stderr).contains('\u{1}'));
    }
}

#[test]
fn restore_reads_a_manifest_past_64_mib() {
    // Review of #1766: a 64 MiB cap on the manifest ended every restore once
    // the daily run had grown it that far. Here the session's line comes
    // after 65 MiB of filler (sparse, one over-long line), and restore still
    // finds it.
    let fx = Fx::new();
    let bodies = archived(&fx);
    let line = fs::read(fx.manifest()).unwrap();
    let f = fs::OpenOptions::new()
        .write(true)
        .open(fx.manifest())
        .unwrap();
    f.set_len(0).unwrap();
    f.set_len(65 * 1024 * 1024).unwrap();
    drop(f);
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(fx.manifest())
        .unwrap();
    f.write_all(b"\n").unwrap();
    f.write_all(&line).unwrap();
    drop(f);
    assert!(fs::metadata(fx.manifest()).unwrap().len() > 65 * 1024 * 1024);
    let out = fx.restore(ID);
    ok(&out);
    for (rel, body) in &bodies {
        assert_eq!(
            &fs::read(fx.project_dir().join(rel)).unwrap(),
            body,
            "{rel}"
        );
    }
}

#[test]
fn restore_skips_corrupt_manifest_lines_and_last_line_wins() {
    let fx = Fx::new();
    archived(&fx);
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(fx.manifest())
        .unwrap();
    writeln!(f, "{{not json").unwrap();
    writeln!(
        f,
        r#"{{"time":"2026-10-29T12:00:00Z","session_id":"{ID}","project":"{PROJECT}","bytes":1,"archive":"{PROJECT}/missing.tar.gz","parts":1,"extra":true}}"#
    )
    .unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "archive {} not found",
            fx.dest.join(PROJECT).join("missing.tar.gz").display()
        ),
    );
}

// ----------------------------------------------- hostile manifests / archives

/// Appends a manifest line for `ID` naming `archive` in `project`.
fn manifest_line(fx: &Fx, project: &str, archive: &str, parts: u64, bytes: u64) {
    fs::create_dir_all(&fx.dest).unwrap();
    chmod(&fx.dest, 0o700);
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(fx.manifest())
        .unwrap();
    writeln!(
        f,
        "{}",
        serde_json::json!({
            "time": "2026-10-29T12:00:00Z", "session_id": ID, "project": project,
            "bytes": bytes, "archive": archive, "parts": parts
        })
    )
    .unwrap();
    chmod(&fx.manifest(), 0o600);
}

#[test]
fn restore_refuses_bad_manifest_fields() {
    let fx = Fx::new();
    let cases = [
        (PROJECT, "/etc/passwd.tar.gz"),
        (PROJECT, &format!("{PROJECT}/../{ID}.tar.gz") as &str),
        (PROJECT, &format!("-other/{ID}.tar.gz") as &str),
        (PROJECT, &format!("{PROJECT}/{ID}.zip") as &str),
        ("../escape", &format!("../escape/{ID}.tar.gz") as &str),
        ("a/b", &format!("a/b/{ID}.tar.gz") as &str),
        (".hidden", &format!(".hidden/{ID}.tar.gz") as &str),
    ];
    for (project, archive) in cases {
        let _ = fs::remove_file(fx.manifest());
        manifest_line(&fx, project, archive, 1, 1);
        let out = fx.restore(ID);
        assert_eq!(exit_code(&out), 1, "{project} {archive}");
        assert_stderr_line(&out, &format!("refused session {ID}: bad manifest line"));
    }
    assert_eq!(
        fs::read_dir(fx.projects()).unwrap().count(),
        0,
        "something was written"
    );
}

/// One raw tar member: `name` bytes as given, no path checks.
struct Member<'a> {
    name: &'a str,
    kind: tar::EntryType,
    data: &'a [u8],
    link: Option<&'a str>,
}

fn file<'a>(name: &'a str, data: &'a [u8]) -> Member<'a> {
    Member {
        name,
        kind: tar::EntryType::Regular,
        data,
        link: None,
    }
}

fn dir(name: &str) -> Member<'_> {
    Member {
        name,
        kind: tar::EntryType::Directory,
        data: b"",
        link: None,
    }
}

/// Writes `<DEST>/<PROJECT>/<ID>.tar.gz` from raw members, plus a manifest
/// line claiming `parts` and `bytes`.
fn hostile(fx: &Fx, members: &[Member], parts: u64, bytes: u64) -> PathBuf {
    let pdest = fx.dest.join(PROJECT);
    fs::create_dir_all(&pdest).unwrap();
    chmod(&pdest, 0o700);
    let tgz = pdest.join(format!("{ID}.tar.gz"));
    let gz =
        flate2::write::GzEncoder::new(fs::File::create(&tgz).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for m in members {
        let mut h = tar::Header::new_old();
        {
            let old = h.as_old_mut();
            old.name.fill(0);
            old.name[..m.name.len()].copy_from_slice(m.name.as_bytes());
            if let Some(l) = m.link {
                old.linkname.fill(0);
                old.linkname[..l.len()].copy_from_slice(l.as_bytes());
            }
        }
        h.set_entry_type(m.kind);
        h.set_mode(0o600);
        h.set_size(m.data.len() as u64);
        h.set_cksum();
        b.append(&h, m.data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap();
    chmod(&tgz, 0o600);
    manifest_line(fx, PROJECT, &format!("{PROJECT}/{ID}.tar.gz"), parts, bytes);
    tgz
}

/// Nothing was created under the projects root or next to it.
fn assert_nothing_written(fx: &Fx) {
    assert!(!fx.project_dir().exists(), "project directory created");
    assert!(
        !fx.base.join("x").exists()
            && !fx.home.join("x").exists()
            && !fx.projects().join("x").exists()
    );
}

#[test]
fn restore_refuses_unsafe_member_paths_and_writes_nothing() {
    let jsonl = format!("{ID}.jsonl");
    let up = format!("{ID}/../../x");
    let other = "0000aaaa-0000-0000-0000-000000000000.jsonl";
    for bad in ["/etc/x", "../x", up.as_str(), other, "x"] {
        let fx = Fx::new();
        let tgz = hostile(&fx, &[file(&jsonl, b"ok"), file(bad, b"evil")], 2, 6);
        let out = fx.restore(ID);
        assert_eq!(exit_code(&out), 1, "{bad}");
        assert_stderr_line(
            &out,
            &format!("refused {}: bad member {bad}", tgz.display()),
        );
        assert_nothing_written(&fx);
    }
}

#[test]
fn restore_refuses_links_and_special_members() {
    let jsonl = format!("{ID}.jsonl");
    let member = format!("{ID}/m");
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::Fifo,
        tar::EntryType::Char,
        tar::EntryType::Block,
    ] {
        let fx = Fx::new();
        let link = Member {
            name: &member,
            kind,
            data: b"",
            link: Some("/etc/passwd"),
        };
        let tgz = hostile(&fx, &[file(&jsonl, b"ok"), link], 2, 2);
        let out = fx.restore(ID);
        assert_eq!(exit_code(&out), 1, "{kind:?}");
        assert_stderr_line(
            &out,
            &format!("refused {}: bad member {member}", tgz.display()),
        );
        assert_nothing_written(&fx);
    }
}

#[test]
fn restore_refuses_pax_path_that_escapes() {
    let fx = Fx::new();
    let pdest = fx.dest.join(PROJECT);
    fs::create_dir_all(&pdest).unwrap();
    chmod(&pdest, 0o700);
    let tgz = pdest.join(format!("{ID}.tar.gz"));
    let gz =
        flate2::write::GzEncoder::new(fs::File::create(&tgz).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    // A PAX header whose `path` overrides an innocent-looking name.
    b.append_pax_extensions([("path", b"../x".as_slice())])
        .unwrap();
    let mut h = tar::Header::new_ustar();
    h.set_path(format!("{ID}.jsonl")).unwrap();
    h.set_size(2);
    h.set_mode(0o600);
    h.set_cksum();
    b.append(&h, &b"ok"[..]).unwrap();
    b.into_inner().unwrap().finish().unwrap();
    manifest_line(&fx, PROJECT, &format!("{PROJECT}/{ID}.tar.gz"), 1, 2);
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &format!("refused {}: bad member ../x", tgz.display()));
    assert_nothing_written(&fx);
}

#[test]
fn restore_refuses_parts_or_bytes_mismatch_before_writing() {
    let jsonl = format!("{ID}.jsonl");
    let sub = ID.to_string();
    let inner = format!("{ID}/a.txt");
    for (parts, bytes) in [(2, 5), (3, 4), (4, 5), (3, 6)] {
        let fx = Fx::new();
        let tgz = hostile(
            &fx,
            &[file(&jsonl, b"ab"), dir(&sub), file(&inner, b"cde")],
            parts,
            bytes,
        );
        let out = fx.restore(ID);
        assert_eq!(exit_code(&out), 1, "{parts} {bytes}");
        assert_stderr_line(
            &out,
            &format!("refused {}: member count or size mismatch", tgz.display()),
        );
        assert_nothing_written(&fx);
    }
    // The matching numbers restore.
    let fx = Fx::new();
    hostile(
        &fx,
        &[file(&jsonl, b"ab"), dir(&sub), file(&inner, b"cde")],
        3,
        5,
    );
    let out = fx.restore(ID);
    ok(&out);
    assert_stdout(
        &out,
        &format!("restored 2 files to {}", fx.project_dir().display()),
    );
}

#[test]
fn restore_of_a_file_that_is_not_a_gzip_tar_writes_nothing() {
    let fx = Fx::new();
    let jsonl = format!("{ID}.jsonl");
    let tgz = hostile(&fx, &[file(&jsonl, b"ok")], 1, 2);
    fs::write(&tgz, b"this is not a gzip stream").unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_nothing_written(&fx);
}

#[test]
fn restore_refuses_a_symlinked_archive() {
    let fx = Fx::new();
    let jsonl = format!("{ID}.jsonl");
    let tgz = hostile(&fx, &[file(&jsonl, b"ok")], 1, 2);
    let real = fx.base.join("real.tar.gz");
    fs::rename(&tgz, &real).unwrap();
    std::os::unix::fs::symlink(&real, &tgz).unwrap();
    let out = fx.restore(ID);
    assert_eq!(exit_code(&out), 1);
    assert_nothing_written(&fx);
}
