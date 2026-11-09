//! Issue #2030 (red phase), decision D7: `amplihack claude -- <args>` rewrites
//! prompt-bearing flags so the Claude child's argv holds no prompt text.
//!
//! API pinned here (`super::passthrough_rewrite`):
//! - `rewrite_claude_passthrough(args: &[String]) -> anyhow::Result<Rewritten>`
//! - `Rewritten { args: Vec<String>, stdin_payload: Option<String>,
//!   user_supplied_append: bool, used_file_flags: bool, .. }`
//!   holding a lazily-created 0700 `TempDir` that is removed on drop.
//! - `impl Debug for Rewritten` redacts `stdin_payload`.

use super::passthrough_rewrite::{Rewritten, rewrite_claude_passthrough};

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const SENTINEL: &str = "SENTINEL-2030-tail-f-dev-null";
const SYS_SENTINEL: &str = "SYSPROMPT-SENTINEL-2030";

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|x| x.to_string()).collect()
}

fn rewrite(items: &[&str]) -> Rewritten {
    rewrite_claude_passthrough(&s(items)).unwrap_or_else(|e| panic!("{items:?}: {e:#}"))
}

fn no_sentinel(r: &Rewritten) {
    for a in &r.args {
        assert!(
            !a.contains(SENTINEL) && !a.contains(SYS_SENTINEL),
            "prompt text left in argv: {:?}",
            r.args
        );
    }
}

/// The path that follows `flag` in the rewritten args.
fn file_after(r: &Rewritten, flag: &str) -> PathBuf {
    let i = r
        .args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} missing: {:?}", r.args));
    PathBuf::from(&r.args[i + 1])
}

fn assert_private_file(path: &Path, expected: &str) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    let dir = std::fs::symlink_metadata(path.parent().unwrap()).unwrap();
    assert_eq!(dir.permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
}

// ---------------------------------------------------------------------------
// -p / --print
// ---------------------------------------------------------------------------

#[test]
fn rewrite_table_cases_print_flags() {
    let cases: &[(&[&str], &[&str])] = &[
        (&["-p", SENTINEL], &["-p"]),
        (&["--print", SENTINEL], &["--print"]),
        (&[&format!("--print={SENTINEL}")], &["--print"]),
        (&[&format!("-p={SENTINEL}")], &["-p"]),
        (
            &["--model", "opus", "-p", SENTINEL, "--verbose"],
            &["--model", "opus", "-p", "--verbose"],
        ),
    ];
    for (input, expected) in cases {
        let r = rewrite(input);
        assert_eq!(r.args, s(expected), "input {input:?}");
        assert_eq!(
            r.stdin_payload.as_deref(),
            Some(SENTINEL),
            "input {input:?}"
        );
        assert!(!r.user_supplied_append);
        no_sentinel(&r);
    }
}

#[test]
fn bare_print_is_left_alone() {
    for input in [
        &["-p"][..],
        &["--print"][..],
        &["-p", "--model", "opus"][..],
        &["--print", "--output-format", "json"][..],
    ] {
        let r = rewrite(input);
        assert_eq!(r.args, s(input), "{input:?}");
        assert_eq!(r.stdin_payload, None, "{input:?}");
        assert!(!r.used_file_flags);
    }
}

#[test]
fn print_equals_keeps_a_prompt_that_starts_with_a_dash() {
    let r = rewrite(&["--print=-starts with a dash"]);
    assert_eq!(r.args, s(&["--print"]));
    assert_eq!(r.stdin_payload.as_deref(), Some("-starts with a dash"));
}

#[test]
fn second_print_with_text_is_an_error_that_does_not_echo_it() {
    let err = rewrite_claude_passthrough(&s(&["-p", SENTINEL, "--print", "again"]))
        .expect_err("two prompts");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("the prompt was given twice (-p/--print); pass it once"),
        "{msg}"
    );
    assert!(!msg.contains(SENTINEL));
}

#[test]
fn print_prompt_after_double_dash_goes_to_stdin() {
    let dashed = format!("-{SENTINEL}");
    let cases: &[(&[&str], &[&str], &str)] = &[
        (&["-p", "--", SENTINEL], &["-p"], SENTINEL),
        (&["-p", "--", &dashed], &["-p"], &dashed),
        (
            &["--print", "--model", "opus", "--", SENTINEL],
            &["--print", "--model", "opus"],
            SENTINEL,
        ),
    ];
    for (input, expected, payload) in cases {
        let r = rewrite(input);
        assert_eq!(r.args, s(expected), "input {input:?}");
        assert_eq!(
            r.stdin_payload.as_deref(),
            Some(*payload),
            "input {input:?}"
        );
        no_sentinel(&r);
    }
}

#[test]
fn print_mode_with_nothing_after_double_dash_drops_it() {
    let r = rewrite(&["-p", "--"]);
    assert_eq!(r.args, s(&["-p"]));
    assert_eq!(r.stdin_payload, None);
}

#[test]
fn more_than_one_print_prompt_around_double_dash_is_an_error() {
    const FIRST: &str = "FIRST-PROMPT-SENTINEL-2030";
    const SECOND: &str = "SECOND-PROMPT-SENTINEL-2030";
    for input in [
        s(&["-p", "--", FIRST, SECOND]),
        s(&["-p", FIRST, "--", SECOND]),
    ] {
        let err = rewrite_claude_passthrough(&input).expect_err("two prompts");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("the prompt was given twice (-p/--print); pass it once"),
            "{msg}"
        );
        assert!(!msg.contains(FIRST) && !msg.contains(SECOND), "{msg}");
    }
}

