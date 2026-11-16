//! Issue #2096: the run's record of index-writing git commands.
//!
//! Contract for `super::git_record` (see "Git command record" in
//! `docs/reference/recipe-runner-logging.md`):
//!
//! - `GitRecord::prepare(run_dir: Option<&Path>) -> GitRecord` creates
//!   `<run_dir>/git-trace2/` (mode 0700, not recursively). No run dir, a
//!   missing run dir or anything already at that path disables the record.
//! - `GitRecord::apply_to(&self, &mut Command)` sets `GIT_TRACE2_EVENT` and
//!   `GIT_TRACE2_ENV_VARS` when enabled and always removes the caller's other
//!   trace variables; when disabled it removes those two as well.
//! - `GitRecord::finish(self)` writes `<run_dir>/git-commands.jsonl` (0600)
//!   and deletes `git-trace2/`. `GitRecord::discard(self)` deletes the empty
//!   `git-trace2/` and writes nothing.
//! - `summarise(trace_dir: &Path) -> Vec<Entry>`, `Entry: Serialize` with the
//!   fields `time`, `argv`, `cwd`, `git_dir`, `index_file`, `sid`.
//!
//! The tests read entries through `serde_json::to_value`, so they pin the
//! written JSON, not the Rust field types.
#![cfg(unix)]

use super::git_record::{GitRecord, RecordingPlatform, summarise};
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

const TRACE_DIR: &str = "git-trace2";
const SUMMARY: &str = "git-commands.jsonl";
const ENV_VARS: &str = "GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD";
const CALLER_TRACE_VARS: &[&str] = &[
    "GIT_TRACE2",
    "GIT_TRACE2_PERF",
    "GIT_TRACE2_CONFIG_PARAMS",
    "GIT_TRACE2_REDACT",
];

#[test]
#[cfg(target_os = "linux")]
fn required_receipt_from_a_different_run_cannot_certify_an_empty_journal() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin(Some(root.path())).unwrap();
    let raw = root.path().join(TRACE_DIR);
    fs::write(raw.join("observer-attempt.jsonl"), b"").unwrap();
    fs::write(
        raw.join("observer-attempt.receipt"),
        serde_json::to_vec(&json!({
            "schema_version":1, "coverage":"established", "complete":true,
            "run_id":"another-run", "attempt_id":"attempt",
            "backend":"linux-ptrace-exec", "scope":"runner-descendants"
        }))
        .unwrap(),
    )
    .unwrap();
    let error = record.finish().unwrap_err();
    assert!(error.to_string().contains("another attempt"), "{error:#}");
    assert!(raw.is_dir(), "retain the rejected evidence for diagnosis");
    assert!(!root.path().join(SUMMARY).exists());
}

// ---------------------------------------------------------------------------
// Trace2 event lines, in the shape git 2.56 writes them.
// ---------------------------------------------------------------------------

fn start(sid: &str, time: Option<&str>, argv: &[&str]) -> String {
    let mut event = json!({
        "event": "start",
        "sid": sid,
        "thread": "main",
        "t_abs": 0.0005,
        "argv": argv,
    });
    if let Some(time) = time {
        event["time"] = json!(time);
    }
    event.to_string()
}

fn param(sid: &str, name: &str, value: &str) -> String {
    json!({
        "event": "def_param",
        "sid": sid,
        "thread": "main",
        "time": "2026-11-10T09:00:00.000000Z",
        "scope": "command",
        "param": name,
        "value": value,
    })
    .to_string()
}

fn repo(sid: &str, worktree: &str) -> String {
    json!({
        "event": "def_repo",
        "sid": sid,
        "thread": "main",
        "time": "2026-11-10T09:00:00.000000Z",
        "repo": 1,
        "worktree": worktree,
    })
    .to_string()
}

fn version(sid: &str) -> String {
    json!({"event": "version", "sid": sid, "thread": "main", "evt": "3", "exe": "2.56.0"})
        .to_string()
}

fn write_events(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

fn trace_dir() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join(TRACE_DIR);
    fs::create_dir(&dir).unwrap();
    (temp, dir)
}

fn entries(dir: &Path) -> Vec<Value> {
    match serde_json::to_value(summarise(dir)).unwrap() {
        Value::Array(items) => items,
        other => panic!("summarise must serialise to an array, got {other}"),
    }
}

fn argvs(entries: &[Value]) -> Vec<Vec<String>> {
    entries
        .iter()
        .map(|e| {
            e["argv"]
                .as_array()
                .unwrap_or_else(|| panic!("argv must be an array: {e}"))
                .iter()
                .map(|a| a.as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

/// What `apply_to` set (`Some`) or removed (`None`) on the command.
fn env_of(command: &Command, key: &str) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v.map(OsStr::to_os_string))
}

fn command_with_caller_trace(caller: &Path) -> Command {
    let mut command = Command::new("true");
    command
        .env("GIT_TRACE2_EVENT", caller.join("event"))
        .env("GIT_TRACE2_ENV_VARS", "HOME,SSH_AUTH_SOCK")
        .env("GIT_TRACE2", caller.join("normal"))
        .env("GIT_TRACE2_PERF", caller.join("perf"))
        .env("GIT_TRACE2_CONFIG_PARAMS", "credential.*")
        .env("GIT_TRACE2_REDACT", "0");
    command
}

fn assert_caller_trace_removed(command: &Command) {
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(command, var),
            Some(None),
            "the caller's {var} must be removed from the runner"
        );
    }
}

// ---------------------------------------------------------------------------
// summarise: what is recorded, and in what order.
// ---------------------------------------------------------------------------

