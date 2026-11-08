//! Parity restorations for `amplihack internal workflow-checkpoint` (#1554):
//! 17.138 #5 (leading-dash actions), the Python `int()` grammar for
//! `advertised-ref` and `after`, the nested `str()` of `worktree-path`,
//! Python float `repr` in `admission-refused-result`, `stage-strict` arity,
//! a relative GIT_INDEX_FILE on the inheriting route, and the
//! `validate-recipe-runner` exit contract.
//!
//! Every expected value was recorded on 2026-11-07 by running the fragment
//! extracted with `git show 918c256cf:<path>` under `python3 -I` (Python
//! unicodedata 16.0.0), outside the repository, in
//! ~/.amplihack/work/pr2020-oracle/:
//!
//! * `checkpoint.py` = `CHECKPOINT_PY` of `workflow_publish_checkpoint.sh`;
//! * `origin_ref.py` = `ORIGIN_REF_PY` of `workflow_pr_head_sync.sh:83-92`;
//! * `admission.py`  = `ADMISSION_RESULT_PY` (`:1155`);
//! * `strict.py`     = `STRICT_PATHS_PY` of `workflow_stage_changes.sh:69`;
//! * `wtp.py`        = the `worktree_path` extraction, `workflow-publish.yaml:137`.
//!
//! Rows whose expectation comes from an owner decision instead of 918c say
//! so in their test name (`owner_17_138_*` or `unresolved_*`).

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Output;

use crate::workflow_checkpoint_support::*;

const UNKNOWN: &[u8] = b"checkpoint scope refusal: unknown private checkpoint operation\n";

fn raw_action(sb: &Sandbox, action: &OsStr) -> Output {
    let (work, _bare) = repo_with_origin(sb);
    sb.checkpoint(action, [""; 0])
        .current_dir(work)
        .output()
        .unwrap()
}

// ------------------------------------------------- 17.138 #5: raw-argv gate

