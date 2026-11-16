//! Issue #2589: the single list of dangerous commands amplihack denies, and
//! the three per-harness renderings of it.
//!
//! One list, declared once ([`DEFAULTS`]), rendered into each supported
//! harness's own allow/deny mechanism: `permissions.deny` for Claude Code
//! ([`claude`]), an execpolicy rules file for Codex ([`codex`]) and
//! `--deny-tool` arguments for Copilot CLI ([`copilot`]). No harness writer
//! declares a command of its own.
//!
//! An installation adds entries through `policy.deny_commands` in its own
//! amplihack user config. Additions follow the defaults and can never remove
//! one: there is no removal syntax and none is added. Nothing that names a
//! particular machine, account or person belongs in this module; that detail
//! lives in config.
//!
//! What these rules are. Each rendering matches a command prefix, so a
//! nested shell (`sh -c '...'`), an interposing command (`env`, `xargs`), a
//! reordered flag or a shell alias passes. They prevent a mistake; they are
//! not a security boundary, and the pull request says so plainly.

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod copilot;
pub(crate) mod prefix;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_prefix;
#[cfg(test)]
mod tests_render;

pub(crate) use prefix::Prefix;

/// One position of a command pattern: a fixed word, or any one of several.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Token {
    Word(&'static str),
    /// Alternatives at one position, expanded into one prefix each for the
    /// renderings that match text, and kept as a list for Codex, which
    /// compares tokens.
    AnyOf(&'static [&'static str]),
}

/// One denied command, with the advice that goes with it.
#[derive(Debug)]
pub(crate) struct Entry {
    /// Stable identifier, used in tests and in the rendered Codex comments.
    pub id: &'static str,
    /// The command pattern, in order.
    pub tokens: &'static [Token],
    /// What to do instead. One line, so it renders inside a Codex string
    /// literal, and free of quote and backslash characters for the same
    /// reason.
    pub justification: &'static str,
    /// Commands this entry denies, checked by the unit tests and written
    /// into the Codex file, where loading validates them.
    pub match_examples: &'static [&'static str],
    /// Commands this entry must not deny, checked and written the same way.
    pub not_match_examples: &'static [&'static str],
}

/// The advice that follows every pattern kill. It names `ps` rather than
/// `pgrep`, which this list also denies.
const PATTERN_KILL_ADVICE: &str = "List processes with `ps -ef` (or `ps aux`) and read the list \
     before stopping anything; stop only a PID this run started and recorded, with `cmd & \
     pid=$!; wait $pid`, or a PID read from a pid file.";

/// The shipped list, in a fixed order: the pattern kills first, then the
/// tmux kills, then the forced and deleting pushes. The order is what makes
/// the idempotence comparisons on disk meaningful.
pub(crate) static DEFAULTS: &[Entry] = &[
    Entry {
        id: "pkill",
        tokens: &[Token::Word("pkill")],
        justification: PATTERN_KILL_ADVICE,
        match_examples: &["pkill -f vitest", "pkill node"],
        not_match_examples: &["ps -ef", "kill 1234"],
    },
    Entry {
        id: "killall",
        tokens: &[Token::Word("killall")],
        justification: PATTERN_KILL_ADVICE,
        match_examples: &["killall node"],
        not_match_examples: &["ps aux", "kill 1234"],
    },
    Entry {
        id: "pgrep",
        tokens: &[Token::Word("pgrep")],
        justification: PATTERN_KILL_ADVICE,
        match_examples: &["pgrep -f scripts/check"],
        not_match_examples: &["ps -ef"],
    },
    Entry {
        id: "tmux-kill-server",
        tokens: &[Token::Word("tmux"), Token::Word("kill-server")],
        justification: "A tmux server is shared, so stopping it ends every other run on this \
             machine; stop the one session you created with `tmux kill-session -t <name>`, or \
             run your own server on a private socket with `tmux -S <socket>`.",
        match_examples: &["tmux kill-server"],
        not_match_examples: &[
            "tmux -S /tmp/own.sock kill-server",
            "tmux list-sessions",
            "tmux kill-session -t work",
        ],
    },
    Entry {
        id: "tmux-kill-session-all",
        tokens: &[
            Token::Word("tmux"),
            Token::Word("kill-session"),
            Token::Word("-a"),
        ],
        justification: "The -a form closes every session but the current one, including other \
             runs on this machine; name the session you created instead, with `tmux kill-session \
             -t <name>`.",
        match_examples: &["tmux kill-session -a"],
        not_match_examples: &["tmux kill-session -t work", "tmux kill-session"],
    },
    Entry {
        id: "git-push-force-or-delete",
        tokens: &[
            Token::Word("git"),
            Token::Word("push"),
            Token::AnyOf(&["--force", "-f", "--force-with-lease", "--delete", "-d"]),
        ],
        justification: "Add a new commit rather than rewriting a branch that has been pushed, \
             and remove a remote branch through the review tool; plain `git push`, `git push -u \
             origin <branch>` and `git push origin HEAD:<branch>` are unaffected.",
        match_examples: &["git push --force", "git push -f origin main"],
        not_match_examples: &[
            "git push",
            "git push -u origin main",
            "git push origin HEAD:topic",
        ],
    },
];