#[test]
fn summarise_keeps_index_writing_commands_sorted_by_time_then_sid() {
    let (_t, dir) = trace_dir();
    let add = "20261110T090003.000000Z-Haaaaaaaa-P00000003";
    let commit = "20261110T090001.000000Z-Haaaaaaaa-P00000001";
    let checkout = "20261110T090001.000000Z-Haaaaaaaa-P00000002";
    let untimed = "20261110T090000.000000Z-Haaaaaaaa-P00000000";
    let status = "20261110T090004.000000Z-Haaaaaaaa-P00000004";
    let rev_parse = "20261110T090005.000000Z-Haaaaaaaa-P00000005";
    write_events(
        &dir,
        "one",
        &[
            version(add),
            start(
                add,
                Some("2026-11-10T09:00:03.000000Z"),
                &["git", "-C", "/w", "add", "f"],
            ),
            param(add, "PWD", "/w"),
            param(add, "GIT_INDEX_FILE", "/elsewhere/.git/index"),
            param(add, "GIT_DIR", "/elsewhere/.git"),
            start(
                status,
                Some("2026-11-10T09:00:04.000000Z"),
                &["git", "status"],
            ),
            param(status, "PWD", "/w"),
            "not json at all".to_string(),
        ],
    );
    write_events(
        &dir,
        "two",
        &[
            // Same time as `checkout`: the sid decides.
            start(
                checkout,
                Some("2026-11-10T09:00:01.000000Z"),
                &["/usr/libexec/git-core/git", "checkout", "--", "."],
            ),
            start(
                commit,
                Some("2026-11-10T09:00:01.000000Z"),
                &["git", "commit", "-qm", "x"],
            ),
            repo(commit, "/repo/worktree"),
            start(
                untimed,
                None,
                &["git", "-c", "core.quotepath=off", "stash", "push"],
            ),
            start(
                rev_parse,
                Some("2026-11-10T09:00:05.000000Z"),
                &["git", "rev-parse", "HEAD"],
            ),
        ],
    );

    let got = entries(&dir);
    let sids: Vec<_> = got.iter().map(|e| e["sid"].as_str().unwrap()).collect();
    assert_eq!(
        sids,
        [untimed, commit, checkout, add],
        "only index-writing commands, sorted by time, then sid, untimed first"
    );

    let add_entry = &got[3];
    assert_eq!(add_entry["time"], json!("2026-11-10T09:00:03.000000Z"));
    assert_eq!(add_entry["argv"], json!(["git", "-C", "/w", "add", "f"]));
    assert_eq!(add_entry["cwd"], json!("/w"));
    assert_eq!(add_entry["git_dir"], json!("/elsewhere/.git"));
    assert_eq!(add_entry["index_file"], json!("/elsewhere/.git/index"));

    let commit_entry = &got[1];
    assert_eq!(
        commit_entry["cwd"],
        json!("/repo/worktree"),
        "with no PWD, cwd is the worktree from def_repo"
    );
    assert_eq!(commit_entry["git_dir"], Value::Null);
    assert_eq!(commit_entry["index_file"], Value::Null);

    assert_eq!(got[0]["time"], Value::Null);
    assert_eq!(got[0]["cwd"], Value::Null);
}

#[test]
fn summarise_recognises_every_index_writing_subcommand_and_nothing_read_only() {
    let (_t, dir) = trace_dir();
    let writing = [
        "add",
        "am",
        "apply",
        "checkout",
        "cherry-pick",
        "commit",
        "merge",
        "mv",
        "pull",
        "read-tree",
        "rebase",
        "reset",
        "restore",
        "revert",
        "rm",
        "stash",
        "switch",
        "update-index",
        "worktree",
    ];
    let read_only = [
        "status",
        "diff",
        "log",
        "rev-parse",
        "show",
        "fetch",
        "push",
        "config",
        "ls-files",
    ];
    let mut lines = Vec::new();
    for (n, sub) in writing.iter().chain(read_only.iter()).enumerate() {
        let sid = format!("20261110T090000.000000Z-Hbbbbbbbb-P{n:08}");
        lines.push(start(
            &sid,
            Some(&format!("2026-11-10T09:00:{:02}.000000Z", n % 60)),
            &["git", "-C", "/w", "-c", "a.b=c", sub],
        ));
    }
    write_events(&dir, "all", &lines);

    let mut got: Vec<String> = argvs(&entries(&dir))
        .into_iter()
        .map(|argv| argv[5].clone())
        .collect();
    got.sort();
    let mut want: Vec<String> = writing.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn summarise_does_not_mistake_an_option_value_for_the_subcommand() {
    let (_t, dir) = trace_dir();
    write_events(
        &dir,
        "values",
        &[
            // The `-C` and `-c` values look like subcommands; the real ones do not write.
            start(
                "s1",
                Some("2026-11-10T09:00:01Z"),
                &["git", "-C", "add", "status"],
            ),
            start(
                "s2",
                Some("2026-11-10T09:00:02Z"),
                &["git", "-c", "commit", "log"],
            ),
            // A bare `git` with no subcommand.
            start("s3", Some("2026-11-10T09:00:03Z"), &["git"]),
            start("s4", Some("2026-11-10T09:00:04Z"), &["git", "-C"]),
        ],
    );
    assert_eq!(entries(&dir), Vec::<Value>::new());
}

#[test]
fn summarise_of_an_empty_or_missing_directory_is_empty() {
    let (t, dir) = trace_dir();
    assert!(summarise(&dir).is_empty());
    assert!(summarise(&t.path().join("absent")).is_empty());
}

// ---------------------------------------------------------------------------
// summarise: redaction and untrusted input (R-S2, R-S3).
// ---------------------------------------------------------------------------

#[test]
fn summarise_redacts_config_values_and_url_userinfo_but_classifies_on_raw_argv() {
    let (_t, dir) = trace_dir();
    write_events(
        &dir,
        "secret",
        &[start(
            "s1",
            Some("2026-11-10T09:00:01Z"),
            &[
                "git",
                "-c",
                "http.extraHeader=Authorization: Bearer hunter2",
                "pull",
                "https://user:tok3n@git.example.com/team/app.git",
                "main",
            ],
        )],
    );
    let got = entries(&dir);
    assert_eq!(got.len(), 1, "a pull behind -c is still recorded");
    assert_eq!(
        got[0]["argv"],
        json!([
            "git",
            "-c",
            "http.extraHeader=<redacted>",
            "pull",
            "https://<redacted>@git.example.com/team/app.git",
            "main"
        ])
    );
    let text = serde_json::to_string(&got).unwrap();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("tok3n"), "{text}");
}

