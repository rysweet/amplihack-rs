//! Issue #2030: every row of the Codex and Copilot passthrough tables in
//! `codex_copilot_rewrite.rs`.

use super::codex_copilot_rewrite::{rewrite_codex_passthrough, rewrite_copilot_passthrough};
use amplihack_utils::prompt_delivery::FILE_REFERENCE_PREFIX;
use std::os::unix::fs::PermissionsExt;

const SENTINEL: &str = "SENTINEL-2030-codex-copilot tail -f /dev/null";

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|a| (*a).to_string()).collect()
}

fn codex(items: &[&str]) -> (Vec<String>, Option<String>) {
    let r = rewrite_codex_passthrough(&s(items)).unwrap_or_else(|e| panic!("{items:?}: {e:#}"));
    (r.args.clone(), r.stdin_payload.clone())
}

#[test]
fn codex_exec_positional_goes_to_stdin() {
    for (input, expected) in [
        (vec!["exec", SENTINEL], vec!["exec", "-"]),
        (vec!["e", SENTINEL], vec!["e", "-"]),
        (
            vec!["exec", "--model", "gpt-5", "--json", SENTINEL],
            vec!["exec", "--model", "gpt-5", "--json", "-"],
        ),
        (vec!["exec", "--", SENTINEL], vec!["exec", "--", "-"]),
        (
            vec!["-c", "model=o3", "exec", "-s", "read-only", SENTINEL],
            vec!["-c", "model=o3", "exec", "-s", "read-only", "-"],
        ),
        (
            vec!["exec", "review", SENTINEL],
            vec!["exec", "review", "-"],
        ),
        // Crusty round 3: top-level `codex review` is non-interactive and
        // reads `-` from stdin, like `exec review`.
        (vec!["review", SENTINEL], vec!["review", "-"]),
        (
            vec!["review", "--base", "main", SENTINEL],
            vec!["review", "--base", "main", "-"],
        ),
        (
            vec!["-c", "model=o3", "review", "--commit", "abc123", SENTINEL],
            vec!["-c", "model=o3", "review", "--commit", "abc123", "-"],
        ),
        (vec!["review", "--", SENTINEL], vec!["review", "--", "-"]),
        (
            vec!["exec", "resume", "abc-123", SENTINEL],
            vec!["exec", "resume", "abc-123", "-"],
        ),
        (
            vec!["exec", "resume", "--last", SENTINEL],
            vec!["exec", "resume", "--last", "-"],
        ),
        (
            vec!["exec", "fork", "abc-123", SENTINEL],
            vec!["exec", "fork", "abc-123", "-"],
        ),
        // Crusty round 2: option values are not prompts, per subcommand.
        (
            vec!["exec", "review", "--base", "main", SENTINEL],
            vec!["exec", "review", "--base", "main", "-"],
        ),
        (
            vec![
                "exec", "review", "--commit", "abc123", "--title", "t", SENTINEL,
            ],
            vec!["exec", "review", "--commit", "abc123", "--title", "t", "-"],
        ),
        (
            vec!["exec", "--cyber-access-program", "standard", SENTINEL],
            vec!["exec", "--cyber-access-program", "standard", "-"],
        ),
        // `-i` takes one value under fork and resume, so the text after it
        // is the prompt.
        (
            vec!["exec", "fork", "abc-123", "-i", "a.png", SENTINEL],
            vec!["exec", "fork", "abc-123", "-i", "a.png", "-"],
        ),
        (
            vec!["exec", "resume", "abc-123", "--image", "a.png", SENTINEL],
            vec!["exec", "resume", "abc-123", "--image", "a.png", "-"],
        ),
        // Plain exec: `-i` is variadic, so the prompt goes before it.
        (
            vec!["exec", SENTINEL, "-i", "a.png", "b.png"],
            vec!["exec", "-", "-i", "a.png", "b.png"],
        ),
        (
            vec!["exec", "-i", "a.png", "--", SENTINEL],
            vec!["exec", "-i", "a.png", "--", "-"],
        ),
    ] {
        let (args, payload) = codex(&input);
        assert_eq!(args, s(&expected), "{input:?}");
        assert_eq!(payload.as_deref(), Some(SENTINEL), "{input:?}");
        assert!(!args.iter().any(|a| a.contains("SENTINEL")), "{args:?}");
    }
}

