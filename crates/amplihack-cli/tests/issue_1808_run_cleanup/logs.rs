//! Log retention: archive, manifest and restore (R5, D10; AC9).

use super::fixture::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn recipe_logs(fx: &Fx) -> PathBuf {
    let d = fx.home.join(".amplihack/recipe-logs");
    fs::create_dir_all(&d).unwrap();
    d
}

fn old_log(dir: &Path, name: &str, body: &str, days: i64) -> PathBuf {
    let p = dir.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, body).unwrap();
    set_mtime_days_ago(&p, days);
    p
}

fn manifest_lines(archive_dir: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(archive_dir.join("MANIFEST.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn sha256_hex(path: &Path) -> String {
    let out = Command::new("sha256sum").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// Content of the single regular entry of a `.tar.gz`.
fn tar_gz_single_entry(archive: &Path) -> Vec<u8> {
    let list = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{archive:?} is not a readable tar.gz"
    );
    let names: Vec<_> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(names.len(), 1, "one file per archive: {names:?}");
    let out = Command::new("tar")
        .arg("-xzOf")
        .arg(archive)
        .output()
        .unwrap();
    assert!(out.status.success());
    out.stdout
}

/// AC9 (recipe logs): an old log is archived to
/// `<archive>/recipe-logs/<name>.tar.gz` (mode 0600), recorded in
/// `MANIFEST.jsonl`, verified, and only then removed. A recent log stays.
#[test]
fn ac9_old_recipe_log_is_archived_verified_and_removed() {
    let fx = Fx::new();
    let dir = recipe_logs(&fx);
    let body = "line one\nline two\n".repeat(100);
    let log = old_log(&dir, "0d4e2c1a.log", &body, 41);
    let digest = sha256_hex(&log);
    let recent = old_log(&dir, "recent.log", "new", 1);

    let report = fx.cleanup_json(&["--apply"]);
    let e = entry_for(&report, &log);
    assert_eq!(e["category"], "recipe-log");
    assert_eq!(e["action"], "archived", "{e}");

    let archive_dir = fx.work.join("archive");
    let archive = archive_dir.join("recipe-logs/0d4e2c1a.log.tar.gz");
    assert!(archive.is_file(), "{report:#}");
    assert_eq!(mode_of(&archive), 0o600);
    assert_eq!(mode_of(&archive_dir), 0o700);
    assert_eq!(tar_gz_single_entry(&archive), body.as_bytes());
    assert!(!log.exists(), "the original is removed after verification");
    assert!(recent.exists());
    assert!(
        !archive_dir.join("CACHEDIR.TAG").exists(),
        "archives must be backed up"
    );

    let manifest = manifest_lines(&archive_dir);
    assert_eq!(manifest.len(), 1);
    let m = &manifest[0];
    assert_eq!(m["original_path"], log.display().to_string());
    assert_eq!(m["archive_path"], archive.display().to_string());
    assert_eq!(m["size_bytes"], body.len() as u64);
    assert_eq!(m["digest"], format!("sha256:{digest}"));
    assert!(m["archived_at"].as_str().is_some());
    assert_eq!(mode_of(&archive_dir.join("MANIFEST.jsonl")), 0o600);
}

/// `--log-days` and `AMPLIHACK_LOG_ARCHIVE_DAYS` set the age; `--skip-logs`
/// leaves logs alone; `AMPLIHACK_LOG_ARCHIVE_DIR` moves the archive.
#[test]
fn log_age_skip_and_archive_dir_settings_are_honoured() {
    let fx = Fx::new();
    let dir = recipe_logs(&fx);
    let log = old_log(&dir, "ten-days.log", "x", 10);

    let young = fx.cleanup_json(&[]);
    assert!(
        entries(&young)
            .iter()
            .all(|e| e["path"] != log.display().to_string() || e["action"] == "skipped"),
        "a 10-day-old log is not due with the 30-day default: {young:#}"
    );
    let r = fx.cleanup_json(&["--log-days", "7"]);
    assert_eq!(entry_for(&r, &log)["action"], "would-archive");
    let out = fx.cleanup_with(&["--format", "json"], |c| {
        c.env("AMPLIHACK_LOG_ARCHIVE_DAYS", "7");
    });
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entry_for(&r, &log)["action"], "would-archive");

    let r = fx.cleanup_json(&["--apply", "--log-days", "7", "--skip-logs"]);
    assert!(log.exists());
    assert!(entries(&r).iter().all(|e| e["category"] != "recipe-log"));

    let custom = fx.root.join("custom-archive");
    let out = fx.cleanup_with(&["--apply", "--log-days", "7"], |c| {
        c.env("AMPLIHACK_LOG_ARCHIVE_DIR", &custom);
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(custom.join("recipe-logs/ten-days.log.tar.gz").is_file());
    assert!(!fx.work.join("archive").exists());
}

/// AC9 (codex sessions): nested date folders keep their relative path.
#[test]
fn ac9_codex_session_logs_keep_their_nested_path() {
    let fx = Fx::new();
    let sessions = fx.home.join(".codex/sessions");
    let log = old_log(
        &sessions,
        "2026/09/12/rollout-2026-09-12T08-14-03-7c2e.jsonl",
        "{}\n",
        45,
    );
    let report = fx.cleanup_json(&["--apply"]);
    let e = entry_for(&report, &log);
    assert_eq!(e["category"], "session-log");
    assert_eq!(e["action"], "archived", "{e}");
    assert!(
        fx.work
            .join("archive/codex-sessions/2026/09/12/rollout-2026-09-12T08-14-03-7c2e.jsonl.tar.gz")
            .is_file()
    );
    assert!(!log.exists());
}

/// AC9 (Claude sessions, #1766 not merged): listed as skipped with
/// `awaiting sessions archive (#1766)` and left untouched.
#[test]
fn ac9_claude_project_sessions_await_1766() {
    let fx = Fx::new();
    let log = old_log(
        &fx.home.join(".claude/projects/-tmp-proj"),
        "5c1f.jsonl",
        "{}\n",
        90,
    );
    let before = fs::metadata(&log).unwrap().modified().unwrap();
    let report = fx.cleanup_json(&["--apply"]);
    let e = entry_for(&report, &log);
    assert_eq!(e["category"], "session-log");
    assert_eq!(e["action"], "skipped");
    assert_eq!(e["reason"], "awaiting sessions archive (#1766)");
    assert_eq!(fs::metadata(&log).unwrap().modified().unwrap(), before);
}

/// An archive folder that overlaps a log folder is refused; no log is lost.
#[test]
fn archive_dir_inside_log_dir_is_refused() {
    let fx = Fx::new();
    let dir = recipe_logs(&fx);
    let log = old_log(&dir, "old.log", "x", 60);
    let out = fx.cleanup_with(&["--apply"], |c| {
        c.env("AMPLIHACK_LOG_ARCHIVE_DIR", dir.join("archive"));
    });
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(log.exists());
    assert!(!dir.join("archive").exists());
}

fn archived(fx: &Fx, name: &str, body: &str) -> (PathBuf, PathBuf) {
    let log = old_log(&recipe_logs(fx), name, body, 60);
    let out = fx.cleanup(&["--apply", "--only", "recipe-log"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let archive = fx.work.join(format!("archive/recipe-logs/{name}.tar.gz"));
    assert!(archive.is_file() && !log.exists());
    (log, archive)
}

fn restore(fx: &Fx, args: &[&Path]) -> std::process::Output {
    let mut c = fx.amplihack();
    c.args(["cleanup", "restore"]);
    for a in args {
        c.arg(a);
    }
    c.output().unwrap()
}

/// R5: restore by archive path puts the file back at its original path with
/// the original bytes and mode 0600.
#[test]
fn restore_archive_to_original_path() {
    let fx = Fx::new();
    let (log, archive) = archived(&fx, "a.log", "alpha\n");
    let out = restore(&fx, &[&archive]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert_eq!(fs::read_to_string(&log).unwrap(), "alpha\n");
    assert_eq!(mode_of(&log), 0o600);
}

/// R5: restore never overwrites; a conflict exits 1, the other files are
/// still restored.
#[test]
fn restore_never_overwrites_and_reports_conflict() {
    let fx = Fx::new();
    let (a, _) = archived(&fx, "a.log", "alpha\n");
    let (b, _) = archived(&fx, "b.log", "beta\n");
    fs::write(&a, "newer content").unwrap();

    let out = restore(&fx, &[&fx.work.join("archive/MANIFEST.jsonl")]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        fs::read_to_string(&a).unwrap(),
        "newer content",
        "never overwritten"
    );
    assert_eq!(
        fs::read_to_string(&b).unwrap(),
        "beta\n",
        "other files still restored"
    );
}

/// `--dest` restores under another folder, never to the original path.
#[test]
fn restore_manifest_into_dest() {
    let fx = Fx::new();
    let (log, _) = archived(&fx, "c.log", "gamma\n");
    let dest = fx.root.join("restored");
    let mut c = fx.amplihack();
    c.args(["cleanup", "restore"])
        .arg(fx.work.join("archive/MANIFEST.jsonl"))
        .arg("--dest")
        .arg(&dest);
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        !log.exists(),
        "--dest must not restore to the original path"
    );
    let found: Vec<_> = snapshot(&dest)
        .into_iter()
        .filter(|(p, (kind, _, _))| kind == "file" && p.ends_with("c.log"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        fs::read_to_string(dest.join(&found[0].0)).unwrap(),
        "gamma\n"
    );
}

/// An archive with no manifest line is not restored: exit 1 with
/// `no manifest entry for <archive>`.
#[test]
fn restore_without_manifest_entry_fails() {
    let fx = Fx::new();
    let (log, archive) = archived(&fx, "d.log", "delta\n");
    let copy = archive.with_file_name("copy.log.tar.gz");
    fs::copy(&archive, &copy).unwrap();
    let out = restore(&fx, &[&copy]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no manifest entry for"));
    assert!(!log.exists());
}

/// A digest mismatch fails the restore and leaves no file or `.partial`.
#[test]
fn restore_with_wrong_digest_fails_and_leaves_nothing() {
    let fx = Fx::new();
    let (log, archive) = archived(&fx, "e.log", "epsilon\n");
    let manifest = fx.work.join("archive/MANIFEST.jsonl");
    let text = fs::read_to_string(&manifest).unwrap();
    let mut line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    line["digest"] = format!("sha256:{}", "0".repeat(64)).into();
    fs::write(&manifest, format!("{line}\n")).unwrap();

    let out = restore(&fx, &[&archive]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!log.exists());
    let leftovers: Vec<_> = fs::read_dir(log.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(leftovers.is_empty(), "no partial files: {leftovers:?}");
}

/// A manifest line whose `original_path` is outside the log folders is
/// refused (no `--dest`): nothing is written there.
#[test]
fn restore_refuses_destination_outside_log_roots() {
    let fx = Fx::new();
    let (_, archive) = archived(&fx, "f.log", "zeta\n");
    let manifest = fx.work.join("archive/MANIFEST.jsonl");
    let text = fs::read_to_string(&manifest).unwrap();
    let mut line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let evil = fx.home.join(".bashrc");
    line["original_path"] = evil.display().to_string().into();
    fs::write(&manifest, format!("{line}\n")).unwrap();

    let out = restore(&fx, &[&archive]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!evil.exists());

    line["original_path"] = format!("{}/../../.bashrc", recipe_logs(&fx).display()).into();
    fs::write(&manifest, format!("{line}\n")).unwrap();
    let out = restore(&fx, &[&archive]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "`..` components are rejected: {}",
        describe(&out)
    );
    assert!(!evil.exists());
}

/// §8 test 8: a folder inside a log root that has become a symlink (here to
/// a stand-in for `~/.ssh`) is never written through on restore.
#[test]
fn restore_refuses_symlinked_folder_inside_log_root() {
    let fx = Fx::new();
    let logs = recipe_logs(&fx);
    let log = old_log(&logs, "x/authorized_keys", "ssh-ed25519 AAAA test\n", 60);
    let out = fx.cleanup(&["--apply", "--only", "recipe-log"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(!log.exists());

    let ssh = fx.home.join("fake-ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::remove_dir(logs.join("x")).unwrap();
    std::os::unix::fs::symlink(&ssh, logs.join("x")).unwrap();

    let out = restore(&fx, &[&fx.work.join("archive/MANIFEST.jsonl")]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("symbolic link"),
        "{}",
        describe(&out)
    );
    assert_eq!(
        fs::read_dir(&ssh).unwrap().count(),
        0,
        "nothing may be written through the symlink"
    );
}

/// §8 test 9: `--dest` with a `..` component is refused, and so is a
/// manifest whose archive path climbs out of the archive folder.
#[test]
fn restore_refuses_dotdot_in_dest_and_manifest() {
    let fx = Fx::new();
    let (log, archive) = archived(&fx, "g.log", "eta\n");
    let manifest = fx.work.join("archive/MANIFEST.jsonl");
    let escape = fx.root.join("escape");

    let mut c = fx.amplihack();
    c.args(["cleanup", "restore"])
        .arg(&manifest)
        .arg("--dest")
        .arg(fx.root.join("restored/../escape"));
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!escape.exists() && !fx.root.join("restored").exists());

    let text = fs::read_to_string(&manifest).unwrap();
    let mut line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    line["archive_path"] = format!(
        "{}/../../{}",
        archive.parent().unwrap().display(),
        archive.file_name().unwrap().to_string_lossy()
    )
    .into();
    fs::write(&manifest, format!("{line}\n")).unwrap();
    let mut c = fx.amplihack();
    c.args(["cleanup", "restore"])
        .arg(&manifest)
        .arg("--dest")
        .arg(&escape);
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(!log.exists());
    assert!(
        !escape.exists()
            || snapshot(&escape)
                .values()
                .all(|(kind, _, _)| kind != "file"),
        "nothing restored"
    );
}

/// Replaces `archive` with a `.tar.gz` of `entries` built by tar(1) in `dir`.
fn replace_archive(archive: &Path, dir: &Path, entries: &[&str]) {
    let tmp = archive.with_extension("new");
    let out = Command::new("tar")
        .arg("-czf")
        .arg(&tmp)
        .arg("-C")
        .arg(dir)
        .args(entries)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    fs::rename(&tmp, archive).unwrap();
}

/// §8 test 10: an archive holding a symlink entry, two entries, or more bytes
/// than the manifest records is refused, and nothing is restored.
#[test]
fn restore_refuses_malformed_archives() {
    let cases: [(&str, &str); 3] = [
        ("symlink", "not a regular file"),
        ("two", "more than one entry"),
        ("oversized", "larger than its manifest says"),
    ];
    for (case, message) in cases {
        let fx = Fx::new();
        let (log, archive) = archived(&fx, "h.log", "theta\n");
        let src = fx.root.join(format!("craft-{case}"));
        fs::create_dir_all(&src).unwrap();
        match case {
            "symlink" => {
                std::os::unix::fs::symlink("/etc/hostname", src.join("h.log")).unwrap();
                replace_archive(&archive, &src, &["h.log"]);
            }
            "two" => {
                fs::write(src.join("h.log"), "theta\n").unwrap();
                fs::write(src.join("extra"), "x").unwrap();
                replace_archive(&archive, &src, &["h.log", "extra"]);
            }
            _ => {
                fs::write(src.join("h.log"), "theta\n".repeat(100)).unwrap();
                replace_archive(&archive, &src, &["h.log"]);
            }
        }
        let out = restore(&fx, &[&archive]);
        assert_eq!(out.status.code(), Some(1), "{case}: {}", describe(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(message),
            "{case}: {}",
            describe(&out)
        );
        assert!(
            !log.exists() && fs::symlink_metadata(&log).is_err(),
            "{case}"
        );
    }
}
