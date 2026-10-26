//! `claude_args::parse` (issue #1618): print mode and the positional prompt,
//! read the way Claude's Commander parser reads them, with the values of
//! Claude's value-taking options skipped.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_claude_args;`.

use super::claude_args::{ClaudeArgs, FLAGS, OPTIONAL, REQUIRED, VARIADIC, parse};

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
        // Hidden options: `claude --help` does not list them.
        &["-p", "--max-thinking-tokens", "8000"],
        &["-p", "--thinking", "enabled"],
        &["-p", "--workload", "tag"],
        &["-p", "--prefill", "some text"],
        &["-p", "--sdk-url", "ws://x"],
        &["-p", "--resume-session-at", "msg1"],
        &["-p", "--rewind-files", "msg1"],
        &["-p", "--teammate-mode", "auto"],
        &["-p", "--agent-id", "a1"],
        &["-p", "--parent-session-id", "s1"],
        &["-p", "--team-name", "t"],
        &["-p", "--agent-name", "n"],
        &["-p", "--agent-color", "red"],
        &["-p", "--channels", "srv1", "srv2"],
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

#[test]
fn an_unknown_option_fails_safe_by_taking_the_next_word_as_its_value() {
    // A newer Claude's option this table lacks: its value is not the
    // prompt, so a piped prompt skips the wait instead of waiting hours and
    // rerunning with no prompt.
    assert_eq!(p(&["-p", "--some-new-opt", "value"]), PRINT_ONLY);
    assert_eq!(p(&["-p", "-z", "value"]), PRINT_ONLY);
    // Followed by a real prompt, the prompt is missed too. That is the safe
    // side: `prompt` only matters with stdin on a pipe, where a missed
    // prompt means "no wait", the behaviour before this feature.
    assert_eq!(p(&["-p", "--some-new-flag", "the real prompt"]), PRINT_ONLY);
    // It takes at most one word, and never an option.
    assert_eq!(p(&["-p", "--some-new-opt", "v", "x"]), PRINT_PROMPT);
    assert_eq!(
        p(&["-p", "--some-new-flag", "--verbose", "x"]),
        PRINT_PROMPT
    );
    assert_eq!(p(&["--some-new-opt=v", "-p", "x"]), PRINT_PROMPT);
    assert_eq!(p(&["-p", "--some-new-flag", "--", "x"]), PRINT_PROMPT);
}

#[test]
fn every_option_is_in_one_table_only() {
    let all: Vec<&str> = [REQUIRED, VARIADIC, OPTIONAL, FLAGS].concat();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), all.len(), "an option is in two tables");
}

/// Drift check: every long option `claude --help` lists is in the table of
/// its kind. Ignored by default (it needs `claude` installed); run with
/// `cargo test -p amplihack-cli the_tables_cover_claude_help -- --ignored`
/// after a Claude release. Hidden options cannot be checked this way.
#[test]
#[ignore = "needs the claude binary"]
fn the_tables_cover_claude_help() {
    let out = std::process::Command::new("claude")
        .arg("--help")
        .output()
        .expect("run claude --help");
    let help = String::from_utf8_lossy(&out.stdout);
    let mut missing = Vec::new();
    for line in help.lines() {
        // An option line is indented two spaces; deeper lines continue a
        // description and may quote options.
        let Some(line) = line.strip_prefix("  ").filter(|l| l.starts_with('-')) else {
            continue;
        };
        // `-c, --continue`, `--allowedTools, --allowed-tools <tools...>`.
        let (names, rest) = match line.find(['<', '[']) {
            Some(at) if !line[..at].contains("  ") => line.split_at(at),
            _ => (line.split("  ").next().unwrap_or(line), ""),
        };
        let value = rest.split_whitespace().next().unwrap_or("");
        let table = if value.starts_with('<') && value.contains("...") {
            VARIADIC
        } else if value.starts_with('<') {
            REQUIRED
        } else if value.starts_with('[') {
            OPTIONAL
        } else {
            FLAGS
        };
        for name in names.split(',').map(str::trim) {
            if let Some(long) = name.strip_prefix("--")
                && !table.contains(&long)
            {
                missing.push(format!("--{long} {value}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "options missing from claude_args: {missing:?}"
    );
}
