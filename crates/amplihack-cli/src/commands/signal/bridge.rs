//! `amplihack signal bridge <topic>` — runtime orchestration (gated I/O shell).
//!
//! This drives a Copilot session from a fresh, operator-only Signal group. All
//! decision logic lives in the reusable, unit-tested cores in
//! `amplihack_signal::bridge`; this file performs the effects: config/link
//! check, loopback validation, resume probe, daemon connect, group create, the
//! announcement, the first turn, and the subscriber loop.
//!
//! Security posture (see `docs/SIGNAL_BRIDGE.md`): least-privilege tools by
//! default, **fail-closed** outbound membership verification before every post,
//! loopback-only daemon unless an explicit opt-in, an audit log of every
//! accepted prompt, and outbound secret redaction before chunking. No silent
//! fallbacks — every fatal condition maps to a stable [`BridgeError`] exit code.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use amplihack_signal::bridge::allowlist::ToolAllowlist;
use amplihack_signal::bridge::control::{Control, parse_control};
use amplihack_signal::bridge::membership::{Membership, classify};
use amplihack_signal::bridge::outbound::redact_and_chunk;
use amplihack_signal::bridge::turn::{CopilotTurnRunner, SerialTurnDriver};
use amplihack_signal::bridge::{BridgeError, connect_daemon, validate_endpoint};
use amplihack_signal::config::SignalConfig;
use amplihack_signal::gating::Gate;
use amplihack_signal::session_channel::Inbox;
use amplihack_signal::transport::{GroupId, SignalTransport};

use crate::SignalBridgeArgs;

/// Default reconnect attempts before a clean daemon-down shutdown.
const DEFAULT_RETRY_BUDGET: u32 = 10;

/// The `copilot` binary the bridge drives (turn-based `--session-id` resume).
const COPILOT_BIN: &str = "copilot";

/// Entry point for `amplihack signal bridge`. Blocks on an async runtime and
/// returns the stable [`BridgeError`] taxonomy on failure.
pub fn run_bridge(args: SignalBridgeArgs) -> Result<(), BridgeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            eprintln!("error: failed to start async runtime: {e}");
            BridgeError::NotLinked
        })?;
    runtime.block_on(run_bridge_async(args))
}

/// Best-effort system hostname for the group-name `<host>` token.
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "host".to_string())
}

