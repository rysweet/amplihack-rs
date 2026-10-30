//! Issue #1746: `amplihack sessions archive` moves old Claude Code session
//! logs out of `~/.claude/projects` into `<DEST>/<project>/<id>.tar.gz`.
//!
//! Runs the real binary against a temporary HOME, profile root and DEST
//! (`support::sessions_archive`). Contract: docs/reference/sessions-command.md
//! (Archive, Archive names, Output, Destination layout, Manifest, Messages,
//! Configuration, Exit status).

#![cfg(unix)]

use crate::support::sessions_archive::*;
use std::fs;
use std::os::unix::fs::symlink;

const DRY_ZERO: &str = "would archive 0 sessions, 0.00 GiB (older than 3 days; 0 live sessions kept); dry run, nothing changed (use --apply)";

fn dry(n: &str, days: u32, live: u32) -> String {
    format!(
        "would archive {n} sessions, 0.00 GiB (older than {days} days; {live} live sessions kept); dry run, nothing changed (use --apply)"
    )
}

fn applied(n: &str, days: u32, live: u32, failed: u32) -> String {
    format!(
        "archived {n} sessions, 0.00 GiB (older than {days} days; {live} live sessions kept; {failed} failed)"
    )
}

#[test]
fn missing_projects_root_is_zero_sessions() {
    let fx = Fx::new();
    fs::remove_dir(fx.projects()).unwrap();
    let out = fx.archive(&[]);
    ok(&out);
    assert_stdout(&out, DRY_ZERO);
}

#[test]
fn dry_run_counts_old_sessions_and_changes_nothing() {
    let fx = Fx::new();
    let a = fx.session("aaaa", 10, &["subagents/agent-1.jsonl", "tool.txt"]);
    let b = fx.session("bbbb", 4, &[]);
    let fresh = fx.session("cccc", 1, &[]);
    let out = fx.archive(&[]);
    ok(&out);
    assert_stdout(&out, &dry("2", 3, 0));
    assert!(
        text(&out.stderr).is_empty(),
        "stderr: {}",
        text(&out.stderr)
    );
    for p in [&a, &b, &fresh] {
        assert!(p.is_file(), "{} removed by a dry run", p.display());
    }
    assert!(!fx.dest.exists(), "a dry run created DEST");
}

#[test]
fn apply_archives_old_sessions_removes_originals_and_writes_manifest() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &["subagents/agent-1.jsonl", "tool.txt"]);
    fx.session("bbbb", 5, &[]);
    let fresh = fx.session("cccc", 1, &["x.txt"]);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("2", 3, 0, 0));

    let pdir = fx.project_dir();
    assert!(!pdir.join("aaaa.jsonl").exists());
    assert!(!pdir.join("aaaa").exists());
    assert!(!pdir.join("bbbb.jsonl").exists());
    assert!(fresh.is_file() && pdir.join("cccc/x.txt").is_file());

    assert_eq!(mode(&fx.dest), 0o700);
    assert_eq!(mode(&fx.dest.join(PROJECT)), 0o700);
    assert_eq!(mode(&fx.manifest()), 0o600);
    let tgz = fx.dest.join(PROJECT).join("aaaa.tar.gz");
    assert_eq!(mode(&tgz), 0o600);

    let mut members = fx.members(&tgz);
    members.sort();
    assert_eq!(
        members,
        [
            "aaaa",
            "aaaa.jsonl",
            "aaaa/subagents",
            "aaaa/subagents/agent-1.jsonl",
            "aaaa/tool.txt"
        ]
    );

    let lines = fx.manifest_lines();
    assert_eq!(lines.len(), 2);
    let a = lines.iter().find(|l| l["session_id"] == "aaaa").unwrap();
    assert_eq!(a["project"], PROJECT);
    assert_eq!(a["archive"], format!("{PROJECT}/aaaa.tar.gz"));
    assert_eq!(a["parts"], 5);
    assert!(a["bytes"].as_u64().unwrap() > 0);
    let time = a["time"].as_str().unwrap();
    assert!(
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
            .unwrap()
            .is_match(time),
        "time {time:?}"
    );
    // No temporary file is left.
    for e in fs::read_dir(fx.dest.join(PROJECT)).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.starts_with('.'), "leftover {name}");
    }
}