#[test]
fn codex_option_values_are_not_prompts() {
    // `-o` takes a value; the value is not read as the prompt.
    let (args, payload) = codex(&["exec", "-o", "last.txt", SENTINEL]);
    assert_eq!(args, s(&["exec", "-o", "last.txt", "-"]));
    assert_eq!(payload.as_deref(), Some(SENTINEL));
    let (args, payload) = codex(&["exec", "--model", "exec"]);
    assert_eq!(args, s(&["exec", "--model", "exec"]));
    assert_eq!(payload, None);
}

#[test]
fn codex_cases_left_unchanged() {
    for input in [
        vec!["exec", "-"],
        vec!["exec", "--json", "-"],
        vec!["exec"],
        vec!["exec", "resume", "abc-123"],
        vec!["exec", "resume", "--last"],
        vec!["exec", "fork", "abc-123"],
        vec!["exec", "help", "resume"],
        // Crusty round 2: review option values stay where they are.
        vec!["exec", "review", "--commit", "abc123"],
        vec!["exec", "review", "--base", "main"],
        vec!["exec", "review", "--uncommitted", "--title", "t"],
        vec!["exec", "--cyber-access-program", "standard"],
        vec!["exec", "fork", "abc-123", "-i", "a.png"],
        // One image value, prompt on stdin.
        vec!["exec", "-i", "a.png"],
        vec!["exec", "-i", "a.png", "-i", "b.png"],
        vec!["--model", "gpt-5"],
        vec!["review", "--uncommitted"],
        vec!["review", "--commit", "abc123", "--title", "t"],
        vec!["review", "-"],
        // Interactive Codex and other subcommands: documented exception.
        vec![SENTINEL],
        vec!["resume", "abc-123", SENTINEL],
        vec!["fork", "abc-123", SENTINEL],
        vec!["login"],
        vec![],
    ] {
        let (args, payload) = codex(&input);
        assert_eq!(args, s(&input), "{input:?}");
        assert_eq!(payload, None, "{input:?}");
    }
}

#[test]
fn codex_two_prompts_is_an_error_without_the_text() {
    for input in [
        vec!["exec", SENTINEL, "again"],
        vec!["exec", SENTINEL, "--", "again"],
        vec!["exec", "resume", "--last", SENTINEL, "again"],
        vec!["review", SENTINEL, "again"],
    ] {
        let err = rewrite_codex_passthrough(&s(&input)).expect_err("two prompts");
        let message = format!("{err:#}");
        assert!(
            message.contains("more than one positional prompt"),
            "{message}"
        );
        assert!(!message.contains("SENTINEL"), "{message}");
    }
}

#[test]
fn codex_ambiguous_commands_are_refused_without_the_text() {
    for (input, expected) in [
        // A variadic `--image` took the text: Codex would read it as an
        // image path and keep it on argv.
        (
            vec!["exec", "-i", "a.png", SENTINEL],
            "--image took 2 values",
        ),
        (
            vec!["exec", "--image", "a.png", "b.png", SENTINEL],
            "took 3 values",
        ),
        // An option amplihack does not know, followed by a word.
        (
            vec!["exec", "--option-from-a-later-codex", SENTINEL],
            "does not know whether",
        ),
        // `-m` is a root and `exec review` option, not a top-level
        // `review` one, so after `review` it is unknown.
        (vec!["review", "-m", SENTINEL], "does not know whether"),
    ] {
        let err = rewrite_codex_passthrough(&s(&input)).expect_err("refused");
        let message = format!("{err:#}");
        assert!(message.contains(expected), "{input:?}: {message}");
        assert!(!message.contains("SENTINEL"), "{message}");
    }
}

#[test]
fn codex_debug_redacts_the_payload() {
    let r = rewrite_codex_passthrough(&s(&["exec", SENTINEL])).unwrap();
    assert!(!format!("{r:?}").contains("SENTINEL"));
}

fn pointer_path(arg: &str) -> std::path::PathBuf {
    let rest = arg
        .strip_prefix(FILE_REFERENCE_PREFIX)
        .unwrap_or_else(|| panic!("not a pointer sentence: {arg}"));
    let end = rest.find(". Its contents").expect("template suffix");
    std::path::PathBuf::from(&rest[..end])
}

