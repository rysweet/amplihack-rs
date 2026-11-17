//! Issue #2589 (TDD): the single dangerous-command policy list.
//!
//! Expected API, declared in `policy/mod.rs`:
//!
//! ```ignore
//! pub(crate) enum Token { Word(&'static str), AnyOf(&'static [&'static str]) }
//! pub(crate) struct Entry {
//!     pub id: &'static str,
//!     pub tokens: &'static [Token],
//!     pub justification: &'static str,
//!     pub match_examples: &'static [&'static str],
//!     pub not_match_examples: &'static [&'static str],
//! }
//! pub(crate) static DEFAULTS: &[Entry];
//! impl Entry { pub(crate) fn prefixes(&self) -> Vec<Prefix>; }
//! pub(crate) fn default_prefixes() -> Vec<Prefix>;
//! pub(crate) fn effective_prefixes(instance: &[String]) -> Vec<Prefix>;
//! pub(crate) fn instance_prefixes(instance: &[String]) -> (Vec<Prefix>, Vec<String>);
//! pub(crate) fn install_report_lines(
//!     claude: HarnessWrite,
//!     codex: HarnessWrite,
//! ) -> Vec<ReportLine>;
//! pub(crate) fn write_failure_warning(harness: &str, reason: &str) -> String;
//! ```

use super::*;

/// Every prefix the shipped list renders, in the order F2/D13 fixes: the
/// pattern kills, then the tmux kills, then the forced and deleting pushes.
/// Order is fixed so the idempotence comparisons below mean something.
const EXPECTED_DEFAULT_PREFIXES: &[&str] = &[
    "pkill",
    "killall",
    "pgrep",
    "tmux kill-server",
    "tmux kill-session -a",
    "git push --force",
    "git push -f",
    "git push --force-with-lease",
    "git push --delete",
    "git push -d",
];

/// Whether `prefix` denies `command`. Every rendering matches on leading
/// whole words, so a command is denied when it is the prefix itself or the
/// prefix followed by a space. The match tests below share this one
/// definition rather than spelling it out apiece.
fn denies(prefix: &Prefix, command: &str) -> bool {
    command == prefix.as_str() || command.starts_with(&format!("{} ", prefix.as_str()))
}

fn strings(prefixes: &[Prefix]) -> Vec<String> {
    prefixes.iter().map(|p| p.as_str().to_string()).collect()
}

#[test]
fn defaults_are_the_documented_entries_in_the_documented_order() {
    let ids: Vec<&str> = DEFAULTS.iter().map(|entry| entry.id).collect();
    assert_eq!(
        ids,
        [
            "pkill",
            "killall",
            "pgrep",
            "tmux-kill-server",
            "tmux-kill-session-all",
            "git-push-force-or-delete",
        ],
        "F2/D13: the pattern kills come first, then tmux, then the pushes"
    );
}

#[test]
fn default_prefixes_expand_any_of_in_place() {
    assert_eq!(strings(&default_prefixes()), EXPECTED_DEFAULT_PREFIXES);
}

#[test]
fn each_entry_renders_at_least_one_prefix_and_they_concatenate_to_the_list() {
    let mut all = Vec::new();
    for entry in DEFAULTS {
        let rendered = entry.prefixes();
        assert!(
            !rendered.is_empty(),
            "entry {} renders no prefix at all",
            entry.id
        );
        all.extend(strings(&rendered));
    }
    assert_eq!(all, EXPECTED_DEFAULT_PREFIXES);
}

#[test]
fn every_entry_carries_justification_text_saying_what_to_do_instead() {
    for entry in DEFAULTS {
        let text = entry.justification;
        assert!(
            text.len() > 40,
            "entry {} has no usable justification: {text:?}",
            entry.id
        );
        assert!(
            !text.contains('\n'),
            "entry {} justification must be one line so it renders inside a Codex \
             string literal: {text:?}",
            entry.id
        );
    }
}

