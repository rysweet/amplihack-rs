//! Issue #2589 (TDD): the three per-harness renderings of the one policy
//! list. Every renderer is pure, takes the instance additions, and is stable
//! across calls, which is what makes the idempotence comparisons on disk
//! meaningful.
//!
//! Expected API:
//!
//! ```ignore
//! policy::claude::deny_entries(instance: &[String]) -> Vec<String>;
//! policy::claude::ensure_deny(settings: &mut serde_json::Value, instance: &[String]) -> DenyOutcome;
//! policy::claude::DenyOutcome { Written, AlreadyCurrent, Unparseable(String) }
//! policy::codex::OWNERSHIP_MARKER: &str;
//! policy::codex::rules_file(instance: &[String]) -> String;
//! policy::copilot::deny_args(instance: &[String]) -> Vec<String>;
//! ```

use super::claude::{self, DenyOutcome};
use super::codex;
use super::copilot;
use serde_json::{Value, json};

// --- Claude (F6, D4) --------------------------------------------------------

#[test]
fn claude_renders_one_bash_prefix_entry_per_prefix() {
    let entries = claude::deny_entries(&[]);
    assert_eq!(
        entries,
        [
            "Bash(pkill:*)",
            "Bash(killall:*)",
            "Bash(pgrep:*)",
            "Bash(tmux kill-server:*)",
            "Bash(tmux kill-session -a:*)",
            "Bash(git push --force:*)",
            "Bash(git push -f:*)",
            "Bash(git push --force-with-lease:*)",
            "Bash(git push --delete:*)",
            "Bash(git push -d:*)",
        ]
    );
}

#[test]
fn claude_entries_include_instance_additions_after_the_defaults() {
    let entries = claude::deny_entries(&["shutdown".to_string()]);
    assert_eq!(entries.last().map(String::as_str), Some("Bash(shutdown:*)"));
    assert!(entries.contains(&"Bash(pkill:*)".to_string()));
}

/// Byte-identical output is load-bearing: both file writers compare the
/// rendering with what is on disk and skip the write when they match, so a
/// rendering that varied between calls would rewrite the file on every run.
/// This fails if the order ever becomes set-derived -- swapping the `Vec` in
/// `effective_prefixes` for a `HashSet` is enough.
#[test]
fn claude_entries_are_stable_across_calls() {
    assert_eq!(claude::deny_entries(&[]), claude::deny_entries(&[]));
}

#[test]
fn ensure_deny_creates_the_deny_array_and_reports_written() {
    let mut settings = json!({});
    assert_eq!(
        claude::ensure_deny(&mut settings, &[]),
        DenyOutcome::Written
    );
    let deny = settings["permissions"]["deny"].as_array().unwrap();
    for entry in claude::deny_entries(&[]) {
        assert_eq!(
            deny.iter().filter(|v| v.as_str() == Some(&entry)).count(),
            1,
            "{entry} must appear exactly once in {deny:?}"
        );
    }
}

#[test]
fn a_second_ensure_deny_adds_nothing_and_reports_already_current() {
    let mut settings = json!({});
    claude::ensure_deny(&mut settings, &[]);
    let before = serde_json::to_string(&settings).unwrap();
    assert_eq!(
        claude::ensure_deny(&mut settings, &[]),
        DenyOutcome::AlreadyCurrent
    );
    assert_eq!(serde_json::to_string(&settings).unwrap(), before);
}

#[test]
fn ensure_deny_keeps_a_users_own_entries_and_unrelated_keys() {
    let mut settings = json!({
        "language": "en",
        "permissions": {
            "allow": ["Bash"],
            "deny": ["Bash(rm -rf /:*)", "Read(./.env)"],
            "defaultMode": "bypassPermissions"
        }
    });
    assert_eq!(
        claude::ensure_deny(&mut settings, &[]),
        DenyOutcome::Written
    );
    assert_eq!(settings["language"], "en");
    assert_eq!(settings["permissions"]["defaultMode"], "bypassPermissions");
    assert_eq!(settings["permissions"]["allow"], json!(["Bash"]));
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        &deny[..2],
        ["Bash(rm -rf /:*)", "Read(./.env)"],
        "the user's own entries stay, and stay first"
    );
    assert!(deny.contains(&"Bash(pkill:*)"));
}