#[test]
fn summarise_reads_only_regular_files_at_the_top_and_follows_no_symlink() {
    let (t, dir) = trace_dir();
    let outside = t.path().join("outside");
    fs::write(
        &outside,
        start(
            "out",
            Some("2026-11-10T09:00:01Z"),
            &["git", "add", "outside"],
        ) + "\n",
    )
    .unwrap();
    symlink(&outside, dir.join("link")).unwrap();
    let nested = dir.join("nested");
    fs::create_dir(&nested).unwrap();
    write_events(
        &nested,
        "deep",
        &[start(
            "deep",
            Some("2026-11-10T09:00:02Z"),
            &["git", "add", "deep"],
        )],
    );
    write_events(
        &dir,
        "real",
        &[start(
            "real",
            Some("2026-11-10T09:00:03Z"),
            &["git", "add", "real"],
        )],
    );

    let sids: Vec<_> = entries(&dir)
        .iter()
        .map(|e| e["sid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(sids, ["real"]);
}

#[test]
fn summarise_skips_a_fifo_instead_of_blocking_on_it() {
    let (_t, dir) = trace_dir();
    let fifo = dir.join("fifo");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "mkfifo failed");
    write_events(
        &dir,
        "real",
        &[start(
            "real",
            Some("2026-11-10T09:00:01Z"),
            &["git", "add", "real"],
        )],
    );
    // No writer ever opens the FIFO: opening it for reading would block
    // forever, so a summariser that opens it never returns.
    assert_eq!(entries(&dir).len(), 1);
}

#[test]
fn summarise_refuses_a_trace_dir_that_is_a_symlink() {
    let (t, dir) = trace_dir();
    write_events(
        &dir,
        "real",
        &[start(
            "real",
            Some("2026-11-10T09:00:01Z"),
            &["git", "add", "real"],
        )],
    );
    let link = t.path().join("linked-trace");
    symlink(&dir, &link).unwrap();
    assert!(
        summarise(&link).is_empty(),
        "a symlinked trace dir must not be read"
    );
}

#[test]
fn summarise_skips_an_over_long_line_and_keeps_the_rest_of_the_file() {
    let (_t, dir) = trace_dir();
    let mut long = json!({
        "event": "start",
        "sid": "long",
        "time": "2026-11-10T09:00:01Z",
        "argv": ["git", "add", "x"],
    });
    long["pad"] = json!("p".repeat(1024 * 1024 + 1));
    write_events(
        &dir,
        "mixed",
        &[
            long.to_string(),
            start("short", Some("2026-11-10T09:00:02Z"), &["git", "add", "y"]),
        ],
    );
    let sids: Vec<_> = entries(&dir)
        .iter()
        .map(|e| e["sid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        sids,
        ["short"],
        "the line over 1 MiB is skipped, the next is read"
    );
}

#[test]
fn summarise_skips_a_file_over_64_mib() {
    let (_t, dir) = trace_dir();
    let big = dir.join("big");
    let file = fs::File::create(&big).unwrap();
    let line = start("big", Some("2026-11-10T09:00:01Z"), &["git", "add", "big"]) + "\n";
    fs::write(&big, &line).unwrap();
    // Sparse: the length is over the cap without writing 64 MiB.
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    write_events(
        &dir,
        "small",
        &[start(
            "small",
            Some("2026-11-10T09:00:02Z"),
            &["git", "add", "small"],
        )],
    );
    let sids: Vec<_> = entries(&dir)
        .iter()
        .map(|e| e["sid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(sids, ["small"]);
}

// ---------------------------------------------------------------------------
// GitRecord: environment, folder lifecycle and the summary file.
// ---------------------------------------------------------------------------

#[test]
fn enabled_record_points_git_at_its_folder_and_removes_the_callers_trace_targets() {
    let run = tempfile::tempdir().unwrap();
    let caller = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(run.path()));
    let trace = run.path().join(TRACE_DIR);
    let meta = fs::symlink_metadata(&trace).expect("prepare creates git-trace2/");
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);

    let mut command = command_with_caller_trace(caller.path());
    record.apply_to(&mut command).unwrap();
    let mut want = trace.into_os_string();
    want.push("/");
    assert_eq!(env_of(&command, "GIT_TRACE2_EVENT"), Some(Some(want)));
    assert_eq!(
        env_of(&command, "GIT_TRACE2_ENV_VARS"),
        Some(Some(OsString::from(ENV_VARS))),
        "exactly these variables, never the caller's list"
    );
    assert_caller_trace_removed(&command);
    record.discard().unwrap();
}

#[test]
fn record_without_a_run_dir_removes_every_trace_variable() {
    let caller = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(None);
    let mut command = command_with_caller_trace(caller.path());
    record.apply_to(&mut command).unwrap();
    assert_eq!(env_of(&command, "GIT_TRACE2_EVENT"), Some(None));
    assert_eq!(env_of(&command, "GIT_TRACE2_ENV_VARS"), Some(None));
    assert_caller_trace_removed(&command);
    record.finish().unwrap();
}

#[test]
fn an_existing_trace_path_disables_the_record_and_is_left_alone() {
    let run = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    // Something a step, or an earlier run, put there: a symlink out of the run dir.
    symlink(elsewhere.path(), run.path().join(TRACE_DIR)).unwrap();
    fs::write(elsewhere.path().join("keep"), "x").unwrap();

    let record = GitRecord::prepare(Some(run.path()));
    let mut command = Command::new("true");
    command.env("GIT_TRACE2_EVENT", "/caller/target");
    record.apply_to(&mut command).unwrap();
    assert_eq!(env_of(&command, "GIT_TRACE2_EVENT"), Some(None));
    assert_eq!(env_of(&command, "GIT_TRACE2_ENV_VARS"), Some(None));
    record.finish().unwrap();

    assert!(
        elsewhere.path().join("keep").exists(),
        "finish must not delete through a path it did not create"
    );
    assert!(!run.path().join(SUMMARY).exists());
}

#[test]
fn a_missing_run_dir_disables_the_record_without_creating_it() {
    let parent = tempfile::tempdir().unwrap();
    let run = parent.path().join("never-made");
    let record = GitRecord::prepare(Some(&run));
    let mut command = Command::new("true");
    record.apply_to(&mut command).unwrap();
    assert_eq!(env_of(&command, "GIT_TRACE2_EVENT"), Some(None));
    record.finish().unwrap();
    assert!(!run.exists(), "git-trace2/ is created non-recursively");
}

#[test]
fn finish_writes_a_private_summary_and_deletes_the_trace_folder() {
    let run = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(run.path()));
    let trace = run.path().join(TRACE_DIR);
    write_events(
        &trace,
        "p1",
        &[
            start(
                "b",
                Some("2026-11-10T09:00:02Z"),
                &["git", "commit", "-m", "x"],
            ),
            start("a", Some("2026-11-10T09:00:01Z"), &["git", "add", "x"]),
            start("c", Some("2026-11-10T09:00:03Z"), &["git", "status"]),
        ],
    );
    record.finish().unwrap();

    assert!(!trace.exists(), "git-trace2/ is deleted after summarising");
    let summary = run.path().join(SUMMARY);
    let mode = fs::metadata(&summary).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let text = fs::read_to_string(&summary).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON object"))
        .collect();
    assert_eq!(
        argvs(&lines),
        [vec!["git", "add", "x"], vec!["git", "commit", "-m", "x"]]
    );
    for line in &lines {
        for field in ["time", "argv", "cwd", "git_dir", "index_file", "sid"] {
            assert!(line.get(field).is_some(), "{field} missing from {line}");
        }
    }
}

#[test]
fn finish_with_no_matching_command_writes_an_empty_summary() {
    let run = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(run.path()));
    write_events(
        &run.path().join(TRACE_DIR),
        "p1",
        &[start("a", Some("2026-11-10T09:00:01Z"), &["git", "status"])],
    );
    record.finish().unwrap();
    assert_eq!(fs::read(run.path().join(SUMMARY)).unwrap(), b"");
    assert!(!run.path().join(TRACE_DIR).exists());
}

