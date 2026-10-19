use super::fixtures::*;

#[cfg(unix)]
fn without_jq() -> Fixture {
    let f = Fixture::new();
    let tools = f.temp.path().join("only-bash");
    std::fs::create_dir(&tools).unwrap();
    for (name, path) in [
        ("bash", "/bin/bash"),
        ("tr", "/usr/bin/tr"),
        ("dirname", "/usr/bin/dirname"),
    ] {
        std::os::unix::fs::symlink(path, tools.join(name)).unwrap();
    }
    f.env("PATH", tools.display().to_string())
}

#[cfg(unix)]
#[test]
fn finalizer_collection_and_completion_missing_jq_retain_error_two() {
    for mode in ["collect", "complete"] {
        let r = without_jq().finalizer(mode);
        r.exit(2);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("requires jq"));
    }
}
#[cfg(unix)]
#[test]
fn finalizer_validation_missing_jq_emits_structured_failure_and_exit_one() {
    let r = without_jq().finalizer("validate");
    r.exit(1);
    assert_eq!(r.json()["terminal_state"], "FAILED_MISSING_TOOLING");
    assert_eq!(r.json()["terminal_success"], "false");
}
#[cfg(unix)]
#[test]
fn reporting_status_missing_jq_retains_error_two() {
    let r = without_jq().run(&body("workflow-finalize", "finalizer-step-status"));
    r.exit(2);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.contains("requires jq"));
}
