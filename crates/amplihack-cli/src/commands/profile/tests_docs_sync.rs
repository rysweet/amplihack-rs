use super::store::SHARED_SETTINGS_KEYS;

const REFERENCE: &str = include_str!("../../../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../../../docs/howto/claude-profiles.md");

fn backticked(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

#[test]
fn constant_has_the_twelve_documented_keys() {
    assert_eq!(SHARED_SETTINGS_KEYS.len(), 12);
}

#[test]
fn both_docs_list_every_allowlisted_key() {
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        for key in SHARED_SETTINGS_KEYS {
            assert!(
                doc.contains(&format!("`{key}`")),
                "{name} doc is missing allowlisted key `{key}`"
            );
        }
    }
}

#[test]
fn howto_settings_row_lists_no_unknown_keys() {
    let row = HOWTO
        .lines()
        .find(|l| l.contains("settings.json` (filtered)"))
        .expect("howto settings row");
    let cols: Vec<&str> = row.split('|').collect();
    let keys = backticked(cols[2]);
    assert_eq!(keys.len(), SHARED_SETTINGS_KEYS.len(), "{keys:?}");
    for k in keys {
        assert!(SHARED_SETTINGS_KEYS.contains(&k), "unknown key `{k}`");
    }
}

#[test]
fn both_docs_describe_profile_usage() {
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        assert!(
            doc.contains("amplihack profile usage"),
            "{name} doc must mention `amplihack profile usage`"
        );
    }
}

/// The phrase table in the reference is the code's table, word for word.
#[test]
fn reference_lists_the_usage_phrases_the_parser_matches() {
    use super::usage_parse::{NOT_LOGGED_IN, SPEND_LIMIT};
    for (state, phrases) in [
        ("`not_logged_in`", NOT_LOGGED_IN),
        ("`spend_limit`", SPEND_LIMIT),
    ] {
        let row = REFERENCE
            .lines()
            .find(|l| l.starts_with(&format!("| {state} | `")))
            .unwrap_or_else(|| panic!("reference phrase row for {state}"));
        let documented = backticked(row.split('|').nth(2).unwrap());
        assert_eq!(documented, phrases, "{state} phrases differ from the code");
    }
}

#[test]
fn reference_lists_every_usage_error_reason() {
    use super::usage_cmd::ErrorReason;
    for reason in [
        ErrorReason::ProfileRefused,
        ErrorReason::StartFailed,
        ErrorReason::ReadFailed,
        ErrorReason::WaitFailed,
    ] {
        assert!(
            REFERENCE.contains(&format!("| `{}` |", reason.as_str())),
            "reference is missing error reason {:?}",
            reason.as_str()
        );
    }
}

#[test]
fn docs_state_spawn_time_revalidation_is_not_atomic() {
    for doc in [REFERENCE, HOWTO] {
        let d = doc.to_lowercase();
        assert!(d.contains("just before spawn") || d.contains("immediately before spawn"));
        assert!(d.contains("not atomic"), "docs must state the race remains");
    }
}

/// The `~/.claude` mark, its JSON field and its warnings are documented
/// word for word.
#[test]
fn docs_describe_logins_live_in_default_dir() {
    use super::default_live::{
        AMBIGUOUS_WARNING, LABEL, LEDGER_WARNING, LIST_WARNING, OWN_LOGIN_WARNING,
    };
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        for text in [
            format!("live in {LABEL}"),
            "\"live_in\"".to_string(),
            ".rotator/ledger.json".to_string(),
        ] {
            assert!(doc.contains(&text), "{name} doc is missing {text}");
        }
    }
    for w in [
        LEDGER_WARNING,
        AMBIGUOUS_WARNING,
        OWN_LOGIN_WARNING,
        LIST_WARNING,
    ] {
        assert!(REFERENCE.contains(w), "reference is missing {w:?}");
    }
    let swap = super::usage_cmd::swap_warning(&super::name::ProfileName::parse("work").unwrap());
    assert!(REFERENCE.contains(&swap), "reference is missing {swap:?}");
    assert!(REFERENCE.contains("| `live_in` | string or `null` |"));
}