/// D6: the newer owner statement denies `pgrep`, so the advice text may not
/// send the agent back to it. `ps` is the replacement.
#[test]
fn the_pattern_kill_justification_recommends_ps_and_never_pgrep() {
    for id in ["pkill", "killall", "pgrep"] {
        let entry = DEFAULTS
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("no entry {id}"));
        assert!(
            entry.justification.contains("ps "),
            "{id}: the advice must name `ps`: {:?}",
            entry.justification
        );
        assert!(
            !entry.justification.contains("pgrep"),
            "{id}: the advice must not recommend a denied command: {:?}",
            entry.justification
        );
        assert!(
            entry.justification.contains("PID"),
            "{id}: the advice must say to stop only a recorded PID: {:?}",
            entry.justification
        );
    }
}

/// A10/F3: these are the forms that must keep working, and no default prefix
/// may be a prefix of any of them.
const ORDINARY_COMMANDS: &[&str] = &[
    "git push",
    "git push -u origin main",
    "git push origin HEAD:topic",
    "tmux -S /tmp/s kill-server",
    "tmux kill-session -t work",
    "ps -ef",
    "kill 1234",
];

#[test]
fn no_default_prefix_matches_an_ordinary_command() {
    for prefix in default_prefixes() {
        for command in ORDINARY_COMMANDS {
            assert!(
                !denies(&prefix, command),
                "prefix {:?} would deny the ordinary command {command:?}",
                prefix.as_str()
            );
        }
    }
}

/// A `not_match` example is a promise that the command still runs, and the
/// promise is about the shipped list as a whole rather than the one rule the
/// example is written under. Codex validates `not_match` per rule when the
/// file loads, so an example its own entry spares and another entry's prefix
/// denies would load cleanly and still break the command; the Claude and
/// Copilot renderings have no cross-entry check at all. Hence every example
/// is held against every default prefix here.
#[test]
fn every_not_match_example_escapes_every_prefix_in_the_whole_list() {
    let all = default_prefixes();
    for entry in DEFAULTS {
        for example in entry.not_match_examples {
            for prefix in &all {
                assert!(
                    !denies(prefix, example),
                    "entry {}: not_match example {example:?} promises the command runs, but \
                     prefix {:?} denies it",
                    entry.id,
                    prefix.as_str()
                );
            }
        }
    }
}

#[test]
fn every_match_example_is_matched_by_a_prefix_of_its_own_entry() {
    for entry in DEFAULTS {
        assert!(
            !entry.match_examples.is_empty(),
            "entry {} has no match example",
            entry.id
        );
        for example in entry.match_examples {
            let matched = entry
                .prefixes()
                .iter()
                .any(|prefix| denies(prefix, example));
            assert!(
                matched,
                "entry {}: match example {example:?} is matched by no prefix of the entry",
                entry.id
            );
        }
    }
}

// --- instance additions (F5, R6) -------------------------------------------

#[test]
fn an_instance_addition_follows_the_defaults_and_removes_none_of_them() {
    let instance = vec!["shutdown".to_string(), "reboot".to_string()];
    let effective = strings(&effective_prefixes(&instance));
    assert_eq!(
        &effective[..EXPECTED_DEFAULT_PREFIXES.len()],
        EXPECTED_DEFAULT_PREFIXES,
        "the defaults stay, in order, ahead of the additions"
    );
    assert_eq!(
        &effective[EXPECTED_DEFAULT_PREFIXES.len()..],
        ["shutdown", "reboot"]
    );
}

#[test]
fn an_instance_entry_equal_to_a_default_adds_nothing() {
    let effective = strings(&effective_prefixes(&["pkill".to_string()]));
    assert_eq!(effective, EXPECTED_DEFAULT_PREFIXES);
}

#[test]
fn duplicate_instance_entries_are_rendered_once() {
    let instance = vec!["shutdown".to_string(), "shutdown".to_string()];
    let effective = strings(&effective_prefixes(&instance));
    assert_eq!(
        effective.iter().filter(|p| *p == "shutdown").count(),
        1,
        "{effective:?}"
    );
}

#[test]
fn no_instance_entry_can_remove_a_default() {
    // There is no removal syntax: anything that looks like one is rejected
    // as a prefix and the defaults still render in full.
    for attempt in ["!pkill", "-pkill", "~pkill"] {
        let effective = strings(&effective_prefixes(&[attempt.to_string()]));
        assert_eq!(
            &effective[..EXPECTED_DEFAULT_PREFIXES.len()],
            EXPECTED_DEFAULT_PREFIXES,
            "{attempt:?} must not remove a default"
        );
    }
}

