//! Binary-boundary contract of `amplihack internal workflow-checkpoint`
//! (#1554). Statuses 2, 78, 79 and 3 must leave the process unchanged; a
//! refusing run must not touch HOME; stdout is empty on every non-zero exit.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use crate::workflow_checkpoint_support::*;

#[test]
fn actions_match_the_engine_table() {
    let engine: Vec<&str> = amplihack_cli::workflow_checkpoint::ACTIONS
        .iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(engine, ACTIONS);
}

#[test]
fn unknown_action_refuses_with_2_like_the_918c_dispatch() {
    let sb = Sandbox::new();
    let out = sb.checkpoint("no-such-action", [""; 0]).output().unwrap();
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty(), "stdout must be empty on refusal");
    assert_eq!(
        stderr(&out),
        "checkpoint scope refusal: unknown private checkpoint operation\n"
    );
}

#[test]
fn missing_action_is_the_918c_index_error_with_no_stdout() {
    // 918c `action = sys.argv[1]` raised IndexError: status 1.
    let sb = Sandbox::new();
    let mut c = sb.command(AMPLIHACK);
    c.args(["internal", "workflow-checkpoint"]);
    let out = c.output().unwrap();
    assert_eq!(status(&out), 1, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).starts_with("checkpoint internal error: "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn help_never_writes_record_shaped_output_to_stdout() {
    let sb = Sandbox::new();
    let mut c = sb.command(AMPLIHACK);
    c.args(["internal", "workflow-checkpoint", "--help"]);
    let out = c.output().unwrap();
    assert!(!out.stdout.contains(&0), "help printed a NUL record");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.trim_start().starts_with('{'),
        "help printed JSON: {text}"
    );
}

#[test]
fn non_utf8_action_name_is_refused_with_2() {
    let sb = Sandbox::new();
    let action = OsString::from_vec(b"sha\xff256".to_vec());
    let out = sb.checkpoint(action, [""; 0]).output().unwrap();
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert!(stderr(&out).contains(REFUSAL_PREFIX));
}

#[test]
fn every_forbidden_selector_is_refused_by_name_before_any_git_child() {
    for name in FORBIDDEN_SELECTORS {
        let sb = Sandbox::new();
        let shim = sb.git_shim(b"", 0);
        let out = sb
            .checkpoint("selectors", [""; 0])
            .env(name, sb.path())
            .output()
            .unwrap();
        assert_eq!(status(&out), 2, "{name} was not refused: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "{name}: stdout not empty");
        assert!(
            stderr(&out).contains("checkpoint scope refusal: unsupported inherited Git selector"),
            "{name}: {}",
            stderr(&out)
        );
        assert_eq!(
            shim.calls(),
            "",
            "{name}: a git child ran before the refusal"
        );
    }
}

#[test]
fn every_forbidden_selector_is_refused_by_every_checkpoint_action() {
    // CHECKPOINT_PY refused selectors in selectors()/context(); the six
    // snippet replacements never looked (see the legacy parity tests).
    // GIT_INDEX_FILE is what pre-push exports. `capture`, `admit` and `after`
    // index sys.argv before context(), so with no arguments they are that
    // IndexError (1); with their argument the selector is refused (2).
    use amplihack_cli::workflow_checkpoint::{ACTIONS as TABLE, Selectors};
    for spec in TABLE.iter().filter(|s| s.selectors == Selectors::Context) {
        let sb = Sandbox::new();
        let run = |args: &[&str]| {
            sb.checkpoint(spec.name, args)
                .env("GIT_INDEX_FILE", sb.path().join("index"))
                .output()
                .unwrap()
        };
        let indexed = matches!(spec.name, "capture" | "admit" | "after");
        let bare = run(&[]);
        assert_eq!(
            status(&bare),
            if indexed { 1 } else { 2 },
            "{}: {}",
            spec.name,
            stderr(&bare)
        );
        assert!(bare.stdout.is_empty(), "{}: stdout not empty", spec.name);
        if matches!(spec.name, "capture" | "admit") {
            let out = run(&["publish"]);
            assert_eq!(status(&out), 2, "{}: {}", spec.name, stderr(&out));
            assert!(
                stderr(&out).contains("unsupported inherited Git selector"),
                "{}: {}",
                spec.name,
                stderr(&out)
            );
        }
    }
}

#[test]
fn admission_refused_result_still_prints_when_a_selector_is_inherited() {
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("admission-refused-result", [""; 0])
        .env("GIT_INDEX_FILE", sb.path().join("index"))
        .env("RECIPE_VAR_worktree_setup", r#"{"branch_name":"feat/x"}"#)
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"{\"pushed\": \"false\", \"sha\": \"\", \"branch\": \"feat/x\", \"reason\": \"scope-admission-refused\"}\n"
    );
}

#[test]
fn config_key_and_value_without_count_are_not_refused() {
    // Python parity: GIT_CONFIG_KEY_n/VALUE_n are inert without COUNT.
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("selectors", [""; 0])
        .env("GIT_CONFIG_KEY_0", "core.pager")
        .env("GIT_CONFIG_VALUE_0", "cat")
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn statuses_2_3_78_have_empty_stdout() {
    let sb = Sandbox::new();
    let refused = sb.checkpoint("no-such-action", [""; 0]).output().unwrap();
    assert_eq!(status(&refused), 2);
    let saved = sb.tmp().join("saved");
    let advertised = |body: &[u8], rc: &str| {
        std::fs::write(&saved, body).unwrap();
        sb.checkpoint(
            "advertised-ref",
            [saved.as_os_str(), "refs/heads/feat/x".as_ref(), rc.as_ref()],
        )
        .output()
        .unwrap()
    };
    let absent = advertised(format!("{}\trefs/heads/main\n", oid1('a')).as_bytes(), "0");
    assert_eq!(status(&absent), 3, "stderr: {}", stderr(&absent));
    let malformed = advertised(b"not an advertisement\n", "0");
    assert_eq!(status(&malformed), 78, "stderr: {}", stderr(&malformed));
    for out in [&refused, &absent, &malformed] {
        assert!(
            out.stdout.is_empty(),
            "stdout must be empty on non-zero exit"
        );
    }
}

#[test]
fn a_refusing_run_leaves_an_isolated_home_unchanged() {
    let sb = Sandbox::new();
    std::fs::write(sb.home().join("marker"), b"before").unwrap();
    let before = tree_snapshot(&sb.home());
    let out = sb
        .checkpoint("selectors", [""; 0])
        .env("GIT_DIR", sb.path())
        .output()
        .unwrap();
    assert_eq!(status(&out), 2);
    let after = tree_snapshot(&sb.home());
    assert_eq!(before, after, "the internal subcommand wrote under HOME");
    assert!(!sb.home().join(".amplihack").exists());
}

#[test]
fn a_successful_run_also_leaves_home_unchanged() {
    // Internal routing bypasses the update notice, asset repair and profile
    // start-up, so even a success writes nothing under HOME.
    let sb = Sandbox::new();
    let before = tree_snapshot(&sb.home());
    let out = sb
        .checkpoint("cleanup-skip-result", ["reason"])
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(before, tree_snapshot(&sb.home()));
}

#[test]
fn every_action_without_inputs_fails_closed_with_empty_stdout() {
    // No input, no authority record: only the three actions 918c ran
    // without input succeed. Everything else is a refusal (2) or the
    // IndexError/KeyError the Python left uncaught (1), never a partial
    // record.
    let pure = ["selectors", "admission-refused-result", "sha256"];
    for action in ACTIONS {
        let sb = Sandbox::new();
        let out = sb
            .checkpoint(action, [""; 0])
            .current_dir(sb.path())
            .output()
            .unwrap();
        let code = status(&out);
        if pure.contains(&action) {
            continue;
        }
        assert!(
            [1, 2].contains(&code),
            "{action}: unexpected status {code}: {}",
            stderr(&out)
        );
        assert!(
            out.stdout.is_empty(),
            "{action}: stdout not empty on {code}"
        );
    }
}

#[test]
fn checkpoint_is_not_reachable_through_the_public_workflow_command() {
    let sb = Sandbox::new();
    let mut c = sb.command(AMPLIHACK);
    c.args(["workflow", "checkpoint", "selectors"]);
    let out = c.output().unwrap();
    assert_ne!(status(&out), 0, "public route reached the checkpoint");
}

#[test]
fn a_refusal_is_exactly_one_stderr_line_with_empty_stdout() {
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("selectors", [""; 0])
        .env("GIT_DIR", sb.path())
        .output()
        .unwrap();
    assert_eq!(status(&out), 2);
    assert!(out.stdout.is_empty());
    assert_eq!(
        stderr(&out),
        "checkpoint scope refusal: unsupported inherited Git selector\n"
    );
}

#[test]
fn a_non_utf8_reason_is_written_as_a_surrogate_escape() {
    // CLEANUP_SKIP_JSON took sys.argv[1], which Python decodes with
    // surrogateescape; json.dumps then writes the byte as \udcXX.
    let sb = Sandbox::new();
    let reason = OsString::from_vec(b"gone \xff".to_vec());
    let out = sb
        .checkpoint("cleanup-skip-result", [reason])
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"{\"skipped\": true, \"admitted\": false, \"reason\": \"gone \\udcff\"}\n"
    );
}

#[test]
fn no_refusal_observation_is_written_before_admission() {
    // `check` refuses before any authority record is loaded, so nothing may
    // appear in the private TMPDIR.
    let sb = Sandbox::new();
    let before = tree_snapshot(&sb.tmp());
    let out = sb
        .checkpoint("check", [""; 0])
        .current_dir(sb.path())
        .output()
        .unwrap();
    assert_eq!(status(&out), 2, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert_eq!(
        before,
        tree_snapshot(&sb.tmp()),
        "a refusal wrote into TMPDIR before admission"
    );
}

#[test]
fn every_non_zero_status_has_empty_stdout() {
    // 1 (crash), 2 (refusal), 3 (absent), 78 (malformed).
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved");
    let cases: [(&[u8], &str, i32); 3] = [(b"", "2", 3), (b"junk\n", "0", 78), (b"", "x", 1)];
    for (body, rc, expected) in cases {
        std::fs::write(&saved, body).unwrap();
        let out = sb
            .checkpoint(
                "advertised-ref",
                [saved.as_os_str(), "refs/heads/feat/x".as_ref(), rc.as_ref()],
            )
            .output()
            .unwrap();
        assert_eq!(status(&out), expected, "stderr: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "stdout on {expected}");
    }
    let refused = sb.checkpoint("check", [""; 0]).output().unwrap();
    assert_eq!(status(&refused), 2);
    assert!(refused.stdout.is_empty());
}
