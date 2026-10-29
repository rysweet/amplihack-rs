//! #1728: classification of a failed inventory probe.
use super::probe_tests::*;
use super::*;

/// A real probe error produced by `native` against a fake.
pub(super) fn probe_error(dir: &Path, body: &str) -> anyhow::Error {
    let binary = probe_fake(dir, "probe", body);
    native(&binary, &["plugin", "list", "--json"], dir).unwrap_err()
}

// T4: a non-zero exit from a Codex that has plugins reports the exit status.
#[test]
fn classify_exit_failure_with_capability_names_exit_status() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_error(dir.path(), "printf 'stdout-canary'\nexit 1");
    let error = classify_probe_failure(probe, Ok(true));
    let rendered = format!("{error:#}");
    assert!(rendered.contains("supports native plugins"), "{rendered}");
    assert!(rendered.contains("exit status: 1"), "{rendered}");
    assert!(!rendered.contains("update Codex"), "{rendered}");
    assert!(
        !rendered.contains("requires native plugin support"),
        "{rendered}"
    );
    assert_no_child_output(&rendered);
}

// T5: invalid JSON is reported as a parse failure, with no input bytes quoted.
#[test]
fn classify_invalid_json_with_capability_names_parse_failure() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_error(
        dir.path(),
        "printf 'not-json stdout-canary\\033[31m'\nexit 0",
    );
    let error = classify_probe_failure(probe, Ok(true));
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("invalid native Codex plugin response"),
        "{rendered}"
    );
    assert!(rendered.contains("supports native plugins"), "{rendered}");
    assert!(!rendered.contains("update Codex"), "{rendered}");
    assert!(!rendered.contains("not-json"), "{rendered}");
    assert_no_child_output(&rendered);
}

// T6: a Codex that lacks the subcommand still gets the update advice.
#[test]
fn classify_missing_capability_keeps_update_advice() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_error(dir.path(), "exit 2");
    let error = classify_probe_failure(probe, Ok(false));
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("requires native plugin support; update Codex and retry"),
        "{rendered}"
    );
    assert_no_child_output(&rendered);
}

#[test]
fn classify_capability_error_reports_both_failures() {
    let probe = anyhow::anyhow!("native plugin command failed: exit status: 1");
    let capability = Err(anyhow::anyhow!("failed to spawn subprocess"));
    let error = classify_probe_failure(probe, capability);
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("native plugin inventory probe failed"),
        "{rendered}"
    );
    assert!(
        rendered.contains("capability check also failed"),
        "{rendered}"
    );
    assert!(
        rendered.contains("failed to spawn subprocess"),
        "{rendered}"
    );
    assert!(rendered.contains("exit status: 1"), "{rendered}");
    assert!(!rendered.contains("update Codex"), "{rendered}");
}
