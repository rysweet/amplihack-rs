//! Issue #2589: the Codex rendering of the policy list.
//!
//! Codex's own mechanism is an execpolicy rules file: `prefix_rule` compares
//! the command's leading tokens, so a position holding alternatives stays one
//! rule rather than becoming several, and the `match` and `not_match`
//! examples are validated when the file loads. That is where the promise
//! that plain `git push` still runs gets checked by Codex itself.
//!
//! The file is amplihack's own, and its first line says so. Both ends turn
//! on that line: the install replaces a file at that path only when the bytes
//! there carry the marker, and the uninstall removes one only on the same
//! terms, so a rules file written by hand is neither overwritten by one
//! command nor deleted by the other.

use super::Token;

/// The first line of the file amplihack owns.
pub(crate) const OWNERSHIP_MARKER: &str = "# amplihack: dangerous-command deny rules";

/// What a rule written for an installation's own addition says, since the
/// shipped advice does not apply to a command amplihack does not know.
const INSTANCE_ADVICE: &str = "This command is denied by the policy configured for this \
     amplihack installation; ask whoever set it up what to use instead.";

/// The whole rules file for `instance`, byte-identical across calls.
pub(crate) fn rules_file(instance: &[String]) -> String {
    let mut out = String::new();
    out.push_str(OWNERSHIP_MARKER);
    out.push('\n');
    out.push_str("# Written by amplihack from one policy list, and replaced on the next\n");
    out.push_str("# install or update as long as this first line is still here. Without\n");
    out.push_str("# it the file is left alone and amplihack writes no Codex rules at\n");
    out.push_str("# all. Keep your own rules in another file in this directory;\n");
    out.push_str("# amplihack never reads or replaces those.\n");

    for entry in super::DEFAULTS {
        out.push('\n');
        out.push_str(&format!("# {}\n", entry.id));
        out.push_str(&rule(
            &static_pattern(entry.tokens),
            entry.justification,
            entry.match_examples,
            entry.not_match_examples,
        ));
    }

    for prefix in super::instance_only_prefixes(instance) {
        out.push('\n');
        out.push_str("# added by this installation\n");
        out.push_str(&rule(
            &word_pattern(prefix.as_str()),
            INSTANCE_ADVICE,
            &[prefix.as_str()],
            &[],
        ));
    }
    out
}

/// The `pattern` value for a shipped entry's tokens, keeping alternatives as
/// a nested list so one entry stays one rule.
fn static_pattern(tokens: &[Token]) -> String {
    let parts: Vec<String> = tokens
        .iter()
        .map(|token| match token {
            Token::Word(word) => quoted(word),
            Token::AnyOf(words) => format!("[{}]", quoted_list(words.iter().copied())),
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// The `pattern` value for a validated prefix, one quoted word per token.
fn word_pattern(prefix: &str) -> String {
    format!("[{}]", quoted_list(prefix.split(' ')))
}

/// One rule: forbidden, with its advice and its examples. `not_match` is
/// omitted when the entry has no example to spare.
fn rule(
    pattern: &str,
    justification: &str,
    match_examples: &[&str],
    not_match_examples: &[&str],
) -> String {
    let mut out = String::new();
    out.push_str("prefix_rule(\n");
    out.push_str(&format!("    pattern = {pattern},\n"));
    out.push_str("    decision = \"forbidden\",\n");
    out.push_str(&format!("    justification = {},\n", quoted(justification)));
    out.push_str(&format!(
        "    match = [{}],\n",
        quoted_list(match_examples.iter().copied())
    ));
    if !not_match_examples.is_empty() {
        out.push_str(&format!(
            "    not_match = [{}],\n",
            quoted_list(not_match_examples.iter().copied())
        ));
    }
    out.push_str(")\n");
    out
}

/// A string literal. Every value reaching here is either a validated
/// [`super::Prefix`] or shipped text the unit tests hold free of the quote
/// and backslash characters, and this asserts that again on the way out: the
/// file is execution policy, where a rendering defect is a policy bypass
/// (SEC-V3).
fn quoted(text: &str) -> String {
    debug_assert!(
        !text.contains('"') && !text.contains('\\') && !text.contains('\n'),
        "a rendered Codex literal must hold no quote, backslash or newline"
    );
    let safe: String = text
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && !c.is_control())
        .collect();
    format!("\"{safe}\"")
}

fn quoted_list<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.map(quoted).collect::<Vec<_>>().join(", ")
}
