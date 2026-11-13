//! Public command registration for #2555. The behavioural acceptance contracts
//! live in autodrive/tests_handoff.rs, alongside private production authority.
//! These command checks alone do not establish a successful handoff.

#[test]
fn production_cli_dispatches_policy_and_handoff_helpers() {
    crate::support::quiet_process::in_quiet_process(
        module_path!(),
        "production_cli_dispatches_policy_and_handoff_helpers",
        || {
            crate::support::quiet_process::child_set_var("TMPDIR", "/var/tmp");
            for operation in ["resolve-policy", "prepare-handoff", "complete-handoff"] {
                let output = crate::isolated_amplihack::isolated(
                    crate::support::amplihack_bin::amplihack_bin(),
                )
                .args(["orch", "helper", "autodrive", operation, "--help"])
                .output()
                .unwrap();
                assert!(
                    output.status.success(),
                    "production helper {operation} is not registered: {}",
                    String::from_utf8_lossy(&output.stderr),
                );
            }
        },
    );
}