#[test]
fn discard_removes_the_empty_folder_and_writes_no_summary() {
    let run = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(run.path()));
    assert!(run.path().join(TRACE_DIR).is_dir());
    record.discard().unwrap();
    assert!(!run.path().join(TRACE_DIR).exists());
    assert!(!run.path().join(SUMMARY).exists());
}

/// A trace folder that cannot be removed (here it is no longer empty) is an
/// error the caller receives, not a warning: the folder is left in place and
/// its path is named.
#[test]
fn discard_returns_the_error_when_the_folder_cannot_be_removed() {
    let run = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(run.path()));
    let trace = run.path().join(TRACE_DIR);
    fs::write(trace.join("left-behind"), b"x").unwrap();
    let error = record
        .discard()
        .expect_err("a folder that cannot be removed must fail the discard");
    assert!(
        format!("{error:#}").contains(&trace.display().to_string()),
        "{error:#}"
    );
    assert!(trace.join("left-behind").is_file());
    assert!(!run.path().join(SUMMARY).exists());
}

/// End to end with real git: a command run with the record's environment is
/// found in the summary with its working directory.
#[test]
fn a_real_git_add_run_under_the_record_is_summarised() {
    let run = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let work_dir = fs::canonicalize(work.path()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let git = |args: &[&str], record: Option<&GitRecord>| {
        let mut c = amplihack_git::command();
        c.env("HOME", home.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("PWD", &work_dir)
            .current_dir(&work_dir)
            .args(args);
        if let Some(record) = record {
            record.apply_to(&mut c).unwrap();
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"], None);
    fs::write(work_dir.join("notes.md"), "n").unwrap();

    let record = GitRecord::prepare(Some(run.path()));
    git(&["add", "notes.md"], Some(&record));
    git(&["status", "--short"], Some(&record));
    record.finish().unwrap();

    let text = fs::read_to_string(run.path().join(SUMMARY)).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert_eq!(lines[0]["argv"], json!(["git", "add", "notes.md"]));
    assert_eq!(lines[0]["cwd"], json!(work_dir.to_str().unwrap()));
    assert!(lines[0]["time"].is_string(), "{}", lines[0]);
    assert!(lines[0]["sid"].is_string(), "{}", lines[0]);
}

#[test]
fn issue_2096_alias_resolved_to_add_is_not_silently_omitted() {
    let (_temp, dir) = trace_dir();
    write_events(&dir, "alias", &[
        start("alias", Some("2026-11-10T09:00:00Z"), &["git", "stage", "notes.md"]),
        json!({"event": "alias", "sid": "alias", "alias": "stage", "argv": ["add", "notes.md"]}).to_string(),
        json!({"event": "cmd_name", "sid": "alias", "name": "add", "hierarchy": "add"}).to_string(),
        param("alias", "PWD", "/fixture/private"),
    ]);
    let entries = summarise(&dir);
    assert_eq!(
        entries.len(),
        1,
        "a resolved index-writing alias is relevant Git activity"
    );
    assert_eq!(
        entries[0].argv,
        ["git", "stage", "notes.md"],
        "retain original invocation while classifying resolved behavior"
    );
}

/// A dry run records nothing and does not fail on the missing run folder:
/// `GitRecord::dry_run()` is the disabled, not-required record. Only a dry
/// run may reach it; real runs go through `begin`, which fails closed.
#[test]
fn a_dry_run_record_is_disabled_strips_trace_targets_and_finishes_without_writing() {
    let record = GitRecord::dry_run();
    let mut command = Command::new("true");
    command
        .env("GIT_TRACE2_EVENT", "/caller/chosen/target")
        .env("GIT_TRACE2_ENV_VARS", "HOME");
    record
        .apply_to(&mut command)
        .expect("a dry-run record must not refuse to apply");
    let envs: Vec<(OsString, Option<OsString>)> = command
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(OsStr::to_os_string)))
        .collect();
    for var in ["GIT_TRACE2_EVENT", "GIT_TRACE2_ENV_VARS"] {
        assert!(
            envs.iter().any(|(k, v)| k == var && v.is_none()),
            "{var} must be removed, not passed through: {envs:?}"
        );
    }
    record
        .finish()
        .expect("finishing a dry-run record writes nothing and succeeds");
}

/// `begin(None)` stays fail-closed: the dry-run record is a separate
/// constructor, not a relaxed `begin`.
#[test]
fn begin_without_a_run_folder_still_refuses() {
    assert!(GitRecord::begin(None).is_err());
}

// ---------------------------------------------------------------------------
// Crusty finding 2: off Linux a real run proceeds unobserved and records that
// command recording is unavailable. The platform is injected through
// `GitRecord::begin_on`; only `RecordingPlatform::current()` reads the target.
// ---------------------------------------------------------------------------

const OBSERVER_VARS: &[&str] = &[
    "AMPLIHACK_OBSERVER_PID",
    "AMPLIHACK_OBSERVER_JOURNAL",
    "AMPLIHACK_OBSERVER_RUN",
    "AMPLIHACK_OBSERVER_ATTEMPT",
    "AMPLIHACK_OBSERVER_ACK_FD",
];

#[test]
fn recording_platform_current_is_linux_on_linux() {
    assert_eq!(
        matches!(RecordingPlatform::current(), RecordingPlatform::Linux),
        cfg!(target_os = "linux")
    );
}

#[test]
fn begin_on_other_runs_unobserved() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Other)
        .expect("a real run off Linux proceeds without the observer");
    #[cfg(target_os = "linux")]
    assert!(
        !record.is_required(),
        "off Linux the runner is spawned without the ptrace observer"
    );
    assert!(
        !root.path().join(TRACE_DIR).exists(),
        "no trace folder is made for an unobserved run"
    );
    record
        .discard()
        .expect("discarding an unavailable record succeeds");
}

