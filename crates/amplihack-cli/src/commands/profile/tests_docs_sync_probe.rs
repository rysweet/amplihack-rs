//! Issue #1698: the probe-limit docs say what the code does. Kept apart from
//! `tests_docs_sync` so changes to the two do not collide.

const REFERENCE: &str = include_str!("../../../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../../../docs/howto/claude-profiles.md");
const ROTATE_HOWTO: &str = include_str!("../../../../../docs/howto/claude-profile-rotation.md");
const ENV_VARS: &str = include_str!("../../../../../docs/reference/amplihack-env-vars.md");

#[test]
fn probe_limit_variables_are_documented_where_they_apply() {
    use super::probe_limits::{PARALLEL_ENV, TIMEOUT_ENV};
    for (doc_name, doc) in [
        ("reference", REFERENCE),
        ("rotation how-to", ROTATE_HOWTO),
        ("profiles how-to", HOWTO),
    ] {
        for var in [TIMEOUT_ENV, PARALLEL_ENV] {
            assert!(doc.contains(var), "{doc_name} does not mention {var}");
        }
    }
    let row = ENV_VARS
        .lines()
        .find(|l| l.starts_with(&format!("| `{PARALLEL_ENV}`")))
        .expect("amplihack-env-vars.md has a row for the parallel variable");
    assert!(row.contains("`1` to `16`"), "{row}");
    assert!(
        row.contains("profile-command.md#probe-parallelism"),
        "{row}"
    );
}

#[test]
fn reference_gives_the_resolver_defaults_and_warnings() {
    use super::probe_limits::{DEFAULT_TIMEOUT_SECS, MAX_PARALLEL, MAX_TIMEOUT_SECS};
    let timeout_row = format!(
        "| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | `1` to `{MAX_TIMEOUT_SECS}` | `{DEFAULT_TIMEOUT_SECS}` |"
    );
    assert!(REFERENCE.contains(&timeout_row), "missing: {timeout_row}");
    let parallel_row = format!(
        "| `AMPLIHACK_PROFILE_USAGE_PARALLEL` | `1` to `{MAX_PARALLEL}` | the derived bound |"
    );
    assert!(REFERENCE.contains(&parallel_row), "missing: {parallel_row}");
    assert!(REFERENCE.contains("bound = clamp(floor(free / 8), 2, 4)"));
    // The documented warnings are the code's, word for word.
    let (_, w) = super::probe_limits::timeout_secs(None, Some(std::ffi::OsStr::new("soon")));
    assert!(REFERENCE.contains(&w.unwrap()), "timeout warning drifted");
    let (_, w) =
        super::probe_limits::parallel(Some(std::ffi::OsStr::new("lots")), 72, Some(194.0), 14);
    assert!(REFERENCE.contains(&w.unwrap()), "parallel warning drifted");
}

#[test]
fn reference_documented_bound_table_matches_the_code() {
    use super::probe_limits::parallel;
    for (cpus, load, want) in [(72, 0.0, 4), (72, 38.0, 4), (72, 194.0, 2), (8, 0.0, 2)] {
        assert_eq!(parallel(None, cpus, Some(load), 100).0, want);
        let free = (cpus as f64 - load).max(0.0) as u64;
        let line = REFERENCE
            .lines()
            .find(|l| {
                l.starts_with(&format!("| {cpus} CPUs")) && l.contains(&format!("| {free} |"))
            })
            .unwrap_or_else(|| panic!("no table row for {cpus} CPUs, free {free}"));
        assert!(line.ends_with(&format!("| `{want}` |")), "{line}");
    }
}

#[test]
fn docs_say_a_failed_probe_is_unknown_not_one_hundred_percent() {
    assert!(
        !REFERENCE.contains("counts as 100%"),
        "the reference still says a failed probe counts as 100%"
    );
    assert!(
        !REFERENCE.contains("at most 8"),
        "the old pool size is gone"
    );
    for (doc_name, doc) in [("reference", REFERENCE), ("rotation how-to", ROTATE_HOWTO)] {
        assert!(
            doc.contains(":failed"),
            "{doc_name} shows no `<name>:failed`"
        );
        assert!(doc.contains("unknown"), "{doc_name} does not say unknown");
    }
}