/// SEC-C2: a malformed `permissions` or `deny` is reported, never replaced.
/// Overwriting a mismatched value would be a new way to discard a profile's
/// own permissions.
#[test]
fn ensure_deny_refuses_a_malformed_permissions_value_and_changes_nothing() {
    for malformed in [
        json!({"permissions": "bypass"}),
        json!({"permissions": ["Bash"]}),
        json!({"permissions": {"deny": "Bash(pkill:*)"}}),
        json!({"permissions": {"deny": {"Bash": true}}}),
    ] {
        let mut settings = malformed.clone();
        let outcome = claude::ensure_deny(&mut settings, &[]);
        assert!(
            matches!(outcome, DenyOutcome::Unparseable(_)),
            "expected Unparseable for {malformed}, got {outcome:?}"
        );
        assert_eq!(
            settings, malformed,
            "the value must be left exactly as found"
        );
    }
}

#[test]
fn ensure_deny_refuses_a_settings_root_that_is_not_an_object() {
    let mut settings = json!(["not", "an", "object"]);
    let before = settings.clone();
    assert!(matches!(
        claude::ensure_deny(&mut settings, &[]),
        DenyOutcome::Unparseable(_)
    ));
    assert_eq!(settings, before);
}

// --- Codex (F7, D5) ---------------------------------------------------------

#[test]
fn the_codex_file_opens_with_the_amplihack_ownership_marker() {
    let text = codex::rules_file(&[]);
    assert!(
        text.starts_with(codex::OWNERSHIP_MARKER),
        "the first line identifies the file as amplihack's own, so uninstall \
         removes this file and nothing else: {}",
        text.lines().next().unwrap_or_default()
    );
}

#[test]
fn the_codex_file_holds_one_forbidden_prefix_rule_per_entry() {
    let text = codex::rules_file(&[]);
    assert_eq!(
        text.matches("prefix_rule(").count(),
        super::DEFAULTS.len(),
        "one rule per entry, alternatives collapsed into the pattern"
    );
    assert_eq!(
        text.matches("decision = \"forbidden\"").count(),
        super::DEFAULTS.len()
    );
    assert_eq!(
        text.matches("justification = ").count(),
        super::DEFAULTS.len()
    );
}

#[test]
fn the_codex_file_names_the_pattern_kills_and_the_tmux_and_push_entries() {
    let text = codex::rules_file(&[]);
    for token in [
        "\"pkill\"",
        "\"killall\"",
        "\"pgrep\"",
        "\"kill-server\"",
        "\"kill-session\"",
        "\"--force\"",
        "\"--force-with-lease\"",
        "\"--delete\"",
    ] {
        assert!(text.contains(token), "{token} missing from:\n{text}");
    }
}

#[test]
fn the_codex_push_rule_carries_not_match_examples_for_the_ordinary_forms() {
    let text = codex::rules_file(&[]);
    for example in [
        "git push",
        "git push -u origin main",
        "git push origin HEAD:topic",
    ] {
        assert!(
            text.contains(example),
            "the ordinary push form {example:?} must appear as a not_match example, \
             so the file's own load validates A10:\n{text}"
        );
    }
    assert!(text.contains("not_match = ["), "{text}");
    assert!(text.contains("match = ["), "{text}");
}

#[test]
fn an_instance_addition_renders_its_own_codex_rule() {
    let text = codex::rules_file(&["shutdown".to_string()]);
    assert_eq!(
        text.matches("prefix_rule(").count(),
        super::DEFAULTS.len() + 1
    );
    assert!(text.contains("\"shutdown\""), "{text}");
}

/// Byte-identical output is load-bearing: both file writers compare the
/// rendering with what is on disk and skip the write when they match, so a
/// rendering that varied between calls would rewrite the file on every run.
/// This fails if the order ever becomes set-derived -- swapping the `Vec` in
/// `effective_prefixes` for a `HashSet` is enough.
#[test]
fn the_codex_file_is_byte_identical_across_calls() {
    let instance = vec!["shutdown".to_string()];
    assert_eq!(codex::rules_file(&instance), codex::rules_file(&instance));
}

/// SEC-V3: the file is execution policy, so a rendering defect is a policy
/// bypass. Nothing in it may close a string or a call early.
#[test]
fn no_rendered_codex_line_carries_a_quote_or_backslash_inside_a_pattern_token() {
    let text = codex::rules_file(&["shutdown".to_string()]);
    for line in text.lines().filter(|l| l.contains('"')) {
        let inside: String = line.chars().filter(|c| *c == '"').collect();
        assert_eq!(
            inside.len() % 2,
            0,
            "unbalanced quotes on line {line:?} of the rules file"
        );
        assert!(!line.contains('\\'), "backslash on line {line:?}");
    }
}

// --- Copilot (F8, D1) -------------------------------------------------------