#[test]
fn copilot_prompt_flags_get_the_pointer_sentence() {
    for (input, flag) in [
        (vec!["-p", SENTINEL], "-p"),
        (vec!["--prompt", SENTINEL], "--prompt"),
        (
            vec!["--prompt=SENTINEL-2030-codex-copilot tail -f /dev/null"],
            "--prompt",
        ),
        (
            vec!["-pSENTINEL-2030-codex-copilot tail -f /dev/null"],
            "-p",
        ),
        (vec!["-i", SENTINEL], "-i"),
        (vec!["--interactive", SENTINEL], "--interactive"),
        (vec!["--model", "gpt-5", "-p", SENTINEL, "--silent"], "-p"),
    ] {
        let r = rewrite_copilot_passthrough(&s(&input), true, true).unwrap();
        assert!(
            !r.args.iter().any(|a| a.contains("SENTINEL")),
            "{:?}",
            r.args
        );
        assert_eq!(r.stdin_payload, None);
        assert_eq!(r.args[0], "--add-dir");
        let dir = std::path::PathBuf::from(&r.args[1]);
        let at = r.args.iter().position(|a| a == flag).expect("flag kept");
        let path = pointer_path(&r.args[at + 1]);
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SENTINEL);
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!((file_mode, dir_mode), (0o600, 0o700), "{input:?}");
        drop(r);
        assert!(!dir.exists(), "per-session directory removed on drop");
    }
}

#[test]
fn copilot_without_grant_dir_adds_no_add_dir() {
    let r = rewrite_copilot_passthrough(&s(&["-p", SENTINEL]), false, true).unwrap();
    assert_eq!(r.args.len(), 2);
    assert_eq!(r.args[0], "-p");
    assert!(r.args[1].starts_with(FILE_REFERENCE_PREFIX));
}

#[test]
fn copilot_cases_left_unchanged() {
    let pointer = format!("{FILE_REFERENCE_PREFIX}/tmp/x/p.md. Its contents are the task prompt.");
    for input in [
        vec!["-p", pointer.as_str()],
        vec!["--model", "gpt-5"],
        vec!["-p"],
        vec!["--", "-p", SENTINEL],
        vec![],
    ] {
        let r = rewrite_copilot_passthrough(&s(&input), true, true).unwrap();
        assert_eq!(r.args, s(&input), "{input:?}");
    }
}

#[test]
fn copilot_same_flag_twice_is_an_error_without_the_text() {
    let err = rewrite_copilot_passthrough(&s(&["-p", SENTINEL, "--prompt", "again"]), true, true)
        .expect_err("two prompts");
    let message = format!("{err:#}");
    assert!(message.contains("given twice"), "{message}");
    assert!(!message.contains("SENTINEL"), "{message}");
}

/// Crusty round 2: under the gateway (no `--add-dir`) a `-p` run without path
/// access could not open its own prompt file, so it is refused before any
/// file is written. `-i` is interactive: Copilot asks the user.
#[test]
fn copilot_prompt_without_add_dir_or_path_access_is_refused() {
    for input in [vec!["-p", SENTINEL], vec!["--prompt", SENTINEL]] {
        let err = rewrite_copilot_passthrough(&s(&input), false, false).expect_err("refused");
        let message = format!("{err:#}");
        assert!(message.contains("add --allow-all-paths"), "{message}");
        assert!(!message.contains("SENTINEL"), "{message}");
    }
    let r = rewrite_copilot_passthrough(&s(&["-i", SENTINEL]), false, false).unwrap();
    assert!(r.args[1].starts_with(FILE_REFERENCE_PREFIX), "{:?}", r.args);
    // `--add-dir` alone is enough.
    rewrite_copilot_passthrough(&s(&["-p", SENTINEL]), true, false).unwrap();
}

/// Crusty round 2: on a terminal (not subprocess-safe) a caller's
/// `--allow-all-tools` stops amplihack adding `--allow-all`, so Copilot has
/// no path access, and under the gateway a `-p` launch is refused.
#[test]
fn copilot_path_access_on_a_terminal() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    unsafe { std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL") };
    let access = |args: &[&str], safe: bool| super::copilot_has_path_access(&s(args), safe);
    assert!(!access(&["-p", "x", "--allow-all-tools"], false));
    assert!(access(&["-p", "x", "--allow-all-tools"], true));
    assert!(access(&["-p", "x"], false), "amplihack adds --allow-all");
    for grant in ["--allow-all", "--allow-all-paths", "--yolo"] {
        assert!(access(&["--allow-all-tools", grant], false), "{grant}");
    }
    assert!(!access(
        &["--allow-all-tools", "--", "--allow-all-paths"],
        false
    ));
    unsafe { std::env::set_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1") };
    assert!(!access(&["-p", "x"], true));
    assert!(access(&["-p", "x", "--allow-all-paths"], true));
    unsafe { std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL") };
}
