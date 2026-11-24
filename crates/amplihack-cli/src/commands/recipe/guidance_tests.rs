//! Unit tests for `recipe guidance` (issue #3129): argument checks, the
//! folder list, the JSON shapes and the marked block. Arming and waiting are
//! tested in `run_guidance/wait_tests.rs`; the command end to end, with the
//! real binary, in `tests/issue_3129_goal_loop/guidance_cli.rs`.
//!
//! Every test works in its own temporary folder; none prints on stdout.

use super::*;
use crate::run_guidance::env::listed_dirs;
use crate::run_guidance::{AUTO_DRIVE_DIR, GUIDANCE_DIR};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn private_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, root)
}

fn args(target: Option<&str>, pr: Option<&str>, repo: Option<&str>) -> PrFolderArgs {
    PrFolderArgs {
        target: target.map(str::to_string),
        pr: pr.map(str::to_string),
        pr_repo: repo.map(str::to_string),
    }
}

fn entry(name: &str, dir: &Path, caller: &str, text: &str) -> Entry {
    Entry {
        name: name.to_string(),
        dir: dir.to_path_buf(),
        source: Source::of(dir),
        timestamp: "2026-11-23T18:40:02.000000000Z".to_string(),
        caller: caller.to_string(),
        text: text.to_string(),
        also_in: Vec::new(),
    }
}

const FIRST: &str = "20261123T184002.000000000Z-10-0.md";
const SECOND: &str = "20261123T185541.000000000Z-11-0.md";

#[test]
fn a_pr_number_is_digits_not_starting_with_zero_and_fits_64_bits() {
    for good in ["1", "7", "3129", "18446744073709551615"] {
        assert!(pr_number(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "0",
        "07",
        "x7",
        "+7",
        "-1",
        " 7",
        "7 ",
        "18446744073709551616",
    ] {
        let err = pr_number(bad).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{bad:?}");
    }
}

#[test]
fn the_pull_request_is_a_url_or_pr_with_pr_repo_and_never_both() {
    let url = named_pr(&args(Some("https://github.com/o/r/pull/7"), None, None)).unwrap();
    let flags = named_pr(&args(None, Some("7"), Some("o/r"))).unwrap();
    assert_eq!(url, flags);
    assert_eq!(named_pr(&args(None, None, None)).unwrap(), None);
    for bad in [
        args(Some("7"), None, None),
        args(Some("https://github.com/o/r/issues/7"), None, None),
        args(
            Some("https://github.com/o/r/pull/7"),
            Some("7"),
            Some("o/r"),
        ),
        args(None, Some("7"), None),
        args(None, None, Some("o/r")),
        args(None, Some("7"), Some("o")),
        args(None, Some("7"), Some("o/r/x")),
        args(None, Some("7"), Some("o/..")),
        args(None, Some("7"), Some("../r")),
    ] {
        let err = named_pr(&bad).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{bad:?}: {err}");
    }
}

#[test]
fn since_is_empty_or_an_entry_name() {
    assert!(check_since("").is_ok());
    assert!(check_since(FIRST).is_ok());
    for bad in ["../etc/passwd", "x.md", "20261123T184002Z-1-0.md", " "] {
        assert_eq!(check_since(bad).unwrap_err().exit_code(), 2, "{bad:?}");
    }
}

#[test]
fn json_lists_metadata_marks_new_entries_and_never_holds_text() {
    let (_t, root) = private_root();
    let run = root.join("runs").join("r1").join(GUIDANCE_DIR);
    let pr = root.join(AUTO_DRIVE_DIR).join("k").join(GUIDANCE_DIR);
    let list = vec![
        Folder {
            path: run.clone(),
            check: DirCheck::Trusted,
        },
        Folder {
            path: pr.clone(),
            check: DirCheck::Untrusted("it is a symbolic link".to_string()),
        },
        Folder {
            path: root.join("gone").join(GUIDANCE_DIR),
            check: DirCheck::Missing,
        },
    ];
    let entries = vec![
        entry(FIRST, &run, "alice", "hold the merge ZQTEXT1"),
        entry(SECOND, &run, "alice run=abc-1", "a step note ZQTEXT2"),
    ];
    let line = serde_json::to_string(&show_json(&list, &entries, FIRST)).unwrap();
    assert!(!line.contains("ZQTEXT"), "no guidance text: {line}");
    assert!(!line.contains("\"text\""), "no text key: {line}");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["since"], FIRST);
    assert_eq!(v["newest"], SECOND);
    let folders = v["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 3);
    assert_eq!(folders[0]["state"], "present");
    assert_eq!(folders[0]["source"], "run");
    assert!(
        folders[0].get("reason").is_none(),
        "reason only when untrusted"
    );
    assert_eq!(folders[1]["state"], "untrusted");
    assert_eq!(folders[1]["source"], "pr");
    assert_eq!(folders[1]["reason"], "it is a symbolic link");
    assert_eq!(folders[2]["state"], "missing");
    let e = v["entries"].as_array().unwrap();
    assert_eq!(e[0]["name"], FIRST);
    assert_eq!(e[0]["new"], false, "the entry at --since is not new");
    assert_eq!(e[0]["from_step"], false);
    assert_eq!(e[0]["bytes"], "hold the merge ZQTEXT1".len());
    assert_eq!(e[0]["folder"], run.to_str().unwrap());
    assert_eq!(e[1]["new"], true);
    assert_eq!(e[1]["from_step"], true);
    // Issue #3162 adds `read` to schema 1; no other key is added.
    assert_eq!(e[0]["read"], false, "no guidance.log names it");
    let keys: Vec<&String> = e[0].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 9, "{keys:?}");
    assert_eq!(v["drops"], serde_json::json!([]));
}

#[test]
fn json_with_no_entries_and_no_folders_is_still_one_valid_document() {
    let line = serde_json::to_string(&show_json(&[], &[], "")).unwrap();
    assert_eq!(
        line,
        r#"{"schema_version":1,"since":"","newest":"","folders":[],"entries":[],"drops":[]}"#
    );
}

#[test]
fn a_path_with_a_terminal_sequence_is_escaped_and_the_json_stays_valid() {
    let dir = PathBuf::from("/x/\u{1b}[31mred\u{202e}/guidance");
    let list = vec![Folder {
        path: dir,
        check: DirCheck::Missing,
    }];
    let line = serde_json::to_string(&show_json(&list, &[], "")).unwrap();
    assert_eq!(sanitize_line(&line), line, "nothing left to escape");
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        v["folders"][0]["path"],
        "/x/\\u{001b}[31mred\\u{202e}/guidance"
    );
}