impl Entry {
    /// Every prefix this entry renders: one per combination of its
    /// alternatives, in declaration order.
    pub(crate) fn prefixes(&self) -> Vec<Prefix> {
        let mut built: Vec<String> = vec![String::new()];
        for token in self.tokens {
            let words: &[&str] = match token {
                Token::Word(word) => std::slice::from_ref(word),
                Token::AnyOf(words) => words,
            };
            let mut next = Vec::with_capacity(built.len() * words.len());
            for base in &built {
                for word in words {
                    next.push(if base.is_empty() {
                        (*word).to_string()
                    } else {
                        format!("{base} {word}")
                    });
                }
            }
            built = next;
        }
        built
            .into_iter()
            .map(|raw| {
                // A shipped entry that does not validate is a defect in this
                // file, not a runtime condition; the unit tests cover it.
                Prefix::try_from(raw.as_str()).unwrap_or_else(|error| {
                    panic!(
                        "policy entry {} renders an invalid prefix: {error}",
                        self.id
                    )
                })
            })
            .collect()
    }
}

/// Every prefix the shipped list renders, in list order.
pub(crate) fn default_prefixes() -> Vec<Prefix> {
    DEFAULTS.iter().flat_map(Entry::prefixes).collect()
}

/// The installation's own additions: the entries that validate, deduplicated
/// and in file order, and one reason per entry that did not.
///
/// The reason quotes the rejected value, which is attacker-chosen, so it goes
/// through the config module's escaping first (SEC-V4).
pub(crate) fn instance_prefixes(instance: &[String]) -> (Vec<Prefix>, Vec<String>) {
    let mut accepted: Vec<Prefix> = Vec::new();
    let mut reasons = Vec::new();
    for raw in instance {
        match Prefix::try_from(raw.as_str()) {
            Ok(prefix) => {
                if !accepted.contains(&prefix) {
                    accepted.push(prefix);
                }
            }
            Err(error) => reasons.push(format!(
                "policy.deny_commands entry \"{}\" was not applied: {error}",
                crate::user_config::safe::display_safe(raw)
            )),
        }
    }
    (accepted, reasons)
}

/// The installation's additions that are not already defaults.
pub(crate) fn instance_only_prefixes(instance: &[String]) -> Vec<Prefix> {
    let defaults = default_prefixes();
    instance_prefixes(instance)
        .0
        .into_iter()
        .filter(|prefix| !defaults.contains(prefix))
        .collect()
}

/// The defaults followed by the installation's additions. An addition never
/// removes a default and never reorders one.
pub(crate) fn effective_prefixes(instance: &[String]) -> Vec<Prefix> {
    let mut prefixes = default_prefixes();
    prefixes.extend(instance_only_prefixes(instance));
    prefixes
}

/// What `amplihack install` and `amplihack update` say about the policy, one
/// plain sentence per line.
pub(crate) fn install_report_lines() -> Vec<&'static str> {
    vec![
        "Dangerous-command deny rules written from amplihack's own policy list: Claude Code \
         reads them from permissions.deny in settings.json, and Codex from an execpolicy rules \
         file in its home.",
        "Copilot CLI saves approvals per project and keeps no user-level file for command deny \
         rules, so amplihack supplies the same policy as session arguments on each launch.",
    ]
}

/// The one-line warning for a harness whose deny rules could not be written.
/// A security control that is not installed must not be silent (SEC-C3).
pub(crate) fn write_failure_warning(harness: &str, reason: &str) -> String {
    let one_line = |text: &str| -> String {
        text.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    };
    format!(
        "{}: the dangerous-command deny rules were not written ({}); fix what that names, then \
         run `amplihack install` again",
        one_line(harness),
        one_line(reason)
    )
}