#[test]
fn unavailable_apply_to_sets_no_observer_or_trace() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Other).unwrap();
    let mut command = command_with_caller_trace(root.path());
    for var in OBSERVER_VARS {
        command.env(var, "inherited");
    }
    record.apply_to(&mut command).unwrap();
    for var in OBSERVER_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed when no observer runs"
        );
    }
    for var in ["GIT_TRACE2_EVENT", "GIT_TRACE2_ENV_VARS"] {
        assert_eq!(env_of(&command, var), Some(None), "{var} must be removed");
    }
    assert_caller_trace_removed(&command);
}

#[test]
fn unavailable_finish_writes_marker() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Other).unwrap();
    // No runner was spawned through the record: the marker is still written.
    record.finish().expect("an unavailable record finishes");
    let path = root.path().join("git-recording.json");
    let bytes = fs::read(&path).expect("git-recording.json is written");
    assert!(
        bytes.len() <= 4096,
        "run.json copies the marker through a 4096-byte read"
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let marker: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(marker["schema_version"], 1);
    assert_eq!(marker["coverage"], "unavailable");
    assert_eq!(marker["complete"], false);
    assert_eq!(marker["backend"], Value::Null);
    let reason = marker["reason"].as_str().expect("a reason sentence");
    assert!(!reason.is_empty());
    assert!(
        !reason.contains(root.path().to_str().unwrap()),
        "the marker holds no paths: {reason}"
    );
    assert_ne!(
        marker["coverage"], "established",
        "an unavailable marker can never pass as a coverage receipt"
    );
    assert!(
        !root.path().join(SUMMARY).exists(),
        "no command summary is claimed"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn begin_on_linux_still_requires_observer() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    assert!(record.is_required(), "Linux runs stay under the observer");
    assert!(root.path().join(TRACE_DIR).is_dir());
    record.discard().unwrap();
}

#[test]
fn begin_on_other_without_run_dir_fails() {
    let error = GitRecord::begin_on(None, RecordingPlatform::Other)
        .expect_err("an unavailable record still needs a run folder");
    assert!(
        format!("{error:#}").contains("AMPLIHACK_WORK_ROOT"),
        "{error:#}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn begin_delegates_to_the_current_platform() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin(Some(root.path())).unwrap();
    assert!(record.is_required());
    record.discard().unwrap();
}

// ---------------------------------------------------------------------------
// Issue #2677 symptom 2: on Linux an observer that never attached to the
// runner (PTRACE_TRACEME refused) left an empty `observer-<attempt>.jsonl`
// and no receipt, so `finish` failed with "required observer did not finalize; partial evidence
// retained: No such file or directory". The run now goes on unobserved and
// records `unavailable`, exactly once, and only when nothing was recorded.
//
// Contract (see "The Linux observer could not attach" in
// `docs/reference/recipe-runner-logging.md`):
//
// - `GitRecord::attempt_not_established(&self, attempt_id: &str, errno: i32)
//   -> Result<()>` downgrades a required, enabled record with no established
//   attempt to `unavailable` when nothing was recorded: `git-trace2/` holds
//   nothing but this attempt's journal, which is empty or absent (M5). The
//   empty journal is removed. Every other case returns `Err` and leaves the
//   record and its files unchanged.
// - After a downgrade `is_required()` is false, `apply_to` sets no observer
//   or trace variable, and `finish` writes the Linux `unavailable` marker and
//   removes the empty `git-trace2/` folder.
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
const LINUX_UNAVAILABLE_REASON: &str = "the Linux ptrace observer could not attach to the runner, so this run has no Git command record";

/// What `Observer::spawn` leaves behind when TRACEME is refused: the
/// attempt's journal, created empty (0600, `create_new`) before the spawn.
#[cfg(target_os = "linux")]
fn unattached_attempt(root: &Path, attempt: &str) -> (GitRecord, PathBuf) {
    let record = GitRecord::begin_on(Some(root), RecordingPlatform::Linux).unwrap();
    let journal = root
        .join(TRACE_DIR)
        .join(format!("observer-{attempt}.jsonl"));
    fs::write(&journal, b"").unwrap();
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    (record, journal)
}

#[cfg(target_os = "linux")]
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
#[cfg(target_os = "linux")]
fn unattached_attempt_without_a_downgrade_is_the_reported_failure() {
    // The 1.2.163 failure, reproduced: nothing tells the record the observer
    // never attached, so the missing receipt fails the run.
    let root = tempfile::tempdir().unwrap();
    let (record, _journal) = unattached_attempt(root.path(), "a1");
    let error = format!("{:#}", record.finish().unwrap_err());
    assert!(
        error.contains("required observer did not finalize; partial evidence retained"),
        "{error}"
    );
    assert!(error.contains("No such file or directory"), "{error}");
}

#[test]
#[cfg(target_os = "linux")]
fn unestablished_attempt_records_linux_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let (record, journal) = unattached_attempt(root.path(), "a1");
    record
        .attempt_not_established("a1", libc::EPERM)
        .expect("an observer that never attached downgrades the record");
    assert!(!journal.exists(), "the empty journal is removed");
    assert!(
        !record.is_required(),
        "the one fallback start runs without the observer"
    );
    record
        .finish()
        .expect("a downgraded record finishes instead of failing the run");

    let path = root.path().join("git-recording.json");
    let bytes = fs::read(&path).expect("git-recording.json is written");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let marker: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(marker["schema_version"], 1);
    assert_eq!(marker["coverage"], "unavailable");
    assert_eq!(marker["complete"], false);
    assert_eq!(marker["backend"], Value::Null);
    assert_eq!(marker["reason"], LINUX_UNAVAILABLE_REASON);
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains(root.path().to_str().unwrap()), "{text}");
    if let Ok(home) = std::env::var("HOME") {
        assert!(!text.contains(&home), "{text}");
    }
    assert!(
        !text.contains("EPERM") && !text.contains("os error"),
        "{text}"
    );
    assert!(
        !root.path().join(TRACE_DIR).exists(),
        "the empty git-trace2/ folder is removed"
    );
    assert!(
        !root.path().join(SUMMARY).exists(),
        "no command summary is claimed"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_accepts_an_attempt_whose_journal_was_never_created() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    record
        .attempt_not_established("a1", libc::EPERM)
        .expect("a journal that does not exist holds no evidence");
    record.finish().unwrap();
    let marker: Value =
        serde_json::from_slice(&fs::read(root.path().join("git-recording.json")).unwrap()).unwrap();
    assert_eq!(marker["coverage"], "unavailable");
    assert!(!root.path().join(TRACE_DIR).exists());
}

#[test]
#[cfg(target_os = "linux")]
fn apply_to_after_downgrade_sets_no_observer_env() {
    let root = tempfile::tempdir().unwrap();
    let (record, _journal) = unattached_attempt(root.path(), "a1");
    record.attempt_not_established("a1", libc::EPERM).unwrap();
    // The fallback command is built again after the downgrade.
    let mut command = command_with_caller_trace(root.path());
    for var in OBSERVER_VARS {
        command.env(var, "inherited");
    }
    record.apply_to(&mut command).unwrap();
    for var in OBSERVER_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed when the run goes on unobserved"
        );
    }
    for var in ["GIT_TRACE2_EVENT", "GIT_TRACE2_ENV_VARS"] {
        assert_eq!(env_of(&command, var), Some(None), "{var} must be removed");
    }
    assert_caller_trace_removed(&command);
    record.finish().unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn established_attempt_without_receipt_still_fails_closed() {
    // A real observed attempt: the observer attached, so a missing receipt
    // later is lost evidence, never "unavailable".
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    let (_child, mut observer) = record
        .spawn(Command::new("true"), "a1")
        .expect("the host lets the observer attach (ptrace_scope <= 1)");
    observer.terminate().unwrap();
    let raw = root.path().join(TRACE_DIR);
    let receipt = raw.join("observer-a1.receipt");
    if receipt.exists() {
        fs::remove_file(&receipt).unwrap();
    }

    let refused = record.attempt_not_established("a2", libc::EPERM);
    assert!(
        refused.is_err(),
        "after an established attempt no later attempt may downgrade"
    );
    assert!(record.is_required(), "a refused downgrade changes nothing");

    let error = format!("{:#}", record.finish().unwrap_err());
    assert!(
        error.contains("required observer did not finalize"),
        "{error}"
    );
    assert!(raw.join("observer-a1.jsonl").exists(), "evidence is kept");
    assert!(!root.path().join("git-recording.json").exists());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_a_non_empty_journal() {
    let root = tempfile::tempdir().unwrap();
    let (record, journal) = unattached_attempt(root.path(), "a1");
    let evidence = b"{\"schema_version\":1}\n";
    fs::write(&journal, evidence).unwrap();
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "a journal holding anything is partial evidence"
    );
    assert_eq!(fs::read(&journal).unwrap(), evidence, "evidence is kept");
    assert!(record.is_required());
    assert!(
        record.finish().is_err(),
        "the run still fails closed with its evidence"
    );
    assert!(!root.path().join("git-recording.json").exists());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_with_stray_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (record, _journal) = unattached_attempt(root.path(), "a1");
    let raw = root.path().join(TRACE_DIR);
    fs::write(raw.join("observer-a1.receipt"), b"{}").unwrap();
    let before = names_in(&raw);
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "a receipt means the observer ran"
    );
    assert_eq!(names_in(&raw), before, "files are unchanged");
    assert!(record.is_required());
    assert!(record.finish().is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_any_other_entry_in_the_trace_folder() {
    let root = tempfile::tempdir().unwrap();
    let (record, _journal) = unattached_attempt(root.path(), "a1");
    let raw = root.path().join(TRACE_DIR);
    fs::write(raw.join("observer-a0.jsonl"), b"").unwrap();
    let before = names_in(&raw);
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "another attempt's journal is not this attempt's to discard"
    );
    assert_eq!(names_in(&raw), before, "files are unchanged");
    assert!(record.is_required());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_symlinked_journal() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    let elsewhere = root.path().join("elsewhere");
    fs::write(&elsewhere, b"").unwrap();
    let journal = root.path().join(TRACE_DIR).join("observer-a1.jsonl");
    symlink(&elsewhere, &journal).unwrap();
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "a symlink is not an empty journal (M5: symlink_metadata, never followed)"
    );
    assert!(fs::symlink_metadata(&journal).is_ok(), "the link is kept");
    assert!(elsewhere.exists(), "the target is never touched");
    assert!(record.is_required());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_when_not_required_or_not_enabled() {
    // Off Linux the record is already unavailable; a dry run's record is
    // disabled. Neither has an observer attempt to give up on.
    let root = tempfile::tempdir().unwrap();
    let other = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Other).unwrap();
    assert!(other.attempt_not_established("a1", libc::EPERM).is_err());
    let dry = GitRecord::dry_run();
    assert!(dry.attempt_not_established("a1", libc::EPERM).is_err());
    // A Linux record already downgraded once cannot downgrade again.
    let linux_root = tempfile::tempdir().unwrap();
    let (record, _journal) = unattached_attempt(linux_root.path(), "a1");
    record.attempt_not_established("a1", libc::EPERM).unwrap();
    assert!(
        record.attempt_not_established("a2", libc::EPERM).is_err(),
        "the fallback happens at most once"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn a_runner_that_cannot_exec_is_not_an_unattached_observer() {
    // D8: ENOENT on the runner binary is the runner's own spawn failure. It
    // must not take the unobserved fallback, which would only fail again.
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    let missing = root.path().join("no-such-runner");
    let error = record
        .spawn(Command::new(&missing), "a1")
        .err()
        .expect("a missing runner binary cannot start");
    assert!(
        error
            .downcast_ref::<super::git_record::observer::ObserverNotEstablished>()
            .is_none(),
        "{error:#}"
    );
    assert!(record.is_required(), "nothing is downgraded");
}

// ---------------------------------------------------------------------------
// Issue #2677 B1d: a runner that cannot start (missing binary or folder)
// reported "required observer did not finalize; partial evidence retained:
// No such file or directory", which named the missing receipt, not the
// failed spawn. The spawn's own error now comes first and is the only cause:
// the attempt never started, so its empty journal is released and `finish`
// has nothing to check.
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn a_runner_that_cannot_start_reports_its_own_error_first() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    let error = record
        .spawn(Command::new(root.path().join("no-such-runner")), "a1")
        .err()
        .expect("a missing runner binary cannot start");
    let text = format!("{error:#}");
    assert!(
        error
            .downcast_ref::<super::git_record::observer::RunnerNotStarted>()
            .is_some(),
        "the spawn failure is marked as an attempt that never started: {text}"
    );
    assert_eq!(
        error.chain().next().map(ToString::to_string).as_deref(),
        Some("failed to spawn recipe-runner-rs"),
        "{text}"
    );
    assert_eq!(
        error
            .root_cause()
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error),
        Some(libc::ENOENT),
        "the spawn's errno is kept: {text}"
    );
    for word in ["receipt", "did not finalize", "PTRACE", "tracing"] {
        assert!(!text.contains(word), "{word}: {text}");
    }
    assert!(record.is_required(), "a spawn failure is not a downgrade");

    record
        .finish()
        .expect("an attempt that never started leaves nothing to check");
    assert!(
        !root.path().join(TRACE_DIR).exists(),
        "the empty git-trace2/ folder is released"
    );
    assert!(
        !root.path().join("git-recording.json").exists(),
        "no coverage is claimed for a run whose runner never started"
    );
    assert!(!root.path().join(SUMMARY).exists());
}