#[test]
fn reference_quotes_the_env_credential_warning() {
    let warning = super::usage_cmd::env_credential_warning("ANTHROPIC_API_KEY");
    assert!(
        REFERENCE.contains(&format!("`{warning}`")),
        "reference must quote: {warning}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1592: `profile rotate` is documented, flag for flag.
// ---------------------------------------------------------------------------

const ROTATE_HOWTO: &str = include_str!("../../../../../docs/howto/claude-profile-rotation.md");
const PROFILE_MANAGEMENT: &str = include_str!("../../../../../docs/PROFILE_MANAGEMENT.md");
const MKDOCS: &str = include_str!("../../../../../mkdocs.yml");

/// Every visible long flag of `rotate` and its subcommands.
fn rotate_flags() -> Vec<String> {
    use clap::Args;
    let cmd = super::rotate::RotateArgs::augment_args(clap::Command::new("rotate"));
    let mut flags = Vec::new();
    for c in std::iter::once(&cmd).chain(cmd.get_subcommands()) {
        for a in c.get_arguments() {
            if a.is_hide_set() {
                continue;
            }
            if let Some(l) = a.get_long() {
                flags.push(format!("--{l}"));
            }
        }
    }
    flags.sort();
    flags.dedup();
    flags
}

#[test]
fn rotate_has_the_documented_flags_and_hides_spawned() {
    let flags = rotate_flags();
    for want in [
        "--dry-run",
        "--session-at",
        "--week-at",
        "--margin",
        "--upgrade-margin",
        "--hold",
        "--interval",
    ] {
        assert!(flags.iter().any(|f| f == want), "missing {want}: {flags:?}");
    }
    assert!(
        !flags.iter().any(|f| f == "--spawned"),
        "--spawned is hidden"
    );
}

#[test]
fn reference_and_howto_document_profile_rotate() {
    for (name, doc) in [("reference", REFERENCE), ("rotate howto", ROTATE_HOWTO)] {
        assert!(doc.contains("amplihack profile rotate"), "{name}");
        for sub in ["start", "stop", "status", "--dry-run"] {
            assert!(doc.contains(sub), "{name} doc is missing {sub}");
        }
    }
    for flag in rotate_flags() {
        assert!(
            REFERENCE.contains(&format!("`{flag}")),
            "reference is missing {flag}"
        );
    }
    for var in [
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
        "AMPLIHACK_ROTATE_MARGIN",
        "AMPLIHACK_ROTATE_UPGRADE_MARGIN",
        "AMPLIHACK_ROTATE_HOLD",
        "AMPLIHACK_ROTATE_INTERVAL",
    ] {
        assert!(REFERENCE.contains(var), "reference is missing {var}");
    }
}

#[test]
fn reference_quotes_every_rotate_error_hint() {
    use super::ProfileError as E;
    for e in [
        E::RotateConfig("x".into()),
        E::RotatePrototypeRunning,
        E::RotateRefused("x".into()),
        E::RotateInconsistent("x".into()),
        E::RotateUnsupported,
        E::RotateDaemon("x".into()),
    ] {
        let hint = e.hint();
        assert!(!hint.is_empty(), "{e:?}");
        assert!(!e.to_string().contains('\n'), "{e:?}");
        let quoted = hint.replace('`', "");
        assert!(
            REFERENCE.replace('`', "").contains(&quoted),
            "reference is missing the hint for {e:?}: {hint}"
        );
    }
}

#[test]
fn rotate_docs_are_linked_and_in_the_nav() {
    let link = "claude-profile-rotation.md";
    for (name, doc) in [
        ("reference", REFERENCE),
        ("claude-profiles howto", HOWTO),
        ("PROFILE_MANAGEMENT", PROFILE_MANAGEMENT),
    ] {
        assert!(doc.contains(link), "{name} does not link the rotate how-to");
    }
    assert!(
        MKDOCS.contains("howto/claude-profile-rotation.md"),
        "mkdocs nav"
    );
}

#[test]
fn rotate_log_grammar_is_documented() {
    for line in [
        "cycle live=",
        "reason=threshold",
        "ALERT: no profile has capacity",
        "ALERT: no login in ~/.claude; no swap",
        "idle: auto-switch is off",
        "skip: state lock busy",
        "stop: signal received; cycle abandoned",
        "recovery: rolled back an interrupted swap",
        "recovery: finished an interrupted swap",
        "sessions archive: <summary>",
    ] {
        assert!(REFERENCE.contains(line), "reference is missing `{line}`");
    }
}

/// Rotate revalidation (#1721): the labelled skip line, both `revalidate:`
/// lines and the no-capacity ALERT suffix each have a log-grammar row.
#[test]
fn rotate_revalidation_log_rows_are_documented() {
    for row in [
        "| `skip: state changed during measurement (<labels>)` |",
        "| `revalidate: target <name> no longer eligible (<reason>); reconsidered` |",
        "| `revalidate: ~/.claude.json held no account; read again` |",
        "`ALERT: no profile has capacity live=<name> s=<n>% w=<n>% (candidates: <F> failed, <E> exhausted)`",
        "#### Rotate revalidation",
    ] {
        assert!(REFERENCE.contains(row), "reference is missing `{row}`");
    }
    for label in [
        "`journal`",
        "`live`",
        "`pin`",
        "`overflow`",
        "`target eligibility: <reason>`",
        "`target identity`",
        "`account changed`",
    ] {
        assert!(
            REFERENCE.contains(label),
            "reference is missing label {label}"
        );
    }
    // `Snap::account` is read from ~/.claude.json and `Snap::home_no_oauth`
    // from the login file, so the `live` row has to name both files.
    let live_row = REFERENCE
        .lines()
        .find(|l| l.ends_with("| `live` | Every decision |"))
        .expect("the revalidation table has a `live` row");
    for file in ["`~/.claude.json`", "`~/.claude/.credentials.json`"] {
        assert!(
            live_row.contains(file),
            "the `live` row does not name {file}"
        );
    }
}

/// Rotator autostart and failed-probe damping: the reference names the
/// variable, the command, the status line and the new log lines as the code
/// prints them, and the how-tos describe autostart.
#[test]
fn rotate_autostart_and_failed_probe_damping_are_documented() {
    for s in [
        "AMPLIHACK_ROTATE_AUTOSTART",
        "rotate autostart",
        "autostart:",
        "autostart: on (default)",
        "autostart: off (setting)",
        "autostart: off (env)",
        "autostart: off (corrupt)",
        "— inactive (auto-switch off)",
        "<root>/.auto-switch.d/rotate/autostart",
        "amplihack: started Claude login rotator (pid <pid>); 'amplihack profile rotate autostart off' to disable",
        "autostart: started pid <pid> exe <path>",
        "autostart: failed: <error>",
        "reason=failed-probe",
        "live probe failed",
        "stay: live probe failed (<k>/3)",
        "stay: live probe failed; held after last swap (<N>s left)",
    ] {
        assert!(REFERENCE.contains(s), "reference is missing `{s}`");
    }
    for s in [
        "AMPLIHACK_ROTATE_AUTOSTART",
        "amplihack profile rotate autostart off",
        "autostart: on (default)",
        "stay: live probe failed (1/3)",
        "reason=failed-probe",
        "stay: live probe failed; held after last swap (",
    ] {
        assert!(ROTATE_HOWTO.contains(s), "rotate how-to is missing `{s}`");
    }
    assert!(
        !ROTATE_HOWTO.contains("does not start again after a reboot"),
        "the reboot caveat is replaced by the autostart section"
    );
    assert!(
        !REFERENCE.contains("held after failed-probe swap")
            && !ROTATE_HOWTO.contains("held after failed-probe swap"),
        "the hold message was renamed"
    );
    assert!(
        HOWTO.contains("autostart"),
        "claude-profiles.md notes autostart under auto-switch"
    );
}

/// The how-to quotes each refusal and recovery reason as the code prints it,
/// and its examples are the code's `Display` output.
#[cfg(unix)]
#[test]
fn rotate_howto_quotes_the_refusal_reasons_word_for_word() {
    use super::ProfileError as E;
    for reason in super::rotate::DOCUMENTED_REFUSALS {
        assert!(
            ROTATE_HOWTO.contains(reason),
            "rotate how-to is missing `{reason}`"
        );
    }
    for e in [
        E::RotateRefused("profile work already has a login".into()),
        E::RotateInconsistent("the login files are not where the journal expects".into()),
    ] {
        let line = format!("error: {e}\nhint: {}", e.hint());
        assert!(
            ROTATE_HOWTO.contains(&line),
            "rotate how-to is missing:\n{line}"
        );
    }
}

/// Issue #1592: the profile root command, its sources, the rotate record and
/// both root hints appear in the docs as the code prints them.
#[test]
fn docs_describe_the_saved_profile_root() {
    use super::ProfileError as E;
    for s in [
        "amplihack profile root",
        "source: env",
        "source: config",
        "source: default",
        "~/.amplihack/profile-root",
        "~/.amplihack/rotate-root",
        "pinned by running rotator",
        "saved profile root: ",
        "recorded profile root ",
        "warning: could not record profile root: ",
        "overrides the saved root in this shell",
    ] {
        assert!(REFERENCE.contains(s), "reference is missing `{s}`");
    }
    for e in [
        E::InvalidRoot(String::new()),
        E::InvalidSavedRoot {
            path: String::new(),
            reason: String::new(),
        },
    ] {
        assert!(
            REFERENCE.contains(e.hint()),
            "reference is missing `{}`",
            e.hint()
        );
    }
    for reason in [
        "empty",
        "not an absolute path",
        "not UTF-8",
        "larger than 4096 bytes",
        "contains control characters",
        "contains a .. component",
        "is a symlink",
        "not a regular file",
        "writable by others",
    ] {
        assert!(
            REFERENCE.contains(&format!("| `{reason}` |")),
            "reason `{reason}`"
        );
    }
    assert!(HOWTO.contains("amplihack profile root"));
    assert!(ROTATE_HOWTO.contains("pinned by running rotator"));
}

// ---------------------------------------------------------------------------
// Account identity, duplicate logins and the live default (bugs 1-3).
// ---------------------------------------------------------------------------

#[test]
fn reference_quotes_the_switch_none_messages() {
    for m in [super::CLEARED, super::NOT_SET] {
        assert!(
            REFERENCE.contains(&format!("`{m}`")),
            "reference is missing {m:?}"
        );
        assert!(HOWTO.contains(m), "howto is missing {m:?}");
    }
    for doc in [REFERENCE, HOWTO, ROTATE_HOWTO] {
        assert!(doc.contains("profile switch --none"));
    }
}

#[test]
fn docs_quote_the_live_launch_note() {
    let note = super::live_launch_note("<name>");
    assert!(REFERENCE.contains(&note), "reference is missing {note:?}");
    let team = super::live_launch_note("team");
    assert!(HOWTO.contains(&team), "howto is missing {team:?}");
    assert!(
        ROTATE_HOWTO.contains(&team),
        "rotation howto is missing {team:?}"
    );
}

#[cfg(unix)]
#[test]
fn reference_quotes_the_login_refusal() {
    use super::login_check::{refusal_hint, refusal_message};
    let msg = format!("error: {}", refusal_message("<name>", "<holder>", "<path>"));
    let hint = format!("hint: {}", refusal_hint("<name>"));
    for line in [&msg, &hint] {
        assert!(
            REFERENCE.contains(line.as_str()),
            "reference is missing {line:?}"
        );
    }
    let howto_hint = format!("hint: {}", refusal_hint("rssourcelabs"));
    assert!(
        HOWTO.contains(&howto_hint),
        "howto is missing {howto_hint:?}"
    );
    let howto_msg = refusal_message("rssourcelabs", "profile 'ryan-ryansweet'", "");
    let head = howto_msg.split(" The new login").next().unwrap();
    assert!(
        HOWTO.contains(&format!("error: {head}")),
        "howto is missing {head:?}"
    );
}

#[cfg(unix)]
#[test]
fn docs_quote_the_live_profile_login_refusal() {
    use super::login_check::{LIVE_REFUSAL_HINT, live_refusal_message};
    let hint = format!("hint: {LIVE_REFUSAL_HINT}");
    for (doc, name) in [(REFERENCE, "<name>"), (HOWTO, "team")] {
        let msg = format!("error: {}", live_refusal_message(name));
        assert!(doc.contains(&msg), "docs are missing {msg:?}");
        assert!(doc.contains(&hint), "docs are missing {hint:?}");
    }
}

#[cfg(unix)]
#[test]
fn docs_quote_the_duplicate_login_warning() {
    use super::duplicates::{Holder, warning};
    let p = |s: &str| Holder::Profile(super::name::ProfileName::parse(s).unwrap());
    let template = warning(&p("aaa"), &p("bbb"))
        .replace("aaa", "<a>")
        .replace("bbb", "<b>");
    assert!(
        REFERENCE.contains(&template),
        "reference is missing {template:?}"
    );
    let example = format!(
        "note: {}",
        warning(&p("rssourcelabs"), &p("ryan-ryansweet"))
    );
    assert!(HOWTO.contains(&example), "howto is missing {example:?}");
}

#[test]
fn the_old_same_account_note_is_gone_from_the_docs() {
    for (name, doc) in [
        ("reference", REFERENCE),
        ("howto", HOWTO),
        ("rotation howto", ROTATE_HOWTO),
    ] {
        assert!(
            !doc.contains("their readings are not reliable"),
            "{name} still quotes the old note"
        );
    }
}

#[test]
fn docs_state_the_identity_rule() {
    for (name, doc) in [("reference", REFERENCE), ("howto", HOWTO)] {
        assert!(
            doc.contains("organizationUuid"),
            "{name} must name organizationUuid"
        );
    }
    assert!(REFERENCE.contains("| uuid on one side only | (any) | no |"));
}

/// The docs wrap lines; compare with runs of whitespace collapsed.
fn flat(doc: &str) -> String {
    doc.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Issue #1693: both docs say what a manual `/login` does, and quote the
/// hand-change recovery lines and guidance as the rotator writes them.
#[test]
fn docs_describe_hand_change_recovery() {
    let sentence = "A manual `/login` in `~/.claude` replaces whatever login was there; \
                    the profile that login belonged to is left with no login.";
    for (name, doc) in [("reference", REFERENCE), ("rotation how-to", ROTATE_HOWTO)] {
        let doc = flat(doc);
        assert!(
            doc.contains(sentence),
            "{name} is missing the /login sentence"
        );
        for stem in [
            "recovered: ~/.claude holds",
            "recovered: ~/.claude and",
            "has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login",
            "ALERT: the account in ~/.claude is not the one the ledger names (live=",
            "fix 1: amplihack profile login",
            "no profile has that account; no swap",
            "the next cycle then recovers",
            "none needed; the next cycle reads ~/.claude.json again",
            "~/.claude.json has had no account that can be read for 5 minutes or more (live=",
            "no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace",
            "fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if",
            "then holds an account, the next cycle goes on",
            "its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e",
            "it has no oauthAccount block",
            "ALERT: the account in ~/.claude.json cannot be read (live=",
            "no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace",
            "close the claude sessions that use",
            "wait; the next cycle retries",
            "wait for the auto-switch session to end; the next cycle retries",
            "'s copy by hand: mkdir -m 700",
            "none needed; the next cycle retries",
            "see the Recovery section of docs/howto/claude-profile-rotation.md",
            "Claude holds its refresh lock",
            "an earlier swap stopped half way",
            "~/.claude changed during recovery",
            ".rejected-logins/rotate-",
        ] {
            assert!(doc.contains(stem), "{name} is missing `{stem}`");
        }
        // #1702: one failed read of ~/.claude.json never advises a /login.
        assert!(
            !doc.contains("/login again"),
            "{name} still advises `/login again` for an unreadable ~/.claude.json"
        );
    }
    let reference = flat(REFERENCE);
    for line in [
        "would recover: <recovered line>",
        "move <what> by hand; its path cannot be printed safely",
        "alert: <line>",
        "warning: <line>",
        "lost_login",
        "it matches more than one profile (<M1>, <M2>);",
        "the account of profile <P1>, <P2> cannot be read, so the match is uncertain;",
        "the account in ~/.claude.json cannot be read;",
        "~/.claude has no login;",
        "recovery refused: <reason>;",
        "home_unreadable_since",
        "it does not exist",
        "it cannot be read (a symlink, not a regular file, over 32 MiB or not readable)",
        "it is not valid JSON (empty or cut short)",
        "it is not a JSON object",
        "its oauthAccount block has no usable accountUuid or emailAddress",
    ] {
        assert!(reference.contains(line), "reference is missing `{line}`");
    }
}

/// Issue #1818: the reference defines `session_limit`, the table label and
/// the `rotate status` line, with the contract wording for the JSON field,
/// the 0% rule and the stored-reading label (crusty round 1 on #1829).
#[test]
fn docs_describe_no_session_limit() {
    let flat = flat(REFERENCE);
    for needle in [
        "| `session_limit` | bool | `false` only when a reading showed no session window; `true` otherwise, including rows that could not be read.",
        "no active session window was",
        "shows `no session limit` in full",
        "| `no session window` | Printed only when the list is not empty",
        "The `5h` cell shows `no session window`",
        "Such a line at 0% means the reading has",
        "a value above 0% shows that a session window exists",
        "They therefore print `no session window`, not `no session limit`",
        "shows `s=0%`",
    ] {
        assert!(
            flat.contains(&needle.split_whitespace().collect::<Vec<_>>().join(" ")),
            "reference is missing {needle:?}"
        );
    }
    assert!(REFERENCE.contains("#### No session limit"));
}