#[test]
fn manifest_bytes_is_the_sum_of_file_sizes() {
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &["t.txt"]);
    let want = fs::metadata(&jsonl).unwrap().len()
        + fs::metadata(fx.project_dir().join("aaaa/t.txt"))
            .unwrap()
            .len();
    ok(&fx.apply());
    assert_eq!(fx.manifest_lines()[0]["bytes"].as_u64(), Some(want));
    assert_eq!(fx.manifest_lines()[0]["parts"].as_u64(), Some(3));
}

#[test]
fn one_recent_file_anywhere_keeps_the_whole_session() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &["deep/old.txt", "deep/new.txt"]);
    age(&fx.project_dir().join("aaaa/deep/new.txt"), 0);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert!(fx.project_dir().join("aaaa.jsonl").is_file());
    assert!(fx.project_dir().join("aaaa/deep/old.txt").is_file());
}

#[test]
fn directory_without_jsonl_is_never_touched() {
    let fx = Fx::new();
    let memory = fx.project_dir().join("memory");
    fs::create_dir_all(&memory).unwrap();
    fs::write(memory.join("MEMORY.md"), BODY_MARKER).unwrap();
    age(&memory.join("MEMORY.md"), 30);
    age(&memory, 30);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert!(memory.join("MEMORY.md").is_file());
}

#[test]
fn session_containing_a_symlink_is_skipped_and_left_intact() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &["x.txt"]);
    let outside = fx.base.join("outside.txt");
    fs::write(&outside, BODY_MARKER).unwrap();
    let link = fx.project_dir().join("aaaa/link");
    symlink(&outside, &link).unwrap();
    age(&fx.project_dir().join("aaaa"), 10);
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert_stderr_line(&out, &format!("skipped {PROJECT}/aaaa: symlink"));
    assert!(fx.project_dir().join("aaaa.jsonl").is_file());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(outside.is_file(), "the symlink target was touched");
}

#[test]
fn symlinked_jsonl_is_skipped() {
    let fx = Fx::new();
    let real = fx.base.join("real.jsonl");
    fs::write(&real, BODY_MARKER).unwrap();
    fs::create_dir_all(fx.project_dir()).unwrap();
    symlink(&real, fx.project_dir().join("aaaa.jsonl")).unwrap();
    age(&real, 10);
    let out = fx.apply();
    ok(&out);
    assert_stderr_line(&out, &format!("skipped {PROJECT}/aaaa: symlink"));
    assert!(real.is_file());
}

#[test]
fn session_containing_a_fifo_is_skipped() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &["x.txt"]);
    let fifo = fx.project_dir().join("aaaa/pipe");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: c is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    age(&fx.project_dir().join("aaaa"), 10);
    let out = fx.apply();
    ok(&out);
    assert_stderr_line(&out, &format!("skipped {PROJECT}/aaaa: special file"));
    assert!(fx.project_dir().join("aaaa.jsonl").is_file());
}

#[test]
fn symlinked_project_is_skipped_and_not_counted() {
    let fx = Fx::new();
    let elsewhere = fx.base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("zzzz.jsonl"), BODY_MARKER).unwrap();
    age(&elsewhere.join("zzzz.jsonl"), 30);
    symlink(&elsewhere, fx.projects().join("-linked")).unwrap();
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert!(elsewhere.join("zzzz.jsonl").is_file());
}

#[test]
fn days_flag_and_config_key_set_the_cutoff_flag_wins() {
    let fx = Fx::new();
    fx.session("aaaa", 5, &[]);
    fx.session("bbbb", 10, &[]);

    let out = fx.archive(&["--days", "7"]);
    ok(&out);
    assert_stdout(&out, &dry("1", 7, 0));

    ok(&fx.run_cmd(fx.cmd(&["config", "set", "sessions.archive_days", "4"])));
    let out = fx.archive(&[]);
    ok(&out);
    assert_stdout(&out, &dry("2", 4, 0));

    let out = fx.archive(&["--days", "7"]);
    assert_stdout(&out, &dry("1", 7, 0));

    // Unset: the default 3.
    ok(&fx.run_cmd(fx.cmd(&["config", "unset", "sessions.archive_days"])));
    let out = fx.archive(&[]);
    assert_stdout(&out, &dry("2", 3, 0));
}

