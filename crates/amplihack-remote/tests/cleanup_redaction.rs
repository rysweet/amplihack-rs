//! Outside-in test for issue #882: `Orchestrator::cleanup` must redact
//! sensitive Azure identifiers (subscription/tenant GUIDs, SAS query-parameter
//! values) out of raw `azlin` stderr before it is surfaced in a `RemoteError`.
//!
//! These tests exercise the *real* subprocess boundary through the public API:
//! a stub `azlin` binary is placed on `PATH`, emits secret-laden stderr, and
//! exits non-zero. We then assert that the error returned to a CLI consumer
//! carries `[REDACTED]` placeholders instead of the raw secrets, while keeping
//! enough non-sensitive context to stay diagnosable.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

use amplihack_remote::{Orchestrator, VM};
use tempfile::TempDir;

/// Serializes the `PATH`-sensitive critical section: the integration test
/// binary runs its tests on multiple threads, but they all mutate the shared
/// process `PATH`, so only one may drive `cleanup` at a time.
static PATH_GUARD: Mutex<()> = Mutex::new(());

/// Write an executable stub `azlin` into `dir` whose `kill` invocation prints
/// `stderr_body` to stderr and exits non-zero.
fn install_fake_azlin(dir: &TempDir, stderr_body: &str) {
    let script = format!("#!/bin/sh\ncat >&2 <<'AZLIN_EOF'\n{stderr_body}\nAZLIN_EOF\nexit 1\n");
    let path = dir.path().join("azlin");
    let mut f = fs::File::create(&path).expect("create fake azlin");
    f.write_all(script.as_bytes()).expect("write fake azlin");
    let mut perms = f.metadata().unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake azlin");
}

fn sample_vm() -> VM {
    VM {
        name: "amplihack-tester-000".into(),
        size: "Standard_D2s_v3".into(),
        region: "eastus".into(),
        created_at: None,
        tags: None,
    }
}

/// Run `Orchestrator::cleanup` with a fake `azlin` on `PATH` and return the
/// resulting error message. `force = false` so the failure is surfaced as an
/// explicit `Err` (no silent degradation).
fn cleanup_error_message(stderr_body: &str) -> String {
    let _guard = PATH_GUARD.lock().unwrap();

    let dir = TempDir::new().expect("tempdir");
    install_fake_azlin(&dir, stderr_body);

    let original_path = std::env::var("PATH").unwrap_or_default();
    let patched = format!("{}:{}", dir.path().display(), original_path);
    // SAFETY: access to PATH is serialized by PATH_GUARD for the duration of
    // the cleanup call, and restored before the guard is released.
    unsafe {
        std::env::set_var("PATH", &patched);
    }

    let orchestrator = Orchestrator::with_username("tester");
    let vm = sample_vm();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime.block_on(orchestrator.cleanup(&vm, false));

    unsafe {
        std::env::set_var("PATH", original_path);
    }

    match result {
        Ok(other) => panic!("expected cleanup to fail, got Ok({other})"),
        Err(e) => e.to_string(),
    }
}

/// Scenario 1 (simple): a bare subscription GUID in azlin stderr must not reach
/// the surfaced error; the placeholder and surrounding context must remain.
#[test]
fn cleanup_redacts_subscription_guid_in_error() {
    let secret_guid = "00000000-0000-0000-0000-000000000000";
    let msg = cleanup_error_message(&format!(
        "ERROR: The subscription '{secret_guid}' could not be found."
    ));

    assert!(
        msg.contains("VM cleanup failed:"),
        "unexpected error shape: {msg}"
    );
    assert!(
        !msg.contains(secret_guid),
        "subscription GUID leaked into error: {msg}"
    );
    assert!(
        msg.contains("[REDACTED]"),
        "no redaction placeholder: {msg}"
    );
    // Non-sensitive diagnostic context survives.
    assert!(msg.contains("subscription"), "context lost: {msg}");
    assert!(msg.contains("could not be found"), "context lost: {msg}");
}

/// Scenario 2 (complex / integration): a full SAS-signed blob URL *and* a
/// tenant GUID in the same stderr line. Every secret must be scrubbed while the
/// host/path and parameter *names* remain for debuggability.
#[test]
fn cleanup_redacts_sas_url_and_tenant_guid_in_error() {
    let tenant_guid = "11111111-1111-1111-1111-111111111111";
    let sas_sig = "EXAMPLE-fake-sas-signature-do-not-use";
    let stderr = format!(
        "ERROR: failed to delete blob \
         https://acct.blob.core.windows.net/c/b?sv=2021-08-06&sig={sas_sig}&se=2030-01-01 \
         for subscription {tenant_guid}"
    );

    let msg = cleanup_error_message(&stderr);

    // Secrets are gone.
    assert!(!msg.contains(sas_sig), "SAS sig leaked: {msg}");
    assert!(!msg.contains(tenant_guid), "tenant GUID leaked: {msg}");
    assert!(!msg.contains("2021-08-06"), "sv value leaked: {msg}");
    // Placeholder present and multiple redactions happened.
    assert!(
        msg.contains("[REDACTED]"),
        "no redaction placeholder: {msg}"
    );
    assert!(
        msg.matches("[REDACTED]").count() >= 2,
        "expected >=2 redactions, got: {msg}"
    );
    // Diagnostic context (host + parameter names) preserved.
    assert!(
        msg.contains("acct.blob.core.windows.net"),
        "host context lost: {msg}"
    );
    assert!(msg.contains("sig="), "sig param name lost: {msg}");
}
