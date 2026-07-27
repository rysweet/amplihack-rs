//! `amplihack signal chat <topic>` — runtime orchestration (gated I/O shell).
//!
//! This drives a Copilot session from a fresh, operator-only Signal group. All
//! decision logic lives in the reusable, unit-tested cores in
//! `amplihack_signal::chat`; this file performs the effects: config/link
//! check, loopback validation, resume probe, daemon connect, group create, the
//! announcement, the first turn, and the subscriber loop.
//!
//! Security posture (see `docs/SIGNAL_CHAT.md`): least-privilege tools by
//! default, **fail-closed** outbound membership verification before every post,
//! loopback-only daemon unless an explicit opt-in, an audit log of every
//! accepted prompt, and outbound secret redaction before chunking. No silent
//! fallbacks — every fatal condition maps to a stable [`ChatError`] exit code.

use std::sync::{Arc, Mutex};

use amplihack_signal::chat::allowlist::ToolAllowlist;
use amplihack_signal::chat::membership::Membership;
use amplihack_signal::chat::outbound::redact_and_chunk;
use amplihack_signal::chat::turn::{
    AgentSession, CopilotTurnRunner, PreemptSlot, SerialTurnDriver, TurnError, TurnOutput,
    TurnResult, run_session_loop,
};
use amplihack_signal::chat::{ChatError, connect_daemon, validate_endpoint, verified_send};
use amplihack_signal::config::SignalConfig;
use amplihack_signal::gating::Gate;
use amplihack_signal::signal_channel::SignalChannel;
use amplihack_signal::transport::{GroupId, SignalTransport};

use crate::SignalChatArgs;

/// Default reconnect attempts before a clean daemon-down shutdown.
const DEFAULT_RETRY_BUDGET: u32 = 10;

/// The `copilot` binary the chat drives (turn-based `--session-id` resume).
const COPILOT_BIN: &str = "copilot";

/// Entry point for `amplihack signal chat`. Blocks on an async runtime and
/// returns the stable [`ChatError`] taxonomy on failure.
pub fn run_chat(args: SignalChatArgs) -> Result<(), ChatError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            eprintln!("error: failed to start async runtime: {e}");
            ChatError::NotLinked
        })?;
    runtime.block_on(run_chat_async(args))
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

/// The current tmux session name, if the chat is running inside tmux.
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
/// turn continuity cannot be guaranteed, so the chat refuses to start.
fn probe_copilot_resume() -> Result<(), ChatError> {
    let out = std::process::Command::new(COPILOT_BIN)
        .arg("--help")
        .output()
        .map_err(|_| ChatError::ResumeProbeFailed)?;
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if help.contains("--session-id") {
        Ok(())
    } else {
        Err(ChatError::ResumeProbeFailed)
    }
}

/// Verify group membership, then relay `body` (redacted + chunked) — FAIL
/// CLOSED, re-verifying **before every post**.
///
/// The security posture promises membership is checked before *each* outbound
/// message, not once per body: an operator-only group whose membership changes
/// mid-relay (an unexpected member added between chunks) must not receive any
/// further chunk. So this re-queries and re-classifies membership immediately
/// before each `send_group` chunk. On any verification failure — the first
/// chunk or a later one — it alerts the local terminal and stops sending the
/// remaining chunks (the withheld relay is surfaced, never silently dropped).
pub async fn verify_and_post(
    transport: &mut SignalTransport,
    group_id: &GroupId,
    expected: &[String],
    gate: &mut Gate,
    body: &str,
) {
    for chunk in redact_and_chunk(body) {
        match verified_send(transport, group_id, expected, &chunk).await {
            Ok(Membership::Verified) => gate.record_outbound(&chunk),
            Ok(Membership::Unverified(reason)) => {
                eprintln!(
                    "signal chat: WITHHOLDING outbound relay — group membership unverified before post: {reason}"
                );
                return;
            }
            Err(e) => {
                eprintln!("signal chat: failed to post to group: {e}");
                return;
            }
        }
    }
}

