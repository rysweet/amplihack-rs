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