#[test]
fn copilot_renders_one_deny_tool_flag_pair_per_prefix() {
    let args = copilot::deny_args(&[]);
    assert_eq!(
        args.len(),
        super::default_prefixes().len() * 2,
        "a flag and its value per prefix: {args:?}"
    );
    assert_eq!(&args[..2], ["--deny-tool", "shell(pkill:*)"]);
    assert!(
        args.windows(2)
            .any(|w| w == ["--deny-tool", "shell(tmux kill-session -a:*)"]),
        "{args:?}"
    );
    assert!(
        args.windows(2)
            .any(|w| w == ["--deny-tool", "shell(git push --force:*)"]),
        "{args:?}"
    );
}

#[test]
fn copilot_args_include_instance_additions() {
    let args = copilot::deny_args(&["shutdown".to_string()]);
    assert_eq!(
        &args[args.len() - 2..],
        ["--deny-tool", "shell(shutdown:*)"]
    );
}

/// The rendering is a function of the policy list alone. It once took the
/// caller's argv and skipped a prefix that already appeared after a
/// `--deny-tool`, which an adjacent pair anywhere in argv was enough to
/// trigger. That is why the parameter is gone rather than the scan narrowed:
/// a renderer that cannot read argv cannot be talked out of a rule by it.
#[test]
fn copilot_args_do_not_depend_on_the_callers_own_arguments() {
    let expected = copilot::deny_args(&[]);
    assert_eq!(
        expected.len(),
        super::default_prefixes().len() * 2,
        "every prefix renders: {expected:?}"
    );
    assert!(
        expected.iter().any(|a| a == "shell(pkill:*)"),
        "{expected:?}"
    );
}

/// Byte-identical output is load-bearing: both file writers compare the
/// rendering with what is on disk and skip the write when they match, so a
/// rendering that varied between calls would rewrite the file on every run.
/// This fails if the order ever becomes set-derived -- swapping the `Vec` in
/// `effective_prefixes` for a `HashSet` is enough.
#[test]
fn copilot_args_are_stable_across_calls() {
    assert_eq!(copilot::deny_args(&[]), copilot::deny_args(&[]));
}

/// The rendered flags become process arguments visible to every user on the
/// host, so they carry command names only and never task text.
#[test]
fn copilot_args_carry_short_command_names_only() {
    for arg in copilot::deny_args(&[]) {
        assert!(
            arg.len() <= 64,
            "a deny flag value must stay a short command name: {arg:?}"
        );
        assert!(!arg.contains('\n'), "{arg:?}");
    }
}

// --- the Codex literal invariant (SEC-V4, the shipped half) -----------------
//
// `codex::rules_file` writes every id, justification and example into a
// double-quoted Starlark literal and escapes nothing, because the only values
// that reach it are either shipped constants in `DEFAULTS` or prefixes that
// `Prefix::try_from` has already narrowed. `tests_prefix.rs` holds the second
// half; this is the first. Without it the argument rests on reading the
// constants, and the next entry added to the list -- an apostrophe in a
// justification, a line break for width -- breaks the rendered file silently.

/// Text that renders inside a double-quoted Starlark literal unescaped.
fn safe_in_a_literal(text: &str) -> Result<(), String> {
    if let Some(bad) = text
        .chars()
        .find(|c| *c == '"' || *c == '\\' || c.is_control())
    {
        return Err(format!("{bad:?} in {text:?}"));
    }
    Ok(())
}

#[test]
fn every_shipped_entry_renders_inside_a_codex_string_literal_unescaped() {
    for entry in super::DEFAULTS {
        let mut texts: Vec<(&str, &str)> =
            vec![("id", entry.id), ("justification", entry.justification)];
        texts.extend(entry.match_examples.iter().map(|e| ("match example", *e)));
        texts.extend(
            entry
                .not_match_examples
                .iter()
                .map(|e| ("not_match example", *e)),
        );
        for (what, text) in texts {
            if let Err(bad) = safe_in_a_literal(text) {
                panic!(
                    "policy entry {}: the {what} needs escaping to render into the Codex \
                     rules file, and the renderer escapes nothing: {bad}",
                    entry.id
                );
            }
        }
        for token in entry.tokens {
            let words: &[&str] = match token {
                super::Token::Word(word) => std::slice::from_ref(word),
                super::Token::AnyOf(words) => words,
            };
            for word in words {
                assert!(
                    safe_in_a_literal(word).is_ok(),
                    "policy entry {}: the token {word:?} needs escaping",
                    entry.id
                );
            }
        }
    }
}

/// The rendered file is therefore quote-balanced: every `"` in it opens or
/// closes a literal, and none came out of an entry.
#[test]
fn the_rendered_rules_file_has_balanced_quotes_on_every_line() {
    for line in codex::rules_file(&[]).lines() {
        assert!(
            line.matches('"').count() % 2 == 0,
            "an odd number of quotes means a value broke out of its literal: {line:?}"
        );
    }
}
