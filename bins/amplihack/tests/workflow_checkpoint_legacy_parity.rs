//! Legacy parity of `amplihack internal workflow-checkpoint` at the binary
//! boundary (#1554). The 918c256cf Python is the contract; the only
//! authorised departures are the two 17.126 rows (destination-read 79 and
//! stage-strict 79), which are not exercised here.
//!
//! Every expected status and byte string below was recorded on 2026-11-07 by
//! running the fragment extracted with `git show 918c256cf:<path>` under
//! `python3 -I`, outside the repository:
//!
//! * `ORIGIN_REF_PY`: `workflow_pr_head_sync.sh:83-92`
//!   (sha256 e4fbb280abb1693c1c9e263bd70b076cca4220e2219368f72fd993c28f0b6472);
//! * `ADMISSION_RESULT_PY`: `workflow_publish_checkpoint.sh:1155` and `:1184`;
//! * `CLEANUP_SKIP_JSON`: `workflow_cleanup_checkpoint.sh:17-20`;
//! * the recipe extraction at `workflow-publish.yaml:137`.
//!
//! Rows that conflict with draft-era tests in `workflow_checkpoint_actions.rs`
//! and `workflow_checkpoint_binary_contract.rs` are deliberate: those tests
//! pin draft-only behaviour that the owner has not authorised.

use std::fs;
use std::process::Output;

use crate::workflow_checkpoint_support::*;

/// `advertised-ref <saved-output-file> <ref> <ls-remote-rc>`, the 918c argv
/// order (`workflow_pr_head_sync.sh:82`).
fn advertised_file(sb: &Sandbox, body: &[u8], target: &str, rc: &str) -> Output {
    let saved = sb.tmp().join("saved-advertisement");
    fs::write(&saved, body).unwrap();
    sb.checkpoint(
        "advertised-ref",
        [saved.as_os_str(), target.as_ref(), rc.as_ref()],
    )
    .output()
    .unwrap()
}

fn rec(oid: &str, name: &str, end: &str) -> String {
    format!("{oid}\t{name}{end}")
}

// D1 (three accepted inputs) is in workflow_checkpoint_d1_literals.rs; the
// validate-recipe-runner contract is in workflow_checkpoint_restorations.rs
// (`validate_recipe_runner_failure_prints_json_only`).

// ----------------------------------------------- recorded status table