async fn run_chat_async(args: SignalChatArgs) -> Result<(), ChatError> {
    // 1. Load config (also our linked/configured check). A missing/invalid
    //    config means the host is not onboarded → guide the operator.
    let cfg = SignalConfig::load().map_err(|e| {
        eprintln!(
            "error: signal is not configured on this host ({e}).\n\
             Run `amplihack signal setup` to link a device and start the daemon."
        );
        ChatError::NotLinked
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
        amplihack_signal::chat::naming::group_name(&host, tmux.as_deref(), &args.topic)
    });
    let group_id = transport.create_group(&group_name).await.map_err(|e| {
        eprintln!("error: failed to create Signal group '{group_name}': {e}");
        ChatError::GroupCreateFailed
    })?;
    eprintln!(
        "signal chat: created group '{group_name}' ({})",
        group_id.as_str()
    );

    // 6. Fresh pinned session id + effective allowlist. The id is pinned for the
    //    whole chat so every turn resumes the SAME agent session.
    let session_id = uuid::Uuid::new_v4().to_string();
    let allowlist = ToolAllowlist::from_flags(&args.allow_tool, args.dangerous_all_tools);

    // Shared child-bound pre-empt trigger so a control `stop`/`kill` can
    // pre-empt an in-flight turn even mid-execution, immune to PID reuse. It is
    // held by BOTH the turn runner (which owns the child process) and the
    // SignalChannel actor (which fires it on an inbound `stop`/`kill`).
    let preempt: PreemptSlot = Arc::new(Mutex::new(None));
    let driver = SerialTurnDriver::new(
        CopilotTurnRunner::new(COPILOT_BIN, preempt.clone()),
        &session_id,
        allowlist.clone(),
    );

    // 7. Bounded, operator-configurable turn-queue capacity, then move the
    //    transport into the SignalChannel's background Signal I/O actor. The
    //    channel re-derives its own Gate from cfg (gating.rs untouched) and
    //    services BOTH directions, so inbound is still accepted while a turn
    //    runs — the concurrency the old `tokio::select!` loop provided.
    let capacity = args
        .inbox_capacity
        .unwrap_or_else(SignalChannel::default_capacity);
    let mut channel = SignalChannel::new(transport, &cfg, group_id.clone(), preempt, capacity);
    channel.set_session_id(&session_id);

    // 8. Announce topic, blast radius, and control phrases through the channel's
    //    fail-closed verify → redact → chunk path (recording it in the actor's
    //    echo window). This is the initial "session started" post before the loop.
    let announcement = format!(
        "amplihack signal chat started.\n\
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
    channel.announce_session_started(&announcement).await;

    // 9. First turn: the topic itself is the opening prompt (a trusted CLI arg,
    //    seeded directly into the queue, exactly as the old loop spawned the
    //    first turn from `args.topic`).
    channel.seed_first_prompt(args.topic.clone());

    // 10. Drive the generic sequential turn loop. A failed turn is mapped to
    //     posted output by `ResilientSession` (never fatal), so the chat stays
    //     alive on the SAME session — preserving the old loop's resilience where
    //     a turn error posted `turn failed: {e}` and continued.
    let mut session = ResilientSession { inner: driver };
    if let Err(e) = run_session_loop(&mut session, &mut channel).await {
        eprintln!("signal chat: session loop ended with error: {e}");
    }

    Ok(())
}

/// Wraps the real [`AgentSession`] so a failed turn never ends the generic loop.
///
/// The hand-rolled loop kept the chat alive on a turn error — it posted
/// `turn failed: {e}` and resumed the SAME session on the next prompt. The
/// generic [`run_session_loop`] instead propagates any [`TurnError`] out of the
/// loop (and would terminate the chat). To preserve the old behavior we map
/// every turn error to a *successful* [`TurnOutput`] carrying the
/// operator-visible failure text; a pre-emption maps to empty output (the
/// `stop`/`kill` path has already closed the channel, so the loop ends anyway).
struct ResilientSession<S: AgentSession> {
    inner: S,
}

impl<S: AgentSession> AgentSession for ResilientSession<S> {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        match self.inner.run_turn(prompt).await {
            Ok(out) => Ok(out),
            Err(TurnError::Preempted) => Ok(TurnOutput::from_text("")),
            Err(e) => Ok(TurnOutput::from_text(format!("turn failed: {e}"))),
        }
    }

    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
}
