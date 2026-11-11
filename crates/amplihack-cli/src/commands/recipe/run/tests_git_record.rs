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

use super::git_record::{GitRecord, summarise};
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
    record.discard();
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
    record.discard();
    assert!(!run.path().join(TRACE_DIR).exists());
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
