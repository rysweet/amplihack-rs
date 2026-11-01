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