#[test]
fn owner_17_138_5_leading_dash_actions_are_unknown_private_checkpoint_operations() {
    // 918c: `action = sys.argv[1]` matched nothing, Refusal, status 2.
    let mut wrong = Vec::new();
    for action in ["--help", "-h", "-V", "--version", "-x", "--", "-"] {
        let sb = Sandbox::new();
        let out = raw_action(&sb, OsStr::new(action));
        if status(&out) != 2 || !out.stdout.is_empty() || out.stderr != UNKNOWN {
            wrong.push(format!(
                "{action:?}: status {} stdout {:?} stderr {:?}",
                status(&out),
                String::from_utf8_lossy(&out.stdout),
                stderr(&out)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_empty_action_name_is_an_unknown_operation() {
    let sb = Sandbox::new();
    let out = raw_action(&sb, OsStr::new(""));
    assert_eq!(status(&out), 2, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, UNKNOWN);
}

#[test]
fn a_non_utf8_action_name_is_an_unknown_operation() {
    let sb = Sandbox::new();
    let out = raw_action(&sb, OsStr::from_bytes(b"\xff"));
    assert_eq!(status(&out), 2, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, UNKNOWN);
}

#[test]
fn no_action_at_all_is_status_1_like_the_918c_index_error() {
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let mut c = sb.command(AMPLIHACK);
    c.args(["internal", "workflow-checkpoint"])
        .current_dir(work);
    let out = c.output().unwrap();
    assert_eq!(status(&out), 1, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn leading_dash_arguments_after_a_valid_action_pass_through() {
    // `stage-strict --path-list`: 918c ran os.lstat('--path-list') and
    // refused with FileNotFoundError, status 2. The gate must not eat it.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let out = sb
        .checkpoint("stage-strict", ["--path-list"])
        .current_dir(&work)
        .output()
        .unwrap();
    assert_eq!(status(&out), 2, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(
        err.starts_with("stage_workflow_changes: strict refusal: ") && err.contains("--path-list"),
        "{err}"
    );
    assert!(
        !err.contains("unknown private checkpoint operation"),
        "{err}"
    );
}

// --------------------------------------------- advertised-ref: Python int()

/// ORIGIN_REF_PY with one present record for `refs/heads/x` and the target
/// `refs/heads/y`: status 3 for any rc other than 2, 78 for rc == 2 (records
/// present), 1 (uncaught ValueError) when `int()` refuses.
#[test]
fn advertised_ref_rc_is_parsed_like_python_int() {
    let rows: [(&str, i32); 16] = [
        ("0", 3),
        ("2", 78),
        (" \u{663} ", 3), // ARABIC-INDIC DIGIT THREE, with spaces
        ("\u{662}", 78),  // ARABIC-INDIC DIGIT TWO
        ("\u{ff12}", 78), // FULLWIDTH DIGIT TWO
        ("\u{a0}2", 78),  // NO-BREAK SPACE is whitespace to int()
        ("1_0", 3),
        ("0_2", 78),
        (" +2 ", 78),
        ("-0", 3),
        ("99999999999999999999999", 3),
        ("-99999999999999999999999", 3),
        ("000000000000000000000000000002", 78),
        ("1__0", 1),
        ("x", 1),
        ("", 1),
    ];
    let body = format!("{}\trefs/heads/x\n", oid1('a'));
    let mut wrong = Vec::new();
    for (rc, want) in rows {
        let sb = Sandbox::new();
        let saved = sb.tmp().join("saved");
        fs::write(&saved, &body).unwrap();
        let out = sb
            .checkpoint(
                "advertised-ref",
                [saved.as_os_str(), "refs/heads/y".as_ref(), rc.as_ref()],
            )
            .output()
            .unwrap();
        if status(&out) != want || !out.stdout.is_empty() {
            wrong.push(format!(
                "rc {rc:?}: 918c {want}, Rust {} {}",
                status(&out),
                stderr(&out)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn advertised_ref_rc_beyond_i64_does_not_wrap_to_2() {
    // 2 + 2^64 must not be treated as 2 by a wrapping parse.
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved");
    fs::write(&saved, format!("{}\trefs/heads/x\n", oid1('a'))).unwrap();
    let out = sb
        .checkpoint(
            "advertised-ref",
            [
                saved.as_os_str(),
                "refs/heads/x".as_ref(),
                "18446744073709551618".as_ref(),
            ],
        )
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "{}", stderr(&out));
}

// ------------------------------------------------------ after: IntPolicy

/// A valid descriptor and token, so `after` reaches `int(sys.argv[4])`. The
/// run then refuses at `load_authority` (no record), which proves the rc was
/// accepted. A refused rc never reaches the authority check.
fn after_with_rc(rc: &str) -> Output {
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let token = sb.tmp().join("token.json");
    fs::write(&token, b"{\"head\": \"x\"}").unwrap();
    let identity = {
        use std::os::unix::fs::MetadataExt;
        let m = fs::symlink_metadata(&token).unwrap();
        format!("{}:{}", m.dev(), m.ino())
    };
    let sha = {
        let out = std::process::Command::new("sha256sum")
            .arg(&token)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()[..64].to_owned()
    };
    let descriptor = serde_json::json!({
        "path": token.to_str().unwrap(),
        "identity": identity,
        "sha256": sha,
    })
    .to_string();
    sb.checkpoint("after", [descriptor.as_str(), "commit", rc])
        .current_dir(work)
        .output()
        .unwrap()
}

const PAST_RC: &str = "authority record identity changed";

#[test]
fn after_accepts_every_i64_including_i64_min() {
    // 918c accepted each of these; 17.138 #2 keeps i64::MIN..=i64::MAX.
    let mut wrong = Vec::new();
    for rc in [
        "0",
        "-9223372036854775808",
        "9223372036854775807",
        "+7",
        "-0",
        "1_0",
        " 7 ",
        "0007",
    ] {
        let out = after_with_rc(rc);
        if status(&out) != 2 || !stderr(&out).contains(PAST_RC) {
            wrong.push(format!("{rc:?}: {} {}", status(&out), stderr(&out)));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn after_refuses_a_malformed_rc_with_the_python_value_error_text() {
    for rc in ["1__0", "x", "", "_1", "1_", "--1", "0x10"] {
        let out = after_with_rc(rc);
        assert_eq!(status(&out), 2, "{rc:?}: {}", stderr(&out));
        assert!(out.stdout.is_empty());
        assert_eq!(
            stderr(&out),
            format!("checkpoint scope refusal: invalid literal for int() with base 10: '{rc}'\n"),
            "{rc:?}"
        );
    }
}

#[test]
fn owner_17_138_2_after_refuses_nd_digits_and_values_outside_i64() {
    // 918c ACCEPTED every row here (int() takes Unicode Nd digits and big
    // integers). Decision 17.138 #2 accepts refusing them. Listed under
    // 'Unresolved differences for owner disposition' because cc's 00:42Z
    // request asks for full int() on advertised-ref; this pins `after` only.
    for rc in [
        "\u{663}",
        "\u{ff10}",
        "9223372036854775808",
        "-9223372036854775809",
        "99999999999999999999999",
    ] {
        let out = after_with_rc(rc);
        assert_eq!(status(&out), 2, "{rc:?}: {}", stderr(&out));
        assert!(
            !stderr(&out).contains(PAST_RC),
            "{rc:?} reached the authority check: {}",
            stderr(&out)
        );
    }
}

// ----------------------------------------------------- worktree-path str()

fn worktree_path(input: &str) -> Output {
    let sb = Sandbox::new();
    sb.checkpoint("worktree-path", [""; 0])
        .env("RECIPE_VAR_worktree_setup", input)
        .output()
        .unwrap()
}

#[test]
fn worktree_path_reproduces_python_str_of_nested_values() {
    let rows: [(&str, &str); 5] = [
        (
            r#"{"worktree_path":["a\u0085b","it's","\u2028","\u007f\t",{"k":"\"q\""},"\ud83d\ude00","é"]}"#,
            "['a\\x85b', \"it's\", '\\u2028', '\\x7f\\t', {'k': '\"q\"'}, '\u{1F600}', '\u{e9}']\n",
        ),
        (
            r#"{"worktree_path":{"x":[1.5,1e-05,null,true]}}"#,
            "{'x': [1.5, 1e-05, None, True]}\n",
        ),
        (r#"{"worktree_path":["both'\""]}"#, "['both\\'\"']\n"),
        (
            r#"{"worktree_path":"\u00e9\ud83d\ude00"}"#,
            "\u{e9}\u{1F600}\n",
        ),
        (r#"{"worktree_path":[]}"#, "[]\n"),
    ];
    let mut wrong = Vec::new();
    for (input, want) in rows {
        let out = worktree_path(input);
        if status(&out) != 0 || out.stdout != want.as_bytes() {
            wrong.push(format!(
                "{input}\n  918c {want:?}\n  Rust {} {:?} {}",
                status(&out),
                String::from_utf8_lossy(&out.stdout),
                stderr(&out)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn worktree_path_keeps_a_lone_surrogate_as_a_python_escape_inside_a_list() {
    let out = worktree_path(r#"{"worktree_path":["\ud800"]}"#);
    assert_eq!(status(&out), 0, "{}", stderr(&out));
    assert_eq!(out.stdout, b"['\\ud800']\n");
}

/// Pin test for ApprovedRow 17.138-3: a lone surrogate as the top-level
/// worktree path exits 1 with no stdout (918c's print() raised
/// UnicodeEncodeError). The negative half is the nested case above, which
/// 918c printed through str() and which must still exit 0.
#[test]
fn owner_17_138_3_a_top_level_lone_surrogate_worktree_path_exits_1() {
    let out = worktree_path(r#"{"worktree_path":"\ud800"}"#);
    assert_eq!(status(&out), 1, "{}", stderr(&out));
    assert!(out.stdout.is_empty(), "no partial path on stdout");
    let nested = worktree_path(r#"{"worktree_path":["\ud800"]}"#);
    assert_eq!(status(&nested), 0, "{}", stderr(&nested));
}

// ------------------------------------------ admission-refused-result floats

#[test]
fn admission_refused_result_writes_numeric_branch_names_as_python_repr() {
    let rows: [(&str, &str); 13] = [
        ("1e-05", "1e-05"),
        ("1e-06", "1e-06"),
        ("1e-07", "1e-07"),
        ("2.5e-5", "2.5e-05"),
        ("0.0001", "0.0001"),
        ("1e16", "1e+16"),
        ("1e22", "1e+22"),
        ("0.1", "0.1"),
        ("-0.0", "-0.0"),
        ("5e-324", "5e-324"),
        ("1.0", "1.0"),
        ("100.0", "100.0"),
        ("123456789012345678.0", "1.2345678901234568e+17"),
    ];
    let mut wrong = Vec::new();
    for (input, repr) in rows {
        let sb = Sandbox::new();
        let out = sb
            .checkpoint("admission-refused-result", [""; 0])
            .env(
                "RECIPE_VAR_worktree_setup",
                format!("{{\"branch_name\": {input}}}"),
            )
            .output()
            .unwrap();
        let want = format!(
            "{{\"pushed\": \"false\", \"sha\": \"\", \"branch\": {repr}, \"reason\": \"scope-admission-refused\"}}\n"
        );
        if status(&out) != 0 || out.stdout != want.as_bytes() {
            wrong.push(format!(
                "{input}: 918c {repr}, Rust {} {:?}",
                status(&out),
                String::from_utf8_lossy(&out.stdout)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ------------------------------------------------------ stage-strict arity

#[test]
fn direct_stage_strict_without_arguments_exits_1_with_no_stdout() {
    // STRICT_PATHS_PY read sys.argv[1] outside its except list: IndexError.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let out = sb
        .checkpoint("stage-strict", [""; 0])
        .current_dir(&work)
        .output()
        .unwrap();
    assert_eq!(status(&out), 1, "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    assert!(
        !stderr(&out).contains("strict refusal"),
        "IndexError is not a strict refusal: {}",
        stderr(&out)
    );
}

#[test]
fn direct_stage_strict_uses_only_its_first_argument() {
    // 918c ignored sys.argv[2:]; a missing list is FileNotFoundError, 2.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let out = sb
        .checkpoint("stage-strict", ["no-such-list", "extra", "more"])
        .current_dir(&work)
        .output()
        .unwrap();
    assert_eq!(status(&out), 2, "{}", stderr(&out));
    assert_eq!(
        stderr(&out),
        "stage_workflow_changes: strict refusal: [Errno 2] No such file or directory: 'no-such-list'\n"
    );
}

// ------------------------------------ P1: relative GIT_INDEX_FILE (Inherit)

#[test]
fn stage_strict_resolves_a_relative_git_index_file_against_the_top_like_918c() {
    // 918c: run from <top>/sub with GIT_INDEX_FILE=alt.idx. The first
    // rev-parse runs in the cwd, `add` runs with `-C top`, so Git creates
    // <top>/alt.idx. The caller's .git/index is byte-identical.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    let sub = work.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(work.join("new"), "n\n").unwrap();
    let list = sb.tmp().join("list");
    fs::write(&list, b"new\0").unwrap();
    let index_before = fs::read(work.join(".git/index")).unwrap();
    let out = sb
        .checkpoint("stage-strict", [list.as_os_str()])
        .current_dir(&sub)
        .env("GIT_INDEX_FILE", "alt.idx")
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "{}", stderr(&out));
    assert!(work.join("alt.idx").is_file(), "alt.idx must be at the top");
    assert!(!sub.join("alt.idx").exists(), "not relative to the cwd");
    assert_eq!(fs::read(work.join(".git/index")).unwrap(), index_before);
    let listed = sb
        .command(which("git"))
        .current_dir(&work)
        .env("GIT_INDEX_FILE", work.join("alt.idx"))
        .args(["ls-files", "--", "new"])
        .output()
        .unwrap();
    assert_eq!(listed.stdout, b"new\n");
}

// --------------------------------------------- validate-recipe-runner exit

fn validate(args: &[&str]) -> Output {
    let sb = Sandbox::new();
    let mut c = sb.command(AMPLIHACK);
    c.args(["internal", "validate-recipe-runner"]).args(args);
    c.output().unwrap()
}

/// origin/main's stderr for a relative `--binary`, byte for byte. Captured
/// on 2026-11-08 from amplihack 1.2.107 (v1.2.107; no change to
/// runner_validation.rs, bins/amplihack/src/main.rs or
/// crates/amplihack-cli/src/lib.rs between v1.2.107 and origin/main
/// 7b8d32e7e); the capture and its provenance are in
/// ~/.amplihack/work/pr2020-r17-capture/.
const ORIGIN_MAIN_RELATIVE_BINARY_STDERR: &[u8] = b"{\"error\":{\"code\":\"INVALID_VALIDATION_REQUEST\",\"details\":{\"provider\":\"claude\"},\"message\":\"provide an absolute binary and all or none of cargo-home, repository and revision\"}}\n";

#[test]
fn validate_recipe_runner_failure_prints_json_only() {
    // 918c runner_validation.rs:236-240: one JSON object on stderr, then
    // process::exit(1). Nothing on stdout, no `error:` line from main.
    let out = validate(&["--provider", "claude", "--binary", "relative/runner"]);
    assert_eq!(status(&out), 1);
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    assert_eq!(
        out.stderr,
        ORIGIN_MAIN_RELATIVE_BINARY_STDERR,
        "stderr differs from origin/main: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        !err.lines()
            .any(|l| l.starts_with("error:") || l.contains("error: ")),
        "{err}"
    );
}

#[test]
fn validate_recipe_runner_with_a_missing_absolute_binary_is_still_json_only() {
    let out = validate(&[
        "--provider",
        "codex",
        "--binary",
        "/nonexistent-for-validate-recipe-runner-test/runner",
    ]);
    assert_eq!(status(&out), 1);
    assert!(out.stdout.is_empty());
    let err = String::from_utf8(out.stderr).unwrap();
    assert_eq!(err.lines().count(), 1, "{err:?}");
    assert!(!err.contains("error:"), "{err}");
    let value: serde_json::Value = serde_json::from_str(err.trim_end()).unwrap();
    assert!(
        value["error"]["code"]
            .as_str()
            .unwrap()
            .starts_with("RUNNER_")
            || value["error"]["code"] == "INVALID_VALIDATION_REQUEST"
    );
}

// ------------------------------------------------ test-environment scrub

#[test]
fn checkpoint_commands_are_built_by_one_scrubbing_helper() {
    // T3: pre-push exports GIT_DIR, GIT_WORK_TREE and GIT_INDEX_FILE. Once
    // Ignored rows inherit selectors on purpose, an unscrubbed test command
    // would stage the hook's own repository. The behavioural half is
    // `a_bogus_git_dir_in_the_parent_environment_never_reaches_a_test_child`.
    let support = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/workflow_checkpoint_support.rs"),
    )
    .unwrap();
    assert!(
        support.contains("pub fn checkpoint_cmd("),
        "the single checkpoint_cmd(sandbox) helper must exist"
    );
    assert!(
        support.contains("amplihack_git::scrub("),
        "checkpoint_cmd must scrub REPOSITORY_ENV_VARS with amplihack_git::scrub"
    );
    assert!(
        // Spelled in two parts so this file does not trip its own guard.
        !support.contains(&[".env", "_clear()"].concat()),
        "no bare env_clear: it drops PATH and HOME and makes results host-dependent"
    );
}