#[test]
#[cfg(target_os = "linux")]
fn a_failure_before_the_runner_spawn_keeps_the_attempt_and_fails_closed() {
    // SR-3: only `RunnerNotStarted` releases an attempt. Here the observer
    // cannot create its journal (`create_new` finds one already there), a
    // plain error: the existing file and the attempt count are kept, so
    // `finish` still fails closed.
    let root = tempfile::tempdir().unwrap();
    let (record, journal) = unattached_attempt(root.path(), "a1");
    let error = record
        .spawn(Command::new("true"), "a1")
        .err()
        .expect("an existing journal cannot be created again");
    for marker in [
        error
            .downcast_ref::<super::git_record::observer::RunnerNotStarted>()
            .is_some(),
        error
            .downcast_ref::<super::git_record::observer::ObserverNotEstablished>()
            .is_some(),
    ] {
        assert!(!marker, "{error:#}");
    }
    assert!(journal.exists(), "the journal is not released");
    assert!(record.is_required());
    assert!(
        record.finish().is_err(),
        "the attempt still counts, so its missing receipt fails the run"
    );
    assert!(journal.exists(), "evidence is kept");
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_refuses_when_the_trace_folder_is_gone() {
    // M5: a missing `git-trace2/` is not "nothing was recorded".
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    fs::remove_dir(root.path().join(TRACE_DIR)).unwrap();
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "storage that vanished is not an empty attempt"
    );
    assert!(record.is_required());
}