/// Byte-identical output is load-bearing: both file writers compare the
/// rendering with what is on disk and skip the write when they match, so a
/// rendering that varied between calls would rewrite the file on every run.
/// This fails if the order ever becomes set-derived -- swapping the `Vec` in
/// `effective_prefixes` for a `HashSet` is enough.
#[test]
fn effective_prefixes_is_stable_across_calls() {
    let instance = vec!["shutdown".to_string()];
    assert_eq!(
        strings(&effective_prefixes(&instance)),
        strings(&effective_prefixes(&instance))
    );
}

/// SEC-V2: a value that is not a valid prefix contributes nothing, and the
/// caller is handed one reason per rejection so it can be reported once.
#[test]
fn an_invalid_instance_entry_is_rejected_with_one_reason_and_reaches_no_rendering() {
    let instance = vec![
        "shell(x) --allow-all".to_string(),
        "pkill'; rm -rf /; echo '".to_string(),
        "ok-entry".to_string(),
    ];
    let (accepted, reasons) = instance_prefixes(&instance);
    assert_eq!(strings(&accepted), ["ok-entry"]);
    assert_eq!(
        reasons.len(),
        2,
        "one reason per rejected entry: {reasons:?}"
    );

    let effective = strings(&effective_prefixes(&instance));
    assert_eq!(
        effective,
        [EXPECTED_DEFAULT_PREFIXES, &["ok-entry"]].concat(),
        "a rejected entry never reaches a rendering"
    );
    for rendering in [
        claude::deny_entries(&instance).join("\n"),
        codex::rules_file(&instance),
        copilot::deny_args(&instance, &[]).join(" "),
    ] {
        assert!(!rendering.contains("rm -rf"), "{rendering}");
        assert!(!rendering.contains("--allow-all"), "{rendering}");
    }
}

/// SEC-V4: a rejected value is attacker-chosen, so the reason that quotes it
/// carries no raw control or bidi characters.
#[test]
fn a_rejection_reason_escapes_the_value_it_quotes() {
    let (_, reasons) = instance_prefixes(&["a\u{1b}[31mb\u{202e}c".to_string()]);
    assert_eq!(reasons.len(), 1);
    assert!(
        !reasons[0].contains('\u{1b}') && !reasons[0].contains('\u{202e}'),
        "unescaped control or bidi in {:?}",
        reasons[0]
    );
}

// --- install reporting -----------------------------------------------------
//
// The report is a function of what this run wrote, so its tests live beside
// the outcome type in `tests_report.rs`.

// --- placement rule (R17) ---------------------------------------------------

/// No host name, user path, login or capacity belongs in the shipped list or
/// in anything it renders.
#[test]
fn neither_the_module_source_nor_its_output_carries_a_host_name_or_a_user_path() {
    const SOURCES: &[(&str, &str)] = &[
        ("policy/mod.rs", include_str!("mod.rs")),
        ("policy/prefix.rs", include_str!("prefix.rs")),
        ("policy/claude.rs", include_str!("claude.rs")),
        ("policy/codex.rs", include_str!("codex.rs")),
        ("policy/copilot.rs", include_str!("copilot.rs")),
    ];
    const FORBIDDEN: &[&str] = &[
        "calypso",
        "candide",
        "/home/",
        "/Users/",
        "rysweet",
        "@mistt",
        "bb:0",
        "software-factory",
    ];
    let rendered = format!(
        "{}\n{}\n{}",
        claude::deny_entries(&[]).join("\n"),
        codex::rules_file(&[]),
        copilot::deny_args(&[], &[]).join(" ")
    );
    for (name, source) in SOURCES {
        for needle in FORBIDDEN {
            assert!(
                !source.contains(needle),
                "{name} carries {needle:?}; instance detail belongs in config"
            );
        }
    }
    for needle in FORBIDDEN {
        assert!(!rendered.contains(needle), "a rendering carries {needle:?}");
    }
}