/// The current tmux session name, if the bridge is running inside tmux.
fn tmux_session() -> Option<String> {
    std::env::var_os("TMUX")?;
    let out = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Probe that the installed `copilot` accepts `--session-id` resume. Without it,
/// turn continuity cannot be guaranteed, so the bridge refuses to start.
fn probe_copilot_resume() -> Result<(), BridgeError> {
    let out = std::process::Command::new(COPILOT_BIN)
        .arg("--help")
        .output()
        .map_err(|_| BridgeError::ResumeProbeFailed)?;
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if help.contains("--session-id") {
        Ok(())
    } else {
        Err(BridgeError::ResumeProbeFailed)
    }
}

/// The expected operator-only member set: the allowlisted senders plus the
/// account amplihack itself sends as.
fn expected_members(cfg: &SignalConfig) -> Vec<String> {
    let mut set = cfg.allowlist.clone();
    if !set.contains(&cfg.account) {
        set.push(cfg.account.clone());
    }
    set
}

/// Verify group membership, then relay `body` (redacted + chunked) — FAIL
/// CLOSED. If membership cannot be positively verified, alert the local
/// terminal and skip the relay (never assume "probably fine").
async fn verify_and_post(
    transport: &mut SignalTransport,
    group_id: &GroupId,
    expected: &[String],
    gate: &mut Gate,
    body: &str,
) {
    let actual = transport.group_members(group_id).await.ok();
    let membership = classify(expected, actual.as_deref());
    if let Membership::Unverified(reason) = &membership {
        eprintln!(
            "signal bridge: WITHHOLDING outbound relay — group membership unverified: {reason}"
        );
        return;
    }
    for chunk in redact_and_chunk(body) {
        if let Err(e) = transport.send_group(group_id, &chunk).await {
            eprintln!("signal bridge: failed to post to group: {e}");
            return;
        }
        gate.record_outbound(&chunk);
    }
}

/// Audit-log an accepted prompt (redacted) to the local terminal.
fn audit_accepted(session_id: &str, sender: &str, device: Option<u32>, prompt: &str) {
    let redacted = amplihack_signal::bridge::outbound::redact_for_relay(prompt);
    let preview: String = redacted.chars().take(120).collect();
    tracing::info!(
        session_id,
        sender,
        device = device.unwrap_or(0),
        "signal bridge accepted prompt: {preview}"
    );
    eprintln!("signal bridge: accepted prompt from {sender} (device {device:?}): {preview}");
}

async fn run_bridge_async(args: SignalBridgeArgs) -> Result<(), BridgeError> {
    // 1. Load config (also our linked/configured check). A missing/invalid
    //    config means the host is not onboarded → guide the operator.
    let cfg = SignalConfig::load().map_err(|e| {
        eprintln!(
            "error: signal is not configured on this host ({e}).\n\
             Run `amplihack signal setup` to link a device and start the daemon."
        );
        BridgeError::NotLinked
    })?;

    // 2. Loopback safety (fail closed unless explicit opt-in).
    validate_endpoint(&cfg.endpoint, args.unsafe_remote_endpoint).inspect_err(|_| {
        eprintln!(
            "error: signal-cli endpoint {} is not loopback. Pass --unsafe-remote-endpoint to \
             override (never across an untrusted network).",
            cfg.endpoint
        );
    })?;

    // 3. Copilot resume probe (turn continuity precondition).
    probe_copilot_resume().inspect_err(|_| {
        eprintln!(
            "error: the installed `copilot` did not accept `--session-id` resume; turn \
             continuity cannot be guaranteed."
        );
    })?;

    // 4. Connect to the daemon with a bounded retry budget.
    let retry_budget = args.retry_budget.unwrap_or(DEFAULT_RETRY_BUDGET);
    let mut transport = connect_daemon(&cfg.endpoint, retry_budget)
        .await
        .inspect_err(|_| {
            eprintln!(
                "error: signal-cli daemon at {} was unreachable after {retry_budget} attempts; \
                 shutting down cleanly.",
                cfg.endpoint
            );
        })?;

    // 5. Derive the group name and create a fresh operator-only group.
    let host = args.host.clone().unwrap_or_else(hostname);
    let tmux = tmux_session();
    let group_name = args.group_name.clone().unwrap_or_else(|| {
        amplihack_signal::bridge::naming::group_name(&host, tmux.as_deref(), &args.topic)
    });
    let group_id = transport.create_group(&group_name).await.map_err(|e| {
        eprintln!("error: failed to create Signal group '{group_name}': {e}");
        BridgeError::GroupCreateFailed
    })?;
    eprintln!(
        "signal bridge: created group '{group_name}' ({})",
        group_id.as_str()
    );

    // 6. Fresh pinned session id + effective allowlist.
    let session_id = uuid::Uuid::new_v4().to_string();
    let allowlist = ToolAllowlist::from_flags(&args.allow_tool, args.dangerous_all_tools);
    let expected = expected_members(&cfg);
    let mut gate = Gate::new(&cfg, group_id.as_str());

    // Shared child-PID slot so a control `stop`/`kill` can pre-empt an in-flight
    // turn even mid-execution.
    let current_pid = Arc::new(Mutex::new(None::<u32>));
    let driver = Arc::new(SerialTurnDriver::new(
        CopilotTurnRunner::new(COPILOT_BIN, current_pid.clone()),
        &session_id,
        allowlist.clone(),
    ));

    // 7. Announce topic, blast radius, and control phrases.
    let announcement = format!(
        "amplihack signal bridge started.\n\
         topic: {}\n\
         session: {}\n\
         tools ({}): {}\n\
         controls: `status`, `stop`, `kill` (exact word).",
        args.topic,
        session_id,
        if allowlist.is_dangerous() {
            "DANGEROUS"
        } else {
            "least-privilege"
        },
        allowlist.describe(),
    );
    verify_and_post(
        &mut transport,
        &group_id,
        &expected,
        &mut gate,
        &announcement,
    )
    .await;

    // Bounded turn queue (operator-configurable), mirroring the session inbox.
    let capacity = args.inbox_capacity.unwrap_or_else(Inbox::default_capacity);
    let mut queue: VecDeque<String> = VecDeque::new();

    // 8. First turn: the topic itself is the opening prompt.
    let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel::<std::io::Result<String>>();
    let mut turn_in_flight = spawn_turn(&driver, &turn_tx, args.topic.clone());

    // 9. Subscriber loop.
    loop {
        tokio::select! {
            biased;
            // Post completed turn output promptly, then start the next queued turn.
            Some(result) = turn_rx.recv() => {
                turn_in_flight = false;
                match result {
                    Ok(body) if !body.trim().is_empty() => {
                        verify_and_post(&mut transport, &group_id, &expected, &mut gate, &body).await;
                    }
                    Ok(_) => {
                        verify_and_post(&mut transport, &group_id, &expected, &mut gate,
                            "(turn produced no output)").await;
                    }
                    Err(e) => {
                        // Surface the failure but keep the bridge alive; the next
                        // turn resumes the SAME session (context preserved).
                        verify_and_post(&mut transport, &group_id, &expected, &mut gate,
                            &format!("turn failed: {e}")).await;
                    }
                }
                if !turn_in_flight {
                    let next = queue.pop_front();
                    if let Some(next) = next {
                        turn_in_flight = spawn_turn(&driver, &turn_tx, next);
                    }
                }
            }
            // Inbound Signal frames.
            recv = transport.receive() => {
                let env = match recv {
                    Ok(Some(env)) => env,
                    Ok(None) => {
                        eprintln!("signal bridge: receive stream closed; shutting down.");
                        break;
                    }
                    Err(e) => {
                        eprintln!("signal bridge: receive error: {e}");
                        continue;
                    }
                };
                let sender = env.source.clone().unwrap_or_default();
                let device = env.source_device;
                let Some(body) = gate.evaluate(&env) else { continue };
                if body.is_empty() {
                    continue;
                }
                // Control phrases are parsed BEFORE a body becomes a prompt.
                match parse_control(&body) {
                    Control::Status => {
                        let status = format!(
                            "status: session {} | {} | queue depth {} | membership: verifying before each post",
                            session_id,
                            if turn_in_flight { "turn in flight" } else { "idle" },
                            queue.len(),
                        );
                        verify_and_post(&mut transport, &group_id, &expected, &mut gate, &status).await;
                    }
                    Control::Stop => {
                        eprintln!("signal bridge: stop received; terminating child and closing group.");
                        preempt_child(&current_pid);
                        let _ = transport.quit_group(&group_id).await;
                        break;
                    }
                    Control::Prompt(prompt) => {
                        audit_accepted(&session_id, &sender, device, &prompt);
                        if turn_in_flight {
                            queue.push_back(prompt);
                            while queue.len() > capacity {
                                queue.pop_front();
                                eprintln!(
                                    "signal bridge: turn queue at capacity ({capacity}); dropped oldest pending prompt."
                                );
                            }
                        } else {
                            turn_in_flight = spawn_turn(&driver, &turn_tx, prompt);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Spawn one serialized turn on the driver, delivering its captured stdout (or
/// error) over `tx`. Returns `true` (a turn is now in flight).
fn spawn_turn(
    driver: &Arc<SerialTurnDriver<CopilotTurnRunner>>,
    tx: &tokio::sync::mpsc::UnboundedSender<std::io::Result<String>>,
    prompt: String,
) -> bool {
    let driver = driver.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let output = driver.run_turn(&prompt).await;
        let _ = tx.send(output);
    });
    true
}

/// Terminate the currently-tracked child `copilot` PID, if any, pre-empting an
/// in-flight turn. Best-effort; uses `SIGKILL` via the `kill` syscall.
///
/// # PID-reuse (TOCTOU) window and its mitigation
///
/// This reads a raw PID from a shared `Arc<Mutex<Option<u32>>>` slot and passes
/// it to `libc::kill`, so there is an unavoidable time-of-check/time-of-use gap:
/// between the moment the turn task reaps the child (`wait_with_output` in
/// `CopilotTurnRunner::run_argv`, which lets the OS free the PID) and the moment
/// that same task clears the slot back to `None`, the OS could recycle the PID
/// for an unrelated process. A `preempt_child` firing inside that narrow window
/// would `SIGKILL` the wrong process.
///
/// The window is bounded and best-effort by design:
///   * the runner clears the slot to `None` immediately after the child is
///     reaped (`turn.rs`), keeping the window to a few instructions rather than
///     the lifetime of a turn;
///   * both the publisher (runner) and this consumer take the same mutex, so a
///     read here never observes a torn/partial PID.
///
/// A fully race-free fix would store the owned `tokio::process::Child` handle
/// and call `Child::kill()` (which the runtime binds to the specific child,
/// immune to PID reuse), but that requires restructuring turn ownership so the
/// spawning task and the pre-empting task can share the `Child`. That is out of
/// scope for this hardening pass; the slot-clear-to-`None`-on-exit mitigation
/// above is retained instead.
fn preempt_child(current_pid: &Arc<Mutex<Option<u32>>>) {
    let pid = *current_pid.lock().expect("pid mutex not poisoned");
    if let Some(pid) = pid {
        // SAFETY: `kill(2)` with a real PID and SIGKILL (9). A stale PID simply
        // returns ESRCH, which we ignore.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
