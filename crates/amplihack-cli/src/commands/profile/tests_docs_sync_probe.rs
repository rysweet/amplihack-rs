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