#[test]
fn event_lines_have_the_documented_shapes() {
    let line = |e: &Event<'_>| serde_json::to_string(e).unwrap();
    assert_eq!(
        line(&guidance_event(SECOND)),
        format!(r#"{{"event":"guidance","entry":"{SECOND}"}}"#)
    );
    assert_eq!(
        line(&Event {
            event: "unarmed",
            ..Event::default()
        }),
        r#"{"event":"unarmed"}"#
    );
    let exited = command_event(ExitStatus::from_raw(79 << 8)).unwrap();
    assert_eq!(line(&exited), r#"{"event":"command","exit":79}"#);
    let killed = command_event(ExitStatus::from_raw(libc::SIGTERM)).unwrap();
    assert_eq!(
        line(&killed),
        r#"{"event":"command","exit":143,"signal":15}"#
    );
}

#[test]
fn plain_show_marks_entries_after_since_and_names_each_entry() {
    let (_t, root) = private_root();
    let run = root.join(GUIDANCE_DIR);
    let entries = vec![
        entry(FIRST, &run, "alice", "one"),
        entry(SECOND, &run, "alice", "two"),
    ];
    let block = render_block_marked(&entries, Some(FIRST));
    let headers: Vec<&str> = block.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(
        headers,
        [
            format!("[1] 2026-11-23T18:40:02.000000000Z from alice (run) {FIRST}"),
            format!("[2] 2026-11-23T18:40:02.000000000Z from alice (run) {SECOND} NEW"),
        ]
    );
    let all_new = render_block_marked(&entries, Some(""));
    assert_eq!(all_new.lines().filter(|l| l.ends_with(" NEW")).count(), 2);
    assert_eq!(
        render_block_marked(&entries, None),
        crate::run_guidance::render_block(&entries),
        "no mark: the agent-prompt block, byte for byte"
    );
    assert!(!render_block_marked(&entries, None).contains(FIRST));
}

#[test]
fn listed_folders_keep_missing_and_untrusted_ones_with_their_state() {
    let (_t, root) = private_root();
    let present = root.join("a").join(GUIDANCE_DIR);
    fs::create_dir_all(&present).unwrap();
    for d in [root.join("a"), present.clone()] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let missing = root.join("b").join(GUIDANCE_DIR);
    let open = root.join("c").join(GUIDANCE_DIR);
    fs::create_dir_all(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
    let value = std::env::join_paths([&present, &missing, &open, &present]).unwrap();
    let listed = listed_dirs(&value);
    assert_eq!(listed.len(), 3, "duplicates dropped: {listed:?}");
    assert_eq!(listed[0], (present, DirCheck::Trusted));
    assert_eq!(listed[1], (missing, DirCheck::Missing));
    assert!(matches!(&listed[2], (p, DirCheck::Untrusted(_)) if *p == open));
}

/// Issue #3162: a `guidance.log` that cannot be read leaves the read state
/// unknown. `show --json` says so with `read: null` and the folder's
/// `log_error`, and plain `show` with a line saying the state is unknown.
#[test]
fn an_unreadable_log_makes_read_null_and_sets_the_folder_log_error() {
    let (_t, root) = private_root();
    let run_dir = root.join("runs").join("r1");
    let run = run_dir.join(GUIDANCE_DIR);
    fs::create_dir_all(&run).unwrap();
    let target = root.join("elsewhere.log");
    fs::write(
        &target,
        format!("{{\"event\":\"guidance_injected\",\"entry_names\":[\"{FIRST}\"]}}\n"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, run_dir.join(crate::run_guidance::GUIDANCE_LOG)).unwrap();
    let list = vec![Folder {
        path: run.clone(),
        check: DirCheck::Trusted,
    }];
    let entries = vec![entry(FIRST, &run, "alice", "hold")];
    let line = serde_json::to_string(&show_json(&list, &entries, "")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["entries"][0]["read"], serde_json::Value::Null, "{line}");
    assert!(
        v["entries"][0].as_object().unwrap().contains_key("read"),
        "read is present as null: {line}"
    );
    assert_eq!(
        v["folders"][0]["log_error"], "it is a symbolic link",
        "{line}"
    );
    assert_eq!(v["drops"], serde_json::json!([]));
    assert_eq!(
        read_and_drop_lines(&list, &entries),
        [format!(
            "entry {FIRST}: read state unknown (log unreadable)"
        )]
    );
}

#[test]
fn a_readable_log_in_another_copy_proves_the_entry_read_despite_an_unreadable_one() {
    let (_t, root) = private_root();
    let bad_dir = root.join("runs").join("bad");
    let bad = bad_dir.join(GUIDANCE_DIR);
    let good_dir = root.join("runs").join("good");
    let good = good_dir.join(GUIDANCE_DIR);
    fs::create_dir_all(&bad).unwrap();
    fs::create_dir_all(&good).unwrap();
    std::os::unix::fs::symlink(
        "/nonexistent",
        bad_dir.join(crate::run_guidance::GUIDANCE_LOG),
    )
    .unwrap();
    let log = good_dir.join(crate::run_guidance::GUIDANCE_LOG);
    fs::write(
        &log,
        format!("{{\"event\":\"guidance_injected\",\"entry_names\":[\"{FIRST}\"]}}\n"),
    )
    .unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    let list = vec![
        Folder {
            path: bad.clone(),
            check: DirCheck::Trusted,
        },
        Folder {
            path: good.clone(),
            check: DirCheck::Trusted,
        },
    ];
    let mut e = entry(FIRST, &bad, "alice", "hold");
    e.also_in.push((good.clone(), FIRST.to_string()));
    let v = serde_json::to_value(show_json(&list, &[e], "")).unwrap();
    assert_eq!(v["entries"][0]["read"], true, "{v}");
    assert!(v["folders"][0].get("log_error").is_some(), "{v}");
    assert!(v["folders"][1].get("log_error").is_none(), "{v}");
}

/// A run folder `runs/<name>` with a guidance folder and a `run.json`
/// holding `report`.
fn run_folder(root: &Path, name: &str, report: &str) -> PathBuf {
    let run_dir = root.join("runs").join(name);
    let guidance = run_dir.join(GUIDANCE_DIR);
    fs::create_dir_all(&guidance).unwrap();
    let path = run_dir.join("run.json");
    fs::write(&path, report).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    guidance
}

const ENDED: &str =
    r#"{"schema_version":1,"run_id":"r1","ended_at":"2026-11-24T18:59:41Z","outcome":"success"}"#;
const RUNNING: &str = r#"{"schema_version":1,"run_id":"r2","outcome":""}"#;

/// PR #3190 review: an entry that no step read before its run ended says
/// so, in plain `show` and in JSON. Nothing is inferred: only the end the
/// run's `run.json` records is reported.
#[test]
fn an_unread_entry_of_a_run_that_recorded_its_end_says_the_run_ended() {
    let (_t, root) = private_root();
    let run = run_folder(&root, "r1", ENDED);
    let list = vec![Folder {
        path: run.clone(),
        check: DirCheck::Trusted,
    }];
    let entries = vec![entry(FIRST, &run, "alice", "hold")];
    assert_eq!(
        read_and_drop_lines(&list, &entries),
        [format!(
            "entry {FIRST}: not yet read; the run ended at 2026-11-24T18:59:41Z (success)"
        )]
    );
    let v = serde_json::to_value(show_json(&list, &entries, "")).unwrap();
    assert_eq!(v["entries"][0]["read"], false, "{v}");
    assert_eq!(
        v["entries"][0]["run_ended"],
        serde_json::json!({"at": "2026-11-24T18:59:41Z", "outcome": "success"}),
        "{v}"
    );
}

#[test]
fn a_run_with_no_recorded_end_or_a_read_entry_adds_nothing() {
    let (_t, root) = private_root();
    // No end recorded: the run may still start a step.
    let running = run_folder(&root, "r2", RUNNING);
    let list = vec![Folder {
        path: running.clone(),
        check: DirCheck::Trusted,
    }];
    let entries = vec![entry(FIRST, &running, "alice", "hold")];
    assert_eq!(
        read_and_drop_lines(&list, &entries),
        [format!("entry {FIRST}: not yet read")]
    );
    let v = serde_json::to_value(show_json(&list, &entries, "")).unwrap();
    assert!(v["entries"][0].get("run_ended").is_none(), "{v}");

    // Read before the run ended: `read`, with no suffix.
    let ended = run_folder(&root, "r1", ENDED);
    let log = ended
        .parent()
        .unwrap()
        .join(crate::run_guidance::GUIDANCE_LOG);
    fs::write(
        &log,
        format!("{{\"event\":\"guidance_injected\",\"entry_names\":[\"{FIRST}\"]}}\n"),
    )
    .unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    let list = vec![Folder {
        path: ended.clone(),
        check: DirCheck::Trusted,
    }];
    let entries = vec![entry(FIRST, &ended, "alice", "hold")];
    assert_eq!(
        read_and_drop_lines(&list, &entries),
        [format!("entry {FIRST}: read")]
    );
}

/// A copy in a run that has not recorded an end means a step of that run
/// may still read it, so the entry is not said to be stranded.
#[test]
fn a_copy_in_a_run_with_no_recorded_end_keeps_the_plain_status() {
    let (_t, root) = private_root();
    let ended = run_folder(&root, "r1", ENDED);
    let running = run_folder(&root, "r2", RUNNING);
    let list = vec![
        Folder {
            path: ended.clone(),
            check: DirCheck::Trusted,
        },
        Folder {
            path: running.clone(),
            check: DirCheck::Trusted,
        },
    ];
    let mut e = entry(FIRST, &ended, "alice", "hold");
    e.also_in.push((running.clone(), FIRST.to_string()));
    assert_eq!(
        read_and_drop_lines(&list, std::slice::from_ref(&e)),
        [format!("entry {FIRST}: not yet read")]
    );
    let v = serde_json::to_value(show_json(&list, &[e], "")).unwrap();
    assert!(v["entries"][0].get("run_ended").is_none(), "{v}");
}

/// A pull request's folder is read by a later run, so it has no run end.
#[test]
fn a_pull_request_folder_has_no_run_end() {
    let (_t, root) = private_root();
    let pr = root
        .join(AUTO_DRIVE_DIR)
        .join("https___github.com_o_r_1")
        .join(GUIDANCE_DIR);
    fs::create_dir_all(&pr).unwrap();
    fs::write(pr.parent().unwrap().join("run.json"), ENDED).unwrap();
    assert!(super::super::live_runs::run_end(&pr).is_none());
}

/// The recorded values are a claim and are printed escaped and bounded.
#[test]
fn a_recorded_end_is_escaped_and_bounded() {
    let (_t, root) = private_root();
    let long = "x".repeat(500);
    let report = format!(
        r#"{{"schema_version":1,"run_id":"r3","ended_at":"2026\u001b]0;t\u0007","outcome":"{long}"}}"#
    );
    let run = run_folder(&root, "r3", &report);
    let end = super::super::live_runs::run_end(&run).expect("an end is recorded");
    assert!(!end.at.contains('\u{1b}'), "{:?}", end.at);
    assert!(end.at.contains("\\u{001b}"), "{:?}", end.at);
    assert_eq!(end.outcome.chars().count(), 64, "{:?}", end.outcome);
}