#[test]
fn legacy_advertised_ref_status_table_matches_918c() {
    let a = oid1('a');
    let b = oid256('b');
    let c = oid1('c');
    let upper = oid1('a').to_uppercase();
    let rows: Vec<(&str, String, &str, &str, i32)> = vec![
        ("empty_rc0", String::new(), "refs/heads/x", "0", 3),
        ("empty_rc2", String::new(), "refs/heads/x", "2", 3),
        (
            "present",
            rec(&a, "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            0,
        ),
        (
            "absent",
            rec(&a, "refs/heads/y", "\n"),
            "refs/heads/x",
            "0",
            3,
        ),
        ("head_only", rec(&a, "HEAD", "\n"), "HEAD", "0", 0),
        (
            "rc2_with_records",
            rec(&a, "refs/heads/x", "\n"),
            "refs/heads/x",
            "2",
            78,
        ),
        (
            "rc5_parsed_anyway",
            rec(&a, "refs/heads/x", "\n"),
            "refs/heads/x",
            "5",
            0,
        ),
        (
            "blank_interior",
            format!(
                "{}\n{}",
                rec(&a, "refs/heads/x", "\n"),
                rec(&c, "refs/heads/y", "\n")
            ),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "trailing_double_lf",
            rec(&a, "refs/heads/x", "\n\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "lone_cr_separator",
            format!(
                "{}{}",
                rec(&a, "refs/heads/a", "\r"),
                rec(&c, "refs/heads/b", "\n")
            ),
            "refs/heads/b",
            "0",
            0,
        ),
        (
            "lone_cr_one_field",
            format!("{}junk\n", rec(&a, "refs/heads/a", "\r")),
            "refs/heads/a",
            "0",
            78,
        ),
        (
            "three_fields",
            rec(&a, "refs/heads/x\textra", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        ("one_field", format!("{a}\n"), "refs/heads/x", "0", 78),
        (
            "space_separator",
            format!("{a} refs/heads/x\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "empty_oid",
            rec("", "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "bad_prefix",
            rec(&a, "heads/x", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "uppercase_oid",
            rec(&upper, "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "oid39",
            rec(&a[1..], "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "oid41",
            rec(&format!("{a}a"), "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "duplicate_same_oid",
            format!("{0}{0}", rec(&a, "refs/heads/x", "\n")),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "duplicate_different_oid",
            format!(
                "{}{}",
                rec(&a, "refs/heads/x", "\n"),
                rec(&c, "refs/heads/x", "\n")
            ),
            "refs/heads/x",
            "0",
            78,
        ),
        (
            "vt_kept_in_field",
            rec(&a, "refs/heads/a\x0bb", "\n"),
            "refs/heads/a\x0bb",
            "0",
            0,
        ),
        (
            "ff_kept_in_field",
            rec(&a, "refs/heads/a\x0cb", "\n"),
            "refs/heads/a\x0cb",
            "0",
            0,
        ),
        (
            "fs_kept_in_field",
            rec(&a, "refs/heads/a\x1cb", "\n"),
            "refs/heads/a\x1cb",
            "0",
            0,
        ),
        (
            "gs_kept_in_field",
            rec(&a, "refs/heads/a\x1db", "\n"),
            "refs/heads/a\x1db",
            "0",
            0,
        ),
        (
            "rs_kept_in_field",
            rec(&a, "refs/heads/a\x1eb", "\n"),
            "refs/heads/a\x1eb",
            "0",
            0,
        ),
        (
            "vt_prefix_is_absent",
            rec(&a, "refs/heads/a\x0bb", "\n"),
            "refs/heads/a",
            "0",
            3,
        ),
        (
            "prefix_variant_absent",
            rec(&a, "refs/heads/x2", "\n"),
            "refs/heads/x",
            "0",
            3,
        ),
        (
            "case_variant_absent",
            rec(&a, "refs/heads/X", "\n"),
            "refs/heads/x",
            "0",
            3,
        ),
        (
            "sha256_present",
            rec(&b, "refs/heads/x", "\n"),
            "refs/heads/x",
            "0",
            0,
        ),
        (
            "mixed_widths_second",
            format!(
                "{}{}",
                rec(&a, "refs/heads/a", "\n"),
                rec(&b, "refs/heads/b", "\n")
            ),
            "refs/heads/b",
            "0",
            0,
        ),
        (
            "crlf_then_lf",
            format!(
                "{}{}",
                rec(&a, "refs/heads/x", "\r\n"),
                rec(&c, "refs/heads/y", "\n")
            ),
            "refs/heads/y",
            "0",
            0,
        ),
        (
            "nul_kept_in_field",
            rec(&a, "refs/heads/x\0z", "\n"),
            "refs/heads/x",
            "0",
            3,
        ),
        // Python's int() strips whitespace and accepts a sign.
        (
            "rc_with_spaces",
            rec(&a, "refs/heads/x", "\n"),
            "refs/heads/x",
            " 0 ",
            0,
        ),
        (
            "rc_negative_zero",
            rec(&a, "refs/heads/x", "\n"),
            "refs/heads/x",
            "-0",
            0,
        ),
    ];
    let mut wrong = Vec::new();
    for (case, body, target, rc, expected) in rows {
        let sb = Sandbox::new();
        let out = advertised_file(&sb, body.as_bytes(), target, rc);
        if status(&out) != expected {
            wrong.push(format!(
                "{case}: 918c {expected}, Rust {} ({})",
                status(&out),
                stderr(&out).trim()
            ));
        }
        if !out.stdout.is_empty() {
            wrong.push(format!("{case}: 918c printed no stdout"));
        }
        if !out.stderr.is_empty() {
            wrong.push(format!("{case}: 918c printed no stderr for {expected}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "drift from 918c ORIGIN_REF_PY:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn legacy_advertised_ref_keeps_a_nel_byte_inside_a_non_utf8_ref() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let sb = Sandbox::new();
    let mut body = oid1('a').into_bytes();
    body.extend_from_slice(b"\trefs/heads/a\x85b\n");
    let saved = sb.tmp().join("saved");
    fs::write(&saved, &body).unwrap();
    let out = sb
        .checkpoint(
            "advertised-ref",
            [
                saved.as_os_str(),
                OsStr::from_bytes(b"refs/heads/a\x85b"),
                OsStr::new("0"),
            ],
        )
        .output()
        .unwrap();
    assert_eq!(
        status(&out),
        0,
        "918c: \\x85 is not a bytes.splitlines separator"
    );
}

// ------------------------------------------------- argv shape and crashes

#[test]
fn legacy_advertised_ref_crash_shapes_exit_1() {
    // ORIGIN_REF_PY has no try block: each of these is an uncaught Python
    // exception, status 1.
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved");
    fs::write(&saved, rec(&oid1('a'), "refs/heads/x", "\n")).unwrap();
    let dir = sb.tmp().join("a-directory");
    fs::create_dir(&dir).unwrap();
    let saved = saved.to_str().unwrap().to_owned();
    let missing = sb.tmp().join("missing").to_str().unwrap().to_owned();
    let dir = dir.to_str().unwrap().to_owned();
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("non_integer_rc", vec![&saved, "refs/heads/x", "notint"]),
        ("missing_rc", vec![&saved, "refs/heads/x"]),
        ("no_arguments", vec![]),
        ("unreadable_file", vec![&missing, "refs/heads/x", "0"]),
        ("directory_as_file", vec![&dir, "refs/heads/x", "0"]),
        // The draft's <remote> <ref> order would put the ref where the rc
        // belongs; at 918c int('refs/heads/x') fails, so the order is pinned.
        ("ref_and_rc_swapped", vec![&saved, "0", "refs/heads/x"]),
    ];
    let mut wrong = Vec::new();
    for (case, args) in rows {
        let out = sb.checkpoint("advertised-ref", &args).output().unwrap();
        if status(&out) != 1 {
            wrong.push(format!("{case}: 918c 1, Rust {}", status(&out)));
        }
        if !out.stdout.is_empty() {
            wrong.push(format!("{case}: stdout must stay empty"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn legacy_advertised_ref_ignores_trailing_arguments() {
    // ORIGIN_REF_PY only indexes sys.argv[1..3].
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved");
    fs::write(&saved, rec(&oid1('a'), "refs/heads/x", "\n")).unwrap();
    let out = sb
        .checkpoint(
            "advertised-ref",
            [
                saved.to_str().unwrap(),
                "refs/heads/x",
                "0",
                "extra",
                "more",
            ],
        )
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn legacy_advertised_ref_never_runs_git_itself() {
    // The shell runs ls-remote; the engine only parses the saved bytes.
    let sb = Sandbox::new();
    let shim = sb.git_shim(b"", 0);
    let out = advertised_file(
        &sb,
        rec(&oid1('a'), "refs/heads/x", "\n").as_bytes(),
        "refs/heads/x",
        "0",
    );
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        shim.ls_remote_calls(),
        0,
        "engine ran ls-remote: {}",
        shim.calls()
    );
    assert!(shim.calls().is_empty(), "engine ran git: {}", shim.calls());
}

// --------------------------------------- selector scope (918c had none)

#[test]
fn legacy_helpers_without_a_selector_check_still_run_with_an_inherited_selector() {
    // ORIGIN_REF_PY, CLEANUP_SKIP_JSON, ADMISSION_RESULT_PY, the inline
    // sha256 call and the recipe worktree_path extraction never looked at
    // Git selectors. Refusing them is a draft-only difference.
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved");
    fs::write(&saved, rec(&oid1('a'), "refs/heads/x", "\n")).unwrap();
    let saved = saved.to_str().unwrap().to_owned();
    let rows: Vec<(&str, Vec<&str>, i32)> = vec![
        ("advertised-ref", vec![&saved, "refs/heads/x", "0"], 0),
        ("cleanup-skip-result", vec!["worktree path gone"], 0),
        ("admission-refused-result", vec![], 0),
        ("sha256", vec![], 0),
        ("worktree-path", vec![], 0),
    ];
    let mut wrong = Vec::new();
    for (action, args, expected) in rows {
        let out = sb
            .checkpoint(action, &args)
            .env("GIT_DIR", sb.path().join("not-a-repo"))
            .env(
                "RECIPE_VAR_worktree_setup",
                r#"{"worktree_path":"/w","branch_name":"b"}"#,
            )
            .output()
            .unwrap();
        if status(&out) != expected {
            wrong.push(format!(
                "{action}: 918c {expected}, Rust {} ({})",
                status(&out),
                stderr(&out).trim()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ------------------------------------------------ admission-refused-result

fn admission(sb: &Sandbox, env: Option<&str>) -> Output {
    let mut c = sb.checkpoint("admission-refused-result", [""; 0]);
    if let Some(v) = env {
        c.env("RECIPE_VAR_worktree_setup", v);
    }
    c.output().unwrap()
}

fn admission_line(branch_json: &str) -> Vec<u8> {
    format!(
        "{{\"pushed\": \"false\", \"sha\": \"\", \"branch\": {branch_json}, \"reason\": \"scope-admission-refused\"}}\n"
    )
    .into_bytes()
}

#[test]
fn legacy_admission_refused_result_echoes_a_non_string_branch_name() {
    let rows: [(&str, &str); 7] = [
        (r#"{"branch_name": 7}"#, "7"),
        (r#"{"branch_name": ["a"]}"#, r#"["a"]"#),
        (r#"{"branch_name": null}"#, "null"),
        (r#"{"branch_name": true}"#, "true"),
        (r#"{"branch_name": 1.5}"#, "1.5"),
        (r#"{"branch_name": {"b":1,"a":2}}"#, r#"{"b": 1, "a": 2}"#),
        (r#"{"branch_name":"a","branch_name":"b"}"#, r#""b""#),
    ];
    let mut wrong = Vec::new();
    for (input, branch) in rows {
        let sb = Sandbox::new();
        let out = admission(&sb, Some(input));
        if status(&out) != 0 || out.stdout != admission_line(branch) {
            wrong.push(format!(
                "{input}: 918c {:?}, Rust {} {:?}",
                String::from_utf8_lossy(&admission_line(branch)),
                status(&out),
                String::from_utf8_lossy(&out.stdout)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn legacy_admission_refused_result_falls_back_to_empty_branch_on_value_or_attribute_error() {
    // `except (ValueError, AttributeError): branch = ''`.
    let rows: [Option<&str>; 8] = [
        None,
        Some(""),
        Some("not json"),
        Some("[1,2]"),
        Some("{}"),
        Some(r#""str""#),
        Some("7"),
        Some("null"),
    ];
    for input in rows {
        let sb = Sandbox::new();
        let out = admission(&sb, input);
        assert_eq!(status(&out), 0, "{input:?}: {}", stderr(&out));
        assert_eq!(out.stdout, admission_line("\"\""), "{input:?}");
    }
}

#[test]
fn legacy_admission_refused_result_keeps_python_number_and_surrogate_spellings() {
    // Recorded from 918c; listed separately so an owner can disposition them.
    let rows: [(&str, &str); 4] = [
        (r#"{"branch_name": NaN}"#, "NaN"),
        (r#"{"branch_name": 1e400}"#, "Infinity"),
        (
            r#"{"branch_name": 12345678901234567890123}"#,
            "12345678901234567890123",
        ),
        (r#"{"branch_name": "\ud800"}"#, r#""\ud800""#),
    ];
    let mut wrong = Vec::new();
    for (input, branch) in rows {
        let sb = Sandbox::new();
        let out = admission(&sb, Some(input));
        if status(&out) != 0 || out.stdout != admission_line(branch) {
            wrong.push(format!(
                "{input}: Rust {} {:?}",
                status(&out),
                String::from_utf8_lossy(&out.stdout)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ------------------------------------------------------------ worktree-path

fn worktree_path(sb: &Sandbox, env: Option<&str>) -> Output {
    let mut c = sb.checkpoint("worktree-path", [""; 0]);
    if let Some(v) = env {
        c.env("RECIPE_VAR_worktree_setup", v);
    }
    c.output().unwrap()
}

#[test]
fn legacy_worktree_path_prints_an_empty_path_with_status_0() {
    let sb = Sandbox::new();
    let out = worktree_path(&sb, Some(r#"{"worktree_path":""}"#));
    assert_eq!(
        status(&out),
        0,
        "918c printed an empty line; stderr: {}",
        stderr(&out)
    );
    assert_eq!(out.stdout, b"\n");
}

#[test]
fn legacy_worktree_path_takes_the_last_duplicate_and_keeps_newlines() {
    let sb = Sandbox::new();
    let out = worktree_path(&sb, Some(r#"{"worktree_path":"a","worktree_path":"b"}"#));
    assert_eq!((status(&out), out.stdout.as_slice()), (0, &b"b\n"[..]));
    let out = worktree_path(&sb, Some(r#"{"worktree_path":"a\nb"}"#));
    assert_eq!((status(&out), out.stdout.as_slice()), (0, &b"a\nb\n"[..]));
}

#[test]
fn legacy_worktree_path_crashes_with_status_1_and_no_stdout() {
    // KeyError, JSONDecodeError and TypeError were uncaught: status 1. The
    // missing-variable case never read stdin.
    for input in [None, Some("{}"), Some("not json"), Some("[1]")] {
        let sb = Sandbox::new();
        let out = worktree_path(&sb, input);
        assert_eq!(status(&out), 1, "{input:?}: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "{input:?}");
    }
}

#[test]
fn legacy_worktree_path_prints_python_str_of_a_non_string_value() {
    // print(7) and print(None). Listed for owner disposition if not kept.
    let sb = Sandbox::new();
    let out = worktree_path(&sb, Some(r#"{"worktree_path":7}"#));
    assert_eq!((status(&out), out.stdout.as_slice()), (0, &b"7\n"[..]));
    let out = worktree_path(&sb, Some(r#"{"worktree_path":null}"#));
    assert_eq!((status(&out), out.stdout.as_slice()), (0, &b"None\n"[..]));
}

// ------------------------------------------------------ cleanup-skip-result

#[test]
fn legacy_cleanup_skip_result_ignores_extra_arguments_and_crashes_without_one() {
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("cleanup-skip-result", ["a", "b"])
        .output()
        .unwrap();
    assert_eq!(
        status(&out),
        0,
        "918c used sys.argv[1] only: {}",
        stderr(&out)
    );
    assert_eq!(
        out.stdout,
        b"{\"skipped\": true, \"admitted\": false, \"reason\": \"a\"}\n"
    );
    let out = sb
        .checkpoint("cleanup-skip-result", [""; 0])
        .output()
        .unwrap();
    assert_eq!(status(&out), 1, "IndexError at 918c is status 1");
    assert!(out.stdout.is_empty());
}

// ---------------------------------------------------- main.rs exit scope

#[test]
fn workflow_checkpoint_statuses_pass_through_without_an_error_line() {
    let sb = Sandbox::new();
    let out = sb.checkpoint("no-such-action", [""; 0]).output().unwrap();
    assert_eq!(status(&out), 2);
    assert!(
        !stderr(&out).contains("error: cli exited"),
        "{}",
        stderr(&out)
    );
}

// ------------------------------------------------- D2 authority identity

/// SHA-256 by the system tool, so the test adds no dependency and does not
/// trust the engine's own `sha256` action.
fn sha256_hex(bytes: &[u8]) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("sha256sum");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()[..64].to_owned()
}

fn dev_ino(path: &std::path::Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    let m = fs::symlink_metadata(path).unwrap();
    (m.dev(), m.ino())
}

/// Run `check` against an authority record whose recorded identity is
/// `old` and whose bytes hash to the recorded digest.
fn check_with_record(
    sb: &Sandbox,
    work: &std::path::Path,
    record: &std::path::Path,
    old: (u64, u64),
    bytes: &[u8],
) -> Output {
    sb.checkpoint("check", [""; 0])
        .current_dir(work)
        .env("WF_CHECKPOINT_RECORD", record)
        .env("WF_CHECKPOINT_RECORD_ID", format!("{}:{}", old.0, old.1))
        .env("WF_CHECKPOINT_HASH", sha256_hex(bytes))
        .output()
        .unwrap()
}

#[test]
fn authority_refuses_substitution() {
    // D2: the original inode stays alive through a hard link, so the
    // substitute cannot reuse its (st_dev, st_ino). Same bytes, same hash;
    // only the identity differs.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let private = sb.tmp().join("authority");
    fs::create_dir(&private).unwrap();
    let record = private.join("record.json");
    let bytes = b"{\"binding\": {\"phase\": \"publish\"}}\n";
    fs::write(&record, bytes).unwrap();
    let old = dev_ino(&record);
    fs::hard_link(&record, private.join("original-kept-alive")).unwrap();
    let substitute = private.join("substitute");
    fs::write(&substitute, bytes).unwrap();
    fs::rename(&substitute, &record).unwrap();
    let new = dev_ino(&record);
    assert_ne!(
        new, old,
        "the substitute reused the original identity; the D2 check would prove nothing"
    );
    assert_eq!(dev_ino(&private.join("original-kept-alive")), old);
    assert_eq!(fs::read(&record).unwrap(), bytes);

    let out = check_with_record(&sb, &work, &record, old, bytes);
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).contains("authority record identity changed"),
        "the refusal must come from the identity check: {}",
        stderr(&out)
    );
}

#[test]
fn authority_identity_check_passes_for_the_original_inode() {
    // Companion to authority_refuses_substitution: with the identity intact
    // the run gets past the identity check (it may refuse later for other
    // reasons), so the refusal above is caused by the substitution.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let private = sb.tmp().join("authority");
    fs::create_dir(&private).unwrap();
    let record = private.join("record.json");
    let bytes = b"{\"binding\": {\"phase\": \"publish\"}}\n";
    fs::write(&record, bytes).unwrap();
    let old = dev_ino(&record);
    let out = check_with_record(&sb, &work, &record, old, bytes);
    assert!(
        !stderr(&out).contains("authority record identity changed"),
        "{}",
        stderr(&out)
    );
}
