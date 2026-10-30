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
    let row = format!("| `{PROBE_ENV}` | `auto`, `text` | `auto` |");
    assert!(REFERENCE.contains(&row), "missing: {row}");
    let (mode, w) = probe_mode(Some(std::ffi::OsStr::new("Text")));
    assert_eq!(mode, ProbeMode::Auto);
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