#[test]
fn double_dash_outside_print_mode_is_unchanged() {
    // D9: outside print mode, `--` and everything after it belong to the
    // caller.
    for input in [
        &["--", SENTINEL][..],
        &["--model", "opus", "--", SENTINEL][..],
    ] {
        let r = rewrite(input);
        assert_eq!(r.args, s(input), "{input:?}");
        assert_eq!(r.stdin_payload, None, "{input:?}");
    }
}

#[test]
fn dash_leading_token_after_bare_print_is_passed_through_as_an_option() {
    // Documented in the module table: Claude parses `-x` as an option, so
    // the rewrite leaves it alone. `-p -- -x` is the form for such a prompt.
    let r = rewrite(&["-p", "-x"]);
    assert_eq!(r.args, s(&["-p", "-x"]));
    assert_eq!(r.stdin_payload, None);
}

#[test]
fn args_without_prompt_flags_are_unchanged() {
    let input = ["--model", "opus", "--resume", "abc-123", "--verbose"];
    let r = rewrite(&input);
    assert_eq!(r.args, s(&input));
    assert_eq!(r.stdin_payload, None);
    assert!(!r.used_file_flags);
    assert!(!r.user_supplied_append);
}

// ---------------------------------------------------------------------------
// --system-prompt / --append-system-prompt
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_text_becomes_a_private_file() {
    for input in [
        vec!["--system-prompt".to_string(), SYS_SENTINEL.to_string()],
        vec![format!("--system-prompt={SYS_SENTINEL}")],
    ] {
        let r = rewrite_claude_passthrough(&input).unwrap();
        no_sentinel(&r);
        assert!(r.used_file_flags);
        assert!(!r.args.iter().any(|a| a == "--system-prompt"));
        let path = file_after(&r, "--system-prompt-file");
        assert_private_file(&path, SYS_SENTINEL);
        assert!(!r.user_supplied_append);
    }
}

#[test]
fn append_system_prompt_text_becomes_a_file_and_counts_as_user_supplied() {
    for input in [
        vec![
            "--append-system-prompt".to_string(),
            SYS_SENTINEL.to_string(),
        ],
        vec![format!("--append-system-prompt={SYS_SENTINEL}")],
    ] {
        let r = rewrite_claude_passthrough(&input).unwrap();
        no_sentinel(&r);
        assert!(
            r.user_supplied_append,
            "the contract must not be injected on top"
        );
        assert!(r.used_file_flags);
        let path = file_after(&r, "--append-system-prompt-file");
        assert_private_file(&path, SYS_SENTINEL);
    }
}

#[test]
fn user_append_file_flag_passes_through_and_counts_as_user_supplied() {
    let r = rewrite(&["--append-system-prompt-file", "/mine.md"]);
    assert_eq!(r.args, s(&["--append-system-prompt-file", "/mine.md"]));
    assert!(r.user_supplied_append);
}

#[test]
fn recipe_step_shape_is_fully_cleaned() {
    // What the recipe runner passes: -p <brief> --system-prompt <agent md>.
    let r = rewrite(&[
        "--dangerously-skip-permissions",
        "-p",
        SENTINEL,
        "--system-prompt",
        SYS_SENTINEL,
        "--model",
        "opus",
    ]);
    no_sentinel(&r);
    assert_eq!(r.stdin_payload.as_deref(), Some(SENTINEL));
    assert!(r.args.contains(&"-p".to_string()));
    assert!(r.args.contains(&"--model".to_string()));
    assert_private_file(&file_after(&r, "--system-prompt-file"), SYS_SENTINEL);
}

#[test]
fn system_prompt_and_file_conflict_errors() {
    for input in [
        s(&[
            "--system-prompt",
            SYS_SENTINEL,
            "--system-prompt-file",
            "/x.md",
        ]),
        s(&[
            "--system-prompt-file",
            "/x.md",
            "--system-prompt",
            SYS_SENTINEL,
        ]),
    ] {
        let err = rewrite_claude_passthrough(&input).expect_err("both forms given");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("--system-prompt and --system-prompt-file were both given; pass one"),
            "{msg}"
        );
        assert!(!msg.contains(SYS_SENTINEL));
    }
}

#[test]
fn nothing_after_a_positional_prompt_is_rewritten() {
    // D9: an interactive positional prompt is out of scope; tokens after it
    // belong to it and are passed through untouched.
    let input = ["hello there", "--system-prompt", "x"];
    let r = rewrite(&input);
    assert_eq!(r.args, s(&input));
    assert!(!r.used_file_flags);
}

// ---------------------------------------------------------------------------
// Lifetime and redaction
// ---------------------------------------------------------------------------

#[test]
fn per_session_files_are_removed_on_drop() {
    let r = rewrite(&[
        "--system-prompt",
        SYS_SENTINEL,
        "--append-system-prompt",
        SYS_SENTINEL,
    ]);
    let a = file_after(&r, "--system-prompt-file");
    let b = file_after(&r, "--append-system-prompt-file");
    assert_eq!(a.parent(), b.parent(), "one private dir per session");
    let dir = a.parent().unwrap().to_path_buf();
    assert!(
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("amplihack-prompt-"),
        "{}",
        dir.display()
    );
    drop(r);
    assert!(!a.exists() && !b.exists() && !dir.exists());
}

#[test]
fn debug_redacts_stdin_payload() {
    let r = rewrite(&["-p", SENTINEL]);
    let dbg = format!("{r:?}");
    assert!(!dbg.contains(SENTINEL), "{dbg}");
    assert!(dbg.contains(&format!("{} bytes", SENTINEL.len())), "{dbg}");
}
