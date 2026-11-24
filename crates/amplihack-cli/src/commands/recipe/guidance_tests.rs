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