// ---------------------------------------------------------------------------
// Issue #2677 M5/A5: `attempt_not_established` decides on two conditions and
// no others. It proceeds when the attempt's journal is empty or absent and
// nothing else was recorded (no attempt established, `git-trace2/` holding
// nothing but that journal); otherwise it fails and changes nothing. The
// `required` flag is not one of those conditions: the only caller reaches
// this path because the record was required, and the transition clears the
// flag itself, so testing it only restates the caller.
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn downgrade_rests_on_what_was_recorded_not_on_the_required_flag() {
    let root = tempfile::tempdir().unwrap();
    // `prepare` gives an enabled record that was never marked required.
    let record = GitRecord::prepare(Some(root.path()));
    let journal = root.path().join(TRACE_DIR).join("observer-a1.jsonl");
    fs::write(&journal, b"").unwrap();
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    record
        .attempt_not_established("a1", libc::EPERM)
        .expect("an empty journal and nothing else recorded is the whole rule");
    assert!(!journal.exists(), "the empty journal is removed");
    assert!(!record.is_required());
    record.finish().unwrap();
    let marker: Value =
        serde_json::from_slice(&fs::read(root.path().join("git-recording.json")).unwrap()).unwrap();
    assert_eq!(marker["coverage"], "unavailable");
    assert_eq!(marker["reason"], LINUX_UNAVAILABLE_REASON);
    assert!(!root.path().join(TRACE_DIR).exists());
}

