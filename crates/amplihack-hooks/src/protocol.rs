//! Hook protocol: run_hook(), panic handler, SIGPIPE handling.
//!
//! Every hook binary uses `run_hook()` as the entry point. It handles:
//! - JSON stdin reading
//! - Panic recovery (catch_unwind → `b"{}"`)
//! - SIGPIPE handling (graceful pipe closure)
//! - Telemetry (stderr JSON line per invocation)

use amplihack_types::HookInput;
use serde::Serialize;
use std::io::{self, Read, Write};
use std::panic::AssertUnwindSafe;
use std::time::Instant;

/// Failure policy for a hook.
///
/// Determines what happens when a hook encounters an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    /// On error, output `{}` and exit 0 (don't break the user).
    Open,
    /// On error, output error JSON and exit non-zero (reject on error).
    Closed,
}

/// Trait that all hooks implement.
pub trait Hook {
    /// Process the hook input and return the output as a JSON value.
    fn process(&self, input: HookInput) -> anyhow::Result<serde_json::Value>;

    /// The hook name (for telemetry and logging).
    fn name(&self) -> &'static str;

    /// The failure policy for this hook.
    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }
}

/// Run a hook with full protocol handling.
///
/// This is the main entry point for hook binaries. It:
/// 1. Suppresses panic output to stderr
/// 2. Reads JSON from stdin
/// 3. Calls `hook.process(input)`
/// 4. Writes JSON to stdout
/// 5. On any failure (including panics): outputs `{}` (fail-open) or error (fail-closed)
/// 6. Emits telemetry to stderr
pub fn run_hook<H: Hook>(hook: H) {
    // Suppress default panic output.
    std::panic::set_hook(Box::new(|_| {}));

    let start = Instant::now();
    let hook_name = hook.name();
    let policy = hook.failure_policy();

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| -> anyhow::Result<()> {
        let input_json = read_stdin()?;

        let input: HookInput = serde_json::from_str(&input_json).unwrap_or(HookInput::Unknown);

        // Unknown events get versioned empty output (graceful forward-compat).
        if matches!(input, HookInput::Unknown) {
            write_stdout(br#"{"version":1}"#)?;
            return Ok(());
        }

        let output = hook.process(input)?;
        let output_bytes = serde_json::to_vec(&output)?;
        write_stdout(&output_bytes)?;
        Ok(())
    }));

    let duration = start.elapsed();

    match result {
        Ok(Ok(())) => {
            emit_telemetry(hook_name, duration, "ok", None);
        }
        Ok(Err(e)) => {
            emit_telemetry(hook_name, duration, "error", Some(&e.to_string()));
            match policy {
                FailurePolicy::Open => {
                    if write_stdout(b"{}").is_err() {
                        std::process::exit(3);
                    }
                }
                FailurePolicy::Closed => {
                    let error_output = serde_json::json!({
                        "error": e.to_string()
                    });
                    let _ = write_stdout(
                        serde_json::to_string(&error_output)
                            .unwrap_or_else(|_| "{}".to_string())
                            .as_bytes(),
                    );
                    std::process::exit(2);
                }
            }
        }
        Err(_panic) => {
            emit_telemetry(hook_name, duration, "panic", Some("hook panicked"));
            // Intentional: on panic, write best-effort empty JSON response.
            // If stdout is broken too, there's nothing more we can do.
            let _ = io::stdout().write_all(b"{}");
            let _ = io::stdout().flush();
        }
    }
}

/// Read all of stdin as a string.
fn read_stdin() -> anyhow::Result<String> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    Ok(input)
}

/// Write bytes to stdout, handling SIGPIPE gracefully.
fn write_stdout(data: &[u8]) -> anyhow::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    match handle.write_all(data) {
        Ok(()) => {}
        Err(e) if is_broken_pipe(&e) => return Ok(()), // graceful pipe closure
        Err(e) => return Err(e.into()),
    }
    match handle.write_all(b"\n") {
        Ok(()) => {}
        Err(e) if is_broken_pipe(&e) => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    match handle.flush() {
        Ok(()) => {}
        Err(e) if is_broken_pipe(&e) => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Check if an error is a broken pipe (EPIPE / SIGPIPE).
fn is_broken_pipe(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::BrokenPipe || e.raw_os_error() == Some(32) // EPIPE
}

/// Emit telemetry as a single JSON line to stderr.
#[derive(Serialize)]
struct TelemetryEvent<'a> {
    hook: &'a str,
    duration_us: u128,
    result: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn emit_telemetry(hook: &str, duration: std::time::Duration, result: &str, error: Option<&str>) {
    let event = TelemetryEvent {
        hook,
        duration_us: duration.as_micros(),
        result,
        error,
    };
    if let Ok(json) = serde_json::to_string(&event) {
        // Intentional: telemetry is best-effort; stderr failures are not fatal.
        let _ = io::stderr().write_all(json.as_bytes());
        let _ = io::stderr().write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHook;
    impl Hook for TestHook {
        fn process(&self, _input: HookInput) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        fn name(&self) -> &'static str {
            "test"
        }
    }

    #[test]
    fn broken_pipe_detection() {
        let e = io::Error::from_raw_os_error(32);
        assert!(is_broken_pipe(&e));
    }

    #[test]
    fn test_hook_default_policy() {
        let hook = TestHook;
        assert_eq!(hook.failure_policy(), FailurePolicy::Open);
    }
}
