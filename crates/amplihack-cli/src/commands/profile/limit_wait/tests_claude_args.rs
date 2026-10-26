//! `claude_args::parse` (issue #1618): print mode and the positional prompt,
//! read the way Claude's Commander parser reads them, with the values of
//! Claude's value-taking options skipped.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_claude_args;`.

use super::claude_args::{ClaudeArgs, parse};

fn p(args: &[&str]) -> ClaudeArgs {
    parse(args)
}

const PRINT_PROMPT: ClaudeArgs = ClaudeArgs {
    print: true,
    prompt: true,
};
const PRINT_ONLY: ClaudeArgs = ClaudeArgs {
    print: true,
    prompt: false,
};

#[test]
fn the_prompt_is_any_positional_argument_wherever_it_is() {
    for args in [
        &["-p", "do the step"][..],
        &["do the step", "-p"],
        &["--print", "x"],
        &["-p", "--model", "opus", "x"],
        &["--model", "opus", "-p", "x", "--verbose"],
        &["-p", "--output-format", "json", "--max-turns", "5", "x"],
        &["-p", "--append-system-prompt", "be brief", "x"],
        &["-p", "--model=opus", "x"],
        &["-pc", "x"],
    ] {
        assert_eq!(p(args), PRINT_PROMPT, "{args:?}");
    }
}

#[test]
fn option_values_are_not_the_prompt() {
    for args in [
        &["-p"][..],
        &["-p", "--model", "opus"],
        &["-p", "--output-format", "json"],
        &["-p", "--max-turns", "5"],
        &["-p", "--append-system-prompt", "a system prompt"],
        &["-p", "-n", "session name"],
        &["-p", "-nname"],
        &["-p", "--resume", "abc123"],
        &["-p", "-r", "abc123"],
        // Variadic: every following non-option word is a value.
        &["-p", "--allowedTools", "Bash(git *)", "Edit"],
        &["-p", "--add-dir", "/a", "/b"],
        &["-p", ""],
        &["-p", "-"],
    ] {
        assert_eq!(p(args), PRINT_ONLY, "{args:?}");
    }
}

#[test]
fn an_option_after_a_variadic_one_ends_its_values() {
    assert_eq!(p(&["--tools", "Read", "-p", "x"]), PRINT_PROMPT);
    assert_eq!(p(&["-p", "--resume", "--verbose", "x"]), PRINT_PROMPT);
}

#[test]
fn a_prompt_starting_with_a_dash_is_reachable_only_after_double_dash() {
    assert!(!p(&["-p", "-explain"]).prompt);
    assert_eq!(p(&["-p", "--", "-explain this"]), PRINT_PROMPT);
    assert_eq!(
        p(&["-p", "--", "--model"]),
        PRINT_PROMPT,
        "after --, nothing is an option"
    );
    assert_eq!(
        p(&["--", "-p"]),
        ClaudeArgs {
            print: false,
            prompt: true
        }
    );
    assert_eq!(p(&["-p", "--", ""]), PRINT_ONLY);
}

#[test]
fn print_needs_the_flag_itself() {
    // Claude rejects `--print=x`: the flag takes no value.
    assert!(!p(&["--print=x"]).print);
    assert!(
        !p(&["--model", "-p"]).print,
        "a required value is taken as is"
    );
    assert!(!p(&["--printx", "x"]).print);
    assert!(p(&["-cp"]).print);
    assert_eq!(p(&[] as &[&str]), ClaudeArgs::default());
}