#[test]
#[cfg(target_os = "linux")]
fn downgrade_still_refuses_a_journal_with_content_on_an_unrequired_record() {
    // The other half of the same rule: dropping the `required` precondition
    // widens nothing, because evidence still refuses the downgrade.
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(root.path()));
    let journal = root.path().join(TRACE_DIR).join("observer-a1.jsonl");
    fs::write(&journal, b"{\"event\":\"start\"}\n").unwrap();
    assert!(
        record.attempt_not_established("a1", libc::EPERM).is_err(),
        "a journal with content is evidence, whatever the flag says"
    );
    assert!(journal.exists(), "evidence is kept");
}

// ---------------------------------------------------------------------------
// Issue #2677 A6b: when the release of an unstarted attempt is refused, the
// reason is reported instead of being discarded. The refusal changes nothing
// else: the attempt still counts, the spawn error is still what `spawn`
// returns, and `finish` still fails closed on the kept evidence.
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn a_refused_release_keeps_the_attempt_and_still_reports_the_spawn_error() {
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::begin_on(Some(root.path()), RecordingPlatform::Linux).unwrap();
    // An earlier attempt's journal is in the recorder's storage, so the
    // release of this attempt is refused: storage holds another entry.
    let earlier = root.path().join(TRACE_DIR).join("observer-earlier.jsonl");
    fs::write(&earlier, b"{\"event\":\"start\"}\n").unwrap();
    let error = record
        .spawn(Command::new(root.path().join("no-such-runner")), "a1")
        .err()
        .expect("a missing runner binary cannot start");
    let text = format!("{error:#}");
    assert!(
        error
            .downcast_ref::<super::git_record::observer::RunnerNotStarted>()
            .is_some(),
        "{text}"
    );
    assert_eq!(
        error.chain().next().map(ToString::to_string).as_deref(),
        Some("failed to spawn recipe-runner-rs"),
        "the spawn's own error is still the cause reported: {text}"
    );
    assert!(
        !text.contains("partial evidence retained"),
        "the release refusal is a warning, not the returned cause: {text}"
    );
    assert!(earlier.exists(), "the other attempt's journal is kept");
    assert!(record.is_required());
    assert!(
        record.finish().is_err(),
        "the attempt still counts, so its missing receipt fails the run"
    );
    assert!(earlier.exists(), "evidence survives the failure");
}

#[test]
#[cfg(target_os = "linux")]
fn a_second_downgrade_of_one_record_is_refused_and_deletes_nothing() {
    // M5 leaves one rule on evidence, so the question of what stops a second
    // downgrade falls to the state check. The first call leaves the record
    // `unavailable` rather than enabled, so the second never reaches the
    // journal at all.
    let root = tempfile::tempdir().unwrap();
    let record = GitRecord::prepare(Some(root.path()));
    let trace = root.path().join(TRACE_DIR);
    let first = trace.join("observer-a1.jsonl");
    fs::write(&first, b"").unwrap();
    record
        .attempt_not_established("a1", libc::EPERM)
        .expect("an empty journal and nothing else recorded is the whole rule");
    assert!(!first.exists(), "the first downgrade removed its journal");
    assert!(!record.is_required());

    // A second attempt's empty journal would satisfy the evidence rule on its
    // own. The record is no longer enabled, so the rule is never consulted.
    let second = trace.join("observer-a2.jsonl");
    fs::write(&second, b"").unwrap();
    let error = record
        .attempt_not_established("a2", libc::EPERM)
        .expect_err("a record that is already unavailable is not downgraded again");
    assert!(
        error
            .to_string()
            .contains("Git recording is not observing this run"),
        "the state check answers it, not the evidence rule: {error:#}"
    );
    assert!(
        second.exists(),
        "a refused downgrade removes no journal: {error:#}"
    );
    assert!(!record.is_required(), "the refusal changes nothing");
}

// ---------------------------------------------------------------------------
// Issue #2677 A6b: the refusal warning is built by a named function and
// printed by its caller, so what it carries can be asserted without capturing
// stderr or driving a process. The line is fixed text plus the refusal's own
// chain; no path, command line or environment value goes into it, so a run
// log holding it can be quoted in an issue unread.
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn a_refused_release_warning_carries_its_fixed_prefix_and_no_path() {
    const PREFIX: &str =
        "amplihack: warning: could not release the unstarted attempt's recorder storage: ";
    // The refusal as the release path raises it, wrapped the way `anyhow`
    // delivers it to the caller, with an operating system error beneath.
    let refusal = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOTEMPTY))
        .context("recorder storage holds other entries; partial evidence retained");
    let line = super::git_record::refused_release_warning(&refusal);
    assert!(line.starts_with(PREFIX), "{line}");
    assert!(
        line.contains("recorder storage holds other entries"),
        "the refusal's own reason is what the line is for: {line}"
    );
    assert!(
        line.contains("Directory not empty"),
        "the operating system's own message is kept: {line}"
    );
    assert!(
        !line.contains('/'),
        "no path reaches the line, so it can be quoted unread: {line}"
    );
    assert!(!line.contains('\n'), "one warning is one line: {line:?}");
    for word in ["failed to spawn recipe-runner-rs", "nested"] {
        assert!(
            !line.contains(word),
            "{word} belongs to the returned error, not this warning: {line}"
        );
    }
}