#[test]
fn an_invalid_days_key_names_the_key_without_echoing_it() {
    let fx = Fx::new();
    let file = fx.user_config(&[]);
    for bad in ["0", "-3", "4294967296", "\"SECRET-VALUE\"", "1.5"] {
        fx.user_config(&[&format!("archive_days = {bad}")]);
        let out = fx.run_cmd(fx.cmd(&["sessions", "archive"]));
        assert_eq!(exit_code(&out), 1, "{bad}");
        assert_stderr_line(
            &out,
            &format!(
                "cannot read sessions.archive_days ({}: sessions.archive_days: must be an integer from 1 to 4294967295)",
                file.display()
            ),
        );
        assert!(text(&out.stdout).is_empty());
        assert!(!text(&out.stderr).contains("SECRET"), "{bad}");
    }
    // The flag wins, so the key is never read.
    let out = fx.archive(&["--days", "5"]);
    ok(&out);
    assert_stdout(&out, &dry("0", 5, 0));
}

#[test]
fn an_invalid_dest_key_stops_archive_and_restore_instead_of_using_the_default() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let file = fx.user_config(&["archive_dest = \"relative/dir\""]);
    let line = format!(
        "cannot read sessions.archive_dest ({}: sessions.archive_dest: must be an absolute path without '..', control characters or NUL, at most 4096 bytes)",
        file.display()
    );
    let out = fx.run_cmd(fx.cmd(&["sessions", "archive", "--apply"]));
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &line);
    assert!(fx.project_dir().join("aaaa.jsonl").is_file());
    assert!(!fx.home.join(".amplihack/session-archive").exists());
    let out = fx.run_cmd(fx.cmd(&["sessions", "restore", "aaaa"]));
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(&out, &line);
}

#[test]
fn days_zero_and_bad_flags_are_usage_errors() {
    let fx = Fx::new();
    for args in [
        vec!["sessions", "archive", "--days", "0"],
        vec!["sessions", "archive", "--days", "x"],
        vec!["sessions", "archive", "--bogus"],
        vec!["sessions", "restore"],
    ] {
        let out = fx.run_cmd(fx.cmd(&args));
        assert_eq!(exit_code(&out), 2, "{args:?}: {}", text(&out.stderr));
    }
}

#[test]
fn dest_key_is_used_by_archive_and_restore_when_no_flag() {
    let fx = Fx::new();
    let session = fx.session("aaaa", 10, &[]);
    let dest = fx.dest.to_str().unwrap().to_string();
    ok(&fx.run_cmd(fx.cmd(&["config", "set", "sessions.archive_dest", &dest])));
    let out = fx.run_cmd(fx.cmd(&["sessions", "archive", "--apply"]));
    ok(&out);
    assert!(fx.dest.join(PROJECT).join("aaaa.tar.gz").is_file());
    assert!(!session.exists());
    let out = fx.run_cmd(fx.cmd(&["sessions", "restore", "aaaa"]));
    ok(&out);
    assert!(
        session.is_file(),
        "restore found the archive through the key"
    );
}

#[test]
fn the_old_variables_are_ignored() {
    // Days and DEST were briefly AMPLIHACK_SESSIONS_ARCHIVE_DAYS and _DEST.
    // The background rotator keeps the environment it was started with, so
    // a variable could send the daily run and `restore` to different
    // places; only the config keys and the flags count now.
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let elsewhere = fx.base.join("elsewhere");
    let mut c = fx.cmd(&["sessions", "archive", "--apply"]);
    c.env("AMPLIHACK_SESSIONS_ARCHIVE_DAYS", "30");
    c.env("AMPLIHACK_SESSIONS_ARCHIVE_DEST", &elsewhere);
    let out = fx.run_cmd(c);
    ok(&out);
    assert_stdout(&out, &applied("1", 3, 0, 0));
    assert!(
        fx.home
            .join(".amplihack/session-archive")
            .join(PROJECT)
            .join("aaaa.tar.gz")
            .is_file()
    );
    assert!(!elsewhere.exists());
}

#[test]
fn default_dest_is_under_home_amplihack() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let out = fx.run_cmd(fx.cmd(&["sessions", "archive", "--apply"]));
    ok(&out);
    assert!(
        fx.home
            .join(".amplihack/session-archive")
            .join(PROJECT)
            .join("aaaa.tar.gz")
            .is_file()
    );
}

#[test]
fn relative_dest_is_taken_from_the_current_directory() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let out = fx.run_cmd(fx.cmd(&["sessions", "archive", "--apply", "--dest", "rel-dest"]));
    ok(&out);
    assert!(
        fx.cwd
            .join("rel-dest")
            .join(PROJECT)
            .join("aaaa.tar.gz")
            .is_file()
    );
}

