//! Contract tests for turn-failure error hygiene (issue #1092).
//!
//! When a turn's child process exits non-zero, the surfaced error must NOT
//! embed the child's full stdout/stderr. Instead it carries the exit status
//! plus a BOUNDED TAIL of the combined output; the full output is emitted only
//! at `tracing::debug!`. The tail budget is configurable via
//! `AMPLIHACK_TURN_ERROR_TAIL_BYTES` (default 2048).
//!
//! These drive the real [`CopilotTurnRunner`] over a tiny `sh -c` program that
//! writes to stdout/stderr and exits non-zero, so we exercise the actual
//! non-zero-exit branch (not a mock).
//!
//! The tests are plain `#[test]` functions (not `#[tokio::test]`): each builds
//! its own current-thread runtime and drives it via `block_on` INSIDE the env
//! guard, so the env var is set for the whole failing run and env-mutating
//! tests never race (a nested `#[tokio::test]` + `block_on` would panic).
//!
//! Run: `cargo test -p amplihack-turn --test turn_error_it`.

use std::sync::{Arc, Mutex, OnceLock};

use amplihack_turn::{CopilotTurnRunner, PreemptSlot, TurnRunner};

const TAIL_ENV: &str = "AMPLIHACK_TURN_ERROR_TAIL_BYTES";
const DEFAULT_TAIL_BYTES: usize = 2048;

/// Process-wide lock so env-mutating tests never race each other (the env is
/// global). Mirrors the workspace's env-test serialization pattern.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Set `AMPLIHACK_TURN_ERROR_TAIL_BYTES` for the duration of a closure, then
/// restore the prior value. Edition 2024 requires `unsafe` around
/// `set_var`/`remove_var`.
fn with_tail_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = env_lock().lock().expect("env lock not poisoned");
    let prev = std::env::var(TAIL_ENV).ok();
    unsafe {
        match value {
            Some(v) => std::env::set_var(TAIL_ENV, v),
            None => std::env::remove_var(TAIL_ENV),
        }
    }
    let out = f();
    unsafe {
        match prev {
            Some(v) => std::env::set_var(TAIL_ENV, v),
            None => std::env::remove_var(TAIL_ENV),
        }
    }
    out
}

fn new_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
}

/// Run a `sh -c` script and return the `Err` message from a non-zero exit.
/// Builds and drives its own runtime so it can be called from a sync `#[test]`
/// inside the env guard.
fn run_failing(script: &str) -> String {
    let preempt: PreemptSlot = Arc::new(Mutex::new(None));
    // `sh` is the "program"; the argv is a `-c` script. The real production
    // program is `copilot`, but the runner just spawns whatever program it was
    // built with, so this exercises the identical non-zero-exit code path.
    let runner = CopilotTurnRunner::new("sh", preempt);
    let argv = vec!["-c".to_string(), script.to_string()];
    let err = new_runtime()
        .block_on(runner.run_argv(argv))
        .expect_err("non-zero exit must surface as an error");
    err.to_string()
}

/// The tail portion follows the documented marker; extract it so tests can
/// assert on the bounded segment specifically.
fn tail_of(msg: &str) -> &str {
    let marker = "bytes of output: ";
    let idx = msg
        .find(marker)
        .expect("error message must contain the tail marker");
    &msg[idx + marker.len()..]
}

#[test]
fn prefix_is_preserved() {
    let msg = with_tail_env(None, || run_failing("echo hi; exit 1"));
    assert!(
        msg.contains("copilot turn failed"),
        "prefix must be preserved for downstream consumers; got: {msg}"
    );
    assert!(
        msg.contains("copilot turn failed (exit status:"),
        "the ({{status}}) content must be preserved; got: {msg}"
    );
}

#[test]
fn multibyte_tail_does_not_panic_and_is_bounded() {
    // Emit far more than the budget, ending in 4-byte characters so a naive
    // byte slice would split a char and panic.
    let budget = 16usize;
    let script = "yes '😀' | head -n 2000 | tr -d '\\n'; exit 1";
    let msg = with_tail_env(Some(&budget.to_string()), || run_failing(script));
    let tail = tail_of(&msg);
    assert!(
        tail.len() <= budget,
        "tail ({} bytes) must be <= budget ({budget}); got tail: {tail:?}",
        tail.len()
    );
    // A valid &str here already proves no char boundary was split (would panic).
    assert!(
        !tail.is_empty(),
        "tail should contain some trailing content"
    );
}

#[test]
fn bound_is_honored_for_large_ascii_output() {
    let budget = 32usize;
    let script = "printf '%0.sA' $(seq 1 5000); exit 3";
    let msg = with_tail_env(Some(&budget.to_string()), || run_failing(script));
    let tail = tail_of(&msg);
    assert!(
        tail.len() <= budget,
        "tail ({} bytes) must not exceed configured budget ({budget})",
        tail.len()
    );
}

#[test]
fn env_override_shrinks_tail() {
    let script = "printf '%0.sB' $(seq 1 5000); exit 1";
    let small = with_tail_env(Some("8"), || run_failing(script));
    let large = with_tail_env(Some("64"), || run_failing(script));
    assert!(
        tail_of(&small).len() < tail_of(&large).len(),
        "a smaller AMPLIHACK_TURN_ERROR_TAIL_BYTES must produce a shorter tail"
    );
    assert!(tail_of(&small).len() <= 8);
    assert!(tail_of(&large).len() <= 64);
}

#[test]
fn unparseable_env_falls_back_to_default() {
    // 5000 ASCII bytes exceeds the 2048 default, so the tail is capped at it.
    let script = "printf '%0.sC' $(seq 1 5000); exit 1";
    let msg = with_tail_env(Some("not-a-number"), || run_failing(script));
    let tail = tail_of(&msg);
    assert!(
        tail.len() <= DEFAULT_TAIL_BYTES,
        "unparseable env must fall back to the default budget ({DEFAULT_TAIL_BYTES}); tail was {} bytes",
        tail.len()
    );
    // With 5000 ASCII bytes of output and a 2048 default, the tail is trimmed
    // to exactly the default (ASCII => no boundary snapping needed).
    assert_eq!(
        tail.len(),
        DEFAULT_TAIL_BYTES,
        "5000 ASCII bytes must be trimmed to the default tail budget"
    );
}

#[test]
fn full_output_only_at_debug_and_not_in_error_string() {
    use std::io::Write;
    use tracing::Level;
    use tracing_subscriber::fmt::MakeWriter;

    // A MakeWriter that appends everything into a shared buffer, so we can
    // inspect what was logged at DEBUG level.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_writer(BufWriter(buf.clone()))
        .without_time()
        .finish();

    // A unique marker at the START of the output that must NOT appear in the
    // returned error (it's beyond the small tail) but MUST appear in the debug
    // log (which carries the full combined output).
    let marker = "SECRET_TOKEN_ABCDEF";
    let budget = 8usize;
    let script = format!("printf '{marker}'; printf '%0.sZ' $(seq 1 4000); exit 1");

    let msg = with_tail_env(Some(&budget.to_string()), || {
        tracing::subscriber::with_default(subscriber, || run_failing(&script))
    });

    assert!(
        !msg.contains(marker),
        "full output must NOT leak into the surfaced error string; got: {msg}"
    );

    let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains(marker),
        "full combined output must be emitted at DEBUG level; log was: {logged}"
    );
}
