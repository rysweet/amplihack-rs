//! `sessions::archive` internals (issue #1746): the write and read-back
//! steps that decide whether originals may be removed, and whole runs that
//! reach them. A failure must name a fixed reason, keep the originals and
//! leave no archive, temporary file or manifest line behind.

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

// ------------------------------------------------------------ whole runs

/// No process sets `CLAUDE_CONFIG_DIR`; only `alive` pids are running.
struct Procs {
    alive: Vec<i32>,
}

impl crate::commands::sessions::live::Probe for Procs {
    fn alive(&self, pid: i32) -> bool {
        self.alive.contains(&pid)
    }
    fn config_dirs(&self) -> std::io::Result<Vec<PathBuf>> {
        Ok(Vec::new())
    }
}

/// A HOME with `-p/<id>.jsonl` and `-p/<id>/sub/a` per id, an empty
/// profile root, and DEST not yet created.
struct Run {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    pdir: PathBuf,
    roots: PathBuf,
    dest: PathBuf,
}

impl Run {
    fn new(ids: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let pdir = home.join(".claude/projects/-p");
        for id in ids {
            fs::create_dir_all(pdir.join(id).join("sub")).unwrap();
            fs::write(pdir.join(format!("{id}.jsonl")), format!("log {id}")).unwrap();
            fs::write(pdir.join(id).join("sub/a"), format!("part {id}")).unwrap();
        }
        let roots = tmp.path().join("profiles");
        fs::create_dir_all(&roots).unwrap();
        Run {
            dest: tmp.path().join("dest"),
            _tmp: tmp,
            home,
            pdir,
            roots,
        }
    }

    fn opts(&self) -> crate::commands::sessions::config::Opts {
        crate::commands::sessions::config::Opts {
            home: self.home.clone(),
            days: 0,
            dest: self.dest.clone(),
            apply: true,
        }
    }

    /// `--apply --days 0` through `run_with`; the report and every warning.
    fn go(
        &self,
        probe: &Procs,
        stop: &dyn Fn() -> bool,
        check: &super::Check<'_>,
    ) -> (super::Report, Vec<String>) {
        let mut warned = Vec::new();
        let mut warn = |l: &str| warned.push(l.to_string());
        let report =
            super::run_with(&self.opts(), &self.roots, probe, &mut warn, stop, check).unwrap();
        (report, warned)
    }

    /// Both originals of `id` are present with their bodies.
    fn intact(&self, id: &str) -> bool {
        fs::read(self.pdir.join(format!("{id}.jsonl"))).ok()
            == Some(format!("log {id}").into_bytes())
            && fs::read(self.pdir.join(id).join("sub/a")).ok()
                == Some(format!("part {id}").into_bytes())
    }

    /// Names in `<DEST>/-p` that belong to `id`: its archives and any
    /// temporary file.
    fn archives_of(&self, id: &str) -> Vec<String> {
        let Ok(rd) = fs::read_dir(self.dest.join("-p")) else {
            return Vec::new();
        };
        rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&format!("{id}.")) || n.starts_with(&format!(".{id}.")))
            .collect()
    }

    /// The session ids in the manifest, in order.
    fn manifest_ids(&self) -> Vec<String> {
        fs::read_to_string(self.dest.join("MANIFEST.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["session_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }
}

const NOTHING_RUNS: Procs = Procs { alive: Vec::new() };

#[test]
fn a_set_stop_flag_ends_the_run_before_any_session() {
    let r = Run::new(&[ID]);
    let (report, _) = r.go(&NOTHING_RUNS, &|| true, &super::verify);
    assert_eq!(
        report,
        super::Report {
            stopped: true,
            ..Default::default()
        }
    );
    assert!(r.intact(ID));
    assert!(r.manifest_ids().is_empty());
}

#[test]
fn an_unset_stop_flag_archives_as_usual() {
    let r = Run::new(&[ID]);
    let (report, _) = r.go(&NOTHING_RUNS, &|| false, &super::verify);
    assert_eq!((report.archived, report.failed), (1, 0));
    assert!(!report.stopped);
    assert!(!r.pdir.join("abc.jsonl").exists() && !r.pdir.join(ID).exists());
    assert_eq!(r.manifest_ids(), [ID]);
}

#[test]
fn a_stop_part_way_through_leaves_the_sessions_not_reached_untouched() {
    // `rotate stop` or SIGTERM during the rotator's daily run: the flag is
    // set while the first session is archived (review of #1766).
    let ids = ["s1", "s2", "s3"];
    let r = Run::new(&ids);
    let asked = std::cell::Cell::new(0);
    let stop = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let (report, warned) = r.go(&NOTHING_RUNS, &stop, &super::verify);
    assert_eq!((report.archived, report.failed), (1, 0), "{warned:?}");
    assert!(report.stopped);
    let done = r.manifest_ids();
    assert_eq!(done.len(), 1);
    for id in ids {
        if done.contains(&id.to_string()) {
            assert!(!r.pdir.join(format!("{id}.jsonl")).exists(), "{id}");
            assert_eq!(r.archives_of(id), [format!("{id}.tar.gz")]);
        } else {
            assert!(r.intact(id), "{id} not reached, so untouched");
            assert!(
                r.archives_of(id).is_empty(),
                "{id}: {:?}",
                r.archives_of(id)
            );
        }
    }
}

#[test]
fn a_member_count_mismatch_keeps_the_originals_and_leaves_no_archive() {
    // The issue's test: a simulated mismatch, driven through a whole run.
    // The read-back runs on the real temporary archive of `abc`, against a
    // session that claims one member more than was written.
    let r = Run::new(&[ID, "def"]);
    let check = |tmp: &Path, s: &crate::commands::sessions::scan::Session| {
        let mut s = s.clone();
        if s.id == ID {
            s.parts += 1;
        }
        super::verify(tmp, &s)
    };
    let (report, warned) = r.go(&NOTHING_RUNS, &|| false, &check);
    assert_eq!((report.archived, report.failed), (1, 1), "{warned:?}");
    assert_eq!(warned, ["failed -p/abc: verify mismatch"]);
    assert!(r.intact(ID));
    assert!(r.archives_of(ID).is_empty(), "{:?}", r.archives_of(ID));
    assert_eq!(r.manifest_ids(), ["def"], "the run carried on");
    assert!(!r.pdir.join("def.jsonl").exists());
}

#[test]
fn a_session_resumed_after_the_first_live_check_is_kept() {
    // `claude --resume abc` starts after the run has built its live set
    // and before abc's originals are removed, and has not written yet, so
    // abc's files are unchanged. Its pid file must still keep the session
    // (review of #1766: the set used to be reused for up to 30 s).
    let r = Run::new(&[ID]);
    let sessions = r.home.join(".claude/sessions");
    let check = |tmp: &Path, s: &crate::commands::sessions::scan::Session| {
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("4242.json"),
            br#"{"pid":4242,"sessionId":"abc"}"#,
        )
        .unwrap();
        super::verify(tmp, s)
    };
    let probe = Procs { alive: vec![4242] };
    let (report, warned) = r.go(&probe, &|| false, &check);
    assert_eq!((report.archived, report.failed), (0, 1), "{warned:?}");
    assert_eq!(warned, ["failed -p/abc: now live"]);
    assert!(r.intact(ID));
    assert!(r.archives_of(ID).is_empty(), "{:?}", r.archives_of(ID));
    assert!(r.manifest_ids().is_empty());
}