#[test]
fn existing_archive_name_gets_a_timestamped_name_and_is_kept() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let pdest = fx.dest.join(PROJECT);
    fs::create_dir_all(&pdest).unwrap();
    chmod(&fx.dest, 0o700);
    chmod(&pdest, 0o700);
    fs::write(pdest.join("aaaa.tar.gz"), b"older archive").unwrap();
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("1", 3, 0, 0));
    assert_eq!(
        fs::read(pdest.join("aaaa.tar.gz")).unwrap(),
        b"older archive"
    );
    let re = regex::Regex::new(r"^aaaa\.\d{8}T\d{6}Z\.tar\.gz$").unwrap();
    let names: Vec<String> = fs::read_dir(&pdest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| re.is_match(n))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    let line = &fx.manifest_lines()[0];
    assert_eq!(line["archive"], format!("{PROJECT}/{}", names[0]));
}

#[test]
fn dest_inside_or_equal_to_or_containing_projects_root_is_refused() {
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &[]);
    let root = fx.projects();
    for dest in [root.join("archive"), root.clone(), fx.home.join(".claude")] {
        for extra in [&[][..], &["--apply"][..]] {
            let d = dest.to_str().unwrap().to_string();
            let mut args = vec!["sessions", "archive"];
            args.extend_from_slice(extra);
            args.extend_from_slice(&["--dest", &d]);
            let out = fx.run_cmd(fx.cmd(&args));
            assert_eq!(exit_code(&out), 1, "{args:?}");
            assert_stderr_line(
                &out,
                &format!(
                    "destination {} overlaps {}; nothing changed",
                    dest.display(),
                    root.display()
                ),
            );
            assert!(jsonl.is_file());
        }
    }
    assert!(!root.join("archive").exists(), "DEST was created");
}

#[test]
fn dest_writable_by_others_is_refused() {
    let fx = Fx::new();
    let jsonl = fx.session("aaaa", 10, &[]);
    fs::create_dir_all(&fx.dest).unwrap();
    chmod(&fx.dest, 0o777);
    let out = fx.apply();
    assert_eq!(exit_code(&out), 1);
    assert_stderr_line(
        &out,
        &format!(
            "destination {} is not private (must be your own directory, not writable by group or others)",
            fx.dest.display()
        ),
    );
    assert!(jsonl.is_file());
    assert!(!fx.manifest().exists());
}

#[test]
fn crash_leftover_tmp_files_are_removed_under_the_lock_and_nothing_else() {
    let fx = Fx::new();
    let pdest = fx.dest.join(PROJECT);
    fs::create_dir_all(&pdest).unwrap();
    chmod(&fx.dest, 0o700);
    chmod(&pdest, 0o700);
    let leftover = pdest.join(".aaaa.tar.gz.tmp-12345");
    let keep = [
        pdest.join(".notes"),
        pdest.join("aaaa.tar.gz.tmp-1"),
        fx.dest.join(".x.tar.gz.tmp-1"),
    ];
    fs::write(&leftover, b"partial").unwrap();
    for k in &keep {
        fs::write(k, b"keep").unwrap();
    }
    ok(&fx.apply());
    assert!(!leftover.exists(), "crash leftover kept");
    for k in &keep {
        assert!(k.exists(), "{} removed", k.display());
    }
    // A dry run never cleans up.
    fs::write(&leftover, b"partial").unwrap();
    ok(&fx.archive(&[]));
    assert!(leftover.exists());
}

#[test]
fn thousands_separator_and_word_sessions_in_summary() {
    let fx = Fx::new();
    for i in 0..1001 {
        let id = format!("s{i:04}");
        let p = fx.project_dir().join(format!("{id}.jsonl"));
        fs::create_dir_all(fx.project_dir()).unwrap();
        fs::write(&p, "{}").unwrap();
        age(&p, 10);
    }
    let out = fx.archive(&[]);
    ok(&out);
    assert_stdout(&out, &dry("1,001", 3, 0));
}

#[test]
fn single_session_says_1_sessions() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    let out = fx.archive(&[]);
    assert_stdout(&out, &dry("1", 3, 0));
}

#[test]
fn second_apply_finds_nothing_more() {
    let fx = Fx::new();
    fx.session("aaaa", 10, &[]);
    ok(&fx.apply());
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert_eq!(fx.manifest_lines().len(), 1);
}