// ---------------------------------------------------------------------------
// The usage request (`get_usage`), its method variable and the rotator's
// warnings. Each check names the constant or source the docs quote, so the
// docs cannot describe a probe the code does not have.
// ---------------------------------------------------------------------------

#[test]
fn reference_quotes_the_request_line_and_argv_from_the_code() {
    use super::usage_request::{ARGS, REQUEST_ID, REQUEST_LINE};
    assert!(
        REFERENCE.contains(REQUEST_LINE),
        "the reference does not quote REQUEST_LINE"
    );
    let argv = format!("claude {}", ARGS.join(" "));
    assert!(REFERENCE.contains(&argv), "missing argv: {argv}");
    assert!(REFERENCE.contains(&format!("`{REQUEST_ID}`")));
    // C1: the answer nesting; C2: where the usage-credits flag is.
    for path in [
        "response.request_id",
        "response.subtype",
        "response.response",
        "rate_limits.extra_usage",
    ] {
        assert!(
            REFERENCE.contains(path),
            "the reference does not name {path}"
        );
    }
}

#[test]
fn reference_gives_the_method_row_and_the_resolvers_warning() {
    use super::probe_limits::{PROBE_ENV, ProbeMode, probe_mode};
    // Issue #1844: `direct` is a new value; unset is the ordered default.
    let row = format!(
        "| `{PROBE_ENV}` | `direct`, `auto`, `text` | unset: passive, direct, then `claude` |"
    );
    assert!(REFERENCE.contains(&row), "missing: {row}");
    let (mode, w) = probe_mode(Some(std::ffi::OsStr::new("Text")));
    assert_eq!(mode, ProbeMode::Ordered);
    let w = w.expect("\"Text\" warns");
    assert!(REFERENCE.contains(&w), "method warning drifted: {w}");
}

#[test]
fn the_method_variable_is_documented_where_it_applies() {
    use super::probe_limits::PROBE_ENV;
    let row = ENV_VARS
        .lines()
        .find(|l| l.starts_with(&format!("| `{PROBE_ENV}`")))
        .expect("amplihack-env-vars.md has a row for the method variable");
    assert!(row.contains("profile-command.md#probe-method"), "{row}");
    let compare = HOWTO
        .split("### Compare the two probe methods")
        .nth(1)
        .expect("the profiles how-to has the compare section");
    assert!(compare.contains(&format!("{PROBE_ENV}=text")));
    assert!(ROTATE_HOWTO.contains(PROBE_ENV));
}

#[test]
fn docs_say_overflow_profiles_are_probed_with_usage_only() {
    assert!(REFERENCE.contains("Profiles marked as overflow are probed with `/usage` only."));
    // The code: probe_profile, the one place every profile is probed from,
    // reads the overflow marks and uses Text for them.
    let src = include_str!("usage_cmd.rs");
    assert!(src.contains("overflow_names(store.root()).contains(name)"));
}

