//! Issue #2535: operator guidance in the Copilot prompt file.
//!
//! `rewrite_copilot_passthrough` gains a `prompt_suffix: Option<&str>`
//! argument. The suffix (the operator-instructions block) is appended to the
//! `-p`/`--prompt` text only, before the private prompt file is written. An
//! interactive `-i` prompt never receives it, and `None` leaves every file
//! byte for byte as before.

use super::codex_copilot_rewrite::rewrite_copilot_passthrough;
use amplihack_utils::prompt_delivery::FILE_REFERENCE_PREFIX;

const PROMPT: &str = "the step's own prompt";
const BLOCK: &str = "===== BEGIN OPERATOR INSTRUCTIONS FOR THIS RUN =====\nOperator instructions for this run\nx\n===== END OPERATOR INSTRUCTIONS FOR THIS RUN =====\n";

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|a| (*a).to_string()).collect()
}

fn pointer_path(arg: &str) -> std::path::PathBuf {
    let rest = arg
        .strip_prefix(FILE_REFERENCE_PREFIX)
        .unwrap_or_else(|| panic!("not a pointer sentence: {arg}"));
    let end = rest.find(". Its contents").expect("template suffix");
    std::path::PathBuf::from(&rest[..end])
}

fn file_after(args: &[String], flag: &str) -> String {
    let at = args.iter().position(|a| a == flag).expect("flag kept");
    std::fs::read_to_string(pointer_path(&args[at + 1])).unwrap()
}

#[test]
fn u4_no_suffix_writes_the_prompt_unchanged() {
    for flag in ["-p", "--prompt", "-i"] {
        let r = rewrite_copilot_passthrough(&s(&[flag, PROMPT]), true, true, None).unwrap();
        assert_eq!(file_after(&r.args, flag), PROMPT, "{flag}");
    }
}

#[test]
fn suffix_is_appended_to_the_prompt_file_after_one_blank_line() {
    for (input, flag) in [
        (vec!["-p", PROMPT], "-p"),
        (vec!["--prompt", PROMPT], "--prompt"),
        (vec!["--model", "gpt-5", "-p", PROMPT, "--silent"], "-p"),
    ] {
        let r = rewrite_copilot_passthrough(&s(&input), true, true, Some(BLOCK)).unwrap();
        assert_eq!(
            file_after(&r.args, flag),
            format!("{PROMPT}\n\n{BLOCK}"),
            "{input:?}"
        );
        assert!(
            !r.args.iter().any(|a| a.contains("OPERATOR INSTRUCTIONS")),
            "the block must never reach argv: {:?}",
            r.args
        );
    }
}

#[test]
fn interactive_prompt_never_receives_the_suffix() {
    let r = rewrite_copilot_passthrough(&s(&["-i", PROMPT]), true, true, Some(BLOCK)).unwrap();
    assert_eq!(file_after(&r.args, "-i"), PROMPT);
}

#[test]
fn a_pointer_prompt_given_by_the_caller_is_left_alone() {
    // The caller already passed a file reference: amplihack writes no file,
    // so there is nowhere to add the block; argv is unchanged.
    let pointer = format!("{FILE_REFERENCE_PREFIX}/tmp/x/p.md. Its contents are the task prompt.");
    let input = s(&["-p", &pointer]);
    let r = rewrite_copilot_passthrough(&input, true, true, Some(BLOCK)).unwrap();
    assert_eq!(r.args, input);
}