#[test]
fn bad_session_names_are_skipped() {
    let fx = Fx::new();
    fs::create_dir_all(fx.project_dir()).unwrap();
    // A hidden `.jsonl` and an id with a control character.
    for name in [".hidden.jsonl", "bad\u{1}id.jsonl"] {
        let p = fx.project_dir().join(name);
        fs::write(&p, BODY_MARKER).unwrap();
        age(&p, 10);
    }
    let out = fx.apply();
    ok(&out);
    assert_stdout(&out, &applied("0", 3, 0, 0));
    assert!(fx.project_dir().join("bad\u{1}id.jsonl").is_file());
    assert!(fx.project_dir().join(".hidden.jsonl").is_file());
    // The control character is escaped, never printed raw.
    assert!(!text(&out.stderr).contains('\u{1}'));
}

/// The line a guarded run prints for a HOME or DEST outside its temp dir.
fn guard_line(what: &str, path: &std::path::Path) -> String {
    format!(
        "AMPLIHACK_TEST_HOME_GUARD is set and {what} ({}) is outside the temp dir; nothing changed",
        path.display()
    )
}

#[test]
fn the_test_guard_stops_a_home_or_dest_outside_the_temp_dir_whatever_the_flags() {
    // With `--days` and `--dest` no config key is read, so the key's guard
    // never runs; the command itself must refuse (review of #1766). The
    // child's TMPDIR is a directory inside this fixture, so "outside the
    // temp dir" is still inside this test's own temporary directory and
    // nothing real is at risk if the check is missing.
    let fx = Fx::new();
    let session = fx.session("aaaa", 10, &[]);
    let child_tmp = fx.base.join("child-tmp");
    fs::create_dir_all(&child_tmp).unwrap();
    chmod(&child_tmp, 0o700);
    let dest = child_tmp.join("dest");
    let d = dest.to_str().unwrap().to_string();
    let run = |tmpdir: &std::path::Path, args: &[&str]| {
        let mut c = fx.cmd(args);
        c.env("TMPDIR", tmpdir);
        fx.run_cmd(c)
    };
    let refused = |out: &std::process::Output, line: &str| {
        assert_eq!(exit_code(out), 1, "stderr: {}", text(&out.stderr));
        assert_stderr_line(out, line);
        assert!(out.stdout.is_empty(), "stdout: {}", text(&out.stdout));
    };

    // HOME outside the child's temp dir: archive, with and without --apply.
    let home_line = guard_line("HOME", &fx.home);
    for extra in [&["--apply"][..], &[]] {
        let mut args = vec!["sessions", "archive", "--days", "1", "--dest", &d];
        args.extend_from_slice(extra);
        refused(&run(&child_tmp, &args), &home_line);
    }
    assert!(session.is_file(), "a guarded run archived the session");
    assert!(!dest.exists(), "a guarded run created DEST");

    // Control: the same command with HOME inside the temp dir archives, so
    // the refusal above was the guard and nothing else.
    let out = run(
        &fx.base,
        &[
            "sessions", "archive", "--days", "1", "--dest", &d, "--apply",
        ],
    );
    ok(&out);
    assert!(!session.exists());
    let archive = dest.join(PROJECT).join("aaaa.tar.gz");
    assert!(archive.is_file());

    // HOME outside: restore with --dest writes nothing into HOME.
    refused(
        &run(&child_tmp, &["sessions", "restore", "aaaa", "--dest", &d]),
        &home_line,
    );
    assert!(!session.exists(), "a guarded restore wrote into HOME");
    assert!(archive.is_file());

    // HOME inside the temp dir (TMPDIR is HOME itself), DEST outside it.
    let other = fx.base.join("other-dest");
    let o = other.to_str().unwrap().to_string();
    let dest_line = guard_line("DEST", &other);
    let second = fx.session("bbbb", 10, &[]);
    refused(
        &run(
            &fx.home,
            &[
                "sessions", "archive", "--days", "1", "--dest", &o, "--apply",
            ],
        ),
        &dest_line,
    );
    refused(
        &run(&fx.home, &["sessions", "restore", "aaaa", "--dest", &o]),
        &dest_line,
    );
    assert!(
        second.is_file(),
        "a guarded run archived into an outside DEST"
    );
    assert!(!other.exists(), "a guarded run created an outside DEST");
}