#[test]
fn reference_quotes_the_rotators_probe_warnings() {
    let src = include_str!("rotate/measure.rs");
    for (code, doc) in [
        (
            "warning: rotate: probe of {who} timed out",
            "warning: rotate: probe of <name> timed out",
        ),
        (
            "warning: rotate: probe of {who} failed: {why}",
            "warning: rotate: probe of <name> failed: <reason>",
        ),
    ] {
        assert!(src.contains(code), "measure.rs no longer prints {code:?}");
        assert!(
            REFERENCE.contains(doc),
            "the reference does not quote {doc:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1844: the direct-read variables, the `direct` mode and the fixed
// texts are documented as the code has them.
// ---------------------------------------------------------------------------

#[test]
fn the_new_variables_are_documented_where_they_apply() {
    use super::probe_limits::{DIRECT_PARALLEL_ENV, PASSIVE_MAX_AGE_ENV};
    use super::usage_direct::{API_BASE_ENV, TOKEN_URL_ENV};
    for var in [
        PASSIVE_MAX_AGE_ENV,
        DIRECT_PARALLEL_ENV,
        API_BASE_ENV,
        TOKEN_URL_ENV,
    ] {
        assert!(REFERENCE.contains(var), "reference does not mention {var}");
        assert!(
            ENV_VARS
                .lines()
                .any(|l| l.starts_with(&format!("| `{var}`"))),
            "amplihack-env-vars.md has no row for {var}"
        );
    }
    for var in [PASSIVE_MAX_AGE_ENV, DIRECT_PARALLEL_ENV] {
        assert!(
            ROTATE_HOWTO.contains(var),
            "rotation how-to does not mention {var}"
        );
    }
    let row = ENV_VARS
        .lines()
        .find(|l| l.starts_with("| `AMPLIHACK_PROFILE_USAGE_PROBE`"))
        .unwrap();
    assert!(row.contains("`direct`"), "{row}");
}

#[test]
fn reference_gives_the_new_resolver_rows_and_warnings() {
    use super::probe_limits::{direct_parallel, passive_max_age};
    assert!(
        REFERENCE
            .contains("| `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` | `0` to `3600` | `300` |")
    );
    assert!(
        REFERENCE.contains("| `AMPLIHACK_PROFILE_USAGE_DIRECT_PARALLEL` | `1` to `16` | `8` |")
    );
    let (_, w) = passive_max_age(Some(std::ffi::OsStr::new("ten")));
    assert!(
        REFERENCE.contains(&w.unwrap()),
        "passive age warning drifted"
    );
    let (_, w) = direct_parallel(Some(std::ffi::OsStr::new("lots")), 14);
    assert!(
        REFERENCE.contains(&w.unwrap()),
        "direct parallel warning drifted"
    );
}

#[test]
fn reference_quotes_the_direct_read_constants_and_warning() {
    use super::usage_direct::{
        BETA, DEFAULT_API_BASE, DEFAULT_TOKEN_URL, Endpoints, USAGE_PATH, USER_AGENT,
    };
    assert!(REFERENCE.contains(&format!("GET {DEFAULT_API_BASE}{USAGE_PATH}")));
    assert!(REFERENCE.contains(&format!("anthropic-beta: {BETA}")));
    assert!(REFERENCE.contains(&format!("User-Agent: {USER_AGENT}")));
    assert!(REFERENCE.contains(DEFAULT_TOKEN_URL));
    assert!(ENV_VARS.contains(DEFAULT_TOKEN_URL));
    let (_, w) = Endpoints::resolve(Some(std::ffi::OsStr::new("http://example.com")), None);
    assert!(
        REFERENCE.contains(&w[0]),
        "endpoint warning drifted: {}",
        w[0]
    );
}

#[test]
fn reference_lists_every_direct_failure_reason() {
    use super::usage_direct::reason;
    for r in [
        reason::NO_FILE,
        reason::REFUSED,
        reason::NO_TOKEN,
        reason::EXPIRED,
        reason::TIMEOUT,
        reason::TRANSPORT,
        reason::UNPARSEABLE,
        reason::CREDITS_ON,
        reason::REFRESH_BUSY,
        reason::REFRESH_REFUSED,
        reason::REFRESH_FAILED,
        reason::REFRESH_NOT_SAVED,
        reason::NO_REFRESH_TOKEN,
    ] {
        assert!(
            REFERENCE.contains(&format!("`{r}`")),
            "reason {r:?} undocumented"
        );
    }
}

#[test]
fn reference_quotes_the_fixed_row_texts() {
    use super::usage_cmd::ErrorReason;
    for text in [
        ErrorReason::NoClaude.to_string(),
        "direct read: overflow needs /usage".to_string(),
        "(direct: no claude output)".to_string(),
        "(passive: no claude output)".to_string(),
        "unknown, still running".to_string(),
        "note: profile \"spare\": direct read gave http 5xx; using claude".to_string(),
        super::usage_cmd::not_saved_warning("spare"),
        // Issue #2271: the row text and the JSON error of a login with no
        // token. These rows print no stderr warning (D7).
        ErrorReason::NotSignedIn("no oauth token").to_string(),
        format!(
            "\"error\": \"{}\"",
            ErrorReason::NotSignedIn("no credentials file").json_str()
        ),
        "no refresh token".to_string(),
    ] {
        assert!(
            REFERENCE.contains(&text),
            "the reference does not quote {text:?}"
        );
    }
}

/// The direct-read step timeouts and the refresh budget in the reference
/// are the code's.
#[test]
fn reference_gives_the_direct_read_step_limits_and_refresh_budget() {
    use super::usage_direct::{MAX_STEP, MIN_STEP, REFRESH_BUDGET};
    use std::time::Duration;
    assert_eq!(MAX_STEP, Duration::from_secs(10));
    assert_eq!(MIN_STEP, Duration::from_millis(500));
    assert_eq!(REFRESH_BUDGET, Duration::from_secs(3));
    let text = REFERENCE.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in [
        "`min(10 seconds, deadline - 1 second)`, at least 500 milliseconds",
        "A refresh `POST` is sent only when at least 3 seconds of the deadline are left",
        "`min(10 seconds, time left)`, so never below 3 seconds",
        "If less than 3 seconds of the [deadline](#probe-deadline) are left",
    ] {
        assert!(
            text.contains(phrase),
            "the reference does not say {phrase:?}"
        );
    }
    assert!(
        !text.contains("deadline - 1 second)`, at least 1 second"),
        "the reference still gives a 1 second floor"
    );
}

/// The command table describes `usage` as the direct read it is, not as a
/// `claude` run per profile.
#[test]
fn reference_command_table_describes_usage_as_a_direct_read() {
    let row = REFERENCE
        .lines()
        .find(|l| l.starts_with("| `usage [<name>...]` |"))
        .expect("the command table has a usage row");
    assert!(!row.contains("Asks `claude`"), "{row}");
    assert!(row.contains("OAuth usage endpoint"), "{row}");
}

/// Step 2 of "Token refresh" names the one comparison the code makes: the
/// access token, not the refresh token.
#[test]
fn reference_refresh_step_two_compares_only_the_access_token() {
    assert!(!REFERENCE.contains("If `accessToken` or `refreshToken`"));
    assert!(REFERENCE.contains("Only `accessToken` is\n   compared"));
}

/// Both reasons for a refresh that may have reached the token server are
/// documented as `refresh not saved`, including the late answer.
#[test]
fn reference_says_a_late_refresh_answer_is_not_saved() {
    assert!(REFERENCE.contains("the answer did not arrive before the deadline"));
}

// ---------------------------------------------------------------------------
// Issue #1797: the capacity summary, its history and its settings
// ---------------------------------------------------------------------------

#[test]
fn the_usable_threshold_variable_has_an_env_row_and_reference_rows() {
    use super::capacity_weight::ENV;
    let row = ENV_VARS
        .lines()
        .find(|l| l.starts_with(&format!("| `{ENV}`")))
        .expect("amplihack-env-vars.md has a row for the usable threshold");
    assert!(
        row.contains("`1` to `100`") && row.contains("`95`"),
        "{row}"
    );
    assert!(row.contains("--usable-session-pct"), "{row}");
    assert!(row.contains("profile-command.md#usable-now"), "{row}");
    assert!(
        REFERENCE.contains(&format!("| `{ENV}` | `1` to `100` | `95` |")),
        "reference settings table"
    );
}

#[test]
fn the_documented_threshold_warning_is_the_codes() {
    let (pct, w) =
        super::capacity_weight::usable_threshold(None, Some(std::ffi::OsStr::new("most")));
    assert_eq!(pct, 95);
    assert!(REFERENCE.contains(&w.unwrap()), "threshold warning drifted");
}

#[test]
fn the_documented_weight_warnings_are_the_codes() {
    let (_, w) = super::capacity_weight::parse(br#"{"team": -1}"#);
    let value = w[0].replace("\"team\"", "\"<name>\"");
    assert!(
        REFERENCE.contains(&value),
        "weight warning drifted: {value}"
    );
    let (_, w) = super::capacity_weight::parse(b"not json");
    let start = w[0].split('(').next().unwrap();
    assert!(
        REFERENCE.contains(&format!("{start}(<reason>); every weight is 1.0")),
        "file warning drifted: {}",
        w[0]
    );
}

#[test]
fn the_documented_history_warning_and_reasons_are_the_codes() {
    use super::usage_history::HistoryError;
    let busy = HistoryError::LockBusy.warning().unwrap();
    assert!(REFERENCE.contains(&busy), "{busy}");
    for e in [
        HistoryError::LockBusy,
        HistoryError::UnsafeFile,
        HistoryError::WriteFailed,
        HistoryError::TooLarge,
    ] {
        let w = e.warning().unwrap();
        let reason = w.split('(').nth(1).unwrap().trim_end_matches(')');
        assert!(
            REFERENCE.contains(&format!("`{reason}`")),
            "reason {reason}"
        );
    }
    let trim = HistoryError::TrimFailed.warning().unwrap();
    assert!(REFERENCE.contains(&format!("`{trim}`")), "{trim}");
    let refused = super::usage_history::READ_REFUSED;
    assert!(REFERENCE.contains(refused), "{refused}");
    let skipped = super::usage_history::skipped_warning(3);
    assert!(REFERENCE.contains(&skipped), "{skipped}");
}

#[test]
fn the_documented_history_line_is_what_the_writer_prints() {
    let line = REFERENCE
        .lines()
        .find(|l| l.starts_with("{\"at\":") && l.contains("\"w_reset\""))
        .expect("the reference shows a history line");
    let e = super::usage_history::parse_line(line, 1_793_376_900).expect("it reads back");
    assert_eq!(super::usage_history::to_line(&e), line);
}

#[test]
fn summary_flags_and_files_are_documented() {
    for needle in [
        "--summary-json",
        "--usable-session-pct",
        "capacity-weights.json",
        "usage-history.jsonl",
        "usage-history.lock",
        "capacity: no recorded cycle",
        "#### Capacity summary",
        "#### Summary JSON",
        "#### Usage history",
        "#### Capacity line",
    ] {
        assert!(REFERENCE.contains(needle), "reference lacks {needle}");
    }
    for needle in ["weekly headroom", "capacity:"] {
        assert!(
            ROTATE_HOWTO.contains(needle),
            "rotation how-to lacks {needle}"
        );
    }
}

#[test]
fn the_documented_summary_lines_use_the_codes_labels() {
    for label in [
        "weekly headroom      ",
        "usable now           ",
        "next session reset   ",
        "runway               ",
        "resets in next 24 h  ",
        "unknown              ",
    ] {
        assert_eq!(label.len(), 21);
        assert!(
            REFERENCE.lines().any(|l| l.starts_with(label)),
            "reference has no line starting {label:?}"
        );
    }
}

/// Issue #1856, crusty round 1: every spend value the code prints is a row
/// of the reference's spend-status table and is named in the JSON key's
/// description, and the opt-in `--spend` flag is documented in the flag
/// table, the env-var reference and the how-to.
#[test]
fn spend_values_and_the_spend_flag_are_documented() {
    use super::usage_source::Spend;
    let json_row = REFERENCE
        .lines()
        .find(|l| l.starts_with("| `spend` | string |"))
        .expect("the JSON table has a spend row");
    for v in [
        Spend::NotApplicable,
        Spend::Ok,
        Spend::LimitReached,
        Spend::Unknown,
        Spend::NotProbed,
    ] {
        let name = v.as_str();
        assert!(
            REFERENCE.contains(&format!("\n| `{name}` | ")),
            "the spend-status table lacks `{name}`"
        );
        assert!(json_row.contains(&format!("`\"{name}\"`")), "{json_row}");
    }
    assert!(
        REFERENCE.contains("\n| `--spend` | "),
        "flag table lacks --spend"
    );
    assert!(REFERENCE.contains("##### Spend probe"));
    assert!(
        !REFERENCE.contains("shared deadline"),
        "the 6 s deadline is gone"
    );
    // Crusty round 2: a `--spend` probe gets the full per-probe timeout,
    // so no doc may call it short.
    for doc in [REFERENCE, HOWTO, ROTATE_HOWTO, ENV_VARS] {
        for stale in ["short [spend probe", "short `/usage` text probe"] {
            assert!(!doc.contains(stale), "a doc still says {stale:?}");
        }
    }
    assert!(
        REFERENCE
            .contains("under the normal per-probe timeout (`--timeout-secs`, 120 s by default)")
    );
    assert!(ENV_VARS.contains("only with `--spend`"));
    assert!(HOWTO.contains("amplihack profile usage --spend"));
    assert!(HOWTO.contains("| `not_probed` |"));
}

// ---------------------------------------------------------------------------
// Issue #1970: the rate's window and weighting are stated in the help and
// the reference.
// ---------------------------------------------------------------------------

/// The long help of `profile usage`, rendered in this process.
fn usage_help() -> String {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let usage = cmd
        .find_subcommand_mut("profile")
        .and_then(|p| p.find_subcommand_mut("usage"))
        .expect("profile usage");
    let mut out = Vec::new();
    usage.write_long_help(&mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// `s` with each run of whitespace made one space, so wrapped help reads
/// as one line.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_rate_window_and_weighting_are_in_help_and_docs() {
    let help = flat(&usage_help());
    for want in ["6 h", "4.5 h", "weighted"] {
        assert!(help.contains(want), "help lacks {want:?}: {help}");
    }
    // The option's own entry: `--json`'s help names `--summary-json` too
    // (issue #2001: the Codex rows are only there), so the first mention
    // is not this option.
    let summary_json = help
        .split("--summary-json Print")
        .nth(1)
        .expect("the --summary-json entry in the help");
    assert!(
        summary_json.contains("6 h") && summary_json.contains("4.5 h"),
        "--summary-json does not state the window: {summary_json}"
    );
    // The start rule as `capacity_summary::can_start` applies it: any
    // `profile usage` entry, or a rotator entry holding every login read
    // now, which must itself be at least 4.5 h old.
    assert!(
        !help.contains("first entry that read every login")
            && !help.contains("entry in that window that read every login"),
        "the help describes a stricter start than the code: {help}"
    );
    assert!(help.contains("must itself be at least 4.5 h old"), "{help}");
    assert!(
        summary_json.contains("entry to be at least 4.5 h old"),
        "{summary_json}"
    );

    let reference = flat(REFERENCE);
    for want in [
        "6 h",
        "4.5 h",
        "weighted",
        "more than 10 points",
        "`0.0` or above",
    ] {
        assert!(reference.contains(want), "reference lacks {want:?}");
    }
    assert!(
        !reference.contains("Can be `0.0` or below"),
        "the rate is never negative now"
    );
    assert!(
        !reference.contains(
            "the week percentage fell. The earlier entry's percentage is then added back"
        ),
        "the old reset rule is gone"
    );
}

#[test]
fn the_documented_rate_constants_match_the_code() {
    use super::capacity_summary::{RATE_WINDOW, RESET_FALL};
    use super::usage_history::SAME_WEEK_SLACK;
    assert_eq!(RATE_WINDOW, 6 * 3600);
    let reference = flat(REFERENCE);
    assert!(reference.contains(&format!("more than {RESET_FALL} points")));
    assert!(reference.contains(&format!("fall of {RESET_FALL} points or less")));
    assert_eq!(SAME_WEEK_SLACK, 3600);
    assert!(reference.contains("within one hour of each other"));
    #[cfg(unix)]
    {
        use super::usage_history::STALE_LOOKBACK;
        assert_eq!(STALE_LOOKBACK, 256 * 1024);
        assert!(reference.contains("last 256 KiB"));
    }
}

#[test]
fn the_rotation_howto_says_small_dips_add_nothing() {
    let howto = flat(ROTATE_HOWTO);
    assert!(howto.contains("net rise"), "rotation how-to");
    assert!(howto.contains("10 points or less"), "rotation how-to");
}

/// Issue #2271 (D7): the reference no longer shows a stderr warning for a
/// not-signed-in row.
#[test]
fn reference_shows_no_warning_for_a_not_signed_in_row() {
    use super::usage_cmd::ErrorReason;
    let gone = format!(
        "warning: profile \"scratch\": {}",
        ErrorReason::NotSignedIn("no credentials file")
    );
    assert!(!REFERENCE.contains(&gone), "{gone}");
}
