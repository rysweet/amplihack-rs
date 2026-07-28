# Auto Mode on the Generic Turn Loop (`AutoModeChannel`)

> **Status:** shipped as PR-4 of issue #910. This document describes the
> finished state: the LIVE auto-mode loop in
> `crates/amplihack-cli/src/commands/auto_mode/` no longer hand-rolls its own
> `Clarify → Plan → Execute → Evaluate → Adjust` state machine inside a single
> `AutoModeSession::run()` method. It runs on the crate-generic
> [`amplihack_turn::run_session_loop`](signal-channel.md#crate-api-reference),
> driving an [`AutoModeChannel`](#automodechannel) that implements the reusable
> [`amplihack_turn::Channel`](#the-channel-contract) trait, over an
> [`AutoModeRunner`](#automoderunner) that implements
> [`amplihack_turn::AgentSession`](#the-agentsession-contract). The change is
> **behavior-preserving**: every externally observable behavior — the prompt
> sequence and prompt text, the per-phase log lines, the `AutoModeState`
> transitions (`completed` / `error` / `stopped`), the per-phase exit-code
> semantics, the `CompletionVerifier` gate, and the final `process::exit` value
> — is identical to the previous implementation. The pre-existing
> characterization tests pass unchanged.

This feature is always compiled (auto mode is a core CLI path). It adds no new
dependencies beyond a direct dependency on the already-present, always-compiled
`amplihack-turn` crate.

---

## Contents

- [Why this exists](#why-this-exists)
- [Architecture](#architecture)
- [The generic driver loop](#the-generic-driver-loop)
- [The `AgentSession` contract](#the-agentsession-contract)
- [The `Channel` contract](#the-channel-contract)
- [`TurnOutput::exit_code`](#turnoutputexit_code)
- [`AutoModeRunner`](#automoderunner)
- [`AutoModeChannel`](#automodechannel)
  - [Phase state machine](#phase-state-machine)
  - [`next_prompt` — LISTEN](#next_prompt--listen)
  - [`publish_output` — the state machine](#publish_output--the-state-machine)
  - [Terminal state and exit code](#terminal-state-and-exit-code)
  - [Required-turn failure semantics](#required-turn-failure-semantics)
  - [Appended-instruction handling](#appended-instruction-handling)
- [`run_auto_mode` wiring](#run_auto_mode-wiring)
- [Exit-code contract](#exit-code-contract)
- [Behavior-preservation contract](#behavior-preservation-contract)
- [Removed: the dead launcher `AutoMode`](#removed-the-dead-launcher-automode)
- [Configuration](#configuration)
- [Examples](#examples)
- [Testing](#testing)
- [FAQ](#faq)

---

## Why this exists

Auto mode and the reusable turn primitives grew up separately, so the live loop
in `crates/amplihack-cli/src/commands/auto_mode/session.rs` was a bespoke method
(`AutoModeSession::run()`) that inlined the whole control flow: emit a prompt,
run a subprocess turn, inspect its output, decide the next phase, log, and set a
terminal status. That method re-implemented, by hand, the exact
LISTEN → run turn → REPLAY cadence that PR-2 factored into the agent-generic
[`amplihack-turn`](signal-channel.md#crate-api-reference) crate
(`AgentSession` + `Channel` + `run_session_loop`), and that PR-3 already adopted
for Signal.

PR-4 finishes the pattern for auto mode. The two concerns are split cleanly:

- The **execution** concern (run one prompt in a subprocess, capture stdout and
  the exit code) becomes an [`AutoModeRunner`](#automoderunner) that implements
  `AgentSession`.
- The **control-flow** concern (the phase state machine, completion detection
  and verification, appended-instruction ingestion, logging, and terminal
  status) becomes an [`AutoModeChannel`](#automodechannel) that implements
  `Channel`.

The loop itself becomes a single call to `run_session_loop`. This deletes the
hand-rolled loop body, makes auto mode share the same tested driver as Signal
and every future channel, and removes a large block of genuinely dead duplicate
code (`amplihack-launcher/src/auto_mode_exec.rs`) that no live path ever
constructed.

---

## Architecture

```
run_auto_mode  (sync CLI entry point)
      │
      │  block_on(current-thread runtime)
      ▼
run_session_loop(&mut AutoModeRunner, &mut AutoModeChannel)   ← amplihack-turn
      │
      │   loop {
      │     channel.next_prompt()      ← LISTEN: which phase, what prompt?
      │     session.run_turn(&prompt)  ← EXECUTE: run subprocess, capture output
      │     channel.publish_output(&o) ← REPLAY: advance state machine
      │   }
      ▼
AutoModeRunner (AgentSession)          AutoModeChannel (Channel)
  • holds PromptExecutor                 • owns the phase cursor / state machine
  • run_turn → run_prompt verbatim       • CompletionSignalDetector
  • maps ExecutionResult → TurnOutput    • CompletionVerifier
    .with_exit_code(code)                • WorkSummaryGenerator
  • executor Err → TurnError::Exec       • process_appended_instructions
  • ran-but-non-zero is Ok, NOT an err   • terminal exit_code + abort + AutoModeState
```

The runner is deliberately **dumb**: it executes a prompt and carries the
per-turn exit code back to the channel. The channel is **authoritative** for all
control flow. This split is forced by the loop ordering (below): the only place
a turn's result is handed back to the channel is `publish_output`, so that is
where the state machine must live.

---

## The generic driver loop

`run_session_loop` (in `amplihack-turn`) drives one session from one channel
until the channel closes:

```rust
loop {
    match channel.next_prompt().await? {
        NextPrompt::Ready(prompt) => {
            let out = session.run_turn(&prompt).await?;
            channel.publish_output(&out).await?;
        }
        NextPrompt::Idle   => tokio::time::sleep(IDLE_BACKOFF).await,
        NextPrompt::Closed => break,
    }
}
Ok(())
```

The cycle is strictly `next_prompt → run_turn → publish_output`. Auto mode's
executor is a **synchronous, serial** subprocess, so `AutoModeChannel` never
returns `NextPrompt::Idle` — there is never "nothing to run yet." It returns
`Ready(prompt)` for the current phase, or `Closed` once the run is complete,
stopped, or errored.

---

## The `AgentSession` contract

```rust
#[allow(async_fn_in_trait)]
pub trait AgentSession {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput>;
    fn session_id(&self) -> &str;
}
```

`run_turn` runs ONE turn to natural completion and returns its
[`TurnOutput`](#turnoutputexit_code), or a `TurnError` on failure.
`session_id` returns the stable id of the driven session.

---

## The `Channel` contract

```rust
#[async_trait]
pub trait Channel: Send {
    fn id(&self) -> ChannelId;

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        let _ = out;
        Ok(())
    }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt>;
}

pub enum NextPrompt {
    Ready(String),
    Idle,
    Closed,
}
```

`AutoModeChannel` overrides the default no-op `publish_output` because that is
where its state machine lives.

---

## `TurnOutput::exit_code`

`TurnOutput` gains a single **additive, backward-compatible** field so the
channel can observe a subprocess's exit code without inspecting stdout:

```rust
pub struct TurnOutput {
    text: String,
    exit_code: Option<i32>,
}

impl TurnOutput {
    /// Unchanged signature. Sets `exit_code = None`.
    pub fn from_text(text: impl Into<String>) -> Self { /* ... */ }

    /// Unchanged behavior — the agent's response, captured verbatim.
    pub fn text(&self) -> &str { &self.text }

    /// Builder: attach an exit code. Last write wins.
    #[must_use]
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self
    }

    /// The subprocess exit code for this turn, if one was recorded.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}
```

Contract:

| Constructor / call             | `text()`   | `exit_code()` |
| ------------------------------ | ---------- | ------------- |
| `from_text(s)`                 | `s`        | `None`        |
| `from_text(s).with_exit_code(n)` | `s`      | `Some(n)`     |
| `.with_exit_code(a).with_exit_code(b)` | `s` | `Some(b)`     |

`from_text` and `text()` are unchanged, so every existing `TurnOutput` caller
(including Signal) compiles and behaves identically. Consumers must treat
`exit_code()` as data only — compare/branch on it, never index, allocate, or
cast on it — and must handle `None` without panicking (no `.unwrap()`).

---

## `AutoModeRunner`

`AutoModeRunner<E: PromptExecutor>` is the "dumb" execution half. It holds the
`PromptExecutor` and the fixed per-run context and implements `AgentSession`.

```rust
impl<E: PromptExecutor> AutoModeRunner<E> {
    pub fn new(
        executor: E,
        tool: AutoModeTool,
        exec_dir: PathBuf,
        project_dir: PathBuf,
        passthrough: Vec<String>,
        session_id: String,
    ) -> Self { /* ... */ }
}

impl<E: PromptExecutor> AgentSession for AutoModeRunner<E> {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        match self.executor.run_prompt(
            self.tool, &self.exec_dir, &self.project_dir,
            &self.passthrough, prompt,
        ) {
            Ok(ExecutionResult { exit_code, stdout, .. }) =>
                Ok(TurnOutput::from_text(stdout).with_exit_code(exit_code)),
            Err(e) => Err(TurnError::Exec(e.to_string())),
        }
    }

    fn session_id(&self) -> &str { &self.session_id }
}
```

Key rules:

- **`run_prompt` is called verbatim.** No shell, no `sh -c`, no string
  interpolation — the same arg-vector exec path (and `QUERY_TIMEOUT`) as before.
- **A ran-but-non-zero subprocess is `Ok`, NOT an error.** A non-zero exit code
  means the subprocess ran; it is carried back via `with_exit_code`. Only a
  failure to spawn / capture (the executor's own `Err`) becomes
  `TurnError::Exec`. This distinction is what preserves the crash-vs-complete
  semantics (see [Exit-code contract](#exit-code-contract)).
- `run_turn` is an `async fn` that calls the synchronous `run_prompt` directly.

---

## `AutoModeChannel`

`AutoModeChannel` is the authoritative control-flow half. It owns everything the
old `AutoModeSession` did except subprocess execution: the phase cursor,
`AutoModeState`, the completion detector/verifier, the work-summary generator,
the append/appended directories, logging, the terminal exit code, and the
`abort` flag that flags a required-turn crash.

It implements `Channel` (`#[async_trait]`, `Send`).

### Phase state machine

The phase progression is identical to the old `run()` method:

```
Clarify (turn 1, required)
   └─► Plan (turn 2, required)
         └─► for turn in 3..=max_turns:
               Execute (non-zero → warn + continue)
                  └─► Evaluate (non-zero → status="error", exit that code)
                        ├─ verified complete → status="completed", exit 0
                        ├─ "needs adjustment" → Adjust (required) → next turn
                        └─ otherwise → next turn
         (loop falls through) → status="stopped", exit 0
```

- **Required phases** are Clarify, Plan, and Adjust. A non-zero exit in a
  required phase terminates the run with `status="error"` (see
  [Required-turn failure semantics](#required-turn-failure-semantics)).
- **Execute** warns and continues on a non-zero exit.
- **Evaluate** returns its exit code and sets `status="error"` on a non-zero
  exit.

### `next_prompt` — LISTEN

`next_prompt` emits the prompt for the **current** phase, using the exact
prompt-builder functions and text from the previous implementation
(`build_clarify_prompt`, `build_plan_prompt`, `build_execute_prompt`,
`build_evaluation_prompt`, `build_plan_adjustment_prompt`, all with
`philosophy_context()`):

- Returns `NextPrompt::Ready(prompt)` for Clarify, Plan, Execute, Evaluate, or
  Adjust as the cursor dictates.
- Returns `NextPrompt::Closed` once the run has reached a terminal state
  (completed, stopped, or error).
- **Never returns `NextPrompt::Idle`** — the executor is synchronous and serial,
  so there is always either a prompt to run or a closed run.

`next_prompt` does **not** decide the next phase; it only renders the prompt for
whatever phase the cursor already points at.

**Appended-instruction timing (security-critical).** When the cursor points at
an **Execute** phase, `next_prompt` is the point at which appended instructions
are ingested — they must be read, sanitized, and embedded *while rendering the
Execute prompt* so they land in the **current** turn, exactly as the old
`run()` did (it called `process_appended_instructions` immediately before
`build_execute_prompt`). This preserves the R2.1
read → **sanitize** → embed → archive ordering (see
[Appended-instruction handling](#appended-instruction-handling)). Ingesting in
`publish_output` instead would embed the instructions one turn late, so the
processing is deliberately anchored in `next_prompt`, not `publish_output`.

### `publish_output` — the state machine

`publish_output(&out)` is where all decisions happen, because it is the only
point in the loop that receives a turn's result. For the phase that just ran it:

1. Reads `out.exit_code()` and `out.text()` (stdout).
2. Logs the command result (`"<label> exit code: N (stdout X chars, stderr …)"`)
   and the per-phase transition lines, byte-for-byte as before.
3. Runs `WorkSummaryGenerator` / `CompletionSignalDetector` /
   `CompletionVerifier` for the Evaluate phase, logging the completion score and
   verification status exactly as the old `should_continue_loop` did.
4. Updates `AutoModeState` (`update_turn`, `update_status`) and advances the
   phase cursor.
5. On a terminal condition, records the final exit code, sets `abort` when a
   required turn failed, and marks the run closed so `next_prompt` will
   subsequently return `Closed`.

(Appended instructions are **not** processed here — they are ingested in
`next_prompt` while the Execute prompt is rendered; see
[`next_prompt`](#next_prompt--listen).)

Because the state machine advances the cursor here, the runner stays stateless
with respect to control flow.

### Terminal state and exit code

The final exit code and terminal `AutoModeState` are **readable fields** on the
channel after `run_session_loop` returns. The channel also exposes an `abort`
signal so `run_auto_mode` can distinguish a *required-turn failure* (which must
crash the session) from an *Evaluate failure* (which must complete the session
and exit with the evaluation code):

```rust
struct AutoModeChannel {
    // ... phase cursor, state, detector/verifier, dirs, log ...
    exit_code: i32,           // 0 for completed / stopped; the failing phase's
                              // exit code on Evaluate or required-turn failure
    abort: Option<String>,    // Some(reason) iff a REQUIRED turn (Clarify /
                              // Plan / Adjust) exited non-zero
}

impl AutoModeChannel {
    /// The process exit code to use once the loop has closed.
    /// 0 for completed / stopped; the failing phase's exit code otherwise.
    pub fn exit_code(&self) -> i32 { self.exit_code }

    /// `Some(reason)` iff a required turn failed and the run must be treated as
    /// a crash (equivalent to the old `bail!`). `None` for every other outcome,
    /// including a non-zero Evaluate exit.
    pub fn abort(&self) -> Option<&str> { self.abort.as_deref() }
}
```

No side channel and no change to the loop's `ChannelResult<()>` return type:
`run_auto_mode` reads `channel.abort()` first (bailing if it is `Some`), then
`channel.exit_code()`.

### Required-turn failure semantics

The old implementation used `bail!` when a required turn (Clarify / Plan /
Adjust) exited non-zero, propagating an `anyhow::Error` all the way out of
`run()`. Since the subprocess *ran* (it just failed), the runner returns
`Ok(TurnOutput)` — it is **not** a `TurnError`. The channel reproduces the old
`bail!` behavior gracefully:

- In `publish_output`, when a required phase reports a non-zero exit:
  - set `status = "error"`,
  - store the exit code in `exit_code`,
  - set `abort = Some(reason)` so `run_auto_mode` treats it as a crash,
  - log identically to the old `bail!` path,
  - drive termination by making the next `next_prompt()` return `Closed`.

This is behavior-equivalent at the `run_auto_mode` boundary — same status, same
exit code, same stop point — without converting a ran-but-failed subprocess into
a transport-level error. After the loop, `run_auto_mode` sees `abort` is `Some`,
returns `Err` from its inner closure, and the enclosing
`if result.is_err() { crash_session }` wrapper crashes the session tracker for
this case (see [Exit-code contract](#exit-code-contract)). This is the crucial
distinction from an Evaluate failure, where `abort` stays `None`.

### Appended-instruction handling

Before each Execute prompt, the channel calls
`process_appended_instructions(&append_dir, &appended_dir)`, preserving the
existing security-critical ordering: read → **sanitize** (`sanitize_injected_content`,
with its 50 KiB cap and injection-pattern filters) → embed in the Execute
prompt → archive the source file from `append/` to `appended/` via `fs::rename`.
Sanitized instruction content is embedded as data only; it is never executed or
path-resolved.

---

## `run_auto_mode` wiring

`run_auto_mode` is a synchronous CLI entry point. The rewiring keeps the same
session-tracking lifecycle — including the outer
`let result = (|| -> Result<()> { … })();` closure and the
`if result.is_err() { crash_session }` wrapper — and just swaps the loop body.
The `abort()` check is what preserves the crash-vs-complete distinction: a
required-turn failure returns `Err` (→ `crash_session`), while an Evaluate
failure returns `Ok` and exits with the stored code (→ `complete_session`):

```rust
let result = (|| -> Result<()> {
    // (prompt parsing, --ui guard, and AutoModeTool::Amplifier fast-path unchanged)

    let mut runner = AutoModeRunner::new(
        SystemPromptExecutor { ui_active, node_options },
        tool, execution.execution_dir.clone(), execution.project_dir.clone(),
        parsed.passthrough_args, session_id.clone(),
    );
    let mut channel = AutoModeChannel::new(/* prompt, max_turns, dirs, state, … */)?;

    // Bridge sync → async with a current-thread runtime. NOT #[tokio::main]:
    // run_auto_mode is called from sync context, so a nested multi-thread
    // runtime would panic.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_session_loop(&mut runner, &mut channel))?;

    // Required-turn failure: reproduce the old `bail!` → the outer
    // `if result.is_err()` wrapper crashes the session tracker.
    if let Some(reason) = channel.abort() {
        bail!("{reason}");
    }

    // Completed / stopped / Evaluate-failure: complete the session, then honor
    // the exit code (0 for completed/stopped, the evaluation code otherwise).
    let exit_code = channel.exit_code();
    tracker.complete_session(&session_id)?;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
})();

if result.is_err() {
    let _ = tracker.crash_session(&session_id);
}
result
```

The `--ui` path continues to return the existing
`"--ui is not yet supported in native Rust auto mode"` error, and the
`AutoModeTool::Amplifier` single-shot fast-path is unchanged.

> **Risk note — sync→async bridge:** `run_auto_mode` must use a
> `new_current_thread` runtime with `block_on`, **not** `#[tokio::main]` or a
> multi-thread runtime, to avoid a nested-runtime panic.

> **Naming note:** this new `AutoModeRunner` lives in
> `amplihack-cli/src/commands/auto_mode/` and implements `AgentSession`. It is
> **distinct from** the unrelated `AutoModeRunner` re-exported by
> `amplihack-launcher` (`amplihack-launcher/src/lib.rs`, from
> `auto_mode_coordinator`). The two are unrelated types that happen to share a
> name; the launcher's is not used by this LIVE path.

---

## Exit-code contract

The four exit outcomes are preserved exactly:

| Outcome                         | Where decided        | Session tracker | Process exit |
| ------------------------------- | -------------------- | --------------- | ------------ |
| Objective verified complete     | Evaluate, `publish_output` | `complete_session` | `exit(0)` (returns `Ok`) |
| Max turns reached, no completion | loop fall-through    | `complete_session` | `exit(0)` (returns `Ok`) |
| Evaluate step exits non-zero    | Evaluate, `publish_output` (`abort` stays `None`) | `complete_session` | `process::exit(code)` |
| Required turn exits non-zero    | Clarify/Plan/Adjust, `publish_output` (`abort = Some`) | `crash_session` | inner closure returns `Err`; process exits non-zero |

- For a **required-turn failure**, `publish_output` sets `abort = Some(reason)`
  and stores that turn's own non-zero exit code. `run_auto_mode` sees `abort`
  is `Some`, bails from the inner closure, and the
  `if result.is_err() { crash_session }` wrapper records a crash — matching the
  old `bail!` + `crash_session` path.
- For an **Evaluate failure**, `abort` stays `None`; `run_auto_mode` calls
  `complete_session` and then `process::exit(channel.exit_code())` with the
  non-zero evaluation code — matching the old `Ok(exit_code)` + `process::exit`
  path.

---

## Behavior-preservation contract

Every externally observable behavior is identical to the pre-refactor
`AutoModeSession`:

- **Prompt sequence & text** — identical builders, identical
  `philosophy_context()`, identical Clarify → Plan → Execute → Evaluate → Adjust
  order.
- **Log lines** — identical phase-transition lines, `log_command_result`
  format, completion-score line, verification-discrepancy line, and terminal
  messages (`"Objective achieved"`, `"Reached max turns without verified
  completion"`, the required-turn failure message).
- **`AutoModeState` transitions** — identical `update_turn` cadence and terminal
  `completed` / `error` / `stopped` statuses.
- **Exit-code semantics per phase** — Execute warns-and-continues on non-zero;
  Evaluate returns the code and marks `error`; required turns terminate with
  `status="error"`.
- **`CompletionVerifier`** — identical detection, verification, and
  "verified complete" gating.
- **`process::exit` value** — identical to before at the `run_auto_mode`
  boundary.

Out of scope (untouched): prompt wording, Signal / `SerialTurnDriver` production
behavior, `--ui`, other `auto_mode_*` launcher modules, and any new
dependencies.

---

## Removed: the dead launcher `AutoMode`

`crates/amplihack-launcher/src/auto_mode_exec.rs` held an `AutoMode` struct — a
second, older autonomous-execution engine with its own retry logic and
instruction injection. It was **dead code**: no live path constructed it
(confirmed by a repo-wide grep for `AutoMode` references). PR-4 removes it as
part of consolidating on the single `amplihack-turn`-based loop:

- Delete `crates/amplihack-launcher/src/auto_mode_exec.rs` (the struct and its
  in-file tests).
- Remove `pub mod auto_mode_exec;` and `pub use auto_mode_exec::AutoMode;` from
  `crates/amplihack-launcher/src/lib.rs`.

After removal, a repo-wide grep for `AutoMode` (the launcher struct) returns no
references.

---

## Configuration

No new configuration surface. Auto mode is driven exactly as before via the CLI:

| Flag / env                | Effect                                              |
| ------------------------- | --------------------------------------------------- |
| `--auto`                  | Enable the autonomous loop.                          |
| `--max-turns N`           | Cap the number of Execute/Evaluate turns (default 10). |
| `--ui`                    | Reserved; currently returns "not yet supported".     |
| `--checkout-repo <repo>`  | Check out a repo for the session (unchanged).        |
| `NODE_OPTIONS`            | Passed through to the subprocess (e.g. memory size). |

Appended instructions are still read from the session's `append/` directory and
archived to `appended/` after sanitization.

---

## Examples

Auto mode is invoked exactly as before — the refactor is invisible to callers:

```bash
# Basic auto mode (Copilot)
amplihack copilot --auto -- -p "add retry with backoff to the HTTP client"

# Claude with a higher turn cap
amplihack claude --auto --max-turns 20 -- -p "refactor the API module"

# Inject a mid-run instruction: drop a file into the session's append/ dir.
# It is sanitized, embedded in the next Execute prompt, then archived.
echo "Also update the CHANGELOG." \
  > .claude/runtime/logs/auto_copilot_<ts>/append/note.md
```

Programmatic composition (for tests or embedding) follows the generic pattern:

```rust
use amplihack_turn::run_session_loop;

let mut runner = AutoModeRunner::new(executor, tool, exec_dir, project_dir,
                                     passthrough, session_id);
let mut channel = AutoModeChannel::new(prompt, max_turns, /* dirs, state */)?;

let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
rt.block_on(run_session_loop(&mut runner, &mut channel))?;

// Required-turn failure surfaces via `abort()`; every other outcome via `exit_code()`.
if let Some(reason) = channel.abort() {
    // treat as crash (old `bail!` semantics)
}
let exit_code = channel.exit_code();
```

---

## Testing

| Test                                        | Crate            | Pins                                             |
| ------------------------------------------- | ---------------- | ------------------------------------------------ |
| `turn_output_exit_code_it`                  | `amplihack-turn` | `from_text` → `None`; `with_exit_code` round-trip; `text()` unchanged |
| `agent_session_it`                          | `amplihack-turn` | `AgentSession` runner contract                   |
| `driver_loop_it`                            | `amplihack-turn` | `run_session_loop` ordering                      |
| `auto_mode/tests.rs`                        | `amplihack-cli`  | Per-phase prompt emission; cursor advance in `publish_output`; non-zero-Evaluate → exit path; required-turn non-zero → crash path; `sanitizes_appended_instructions_before_prompt` |
| `run_bails_and_marks_error_when_clarify_turn_fails` | `amplihack-cli` | Required-turn failure sets `abort = Some`, `status="error"` + non-zero exit |
| `evaluate_failure_completes_session_without_abort` | `amplihack-cli` | Evaluate non-zero leaves `abort = None`; `exit_code()` carries the code |
| `tests/auto_mode_prompt_delivery.rs`        | `amplihack-cli`  | Prompt-delivery integration                      |
| launcher suite                              | `amplihack-launcher` | Green after `auto_mode_exec` removal          |

Verification gates:

```bash
cargo build --workspace
cargo test -p amplihack-turn
cargo test -p amplihack-cli
cargo test -p amplihack-launcher
cargo clippy --workspace -- -D warnings
cargo fmt --check
# grep confirms zero launcher `AutoMode` references remain
```

---

## FAQ

**Why does the state machine live in `publish_output` instead of `next_prompt`?**
Because the loop is strictly `next_prompt → run_turn → publish_output`, and
`publish_output` is the only step that receives a turn's result. Deciding the
next phase requires that result (stdout + exit code), so the decision must live
there. `next_prompt` only renders the prompt for the phase the cursor already
points at.

**Why is a non-zero subprocess exit `Ok`, not a `TurnError`?**
Because the subprocess *ran*. `TurnError` is reserved for failing to spawn or
capture the process. Treating a ran-but-non-zero result as `Ok` (carrying the
code via `with_exit_code`) is what lets the channel apply the correct
per-phase policy — warn-and-continue for Execute, exit-with-code for Evaluate,
crash for required turns.

**Does this change the CLI or any prompt text?**
No. The refactor is behavior-preserving. Prompts, logs, statuses, and exit codes
are identical.

**Why `new_current_thread` and `block_on` instead of `#[tokio::main]`?**
`run_auto_mode` is a synchronous function called from a synchronous CLI context.
A current-thread runtime driven with `block_on` bridges into the async
`run_session_loop` without spawning a nested multi-thread runtime, which would
panic.

**What happened to `AutoModeSession`?**
Its execution responsibility moved to [`AutoModeRunner`](#automoderunner) and its
control-flow responsibility moved to [`AutoModeChannel`](#automodechannel). The
hand-rolled loop body is deleted.

**How does `run_auto_mode` tell a required-turn failure apart from an Evaluate failure, when both set `status="error"`?**
Via `channel.abort()`. A required-turn failure sets `abort = Some(reason)`, so
`run_auto_mode` bails from its inner closure and the
`if result.is_err() { crash_session }` wrapper records a crash (old `bail!`
path). An Evaluate failure leaves `abort = None`, so `run_auto_mode` calls
`complete_session` and `process::exit(channel.exit_code())` (old
`Ok(exit_code)` path). Both statuses are `"error"`, but only the required-turn
case crashes the session tracker.
