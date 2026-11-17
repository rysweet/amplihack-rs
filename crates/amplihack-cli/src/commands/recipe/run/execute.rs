use super::correlation::{
    RecipeRunCorrelation, RecipeRunFinalStatus, RunSettings, known_log_paths,
};
use super::failure_class::{
    FAILURE_CLASS_RESULT_KEY, FailureVerdict, classify_error, classify_run_failure,
    emit_failure_class_marker,
};
use super::retry::{AttemptOutcome, RetrySummary, TransientRetryLimits, run_with_transient_retry};
use super::*;
use crate::env_builder::EnvBuilder;
#[cfg(windows)]
use crate::util::run_with_timeout;
use crate::util::truncate_chars_with_notice;
use amplihack_utils::clock::{Clock, SystemClock};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::Child;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

const STDERR_TAIL_LINES: usize = 5;
const CAPTURED_STDERR_LINES: usize = 200;
const RECIPE_RUNNER_DEFAULT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const RECIPE_RUNNER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const RECIPE_RUNNER_TERMINATE_TIMEOUT: Duration = Duration::from_secs(5);
const RECIPE_RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";
/// Env var controlling the SIGTERM -> SIGKILL grace window (whole seconds) used
/// when deterministically tearing down the recipe-runner process tree (#964).
/// `0` escalates to SIGKILL immediately (no graceful window).
#[cfg(unix)]
const RECIPE_RUNNER_TEARDOWN_GRACE_ENV: &str = "AMPLIHACK_TEARDOWN_GRACE_SECS";
/// Default grace window when `AMPLIHACK_TEARDOWN_GRACE_SECS` is unset/invalid.
#[cfg(unix)]
const RECIPE_RUNNER_DEFAULT_TEARDOWN_GRACE: Duration = Duration::from_secs(5);
/// Env var carrying the current session's recursion depth (root = 0). Shared
/// with the session-tree convention; consumed by the fail-closed recursion
/// guard [`enforce_recursion_depth_guard`] (#964).
const SESSION_DEPTH_ENV: &str = "AMPLIHACK_SESSION_DEPTH";
/// Env var carrying the maximum permitted recursion depth, clamped to
/// `MAX_DEPTH_CEILING` before use (#964).
const MAX_DEPTH_ENV: &str = "AMPLIHACK_MAX_DEPTH";
const TREE_ID_ENV: &str = "AMPLIHACK_TREE_ID";
const SESSION_TREE_DIR_ENV: &str = "AMPLIHACK_SESSION_TREE_DIR";
const MAX_SESSIONS_ENV: &str = "AMPLIHACK_MAX_SESSIONS";

/// Exit status for a policy refusal to spawn a nested orchestration (issue #1326).
/// Distinct from 1 so a caller -- human or agent -- can tell "the tool broke" from
/// "the tool declined, and will decline again".
pub const EXIT_ORCHESTRATION_UNAVAILABLE: i32 = 79;

/// #2663 M1: a merge run succeeds only when it merged. `outcome` is the
/// `autodrive_outcome` built by `report()`; success is an allow-list of the
/// single `stop_reason` value `completed` (merged in this run, already merged
/// at entry, or, since #2575 B4, a verify-only run that reached `DONE` or
/// `WAITING`). A missing, non-string or any other value is a failure.
/// docs/reference/autodrive-merge-ready-round.md#success-means-merged.
pub(crate) fn autodrive_run_succeeded(outcome: &serde_json::Value) -> bool {
    outcome
        .get("stop_reason")
        .and_then(serde_json::Value::as_str)
        == Some("completed")
}

/// #2663: the run's final status follows the merge, in both directions. A run
/// that did not merge is not a success; a run whose merge `report()` read as
/// `completed` is a success even when a later step failed. Applied only to an
/// `Ok` run result, after `run_with_transient_retry` has returned, so it never
/// starts a whole-run re-attempt (#1267) and never touches exit 79 or another
/// terminal block, which are `Err`. It runs before the failure class is
/// attached, so a raised run carries none.
/// docs/reference/autodrive-merge-ready-round.md#success-means-merged.
pub(crate) fn apply_autodrive_outcome(
    run_result: &mut RecipeRunResult,
    outcome: &serde_json::Value,
) {
    let succeeded = autodrive_run_succeeded(outcome);
    if run_result.success == succeeded {
        return;
    }
    if succeeded {
        eprintln!(
            "INFO: auto-drive completed; a later step failed, the run is reported as a success [#2663]"
        );
    } else {
        eprintln!(
            "INFO: auto-drive did not complete (stop_reason {}); the run is not reported as a success [#2663]",
            outcome
                .get("stop_reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("missing")
        );
    }
    run_result.success = succeeded;
}

/// Report a terminal refusal and carry the distinguishing exit code.
///
/// The message is printed here because the `CliExitError` path in `main` exits
/// without reporting; a silent 79 would be worse than the status quo.
fn terminal_block(message: String) -> anyhow::Error {
    eprintln!("{message}");
    crate::command_error::exit_error(EXIT_ORCHESTRATION_UNAVAILABLE)
}

/// Decide how much to believe an inherited `AMPLIHACK_SESSION_DEPTH` (issue #1326).
///
/// A depth claim is only actionable if something other than the variable itself
/// supports it. `AMPLIHACK_SESSION_DEPTH` is inherited by every descendant and
/// outlives the run that set it; the host that motivated this work had 4,583
/// processes SIGKILLed at once, so a surviving shell holding stale orchestration
/// variables is ordinary. Refusing on the variable alone would wedge that user's
/// `amplihack recipe run` permanently, with no hint as to why.
///
/// * corroborated (a tree id, or a live orchestrator ancestor) -> believe it; a
///   nested run whose ceiling cannot be verified is then refused by the caller.
/// * uncorroborated -> treat as leftover state and start a fresh tree at depth 0.
///
/// Split out as a pure function so the decision is testable without fabricating
/// a process tree.
pub(crate) fn resolve_claimed_depth(claimed: u32, sealed: Option<u32>, corroborated: bool) -> u32 {
    if claimed > 0 && sealed.is_none() && !corroborated {
        tracing::warn!(
            claimed,
            "AMPLIHACK_SESSION_DEPTH is set but nothing corroborates it; treating it as \
             a stale variable from a previous run and starting a new tree at depth 0 \
             (issue #1326)"
        );
        return 0;
    }
    claimed
}

/// Does this process actually descend from an orchestrator? (issue #1326)
///
/// `AMPLIHACK_SESSION_DEPTH` is an inherited string. A shell that outlived a killed
/// run keeps it -- and on the host that motivated this work, 4,583 processes were
/// SIGKILLed at once, so stale orchestration variables in a surviving tmux pane are
/// ordinary, not exotic. Refusing on the env var alone would permanently wedge that
/// user's `amplihack recipe run` with no indication of why.
///
/// Process ancestry cannot be inherited from a dead run, so it distinguishes "I am
/// genuinely nested" from "I have a leftover variable". Linux-only; elsewhere the
/// presence of `AMPLIHACK_TREE_ID` is the only corroboration available.
///
/// Known limits, both bounded by `AMPLIHACK_TREE_ID` being the primary signal --
/// the runner always sets it for children, so a real nested run is corroborated
/// without needing this at all:
///
/// * a broken chain (an ancestor exited, or the process was reparented to init)
///   reads as "not nested". That is the fail-open direction, and it is why this is
///   a secondary signal rather than the authority.
/// * PID reuse could name an unrelated `amplihack` as an ancestor, reading as
///   "nested" for a genuine root. That fails closed, which is the safe direction.
#[cfg(target_os = "linux")]
fn has_orchestrator_ancestor() -> bool {
    const MAX_HOPS: usize = 64;
    let mut pid = std::process::id();
    for _ in 0..MAX_HOPS {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // `comm` is parenthesised and may itself contain spaces or parens, so scan
        // from the last ')': the fields after it are state, then ppid.
        let Some(close) = stat.rfind(')') else {
            return false;
        };
        let mut fields = stat[close + 1..].split_whitespace();
        let _state = fields.next();
        let Some(ppid) = fields.next().and_then(|v| v.parse::<u32>().ok()) else {
            return false;
        };
        if ppid <= 1 {
            return false;
        }
        let comm = std::fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap_or_default();
        let comm = comm.trim();
        if comm.starts_with("amplihack") || comm.starts_with("recipe-runner") {
            return true;
        }
        pid = ppid;
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn has_orchestrator_ancestor() -> bool {
    false
}

/// What the guard resolved for the child about to be spawned (issue #1326).
struct SpawnGuard {
    tree_id: String,
    child_depth: u32,
    max_depth: u32,
    /// Node admitted for this spawn, released when the child finishes (issue #1329).
    session_id: String,
}

impl Drop for SpawnGuard {
    /// Release the admitted node however this function exits (issue #1329).
    ///
    /// RAII rather than a call after the wait: a spawn failure, an early return on
    /// a terminal error, or a panic would otherwise leak capacity, and leaked
    /// capacity in a tree-global budget is indistinguishable from real load. Stale
    /// entries are still swept by `prune_stale`, but only after hours.
    fn drop(&mut self) {
        crate::commands::session_tree::state::release_session(&self.tree_id, &self.session_id);
    }
}

/// Maximum byte length of a context value that we export as a single
/// environment variable. The kernel rejects any individual argv/envp
/// string longer than `MAX_ARG_STRLEN` (PAGE_SIZE * 32 = 131072 on Linux)
/// with `E2BIG`. We cap conservatively below that so a pathologically large
/// value cannot make the spawn fail. Over-limit values are still delivered
/// to the runner for `{{placeholder}}` substitution — since issue #2582 in
/// the context file, and only on the `--set` path for a runner older than
/// the pin; only the env mirror is skipped (issue #784, regression guard
/// for the E2BIG path).
const CONTEXT_ENV_VALUE_MAX_BYTES: usize = 96 * 1024;

/// Per-entry byte overhead charged to every environment variable when
/// accounting for the aggregate env budget (issue #1023). Each `envp` entry
/// costs the encoded string `NAME=VALUE\0` (the `=` separator + trailing NUL)
/// **plus** one pointer in the `envp` array (8 bytes on a 64-bit target). We
/// use a single flat constant of 16 = 8 (pointer) + `=` + NUL + a few bytes of
/// slack, applied uniformly to both pass-through accounting (`vars_os()`) and
/// the context mirror so the two sides of the budget stay consistent.
pub(super) const ENV_ENTRY_OVERHEAD_BYTES: usize = 16;

/// Operator override for the aggregate context-env budget (issue #1023). When
/// set to a valid `usize` it takes precedence over the runtime-derived budget
/// (`0` is valid and means "mirror essentials only"). Invalid / non-numeric
/// values are ignored (a warning is logged) and the derived budget is used.
pub(super) const CONTEXT_ENV_BUDGET_OVERRIDE_ENV: &str = "AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES";

/// Conservative ARG_MAX fallback (128 KiB) used when `sysconf(_SC_ARG_MAX)` is
/// unavailable or returns an implausibly small value (issue #1023). POSIX only
/// guarantees `_POSIX_ARG_MAX` = 4096, which is far too small to reason about;
/// a real limit below [`ARG_MAX_MIN_PLAUSIBLE_BYTES`] signals a broken /
/// emulated environment. 128 KiB sits safely below every real platform's true
/// limit while still leaving room to mirror the small essential context keys.
const ARG_MAX_FALLBACK_BYTES: usize = 131_072;

/// Any `sysconf(_SC_ARG_MAX)` result `<= 0` or below this threshold (64 KiB) is
/// treated as implausible and replaced with [`ARG_MAX_FALLBACK_BYTES`].
const ARG_MAX_MIN_PLAUSIBLE_BYTES: usize = 65_536;

/// Flat reservation (128 KiB) subtracted from ARG_MAX before computing the
/// aggregate context-env budget (issue #1023). Covers, with a single generous
/// margin: the process `argv`, the recipe-runner's own re-exported
/// `RECIPE_VAR_*` variables, the `AMPLIHACK_*` variables added by
/// [`EnvBuilder`], and general safety slack. A flat reservation is simpler and
/// safer than modelling each source individually.
const ENV_BUDGET_RESERVATION_BYTES: usize = 131_072;

/// Reserved / dangerous environment-variable names that must never be set
/// from untrusted recipe context (issue bodies, task descriptions and
/// third-party recipes all flow into the context map). These names are
/// NOT managed by `EnvBuilder`, so without this denylist a pathological
/// context key could clobber a process-critical variable or inject code
/// into a child shell/interpreter. The `AMPLIHACK_` namespace is handled
/// separately (prefix check) because it is owned by `EnvBuilder`.
///
/// Names are compared after uppercasing the context key (see
/// [`context_env_pairs`]). Covers: path/identity, the dynamic linker,
/// shell-startup remote-code-execution vectors, word-splitting and
/// interpreter option-injection vectors.
const RESERVED_ENV_DENYLIST: &[&str] = &[
    // path / identity
    "PATH",
    "HOME",
    "SHELL",
    "PWD",
    "USER",
    "LOGNAME",
    // dynamic linker
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "GLIBC_TUNABLES",
    // shell-startup remote-code-execution vectors
    "BASH_ENV",
    "ENV",
    "PS4",
    "PROMPT_COMMAND",
    "SHELLOPTS",
    "BASHOPTS",
    // word splitting
    "IFS",
    // interpreter option injection
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "NODE_OPTIONS",
    "PERL5OPT",
    "RUBYOPT",
];

/// Names whose VALUE is executed, or which select the executable that will
/// be. Context cannot set these on either transport — see
/// [`is_reserved_context_key`], which refuses them outright rather than
/// merely skipping the env mirror.
///
/// The Git/SSH entries are the command-running subset of
/// `install::clone::GIT_SCRUBBED_ENV`, plus the two named in
/// `amplihack-hooks`' tmux guard ("`GIT_EXTERNAL_DIFF=…`, `GIT_SSH_COMMAND=…`
/// and the like run their value"). Both places already classify these as
/// code execution; this is the same list applied to recipe context, which is
/// untrusted in exactly the same way.
const EXECUTABLE_SELECTOR_ENV: &[&str] = &[
    // The recipe runner this run and every nested `amplihack` execs, and
    // (issue #2582) the switch deciding whether context lands on argv.
    "RECIPE_RUNNER_RS_PATH",
    // Git and SSH names whose value is run as a command.
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_PROXY_COMMAND",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_EXTERNAL_DIFF",
    "GIT_SEQUENCE_EDITOR",
    "GIT_EDITOR",
    "GIT_PAGER",
];

/// `true` when `name` is a valid POSIX environment-variable identifier,
/// i.e. it matches `^[A-Z_][A-Z0-9_]*$`. The transform in
/// [`context_env_pairs`] uppercases the key first, so only uppercase
/// ASCII letters, digits and underscores are expected here; anything else
/// (hyphens, dots, spaces, a leading digit, non-ASCII, empty) is rejected.
fn is_valid_env_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `true` when `key` (the ORIGINAL, lowercased context key — checked before
/// uppercasing) names an ESSENTIAL context variable that bash steps reference
/// under `set -u` and that must therefore always be mirrored into the env,
/// regardless of the aggregate budget (issue #1023). The set is small and
/// known-required: `task_description`, `repo_path`, `existing_branch`, and any
/// `should_*` flag.
fn is_essential_context_key(key: &str) -> bool {
    matches!(key, "task_description" | "repo_path" | "existing_branch")
        || key.starts_with("should_")
}

/// Byte cost charged to a single exported env entry for budget accounting:
/// `name.len() + value.len() + ENV_ENTRY_OVERHEAD_BYTES`.
fn env_entry_bytes(name: &str, value: &str) -> usize {
    name.len() + value.len() + ENV_ENTRY_OVERHEAD_BYTES
}

/// Context cannot select an amplihack profile, restore Git routing and
/// executable configuration that the command policy removed before launch,
/// or name a program that this run (or anything it spawns) will execute.
///
/// Issue #2582: `pub(super)` so `context_transport::refuse_reserved_keys`
/// applies the same predicate before it writes a context file, rather than
/// carrying a second copy of the rule that could drift from this one.
///
/// [`EXECUTABLE_SELECTOR_ENV`] names are reserved here rather than only in
/// [`RESERVED_ENV_DENYLIST`] because they are *executable selectors*, not
/// merely dangerous env names: their value is run, or chooses what is run.
/// Context is untrusted input — issue bodies and third-party recipes flow
/// into it — so these must be refused by the same rule BOTH transports
/// consult, not dropped by the env filter alone. Dropping is wrong here: the
/// `--set` path would still hand the pair to the runner for `{{placeholder}}`
/// substitution even after the env mirror skipped it.
///
/// `*_BINARY_PATH` is a suffix rule, not a list, because
/// `amplihack_utils::launch_target` resolves `{TOOL}_BINARY_PATH` for ANY
/// tool name as an explicit, user-supplied override at the highest
/// precedence. Enumerating today's tools would leave the next one open.
pub(super) fn is_reserved_context_key(key: &str) -> bool {
    let name = key.to_ascii_uppercase();
    name.starts_with("AMPLIHACK_")
        || amplihack_git::REPOSITORY_ENV_VARS.contains(&name.as_str())
        || name.starts_with("GIT_CONFIG")
        || name.ends_with("_BINARY_PATH")
        || EXECUTABLE_SELECTOR_ENV.contains(&name.as_str())
        || matches!(name.as_str(), "GIT_EXEC_PATH" | "GIT_TEMPLATE_DIR")
}

/// Validate a single recipe context entry for env export (issue #784 / #1023),
/// independent of any budget accounting. Returns the uppercased env name on
/// success, or a static, name-only skip `reason` on rejection. The filters run
/// in the fixed order documented on [`context_env_pairs`].
fn validate_context_entry(key: &str, value: &str) -> Result<String, &'static str> {
    let name = key.to_ascii_uppercase();
    if !is_valid_env_identifier(&name) {
        return Err("invalid_identifier");
    }
    if is_reserved_context_key(key) || RESERVED_ENV_DENYLIST.contains(&name.as_str()) {
        return Err("reserved_name");
    }
    if value.contains('\0') {
        return Err("value_contains_nul");
    }
    if value.len() > CONTEXT_ENV_VALUE_MAX_BYTES {
        return Err("value_too_large");
    }
    Ok(name)
}

/// Aggregate byte budget available for the context env mirror (issue #1023).
///
/// Pure and injectable so unit tests can supply values without depending on the
/// host's real ARG_MAX. Returns `arg_max` minus the bytes already consumed by
/// the inherited (pass-through) environment minus a reservation for argv and
/// runner-added variables, saturating to `0` so an over-subscribed budget can
/// never underflow into an enormous bogus value.
pub(super) fn context_env_budget(
    arg_max: usize,
    inherited_env_bytes: usize,
    reservation: usize,
) -> usize {
    arg_max.saturating_sub(inherited_env_bytes.saturating_add(reservation))
}

/// Best-effort kernel argv+envp limit in bytes. Queries
/// `sysconf(_SC_ARG_MAX)` on Unix; falls back to [`ARG_MAX_FALLBACK_BYTES`]
/// when the value is unavailable, non-positive, or implausibly small.
fn current_arg_max() -> usize {
    #[cfg(unix)]
    {
        // SAFETY: `sysconf` is a pure query with no memory-safety obligations.
        let raw = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
        if raw > 0 && (raw as u64) >= ARG_MAX_MIN_PLAUSIBLE_BYTES as u64 {
            return raw as usize;
        }
    }
    ARG_MAX_FALLBACK_BYTES
}

/// The runner's file-first context for the parent step (issue #1570).
const CONTEXT_FILE_ENV: &str = "AMPLIHACK_CONTEXT_FILE";

/// Prefix of the runner's per-step context variables.
const RECIPE_VAR_PREFIX: &str = "RECIPE_VAR_";

/// Issue #1570: `true` when the inherited variable `name` is removed from the
/// runner command, because this run passes its own context explicitly and a
/// parent's context would otherwise accumulate across nesting levels.
///
/// Removed: every `RECIPE_VAR_<key>`, the uppercase alias of each inherited
/// `RECIPE_VAR_<key>` whose `<key>` has no `__`, and `AMPLIHACK_CONTEXT_FILE`.
/// Never removed: [`RESERVED_ENV_DENYLIST`] names, other `AMPLIHACK_*` names
/// and names that are not valid environment identifiers.
/// `inherited_names` is the set of every inherited variable name.
pub(super) fn is_stripped_inherited_name(name: &str, inherited_names: &BTreeSet<String>) -> bool {
    if name == CONTEXT_FILE_ENV || name.starts_with(RECIPE_VAR_PREFIX) {
        return true;
    }
    if !is_valid_env_identifier(name)
        || RESERVED_ENV_DENYLIST.contains(&name)
        || name.starts_with("AMPLIHACK_")
    {
        return false;
    }
    inherited_names
        .iter()
        .filter_map(|n| n.strip_prefix(RECIPE_VAR_PREFIX))
        .any(|key| !key.contains("__") && key.eq_ignore_ascii_case(name))
}

/// Names of the current process environment that are valid UTF-8. A name
/// that is not UTF-8 is never stripped.
fn inherited_env_names() -> BTreeSet<String> {
    utf8_env_names(&std::env::vars_os().collect::<Vec<_>>())
}

fn utf8_env_names(vars: &[(OsString, OsString)]) -> BTreeSet<String> {
    vars.iter()
        .filter_map(|(name, _)| name.to_str().map(str::to_owned))
        .collect()
}

/// Total bytes the current process environment will consume as it passes
/// THROUGH to the spawned recipe-runner (and is re-exported to every bash
/// step). See [`inherited_env_bytes_from`].
fn inherited_env_bytes() -> usize {
    inherited_env_bytes_from(std::env::vars_os())
}

/// Bytes `vars` will consume in the runner's environment, summed with the
/// same per-entry accounting used for the mirror. Names removed by
/// [`is_stripped_inherited_name`] are not counted.
pub(super) fn inherited_env_bytes_from<I>(vars: I) -> usize
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let vars: Vec<_> = vars.into_iter().collect();
    let names = utf8_env_names(&vars);
    vars.iter()
        .filter(|(name, _)| {
            !name
                .to_str()
                .is_some_and(|name| is_stripped_inherited_name(name, &names))
        })
        .map(|(name, value)| name.len() + value.len() + ENV_ENTRY_OVERHEAD_BYTES)
        .sum()
}

/// Resolve the aggregate context-env budget at runtime (issue #1023).
///
/// Precedence:
/// 1. A valid [`CONTEXT_ENV_BUDGET_OVERRIDE_ENV`] override (`usize`; `0` valid)
///    wins verbatim. Invalid values are logged (name-only) and ignored.
/// 2. Otherwise derive it from the kernel ARG_MAX minus the inherited env minus
///    the fixed reservation via [`context_env_budget`].
pub(super) fn resolve_context_env_budget() -> usize {
    if let Some(raw) = std::env::var_os(CONTEXT_ENV_BUDGET_OVERRIDE_ENV) {
        match raw
            .to_str()
            .map(str::trim)
            .and_then(|s| s.parse::<usize>().ok())
        {
            Some(value) => return value,
            None => tracing::warn!(
                env = %CONTEXT_ENV_BUDGET_OVERRIDE_ENV,
                reason = %"invalid_env_budget_override",
                "operator env-budget override is not a valid non-negative integer; \
                 using derived budget"
            ),
        }
    }
    context_env_budget(
        current_arg_max(),
        inherited_env_bytes(),
        ENV_BUDGET_RESERVATION_BYTES,
    )
}

/// Deterministically map recipe context entries to environment variables for
/// the spawned recipe runner (and, by OS inheritance, every bash step and
/// nested sub-recipe it runs). This is the fix for issue #784 / #4583: bash
/// steps under `set -u` reference `$TASK_DESCRIPTION` / `$REPO_PATH`, which
/// must exist in the environment rather than only being substituted into
/// `{{placeholder}}` text.
///
/// Per-key filters (pure, total — invalid entries are skipped, never fatal),
/// applied BEFORE any budget accounting:
/// 1. Uppercase the key (`task_description` → `TASK_DESCRIPTION`).
/// 2. Drop keys that are not valid env identifiers after uppercasing
///    (empty, leading digit, hyphen/dot/space, non-ASCII).
/// 3. Drop keys in the `AMPLIHACK_` namespace (owned by `EnvBuilder`).
/// 4. Drop reserved/dangerous names ([`RESERVED_ENV_DENYLIST`]).
/// 5. Drop values containing a NUL byte (rejected by the OS for env vars;
///    would otherwise panic at spawn time).
/// 6. Drop values larger than [`CONTEXT_ENV_VALUE_MAX_BYTES`] (would risk
///    `E2BIG`); they still reach the runner through the chosen context
///    transport — the context file since issue #2582, or `--set` arguments
///    for a runner older than the pin.
///
/// Aggregate budget (issue #1023): the total bytes of the exported mirror must
/// stay within `budget` so the environment inherited by every bash step cannot
/// cross the kernel's ARG_MAX. Surviving entries are partitioned into ESSENTIAL
/// keys (see [`is_essential_context_key`]) — always exported, even if that
/// alone exceeds the budget (a loud warning is logged) — and non-essential
/// keys, which fill the remaining budget SMALLEST-FIRST so the maximum number
/// of useful vars survive. A non-essential entry that does not fit is dropped
/// from the mirror ONLY; it is still delivered to the runner via
/// `--set` (see [`pass_context`]) for `{{placeholder}}`
/// substitution (same contract as the per-value cap at #784).
///
/// Skipped entries are logged name-only at `warn` level — values may carry
/// sensitive data and are never logged.
pub(super) fn context_env_pairs(
    context: &BTreeMap<String, String>,
    budget: usize,
) -> Vec<(String, String)> {
    // Borrow values during classification/sorting so entries that never make
    // the mirror (validation-rejected, or non-essentials that overflow the
    // budget) are not cloned. Values may be up to `CONTEXT_ENV_VALUE_MAX_BYTES`
    // (96 KiB), so cloning only the survivors avoids wasted allocation.
    let mut essential: Vec<(String, &str)> = Vec::new();
    let mut optional: Vec<(String, &str)> = Vec::new();
    for (key, value) in context {
        match validate_context_entry(key, value) {
            Ok(name) if is_essential_context_key(key) => essential.push((name, value.as_str())),
            Ok(name) => optional.push((name, value.as_str())),
            Err(reason) => tracing::warn!(
                name = %key,
                reason = %reason,
                "recipe context key skipped for env export"
            ),
        }
    }

    let mut pairs = Vec::with_capacity(essential.len() + optional.len());
    let mut used: usize = 0;

    // Essential keys are always exported, even if they alone exceed the budget.
    for (name, value) in essential {
        used = used.saturating_add(env_entry_bytes(&name, value));
        pairs.push((name, value.to_owned()));
    }
    if used > budget {
        tracing::warn!(
            reason = %"essential_env_exceeds_budget",
            used_bytes = used,
            budget_bytes = budget,
            "essential recipe context exceeds the aggregate env budget; \
             exporting essentials anyway and dropping all non-essential keys"
        );
    }

    // Non-essential keys fill the remaining budget smallest-first.
    optional.sort_by(|a, b| {
        env_entry_bytes(&a.0, a.1)
            .cmp(&env_entry_bytes(&b.0, b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    for (name, value) in optional {
        let cost = env_entry_bytes(&name, value);
        if used.saturating_add(cost) <= budget {
            used = used.saturating_add(cost);
            pairs.push((name, value.to_owned()));
        } else {
            tracing::warn!(
                name = %name,
                reason = %"aggregate_env_budget",
                "recipe context key skipped for env export"
            );
        }
    }
    pairs
}

/// #2329: the private auto-drive state directory for one invocation.
///
/// Resolution order: the operator's explicit `autodrive_state_dir` (already
/// validated by `InvocationAuthorization::capture`), then `autodrive-state`
/// in this run's durable folder, then `autodrive-state` in the dry run's
/// private launch copy. The directory is created 0700, or an existing one is
/// checked for a non-symlink leaf, ownership and private modes; it is never
/// chmodded to pass. The returned path is the absolute path before
/// canonicalisation: the runner binds it, passes the same string to the child
/// and the report compares against it lexically.
fn assign_autodrive_state_dir(
    explicit_inputs: &BTreeMap<String, String>,
    workspace: &crate::commands::cleanup::run_dir::RunWorkspace,
    launch_home: &Path,
) -> Result<PathBuf> {
    let directory = match explicit_inputs.get("autodrive_state_dir") {
        Some(value) => PathBuf::from(value),
        None => match workspace.durable_run_dir()? {
            Some(run_dir) => run_dir.join("autodrive-state"),
            None => launch_home.join("autodrive-state"),
        },
    };
    anyhow::ensure!(
        directory.is_absolute(),
        "autodrive_state_dir must be an absolute path"
    );
    anyhow::ensure!(
        directory.to_str().is_some(),
        "auto-drive state directory {directory:?} is not valid UTF-8"
    );
    crate::commands::orch::autodrive::state::EvidenceStore::create(&directory)
        .with_context(|| format!("cannot prepare auto-drive state directory {directory:?}"))?;
    Ok(directory)
}

// The resolved agent binary is an extra argument on purpose: the caller
// decided it once, for the pre-flight and the steps alike (issue #1481).
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    explicit_inputs: &BTreeMap<String, String>,
    dry_run: bool,
    verbose: bool,
    working_dir: &Path,
    search_dirs: &[PathBuf],
    step_timeout: Option<u64>,
    agent_binary: amplihack_utils::agent_binary::Resolution,
) -> Result<RecipeRunResult> {
    execute_recipe_via_rust_fresh(
        recipe_path,
        context,
        explicit_inputs,
        dry_run,
        verbose,
        working_dir,
        search_dirs,
        step_timeout,
        agent_binary,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_recipe_via_rust_fresh(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    explicit_inputs: &BTreeMap<String, String>,
    dry_run: bool,
    verbose: bool,
    working_dir: &Path,
    search_dirs: &[PathBuf],
    step_timeout: Option<u64>,
    agent_binary: amplihack_utils::agent_binary::Resolution,
    fresh: bool,
) -> Result<RecipeRunResult> {
    use crate::commands::orch::autodrive::{authorization::InvocationAuthorization, trusted};
    let invocation_name = recipe_name_for_correlation(recipe_path);
    // #2329: authority is captured once, from the raw operator inputs of this
    // invocation, before defaults, recovery normalisation or admission. A
    // --fresh launch or a recovery resume captures new authority (and a new
    // run_id) like any other invocation; stored context never grants it.
    let mut authorization = trusted::applies(&invocation_name)
        .then(|| InvocationAuthorization::capture(&invocation_name, explicit_inputs))
        .transpose()?;
    if dry_run && let Some(auth) = &mut authorization {
        auth.restrict_dry_run();
    }
    // Issue #964: fail-closed recursion-depth guard. Refuse to spawn a nested
    // recipe-runner once the session has reached the configured maximum depth,
    // so a failing / misbehaving orchestration can never recursively re-enter
    // the orchestrator and fork-bomb the host. Runs BEFORE any work (binary
    // lookup, temp dirs, bundle copies, spawn) so no descendant is ever
    // created past the limit.
    let guard = enforce_recursion_depth_guard(working_dir)?;
    let launch = if let Some(auth) = &mut authorization {
        let env = EnvBuilder::new().with_amplihack_home_from(working_dir);
        let home = env
            .var("AMPLIHACK_HOME")
            .context("trusted AMPLIHACK_HOME missing for auto-drive")?;
        Some(trusted::LaunchBundle::prepare(Path::new(home), auth)?)
    } else {
        None
    };
    let pinned_recipe = launch.as_ref().map(|l| {
        l.home
            .join("amplifier-bundle/recipes")
            .join(format!("{invocation_name}.yaml"))
    });
    let recipe_path = pinned_recipe.as_deref().unwrap_or(recipe_path);
    let pinned_search = launch
        .as_ref()
        .map(|l| vec![l.home.join("amplifier-bundle/recipes")]);
    let search_dirs = pinned_search.as_deref().unwrap_or(search_dirs);
    // Expand auto-drive defaults only after trusted entrypoint selection.
    // Neither requested recipe defaults nor inferred values grant authority.
    let (mut invocation_context, inferred) = if launch.is_some() {
        let recipe = super::parse_recipe_from_path(recipe_path)?;
        let (mut expanded, inferred) =
            super::infer_missing_context(&recipe.context, explicit_inputs);
        // Recovery identity is normalised on this derived copy only, never on
        // `explicit_inputs`, which stays the raw source of authority.
        super::recovery::normalize(&mut expanded, working_dir);
        (expanded, inferred)
    } else {
        (context.clone(), Vec::new())
    };
    if verbose && !inferred.is_empty() {
        eprintln!(
            "[context] Inferred {} variable(s): {}",
            inferred.len(),
            inferred.join(", ")
        );
    }
    // Issue #1606: from here on a SIGHUP/SIGINT/SIGTERM stops the run, at any
    // point: setup, a running attempt, or retry backoff.
    super::stop_signal::arm();
    // Issue #1976: a nested run stops when its parent `recipe run` exits. A
    // parent already gone sets the stop here, before the runner is spawned.
    super::parent_watch::start();

    let binary = super::binary::find_recipe_runner_binary()?
        .canonicalize()
        .context("resolve recipe runner before changing its launch directory")?;
    let recipe_name = recipe_name_for_correlation(recipe_path);
    // Issue #2582: the report is kept, so the context transport can check
    // for `context_file_v1` without a second probe.
    let runner_report = match crate::runner_validation::probe(&binary, &agent_binary.binary) {
        Ok(report) => report,
        Err(error) => {
            let correlation = RecipeRunCorrelation::new(
                recipe_name.clone(),
                working_dir,
                &BTreeMap::new(),
                binary.as_path(),
            );
            correlation.emit_early();
            eprintln!("amplihack: runner capability check failed: {error}");
            return Err(fail_with_final_record(
                &correlation,
                RecipeRunFinalStatus::SpawnFailure,
                None,
                None,
                error,
            ));
        }
    };

    // Issue #1481: the caller resolved the agent binary once, from
    // `working_dir`, while this process could still see the session markers of
    // the CLI that invoked it; the #1482 pre-flight checked that same answer.
    // Every step below runs under recipe-runner-rs's curated environment, and a
    // nested `amplihack` resolving on its own there has lost the evidence -- it
    // fell through to the vendor default and ran every agent step under Copilot
    // from inside a Claude Code session.
    crate::agent_binary_notice::report_agent_binary(
        crate::agent_binary_notice::Reporter::RecipeRun,
        &agent_binary,
    );

    // Issue #1606: record what this run resolved in every log pointer. The
    // home is resolved the same way `build_command` resolves it below.
    let run_settings = RunSettings {
        agent_binary: Some(agent_binary.binary.clone()),
        agent_binary_source: Some(agent_binary.source.label()),
        amplihack_home: EnvBuilder::new()
            .with_amplihack_home_from(working_dir)
            .build()
            .remove("AMPLIHACK_HOME"),
    };

    // Issue #1808: the run's folder under the work root holds run.json and
    // owned-paths; its runtime/ (TMPDIR, TMP, TEMP and artifacts) is removed
    // when the run ends, on every exit path including a panic.
    let workspace = RefCell::new(if dry_run {
        crate::commands::cleanup::run_dir::RunWorkspace::dry_runtime()?
    } else {
        crate::commands::cleanup::run_dir::RunWorkspace::begin_durable(&recipe_name, working_dir)?
    });
    let delegated = recipe_runner_rs::recovery::ipc::Client::from_environment()?;
    let admission = if !dry_run && delegated.is_none() {
        // #2329: whether auto-drive authority applies is part of the resume
        // settings, so a resume cannot switch it. No per-run value (run_id,
        // state directory) enters the fingerprint. This protects resume
        // consistency only; it is not an authority check.
        let settings = recipe_runner_rs::recovery::model::fingerprint(&(
            &agent_binary.binary,
            step_timeout,
            authorization.is_some(),
            env!("CARGO_PKG_VERSION"),
        ))?;
        Some(super::recovery::admit(
            &workspace.borrow(),
            recipe_path,
            &recipe_name,
            working_dir,
            &invocation_context,
            fresh,
            settings,
        )?)
    } else {
        None
    };
    // #2329 (S2): the runner writes its own keys last, after admission and
    // immediately before launch, so no stored, recovered or operator-supplied
    // value for them reaches the child. The private state directory is
    // assigned and bound here too: the report opens evidence only from it.
    if let Some(auth) = &mut authorization {
        let launch = launch
            .as_ref()
            .context("trusted launch bundle missing for auto-drive")?;
        let state_dir =
            assign_autodrive_state_dir(explicit_inputs, &workspace.borrow(), &launch.home)?;
        auth.bind_state_dir(state_dir.clone());
        launch.record_state_dir(auth)?;
        invocation_context.insert("no_merge".into(), auth.no_merge().to_string());
        invocation_context.insert("autodrive_dry_run".into(), auth.dry_run().to_string());
        invocation_context.insert(
            "autodrive_authorization".into(),
            serde_json::to_string(auth)?,
        );
        invocation_context.insert(
            "autodrive_invocation_file".into(),
            launch
                .home
                .join("invocation.json")
                .to_string_lossy()
                .into_owned(),
        );
        invocation_context.insert(
            "autodrive_state_dir".into(),
            state_dir.to_string_lossy().into_owned(),
        );
    }
    let authorization = authorization;
    let context = &invocation_context;

    // Prepare private Git metadata outside disposable runtime/ before any
    // executable step. An unborn or unsupported caller state rejects launch.
    let scratch = workspace.borrow().runtime_dir().to_path_buf();
    // A run with a recovery identity (its own admission, or a nested run under
    // its parent's delegated recovery) prepares each attempt's fresh checkout
    // at a path derived from that identity, so a relaunch presents the same
    // repository path and replay can reuse checkout-dependent steps (#2493).
    let recovering = admission.is_some() || delegated.is_some();
    let stable_path = match (recovering, workspace.borrow().work_root_path()) {
        (true, Some(root)) => {
            super::recovery::identity(recipe_path, &recipe_name, working_dir, context).map(
                |identity| {
                    root.join(crate::commands::cleanup::work_root::EXEC_DIR)
                        .join(&identity[..identity.len().min(32)])
                },
            )
        }
        _ => None,
    };
    let owner_run_dir = workspace.borrow().run_dir().map(Path::to_path_buf);
    let stable_storage = match (&stable_path, &owner_run_dir) {
        (Some(path), Some(owner_run_dir)) => Some(super::run_checkout::StableStorage {
            path,
            owner_run_dir,
            owner_ended: crate::commands::cleanup::eligibility::run_has_ended,
        }),
        _ => None,
    };
    let mut checkout = match super::run_checkout::RunCheckout::prepare_with_inputs(
        working_dir,
        &scratch,
        dry_run,
        &[recipe_path.to_path_buf()],
        stable_storage,
    ) {
        Ok(checkout) => checkout,
        Err(error) => {
            return fail_before_execution(
                workspace.into_inner(),
                admission.as_ref(),
                error,
                dry_run,
            );
        }
    };
    let exec_dir = checkout.exec_dir().to_path_buf();
    // The recovery descriptor lives in the run folder; one inside the private
    // checkout would be state a step could rewrite (S-AZ4). A run outside Git
    // executes in the caller's own directory, which may hold the work root,
    // so only a private checkout is checked.
    let descriptor_outside_checkout = || -> Result<()> {
        if let Some(admission) = &admission
            && matches!(checkout, super::run_checkout::RunCheckout::Private { .. })
        {
            refuse_descriptor_inside(&admission.descriptor_path(), &exec_dir)?;
        }
        Ok(())
    };
    let mapped = descriptor_outside_checkout().and_then(|()| {
        checkout.map_path(recipe_path).and_then(|recipe| {
            checkout
                .map_context(context)
                .map(|context| (recipe, context))
        })
    });
    let (mapped_recipe, mapped_context) = match mapped {
        Ok(mapped) => mapped,
        Err(mut error) => {
            if let Err(release) = checkout.remove() {
                error = error.context(format!("execution release also failed: {release:#}"));
            }
            return fail_before_execution(
                workspace.into_inner(),
                admission.as_ref(),
                error,
                dry_run,
            );
        }
    };
    let context = &mapped_context;

    // Issue #2582: when the runner reports `context_file_v1`, the context
    // goes to it in a 0600 file under the run's private runtime folder and
    // argv holds only `--context-file <path>`; otherwise the `--set` path
    // runs, which only a runner older than the pin supplied through
    // `RECIPE_RUNNER_RS_PATH` takes. Written once and reused by every retry.
    //
    // It is chosen here, after `map_context`, so the file holds the same
    // values the runner would have been given as `--set` pairs: the mapped
    // paths, not the caller's. It is also after the auto-drive authorization
    // keys are written, so the file carries what that step produced.
    //
    // A failed write (or a reserved key, normally refused earlier at intake)
    // ends the run as a failure and never falls back to `--set`. The checkout
    // is released first, as on every other preparation error here.
    // Owned before the match: a `workspace.borrow()` temporary in the
    // scrutinee would still be live in the error arm, where the workspace is
    // moved into `fail_before_execution`.
    let context_runtime_dir = workspace.borrow().runtime_dir().to_path_buf();
    let transport = match super::context_transport::ContextTransport::choose(
        &runner_report,
        &context_runtime_dir,
        context,
    ) {
        Ok(transport) => transport,
        Err(mut error) => {
            if let Err(release) = checkout.remove() {
                error = error.context(format!("execution release also failed: {release:#}"));
            }
            return fail_before_execution(
                workspace.into_inner(),
                admission.as_ref(),
                error,
                dry_run,
            );
        }
    };
    transport.record();

    // Issue #1267: a `std::process::Command` is consumed by `spawn`, and every
    // attempt needs its own correlation run id, so command construction is a
    // closure invoked once per attempt rather than hoisted state. The runtime
    // dir path is deliberately NOT rebuilt: a retry of a transient transport
    // fault should reuse the same artifact/tmp scratch space it was already
    // given. Issue #2486: its tmp/ and artifacts/ are re-created there if
    // missing, so a step never starts with a TMPDIR that no longer exists.
    // Issue #2535: the guidance folders this run's agent steps read, computed
    // once: this run's folder, the per-PR folder and the inherited list.
    let guidance_dirs = crate::run_guidance::env::runner_env_value(
        std::env::var_os(crate::run_guidance::GUIDANCE_DIRS_ENV).as_ref(),
        workspace.borrow().run_dir(),
        context,
        working_dir,
        crate::run_guidance::pr_key::state_root()
            .as_deref()
            .map_err(|e| *e),
    );
    // Issue #1619: the run summary's `Model tiers:` section reads the records
    // this run's agent steps write into the run directory. A top-level
    // `recipe run` hands AMPLIHACK_RUN_DIR to its children without setting it
    // on itself, so the directory is named here rather than read back from
    // the environment.
    if let Some(run_dir) = workspace.borrow().run_dir() {
        super::format::remember_run_dir(run_dir);
    }
    // Issue #1619: an instance may give one named step a tier of its own. The
    // step id never reaches the child, so the override is applied recipe-side:
    // patched copies of only the recipes that declare a named step, written
    // into this run's own directory and handed to the runner as its first
    // `-R`. With nothing configured nothing is staged, no argument is added
    // and the run behaves exactly as it did before. A staging failure is
    // reported and the run continues on the recipes as written: a tier
    // preference must never stop a run.
    let staged_recipes =
        stage_step_tier_overrides(workspace.borrow().run_dir(), search_dirs, &mapped_recipe);
    let build_command = |run_id: &str| -> Result<Command> {
        // Issue #1567: no `env_clear` here, so a pinned AMPLIHACK_CLAUDE_PROFILE
        // (and AMPLIHACK_CLAUDE_PROFILE_ROOT) reaches the runner. The runner's
        // own `bounded_env` clears the environment for agent steps but always
        // keeps AMPLIHACK_* variables, which is why only the profile name (not
        // CLAUDE_CONFIG_DIR) needs to survive: each step's nested
        // `amplihack claude` rebuilds the dir from it. `pass_context` refuses
        // `AMPLIHACK_*` keys from `-c` and this run's context. The runner still
        // exports top-level context keys under their uppercase names
        // (`legacy_uppercase_alias`), so a recipe's own YAML `context:`
        // defaults or a step `output:` named `amplihack_claude_profile` can
        // still override the pinned value until the runner refuses that
        // namespace too (rysweet/amplihack-recipe-runner#146).
        let mut command = Command::new(&binary);
        // Issue #1619: a `-R` directory is searched for a name, and the
        // top-level recipe is passed as a path, so `-R` alone never reaches
        // it. When staging patched that recipe, the positional argument is
        // its staged copy instead — the only route by which a single-step
        // override covers a step of the recipe named on the command line.
        let staged_top_level = staged_recipes
            .as_ref()
            .and_then(|staged| staged.top_level.as_ref());
        let recipe_arg = match staged_top_level {
            Some(path) => checkout.map_path(path)?,
            None => mapped_recipe.clone(),
        };
        command
            .current_dir(&exec_dir)
            .arg(&recipe_arg)
            .arg("--output-format")
            .arg("json")
            .arg("-C")
            .arg(&exec_dir);
        // Issue #2096: no inherited GIT_DIR, GIT_INDEX_FILE or git config
        // override may send the run's git commands back to the caller.
        checkout.apply_to(&mut command);

        if fresh {
            command.arg("--fresh");
        }
        if let Some(admission) = &admission {
            command
                .arg("--recovery-descriptor")
                .arg(admission.descriptor_path());
        }
        // Issue #1619: the staged copies go ahead of every other -R. The
        // runner resolves a sub-recipe name from its -R list first, in the
        // order given, and amplihack forwards the bundle below, so anything
        // offered after it is outranked by the recipe it was patched from.
        let mut recipe_dir_args: Vec<PathBuf> = Vec::new();
        if let Some(staged) = &staged_recipes {
            recipe_dir_args.push(checkout.map_path(&staged.dir)?);
        }
        // Issue #494: forward sub-recipe search dirs as -R flags so
        // recipe-runner-rs can resolve sub-recipes the same way amplihack
        // resolves top-level recipes. One -R per non-empty entry, in order.
        for dir in search_dirs {
            if dir.as_os_str().is_empty() {
                continue;
            }
            recipe_dir_args.push(checkout.map_path(dir)?);
            tracing::debug!(dir = %dir.display(), "forwarding -R to recipe-runner-rs");
        }
        // Issue #1619: substituting the positional argument moves the
        // runner's anchored search — its `recipe_origin_dir` — to the staging
        // directory, so a recipe that resolves a sibling sub-recipe by name
        // would stop finding it. The original recipe's own directory goes
        // back on the end of the list rather than the front: appended, it
        // cannot shadow anything that already ranked above it, so a
        // repository-local file still does not displace a bundled one.
        if staged_top_level.is_some()
            && let Some(parent) = mapped_recipe.parent()
            && !parent.as_os_str().is_empty()
        {
            let parent = checkout.map_path(parent)?;
            if !recipe_dir_args.contains(&parent) {
                recipe_dir_args.push(parent);
            }
        }
        for dir in &recipe_dir_args {
            command.arg("-R").arg(dir);
        }

        if dry_run {
            command.arg("--dry-run");
        }

        // Issue #1570 / #2582: context goes to the runner as `--context-file
        // <path>` or, for a runner older than the pin, as `--set` arguments.
        // Whichever it is, the argv bytes it actually costs come out of the
        // env-mirror budget below, so argv + envp stay under ARG_MAX
        // together. The file transport costs the flag and the path, which is
        // why `apply` returns a real count rather than zero.
        let context_argv_bytes = transport.apply(&mut command, context)?;
        let argv_room = current_arg_max().saturating_sub(inherited_env_bytes());
        if context_argv_bytes > argv_room.saturating_sub(ENV_BUDGET_RESERVATION_BYTES) {
            anyhow::bail!(
                "recipe context totals {context_argv_bytes} bytes of {} arguments, more than \
                 the {argv_room} bytes of ARG_MAX left after the inherited environment. Pass \
                 large values as file paths in the context instead",
                transport.name()
            );
        }

        // Issue #1570: remove inherited recipe context before adding this
        // run's own, so a nested run does not carry every ancestor's
        // `RECIPE_VAR_*`, uppercase aliases and context file.
        let inherited_names = inherited_env_names();
        for name in &inherited_names {
            if is_stripped_inherited_name(name, &inherited_names) {
                command.env_remove(name);
            }
        }

        // Issue #784 / #4583: export recipe context as environment variables so
        // bash steps (and nested sub-recipes, via OS inheritance) can read
        // $TASK_DESCRIPTION / $REPO_PATH under `set -u`. Applied at the LOWEST
        // precedence — written BEFORE EnvBuilder and the run-id below — so every
        // amplihack-managed/protective variable deterministically wins over any
        // colliding context key. Reserved/dangerous names are dropped upstream in
        // `context_env_pairs` (they are not EnvBuilder-managed). The aggregate byte
        // budget (#1023) keeps the total mirrored env under the kernel's ARG_MAX so
        // late bash steps cannot fail with "Argument list too long".
        command.envs(context_env_pairs(
            context,
            resolve_context_env_budget().saturating_sub(context_argv_bytes),
        ));

        let env_builder = EnvBuilder::new()
            .with_resolved_agent_binary(agent_binary.binary.as_str(), agent_binary.source)
            .with_session_tree_context()
            .with_amplihack_home_from(working_dir)
            .with_asset_resolver()
            .with_pager_safe_defaults()
            .with_python_sanitization()
            .unset("CLAUDECODE")
            .set("AMPLIHACK_NONINTERACTIVE", "1")
            .with_project_graph_db(working_dir)?;

        let env_builder = match &launch {
            Some(launch) => {
                let executable = std::env::current_exe()
                    .context("resolve current auto-drive helper executable")?;
                env_builder
                    .prepend_path(
                        executable
                            .parent()
                            .context("auto-drive helper executable has no parent")?,
                    )
                    .set("AMPLIHACK_BIN", executable.to_string_lossy().into_owned())
                    .set("AMPLIHACK_HOME", launch.home.to_string_lossy().into_owned())
            }
            None => env_builder,
        };
        // Issue #439: propagate --step-timeout as AMPLIHACK_STEP_TIMEOUT env var.
        // When Some(n), the child process sees AMPLIHACK_STEP_TIMEOUT=n (0 = disable).
        // When None, the env var is not injected (parent-inherited values flow through).
        let env_builder = match step_timeout {
            Some(seconds) => env_builder.set("AMPLIHACK_STEP_TIMEOUT", seconds.to_string()),
            None => env_builder,
        };

        env_builder.apply_to_command(&mut command);
        crate::commands::orch::autodrive::trusted::clean_retired_environment(&mut command)?;
        // Issue #1326: pin the session-tree directory for every descendant. Without
        // this each level re-derives it, and the previous derivation was based on
        // TMPDIR -- which we replace below with a fresh per-run tempdir, giving every
        // level its own empty tree and a session cap that counts nothing.
        if let Ok(dir) = crate::commands::session_tree::state::state_dir() {
            command.env(SESSION_TREE_DIR_ENV, dir);
        }
        // Issue #1326: the runner owns tree identity and depth. Handing them to the
        // child explicitly is what makes the sealed ceiling reachable one level down;
        // previously nothing seeded AMPLIHACK_TREE_ID at a root, so `sealed` was None
        // for the whole chain and the environment won by default.
        command.env(TREE_ID_ENV, &guard.tree_id);
        command.env(SESSION_DEPTH_ENV, guard.child_depth.to_string());
        command.env(MAX_DEPTH_ENV, guard.max_depth.to_string());
        command.env("AMPLIHACK_RECIPE_RUN_ID", run_id);
        // Issue #2486: this run's own TMPDIR must exist before the step starts.
        workspace.borrow_mut().prepare_attempt()?;
        // Must stay last among the TMPDIR/TMP/TEMP setters: it overrides the
        // inherited environment and the recipe context, so a step never gets
        // another run's (or a removed) TMPDIR.
        command.envs(workspace.borrow().env());
        if let Some(dirs) = &guidance_dirs {
            command.env(crate::run_guidance::GUIDANCE_DIRS_ENV, dirs);
        }
        if dry_run {
            command.env("AMPLIHACK_RUN_ID", run_id);
        }
        // Issue #1976: name this `recipe run` as the parent of any nested run
        // a step starts, on the child only (never set_var). Without a start
        // time the pair is omitted: a pid alone could be reused.
        match super::parent_watch::own_starttime() {
            Some(starttime) => {
                command.env(
                    super::parent_watch::PARENT_PID_ENV,
                    std::process::id().to_string(),
                );
                command.env(
                    super::parent_watch::PARENT_STARTTIME_ENV,
                    starttime.to_string(),
                );
            }
            None => {
                command.env_remove(super::parent_watch::PARENT_PID_ENV);
                command.env_remove(super::parent_watch::PARENT_STARTTIME_ENV);
            }
        }
        // Issue #1976: only the round's own `recipe run` may write the drive's
        // orphaned.json; nested runs below it must not overwrite it. This
        // level reads the variable from its own environment, so removing it
        // on the child does not affect this run.
        command.env_remove(super::orphan_record::AUTODRIVE_STATE_DIR_ENV);
        // Apply repository selection last so inherited environment builders
        // cannot reintroduce caller routing or configuration injection.
        checkout.apply_to(&mut command);

        Ok(command)
    };

    // Issue #1267: a transient transport fault (529/503/429, reset connection,
    // socket timeout) is retried with bounded backoff instead of unwinding the
    // whole run. Everything else — a failing test, a missing binary, a policy
    // refusal — is terminal here by design, and is handed to the agentic layer
    // with its classification attached. Whether continuing is *worthwhile* is
    // not decided in this file; see `retry.rs` and `loop-health-evaluator.yaml`.
    let limits = TransientRetryLimits::from_env();
    let timeout = recipe_runner_timeout();
    let last_verdict: RefCell<Option<FailureVerdict>> = RefCell::new(None);

    let (result, summary) = run_with_transient_retry(
        &limits,
        |attempt_no| {
            // Issue #1606: never start an attempt after a stop signal, e.g.
            // one that arrived during retry backoff.
            if let Some(signal) = super::stop_signal::pending() {
                // Issue #1976: an orphan stop before any runner started still
                // leaves its final record and orphaned.json.
                // The captured value alone decides the status (C-S6).
                if signal == super::stop_signal::ORPHANED as i32 {
                    let correlation = RecipeRunCorrelation::new(
                        recipe_name.clone(),
                        working_dir,
                        context,
                        binary.as_path(),
                    )
                    .with_settings(run_settings.clone());
                    // The pointer contract is one `early` then one `final`
                    // per run_id, even when no runner is spawned.
                    correlation.emit_early();
                    record_orphan(&correlation, None, &guard.tree_id);
                }
                return AttemptOutcome::Final(Err(anyhow::Error::new(
                    super::stop_signal::StoppedBySignal(signal),
                )));
            }
            let correlation = RecipeRunCorrelation::new(
                recipe_name.clone(),
                working_dir,
                context,
                binary.as_path(),
            )
            .with_settings(run_settings.clone());
            if let Err(error) = workspace
                .borrow_mut()
                .record_attempt_checked(correlation.run_id())
            {
                return AttemptOutcome::Final(Err(error));
            }
            let command = match build_command(correlation.run_id()) {
                Ok(built) => built,
                // Failing to BUILD the invocation (temp file, project graph db)
                // has nothing transport-level about it. Terminal.
                Err(error) => return AttemptOutcome::Final(Err(error)),
            };
            let mut stderr_tail = String::new();
            // Cloned so no RefCell borrow is held across the attempt.
            let run_dir = workspace.borrow().run_dir().map(Path::to_path_buf);
            let attempt_result = spawn_with_streaming_stderr(
                command,
                correlation,
                recipe_path,
                run_dir.as_deref(),
                timeout,
                &guard.tree_id,
                &mut stderr_tail,
            );

            // Structured runner failures and spawn/parse errors both enter
            // the failure-classification path below.
            let failed = match &attempt_result {
                Err(_) => true,
                Ok(run_result) => !run_result.success,
            };
            // Issue #1606: a stop signal is the user's decision, never a
            // transient fault to retry.
            if let Err(error) = &attempt_result
                && error
                    .downcast_ref::<super::stop_signal::StoppedBySignal>()
                    .is_some()
            {
                return AttemptOutcome::Final(attempt_result);
            }
            if !failed {
                return AttemptOutcome::Final(attempt_result);
            }
            let verdict = match &attempt_result {
                Ok(run_result) => classify_run_failure(run_result, &stderr_tail),
                Err(error) => classify_error(error),
            };
            *last_verdict.borrow_mut() = Some(verdict.clone());

            if verdict.class.is_mechanically_retryable() {
                AttemptOutcome::Transient(attempt_result, verdict)
            } else {
                emit_failure_class_marker(&verdict, attempt_no, "terminal");
                eprintln!(
                    "ERROR: recipe run terminated on attempt {attempt_no} — {}. \
                     Not retried: only an unambiguously transient transport fault is. \
                     [issue #1267]",
                    verdict.reasoning()
                );
                AttemptOutcome::Final(attempt_result)
            }
        },
        |verdict, attempt_no, delay| {
            emit_failure_class_marker(verdict, attempt_no, "retry");
            eprintln!(
                "WARNING: recipe run attempt {attempt_no} hit a transient transport fault — {}. \
                 Retrying in {delay:?}. [issue #1267]",
                verdict.reasoning()
            );
        },
        // Issue #1606: backoff can last minutes; a stop signal ends it.
        super::stop_signal::sleep_unless_stopped,
    );

    if let Some(stop_reason) = summary.stop_reason
        && let Some(verdict) = &summary.last_verdict
    {
        emit_failure_class_marker(verdict, summary.attempts, "terminal_budget_exhausted");
        let waited: Duration = summary.waited.iter().sum();
        eprintln!(
            "ERROR: recipe run terminated after {} of at most {} attempt(s), {waited:?} spent \
             waiting — {}; {}. Completed steps: {}. [issue #1267]",
            summary.attempts,
            limits.max_attempts(),
            verdict.reasoning(),
            stop_reason.describe(),
            if verdict.completed_steps.is_empty() {
                "none".to_string()
            } else {
                verdict.completed_steps.join(", ")
            }
        );
    }

    // Attach the classification to the structured result so the agentic layer
    // reads it as DATA (issue #1337's evaluator) instead of scraping prose.
    let final_result = match result {
        Ok(mut run_result) => {
            if let Some(auth) = &authorization {
                let outcome = crate::commands::orch::autodrive::report::report(
                    auth,
                    &run_result.context,
                    context.get("pr_number").map(String::as_str),
                );
                apply_autodrive_outcome(&mut run_result, &outcome);
                run_result
                    .extra
                    .insert("autodrive_outcome".into(), outcome.clone());
                run_result
                    .context
                    .insert("autodrive_summary".into(), outcome);
                let value = serde_json::to_value(auth)?;
                run_result
                    .extra
                    .insert("autodrive_authorization".into(), value.clone());
                run_result
                    .context
                    .insert("autodrive_authorization".into(), value);
                run_result
                    .context
                    .insert("no_merge".into(), auth.no_merge().to_string().into());
                run_result.context.insert(
                    "autodrive_dry_run".into(),
                    auth.dry_run().to_string().into(),
                );
            }
            if !run_result.success
                && let Some(verdict) = last_verdict.borrow().as_ref()
            {
                run_result.extra.insert(
                    FAILURE_CLASS_RESULT_KEY.to_string(),
                    verdict.to_json(summary.attempts, terminal_action(&summary)),
                );
            }
            Ok(run_result)
        }
        Err(error) => Err(error),
    };
    // Release order: the private checkout, then the recovery admission
    // (judged from the result after it), then the run folder.
    release_in_order(
        final_result,
        vec![
            ("execution release", Box::new(|_| checkout.remove())),
            (
                "recovery admission finish",
                Box::new(|result| {
                    match (
                        &admission,
                        admission_outcome(result, last_verdict.borrow().as_ref()),
                    ) {
                        (Some(admission), Some(outcome)) => admission
                            .finish(outcome)
                            .context("cannot finish recovery admission"),
                        _ => Ok(()),
                    }
                }),
            ),
            // Issue #2582: the context file goes after the last runner child
            // has been reaped and before the run folder it lives in, so no
            // value outlives the run and the run report never finds the
            // folder half-removed. Dropping the transport removes the file
            // and then its folder.
            (
                "recipe context file",
                Box::new(move |_| {
                    drop(transport);
                    Ok(())
                }),
            ),
            (
                "run report",
                Box::new(|result| finish_workspace(workspace.into_inner(), result, dry_run)),
            ),
        ],
    )
}

/// One release step: its name in the error chain, and the step, which sees
/// the result so far.
pub(super) type ReleaseStep<'a> = (
    &'static str,
    Box<dyn FnOnce(&Result<RecipeRunResult>) -> Result<()> + 'a>,
);

/// Runs every release step, in order, whatever happened before it. A step's
/// error becomes the result of a run that had succeeded; otherwise it is kept
/// as context ("<name> also failed: ...") on the error before it, so no
/// release failure hides another.
pub(super) fn release_in_order(
    mut result: Result<RecipeRunResult>,
    steps: Vec<ReleaseStep<'_>>,
) -> Result<RecipeRunResult> {
    for (name, step) in steps {
        if let Err(step_error) = step(&result) {
            result = Err(match result {
                Err(error) => error.context(format!("{name} also failed: {step_error:#}")),
                Ok(_) => step_error,
            });
        }
    }
    result
}

/// The recovery descriptor lives in the run folder; one whose folder resolves
/// inside the private checkout would be state a step could rewrite (S-AZ4).
pub(super) fn refuse_descriptor_inside(descriptor: &Path, exec_dir: &Path) -> Result<()> {
    let parent = descriptor
        .parent()
        .context("recovery descriptor path has no parent folder")?;
    let resolved = parent
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", parent.display()))?;
    let checkout_root = exec_dir
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", exec_dir.display()))?;
    if resolved.starts_with(&checkout_root) {
        anyhow::bail!(
            "recovery descriptor {} resolves inside the run's private checkout {}; \
             refusing to launch",
            descriptor.display(),
            exec_dir.display()
        );
    }
    Ok(())
}

/// The recovery outcome for a finished run (#2334): `None` for a run stopped
/// by a signal, which stays resumable.
fn admission_outcome(
    final_result: &Result<RecipeRunResult>,
    last_verdict: Option<&FailureVerdict>,
) -> Option<recipe_runner_rs::recovery::model::Outcome> {
    use recipe_runner_rs::recovery::model::Outcome;
    match final_result {
        Ok(result) if result.success => Some(Outcome::Success),
        Ok(_) if last_verdict.is_some_and(|v| v.class == super::FailureClass::UsageLimit) => {
            Some(Outcome::UsageLimit)
        }
        Err(error)
            if error
                .downcast_ref::<super::stop_signal::StoppedBySignal>()
                .is_some() =>
        {
            None
        }
        _ => Some(Outcome::Failure),
    }
}

/// Preserve preparation errors while finishing the recovery admission (as
/// `Failure`, unless a stop signal ended the run) and publishing a terminal
/// run report.
fn fail_before_execution(
    workspace: crate::commands::cleanup::run_dir::RunWorkspace,
    admission: Option<&recipe_runner_rs::recovery::admission::Admission>,
    error: anyhow::Error,
    dry_run: bool,
) -> Result<RecipeRunResult> {
    let mut error = error;
    if let Some(admission) = admission
        && error
            .downcast_ref::<super::stop_signal::StoppedBySignal>()
            .is_none()
        && let Err(admission_error) =
            admission.finish(recipe_runner_rs::recovery::model::Outcome::Failure)
    {
        error = error.context(format!(
            "recovery admission finish also failed: {admission_error:#}"
        ));
    }
    let failure = Err(error);
    if let Err(finalization) = finish_workspace(workspace, &failure, dry_run) {
        return failure
            .map_err(|error| error.context(format!("run report also failed: {finalization:#}")));
    }
    failure
}

/// Issue #1808: removes runtime/, finalises run.json with the outcome, step
/// statuses and log path, and runs the end-of-run sweep unless the run was a
/// dry run (a dry run must not remove anything).
fn finish_workspace(
    workspace: crate::commands::cleanup::run_dir::RunWorkspace,
    result: &Result<RecipeRunResult>,
    dry_run: bool,
) -> Result<()> {
    use crate::commands::cleanup::run_dir::{Outcome, StepStatus};
    let (outcome, steps, log_path) = match result {
        Ok(run_result) => (
            if run_result.success {
                Outcome::Success
            } else {
                Outcome::Failure
            },
            run_result
                .step_results
                .iter()
                .map(|step| StepStatus {
                    id: step.step_id.clone(),
                    status: step.status.clone(),
                })
                .collect(),
            known_log_paths(Some(run_result))
                .get("jsonl")
                .and_then(JsonValue::as_str)
                .map(String::from),
        ),
        Err(error)
            if error
                .downcast_ref::<super::stop_signal::StoppedBySignal>()
                .is_some() =>
        {
            (Outcome::Interrupted, Vec::new(), None)
        }
        Err(_) => (Outcome::Failure, Vec::new(), None),
    };
    workspace.finish_checked(outcome, steps, log_path, !dry_run)
}

/// The `action` recorded on the classification attached to a terminal result.
fn terminal_action(summary: &RetrySummary) -> &'static str {
    match summary.stop_reason {
        Some(_) => "terminal_budget_exhausted",
        None => "terminal",
    }
}

/// Spawn the runner with stdout captured (we need to parse JSON from it)
/// and stderr "teed": each line is forwarded live to our own stderr AND
/// captured in a buffer so the error path can still surface a meaningful
/// stderr tail. (#357)
fn spawn_with_streaming_stderr(
    mut command: Command,
    correlation: RecipeRunCorrelation,
    recipe_path: &Path,
    run_dir: Option<&Path>,
    timeout: Duration,
    tree_id: &str,
    stderr_sink: &mut String,
) -> Result<RecipeRunResult> {
    arm_runner(&mut command);
    let spawned = command.spawn().context("failed to spawn recipe-runner-rs");
    let mut child = spawned.map_err(|error| {
        correlation.emit_early();
        fail_with_final_record(
            &correlation,
            RecipeRunFinalStatus::SpawnFailure,
            None,
            None,
            error,
        )
    })?;
    let child_pid = Some(child.id());
    // Issue #2175: the record `recipe stop <run_id>` finds this attempt's
    // processes by, written before `early` so a run id seen there always has
    // one. Held for this attempt only: dropping the guard on any return
    // removes it. A dry run has no run folder and writes none.
    #[cfg(target_os = "linux")]
    let _run_record = run_dir.and_then(|dir| {
        match super::super::run_record::write(dir, correlation.run_id(), child.id()) {
            Ok(guard) => Some(guard),
            Err(error) => {
                eprintln!(
                    "amplihack: warning: recipe run {:?} wrote no process record, so \
                     `amplihack recipe stop` cannot stop it by run id: {error:#}",
                    correlation.run_id()
                );
                None
            }
        }
    });
    #[cfg(not(target_os = "linux"))]
    let _ = run_dir;
    correlation.emit_early();

    let captured_stderr: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let dropped_stderr_lines: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let stderr_handle = child.stderr.take().expect("piped stderr");
    let captured_clone = Arc::clone(&captured_stderr);
    let dropped_clone = Arc::clone(&dropped_stderr_lines);
    let (stderr_done_tx, stderr_done_rx) = mpsc::channel();
    thread::spawn(move || {
        // Read RAW BYTES, not str-typed lines(): an Err(InvalidData) from
        // non-UTF-8 stderr would otherwise terminate the pump silently and
        // the child can then block on a full pipe (#366 / COE feedback).
        let mut reader = BufReader::new(stderr_handle);
        let stderr = io::stderr();
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    let _ = writeln!(stderr.lock(), "{trimmed}");
                    push_bounded_stderr_line(&captured_clone, &dropped_clone, trimmed.to_string());
                }
                // I/O error reading from the pipe: log and stop pumping —
                // we MUST NOT spin or leak the thread, but the child will
                // still close stderr at exit and `wait()` will return.
                Err(_) => break,
            }
        }
        let _ = stderr_done_tx.send(());
    });

    let mut stdout_handle = child.stdout.take().expect("piped stdout");
    use std::io::Read;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdout_buf = String::new();
        let result = stdout_handle
            .read_to_string(&mut stdout_buf)
            .map(|_| stdout_buf);
        let _ = stdout_tx.send(result);
    });

    // One clock for this attempt's timeout and every teardown grace below:
    // `terminate_recipe_runner`, `sweep_runner_tree` and
    // `reap_recipe_runner_group` all wait on it.
    let runner = RunnerClock::current();
    let waited = wait_for_recipe_runner(&mut child, timeout, &runner);
    let status = match waited
        .context("failed to wait for recipe-runner-rs")
        .map_err(|error| {
            fail_with_final_record(
                &correlation,
                RecipeRunFinalStatus::Failure,
                child_pid,
                None,
                error,
            )
        })? {
        RunnerWait::Exited(status) => {
            // Issue #1606: a signal that arrived as the runner exited still
            // stops the run; it must not end in a success.
            if let Some(signal) = super::stop_signal::pending() {
                // The runner has exited and been reaped, so its pid may already
                // belong to another process: seed from its group's remaining
                // members only. Every descendant that left the group is still
                // found through them.
                let tree = crate::commands::recipe::stop::snapshot_group(child.id() as i32);
                #[cfg(unix)]
                reap_recipe_runner_group(child.id(), &runner);
                sweep_runner_tree(&tree, &runner);
                // Issue #1976: the captured signal value alone decides the
                // status; a signal stored after an orphan request replaces it
                // and is a Failure.
                if signal == super::stop_signal::ORPHANED as i32 {
                    record_orphan(&correlation, child_pid, tree_id);
                } else {
                    correlation.emit_final(
                        RecipeRunFinalStatus::Failure,
                        child_pid,
                        status.code(),
                        known_log_paths(None),
                    );
                }
                return Err(anyhow::Error::new(super::stop_signal::StoppedBySignal(
                    signal,
                )));
            }
            status
        }
        RunnerWait::Stopped(signal) => {
            // Issue #1606: the same teardown as a timeout, for a signal, plus
            // every descendant that left the runner's group (an agent that
            // setsid'd again): the whole tree is found before anything is
            // signalled, and swept after the group teardown.
            //
            // The child is reaped only by this executor's own wait, so its
            // pid is still the runner's and the walk starts there.
            let roots = std::collections::BTreeSet::from([child.id() as i32]);
            let tree = crate::commands::recipe::stop::snapshot_tree(&roots);
            // Issue #1976: the flag value alone decides the status; a signal
            // stored after an orphan request replaces it and is a Failure.
            let summary = if signal == super::stop_signal::ORPHANED as i32 {
                record_orphan(&correlation, child_pid, tree_id)
            } else {
                correlation.emit_final(
                    RecipeRunFinalStatus::Failure,
                    child_pid,
                    None,
                    known_log_paths(None),
                )
            };
            let detail = super::format::format_log_pointer_summary(&summary);
            // The sweep runs whatever the group teardown reports: a runner
            // that could not be reaped must not leave its descendants behind.
            let teardown = terminate_recipe_runner(&mut child, &runner);
            sweep_runner_tree(&tree, &runner);
            let stopped = anyhow::Error::new(super::stop_signal::StoppedBySignal(signal));
            return Err(match teardown {
                Ok(()) => stopped,
                Err(error) => stopped.context(format!(
                    "recipe run {} could not terminate its runner: {error:#}; {detail}",
                    summary.run_id
                )),
            });
        }
        RunnerWait::TimedOut => {
            let pid = child.id();
            // Issue #1304: emit the terminal record BEFORE terminating. The
            // `?` on terminate_recipe_runner used to be able to return first,
            // so a timeout whose cleanup also failed produced no record at all
            // -- the worst case reported as an opaque exit 1.
            let summary = correlation.emit_final(
                RecipeRunFinalStatus::Failure,
                child_pid,
                None,
                known_log_paths(None),
            );
            let detail = super::format::format_log_pointer_summary(&summary);
            let run_id = summary.run_id.clone();
            let teardown = terminate_recipe_runner(&mut child, &runner);
            teardown.with_context(|| {
                format!("recipe run {run_id} timed out and could not be terminated: {detail}")
            })?;
            anyhow::bail!(
                "recipe-runner-rs timed out after {:?} (pid {}, recipe {}, working dir {}); recipe run {} ended: {}",
                timeout,
                pid,
                recipe_path.display(),
                correlation.cwd(),
                run_id,
                detail
            );
        }
    };

    // Issue #964: the session leader has exited (and been reaped by
    // `wait_for_recipe_runner`), but on any NON-timeout early-exit path
    // (success OR failure) it may leave orphaned descendants behind in its
    // process group. Deterministically reap that tree BEFORE draining pipes so
    // a failed/early-exiting runner cannot leak a recursive subprocess tree and
    // cannot wedge the drain by holding the inherited stdout pipe open.
    #[cfg(unix)]
    reap_recipe_runner_group(child.id(), &runner);

    let stdout_buf = stdout_rx
        .recv_timeout(RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT)
        .with_context(|| {
            format!(
                "recipe-runner-rs stdout did not close within {:?} after process exit",
                RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT
            )
        })
        .and_then(|read| read.context("failed to read recipe-runner-rs stdout"))
        .map_err(|error| {
            fail_with_final_record(
                &correlation,
                RecipeRunFinalStatus::Failure,
                child_pid,
                status.code(),
                error,
            )
        })?;
    let _ = stderr_done_rx.recv_timeout(RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT);

    let captured = captured_stderr.lock().expect("stderr mutex");
    let dropped = *dropped_stderr_lines
        .lock()
        .expect("stderr drop-count mutex");
    let stderr_joined = captured
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    // Issue #1267: hand the tail back to the caller so a failure can be
    // classified from everything the runner said, not just its JSON.
    stderr_sink.clone_from(&stderr_joined);
    match parse_recipe_output_with_stderr_drops(
        &stdout_buf,
        &stderr_joined,
        status.success(),
        dropped,
    ) {
        Ok(mut result) => {
            let final_status = if status.success() && result.success {
                RecipeRunFinalStatus::Success
            } else {
                RecipeRunFinalStatus::Failure
            };
            let summary = correlation.emit_final(
                final_status,
                child_pid,
                status.code(),
                known_log_paths(Some(&result)),
            );
            result.run_id = Some(summary.run_id.clone());
            result.log_pointer = Some(summary);
            Ok(result)
        }
        Err(error) => {
            let final_status = if status.success() || !stdout_buf.trim().is_empty() {
                RecipeRunFinalStatus::ParseFailure
            } else {
                RecipeRunFinalStatus::Failure
            };
            let summary = correlation.emit_final(
                final_status,
                child_pid,
                status.code(),
                known_log_paths(None),
            );
            // Issue #1304: carry the correlation detail into the error. The
            // success path returns it in `result.log_pointer`; a failure used
            // to drop it, leaving the operator no run id to search for.
            let detail = super::format::format_log_pointer_summary(&summary);
            Err(error).with_context(|| {
                format!(
                    "recipe-runner-rs exited with {}; recipe run {} ended: {}",
                    exit_status_label(&status),
                    summary.run_id,
                    detail
                )
            })
        }
    }
}

/// The runner's process setup: captured stdout and stderr, its own session,
/// and on Linux a SIGTERM when this process dies.
fn arm_runner(command: &mut Command) {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Issue #1606: SIGKILL cannot be caught, so a `recipe run` killed that
        // way never tears the runner down. On Linux the kernel then sends the
        // runner SIGTERM (PR_SET_PDEATHSIG). Captured before the fork: the
        // check after prctl catches a parent that died in between.
        #[cfg(target_os = "linux")]
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: `pre_exec` runs after fork and before exec. `setsid`,
        // `prctl` and `getppid` are async-signal-safe. `setsid` lets timeout
        // cleanup terminate the recipe tree.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        // No allocation after fork: ESRCH, "no such process",
                        // for the parent that is already gone.
                        return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                    }
                }
                Ok(())
            });
        }
    }
}

/// SIGTERM every process of `tree`, wait up to the teardown grace on
/// `runner`'s clock for them to exit, then SIGKILL the survivors. A test that
/// has cancelled its run ends the grace at once. Returns how many had to be
/// killed.
pub(super) fn sweep_runner_tree(
    tree: &crate::commands::recipe::stop::Tree,
    runner: &RunnerClock,
) -> usize {
    crate::commands::recipe::stop::terminate(tree);
    crate::commands::recipe::stop::sweep_on(
        tree,
        recipe_runner_teardown_grace(),
        runner.clock(),
        &|| runner.cancelled(),
    )
}

fn recipe_name_for_correlation(recipe_path: &Path) -> String {
    std::fs::read_to_string(recipe_path)
        .ok()
        .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
        .and_then(|value| {
            value
                .get("name")
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            recipe_path
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| recipe_path.display().to_string())
}

fn push_bounded_stderr_line(
    captured: &Arc<Mutex<VecDeque<String>>>,
    dropped: &Arc<Mutex<usize>>,
    line: String,
) {
    let mut captured = captured.lock().expect("stderr mutex");
    if captured.len() == CAPTURED_STDERR_LINES {
        captured.pop_front();
        *dropped.lock().expect("stderr drop-count mutex") += 1;
    }
    captured.push_back(line);
}

/// Pure parser for recipe-runner-rs subprocess output.
///
/// Behavior:
/// - Empty/whitespace-only stdout + success returns an explicit hollow-success
///   terminal failure. A runner that produced no structured result must not
///   become a success-shaped no-op.
/// - Empty/whitespace-only stdout + failure: errors with the meaningful stderr
///   tail surfaced so callers see the upstream cause.
/// - Non-empty stdout: parses as JSON; on failure, errors with a bounded stdout
///   preview that reports discarded chars and stderr tail in the `anyhow::Context`.
///
/// `RecipeRunResult` does not use `deny_unknown_fields`, so future
/// recipe-runner-rs versions may add fields without breaking us.
#[cfg(test)]
pub(super) fn parse_recipe_output(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
) -> Result<RecipeRunResult> {
    parse_recipe_output_with_stderr_drops(stdout, stderr, exit_success, 0)
}

fn parse_recipe_output_with_stderr_drops(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
    prior_discarded_stderr_lines: usize,
) -> Result<RecipeRunResult> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        if exit_success {
            let mut extra = JsonMap::new();
            extra.insert(
                "workflow_result".into(),
                serde_json::json!({
                    "terminal_state": "HOLLOW_SUCCESS",
                    "terminal_success": false,
                    "terminal_reason": "recipe-runner-rs exited successfully but produced no structured workflow output",
                    "required_next_action": "Inspect recipe-runner logs and rerun with structured JSON output."
                }),
            );
            return Ok(RecipeRunResult {
                success: false,
                status: Some("HOLLOW_SUCCESS".into()),
                phase: Some("finalization".into()),
                extra,
                ..RecipeRunResult::default()
            });
        }
        anyhow::bail!(
            "recipe-runner-rs produced no output and exited with failure\nstderr tail:\n{}",
            meaningful_stderr_tail_with_prior_drops(stderr, prior_discarded_stderr_lines)
        );
    }

    serde_json::from_str::<RecipeRunResult>(trimmed).with_context(|| {
        let preview = truncate_chars_with_notice(trimmed, 200);
        format!(
            "recipe-runner-rs produced non-JSON stdout preview:\n{}\nstderr tail:\n{}",
            preview,
            meaningful_stderr_tail_with_prior_drops(stderr, prior_discarded_stderr_lines)
        )
    })
}

/// Largest single argv string the kernel accepts (`MAX_ARG_STRLEN`,
/// PAGE_SIZE * 32 on Linux), including its trailing NUL.
pub(super) const MAX_ARG_STRLEN_BYTES: usize = 131_072;

/// Pass context key-value pairs to the command as `--set KEY=VALUE`
/// arguments, which puts every value on the runner's argv.
///
/// Issue #2582: this runs only for a runner that does not report
/// `context_file_v1`. The pinned runner reports it, so in a normal run
/// `context_transport` passes `--context-file <path>` instead and no value is
/// on argv at all. This path is reached only by a runner older than the pin
/// supplied through `RECIPE_RUNNER_RS_PATH`; `ContextTransport::record`
/// prints a line to stderr when it is taken, unconditionally, so that case is
/// visible to an operator who set no `RUST_LOG` and would otherwise find it
/// only in `/proc`.
///
/// Returns the argv bytes the pairs occupy, so the caller can take them out
/// of the env-mirror budget. A pair longer than one argv string allows is
/// refused with an error naming the key and size, never the value.
pub(super) fn pass_context(
    command: &mut Command,
    context: &BTreeMap<String, String>,
) -> Result<usize> {
    super::context_transport::refuse_reserved_keys(context)?;
    if let Some((key, value)) = context
        .iter()
        .find(|(k, v)| k.len() + 1 + v.len() + 1 > MAX_ARG_STRLEN_BYTES)
    {
        anyhow::bail!(
            "recipe context value for key {} is {} bytes; the runner supplied through \
             RECIPE_RUNNER_RS_PATH does not report {}, so this run passes context as --set \
             arguments, and one argument cannot exceed {} bytes. Use a runner that reports \
             {}, which takes the context as a file and has no per-value limit; failing that, \
             pass a file path in the context instead and read the file in the step",
            crate::commands::profile::escape_for_error(key),
            value.len(),
            super::context_transport::CONTEXT_FILE_CAPABILITY,
            MAX_ARG_STRLEN_BYTES,
            super::context_transport::CONTEXT_FILE_CAPABILITY
        );
    }
    let mut argv_bytes = 0;
    for (key, value) in context {
        let pair = format!("{key}={value}");
        // `--set\0`, `KEY=VALUE\0` and two argv pointers.
        argv_bytes += "--set".len() + 1 + pair.len() + 1 + 2 * std::mem::size_of::<usize>();
        command.arg("--set").arg(pair);
    }
    Ok(argv_bytes)
}

// Issue #1304: every exit from `run_recipe_runner` must leave a durable
// terminal record behind. A run that dies without one is indistinguishable
// from a run that never started: the operator is left with a bare `exit 1`,
// no run id to search the logs for, and no way to tell a crashed runner from
// a runner that was never spawned.
//
// Failure paths used to discard the summary (`let _summary = ...`), so a
// failing run told the operator *less* than a succeeding one -- exactly
// backwards. This attaches the same correlation detail the success path
// already returns.
fn fail_with_final_record(
    correlation: &RecipeRunCorrelation,
    status: RecipeRunFinalStatus,
    child_pid: Option<u32>,
    exit_code: Option<i32>,
    error: anyhow::Error,
) -> anyhow::Error {
    let summary = correlation.emit_final(status, child_pid, exit_code, known_log_paths(None));
    let detail = super::format::format_log_pointer_summary(&summary);
    error.context(format!("recipe run {} ended: {detail}", summary.run_id))
}

/// Issue #1976: the final record and `orphaned.json` of an orphan stop. The
/// existing teardown runs unchanged around it.
fn record_orphan(
    correlation: &RecipeRunCorrelation,
    child_pid: Option<u32>,
    tree_id: &str,
) -> crate::commands::recipe::RecipeLogPointerSummary {
    let parent_pid = super::stop_signal::orphan_info().map_or(0, |info| info.parent_pid);
    let summary = correlation.emit_orphaned(
        child_pid,
        super::correlation::OrphanFields {
            parent_pid,
            tree_id,
        },
    );
    super::orphan_record::write_to_state_dir(&super::orphan_record::OrphanRecord {
        parent_pid,
        tree_id: tree_id.to_string(),
        run_id: correlation.run_id().to_string(),
    });
    summary
}

fn exit_status_label(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!(
                "signal {} ({})",
                amplihack_utils::launch_target::signal_name(signal),
                signal
            );
        }
    }

    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
pub(super) fn meaningful_stderr_tail(stderr: &str) -> String {
    meaningful_stderr_tail_with_prior_drops(stderr, 0)
}

pub(super) fn meaningful_stderr_tail_with_prior_drops(
    stderr: &str,
    prior_discarded_stderr_lines: usize,
) -> String {
    let lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let meaningful = lines
        .iter()
        .copied()
        .filter(|line| {
            !matches!(line.chars().next(), Some('▶' | '✓' | '⊘' | '✗'))
                && !line.starts_with("[agent]")
        })
        .collect::<Vec<_>>();

    let (selected, discarded) = if meaningful.is_empty() {
        let discarded = lines.len().saturating_sub(STDERR_TAIL_LINES);
        (
            lines
                .into_iter()
                .rev()
                .take(STDERR_TAIL_LINES)
                .collect::<Vec<_>>(),
            discarded,
        )
    } else {
        let discarded = meaningful.len().saturating_sub(STDERR_TAIL_LINES);
        (
            meaningful
                .into_iter()
                .rev()
                .take(STDERR_TAIL_LINES)
                .collect::<Vec<_>>(),
            discarded,
        )
    };

    let mut tail = selected.into_iter().rev().collect::<Vec<_>>().join("\n");
    let discarded = discarded + prior_discarded_stderr_lines;
    if discarded > 0 {
        if !tail.is_empty() {
            tail.push('\n');
        }
        tail.push_str(&format!("[truncated: discarded {discarded} stderr lines]"));
    }
    tail
}

pub(super) fn recipe_runner_timeout() -> Duration {
    std::env::var(RECIPE_RUNNER_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(RECIPE_RUNNER_DEFAULT_TIMEOUT)
}

/// How waiting for the runner ended.
enum RunnerWait {
    Exited(ExitStatus),
    TimedOut,
    /// Issue #1606: `recipe run` received this stop signal.
    Stopped(i32),
}

/// The clock a runner attempt's timeout and teardown grace are measured on,
/// and whether the attempt has been cancelled. Production reads the system
/// clock and is never cancelled. A lib test that runs the executor under
/// `test_runner::RunnerWorker` gets that worker's control instead: its
/// (optionally virtual) clock and its cancel flag. The wait and every
/// teardown take it from here, so neither runs on a clock the test does not
/// control.
pub(super) struct RunnerClock {
    #[cfg(all(test, unix))]
    control: Option<Arc<super::test_runner::Control>>,
}

impl RunnerClock {
    /// The control installed for this thread, if any. Read on the executor's
    /// thread, where `RunnerWorker` installs it.
    pub(super) fn current() -> Self {
        Self {
            #[cfg(all(test, unix))]
            control: super::test_runner::current(),
        }
    }

    fn clock(&self) -> &dyn Clock {
        #[cfg(all(test, unix))]
        if let Some(control) = self.control.as_deref() {
            return control;
        }
        &SystemClock
    }

    fn cancelled(&self) -> bool {
        #[cfg(all(test, unix))]
        if let Some(control) = self.control.as_deref() {
            return control.cancelled();
        }
        false
    }
}

/// Polls the runner until it exits, a stop signal arrives, or `timeout`
/// passes on `runner`'s clock.
///
/// Each poll asks `try_wait` first, so a runner that has exited is reported
/// as `Exited` even when the run was cancelled in the meantime. A cancelled
/// run whose runner `try_wait` has just found still running has its process
/// group killed and is reported as a timeout, so the usual teardown ends it.
fn wait_for_recipe_runner(
    child: &mut Child,
    timeout: Duration,
    runner: &RunnerClock,
) -> std::io::Result<RunnerWait> {
    let clock = runner.clock();
    let started = clock.now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(RunnerWait::Exited(status));
        }
        if runner.cancelled() {
            // SAFETY: `try_wait` has just found the child still running, and
            // only this executor waits on it, so it is still unreaped and its
            // pid, the id of the group it leads, is still this runner's.
            // Cancel only that group before the test worker is joined.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL)
            };
            return Ok(RunnerWait::TimedOut);
        }
        if let Some(signal) = super::stop_signal::pending() {
            return Ok(RunnerWait::Stopped(signal));
        }
        // Single clock read per poll: derive the remaining budget once and reuse
        // it for both the deadline check and the sleep cap. A zero (or elapsed)
        // remaining is the timeout.
        let remaining = match timeout.checked_sub(clock.now().saturating_duration_since(started)) {
            Some(remaining) if !remaining.is_zero() => remaining,
            _ => return Ok(RunnerWait::TimedOut),
        };
        clock.sleep(RECIPE_RUNNER_POLL_INTERVAL.min(remaining));
    }
}

fn terminate_recipe_runner(child: &mut Child, runner: &RunnerClock) -> Result<()> {
    let pid = child.id();
    #[cfg(unix)]
    {
        // Issue #964: reap the recipe-runner tree DETERMINISTICALLY and
        // GRACEFULLY. The runner is a session leader (see the `setsid` in
        // `spawn_with_streaming_stderr`), so its PID doubles as the process-group
        // id shared by every descendant. Three phases:
        //   1. SIGTERM the whole group so the runner (and children) can run any
        //      cleanup / trap handlers,
        //   2. wait up to the configurable grace window for the runner to exit,
        //   3. SIGKILL the group to guarantee nothing survives — this reaps both
        //      a runner that ignored SIGTERM and any lingering descendants.
        // The signal target is `-pgid`, so this is scoped strictly to the
        // runner's own session and never touches unrelated / parent processes.
        let pgid = pid as libc::pid_t;
        terminate_recipe_runner_with(
            runner.clock(),
            || {
                // A test's virtual grace may be frozen while it awaits the
                // TERM handler. Unwinding must still cancel and join it.
                !runner.cancelled() && matches!(child.try_wait(), Ok(None))
            },
            |signal| signal_process_group(pgid, signal),
        );
    }
    #[cfg(not(unix))]
    let _ = runner;
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = run_with_timeout(command, RECIPE_RUNNER_TERMINATE_TIMEOUT);
    }
    child
        .kill()
        .or_else(|kill_error| match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(kill_error),
            Err(wait_error) => Err(wait_error),
        })
        .with_context(|| format!("failed to terminate timed-out recipe-runner-rs pid {pid}"))?;
    child
        .wait()
        .with_context(|| format!("failed to wait for timed-out recipe-runner-rs pid {pid}"))?;
    Ok(())
}

/// Apply the configured timeout teardown policy with injectable clock and
/// process operations, so tests observe signals without real-time races.
#[cfg(unix)]
pub(super) fn terminate_recipe_runner_with(
    clock: &dyn Clock,
    still_running: impl FnMut() -> bool,
    mut signal: impl FnMut(libc::c_int),
) {
    let grace = recipe_runner_teardown_grace();
    signal(libc::SIGTERM);
    wait_until_exited(grace, clock, still_running);
    signal(libc::SIGKILL);
}

/// Resolve the SIGTERM -> SIGKILL grace window for recipe-runner teardown.
///
/// Honors `AMPLIHACK_TEARDOWN_GRACE_SECS` (whole seconds, `0` allowed to mean
/// "escalate to SIGKILL immediately"); falls back to
/// [`RECIPE_RUNNER_DEFAULT_TEARDOWN_GRACE`] when unset or unparsable.
#[cfg(unix)]
fn recipe_runner_teardown_grace() -> Duration {
    std::env::var(RECIPE_RUNNER_TEARDOWN_GRACE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(RECIPE_RUNNER_DEFAULT_TEARDOWN_GRACE)
}

/// Poll `still_running` until it reports the target has exited or `grace`
/// elapses; returns `true` if it exited within the window. Sleeps in short
/// `RECIPE_RUNNER_POLL_INTERVAL` steps (never past the remaining grace), so a
/// zero grace escalates immediately. Shared by both teardown
/// paths so the SIGTERM -> grace -> SIGKILL timing policy lives in one place.
#[cfg(unix)]
fn wait_until_exited(
    grace: Duration,
    clock: &dyn Clock,
    mut still_running: impl FnMut() -> bool,
) -> bool {
    let started = clock.now();
    while still_running() {
        let elapsed = clock.now().saturating_duration_since(started);
        if elapsed >= grace {
            return false;
        }
        let remaining = grace.saturating_sub(elapsed);
        clock.sleep(RECIPE_RUNNER_POLL_INTERVAL.min(remaining));
    }
    true
}

/// Send `signal` to the process group `pgid` (targets `-pgid`). A missing group
/// (`ESRCH`) is expected and silent; any other failure is logged.
#[cfg(unix)]
fn signal_process_group(pgid: libc::pid_t, signal: libc::c_int) {
    let result = unsafe { libc::kill(-pgid, signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(pgid, signal, %error, "failed to signal recipe-runner process group");
        }
    }
}

/// Deterministically reap orphaned descendants left in an already-exited
/// runner's process group (issue #964). The runner itself has already been
/// reaped by [`wait_for_recipe_runner`]; this sweeps any survivors that
/// outlived the session leader using the same graceful SIGTERM -> grace ->
/// SIGKILL contract, scoped strictly to `-pgid`. The grace is measured on
/// `runner`'s clock, like every other teardown grace of the attempt, and a
/// cancelled test gives it up at once and kills the group.
#[cfg(unix)]
fn reap_recipe_runner_group(pgid_pid: u32, runner: &RunnerClock) {
    let pgid = pgid_pid as libc::pid_t;
    // `kill(-pgid, 0)` probes group liveness without delivering a signal.
    let group_alive = || unsafe { libc::kill(-pgid, 0) } == 0;
    // Fast path: the group is already empty (well-behaved runner, no orphans).
    if !group_alive() {
        return;
    }
    signal_process_group(pgid, libc::SIGTERM);
    let gave_up = std::cell::Cell::new(false);
    let exited = wait_until_exited(recipe_runner_teardown_grace(), runner.clock(), || {
        if runner.cancelled() {
            gave_up.set(true);
            return false;
        }
        group_alive()
    });
    if !exited || gave_up.get() {
        signal_process_group(pgid, libc::SIGKILL);
    }
}

/// Fail-closed recursion-depth guard for `amplihack recipe run` (issue #964).
///
/// Refuses to spawn a nested recipe-runner once the current session has reached
/// the configured maximum recursion depth, so a failing / misbehaving
/// orchestration can never recursively re-enter the orchestrator and fork-bomb
/// the host. It reuses the existing session-tree depth convention rather than
/// forking a second source of truth: the same `AMPLIHACK_SESSION_DEPTH` /
/// `AMPLIHACK_MAX_DEPTH` env vars and the shared `DEFAULT_MAX_DEPTH` (`3`) /
/// `MAX_DEPTH_CEILING` (`32`) constants from `commands::session_tree::state`.
///
/// Contract:
/// * `AMPLIHACK_SESSION_DEPTH` unset -> treated as the root (depth `0`);
/// * a malformed / non-numeric / non-UTF-8 `AMPLIHACK_SESSION_DEPTH` is
///   **fail-closed**: treated as "already at the limit" (bail), never silently
///   coerced to `0` (which would defeat the guard);
/// * `AMPLIHACK_MAX_DEPTH` may only LOWER the ceiling the root sealed into the
///   tree state, never raise it (issue #1326); it is additionally clamped to
///   `MAX_DEPTH_CEILING`, and an unset / malformed value falls back to the
///   configured `orchestrator.max_depth` (issue #1606, under the same rules),
///   then `DEFAULT_MAX_DEPTH`;
/// * below the limit the caller spawns normally (no over-blocking).
///
/// Logs numeric depth/limit fields only — never env-var *values*, which may
/// carry session tokens or secrets.
fn enforce_recursion_depth_guard(working_dir: &Path) -> Result<SpawnGuard> {
    use crate::commands::session_tree::state::{
        DEFAULT_MAX_SESSIONS, admit_session, effective_max_depth, memory_shortfall_mib,
        sealed_ceiling, validate_tree_id,
    };

    // Issue #1326: the environment may LOWER this ceiling but never raise it.
    // `AMPLIHACK_MAX_DEPTH` is inherited by every descendant and is writable by
    // anything in the tree, so on its own it is a suggestion, not a limit. The
    // authority is the value the root sealed into the tree state.
    let env_max_depth = std::env::var(MAX_DEPTH_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .or_else(|| crate::user_config::max_depth(working_dir));
    let tree_id = std::env::var(TREE_ID_ENV)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        // Issue #1811: a root run uses the tree id the capacity governor
        // recorded with its slot, so the agent steps find that slot.
        .or_else(crate::commands::governor::settled_tree_id);
    let sealed = tree_id.as_deref().and_then(sealed_ceiling);
    let max_depth = effective_max_depth(sealed, env_max_depth);

    // Fail-closed: distinguish "unset" (root, depth 0) from "set but unparseable"
    // (treat as at-the-limit) using `var_os`, so a corrupted / forged value can
    // never bypass the guard by parsing as 0.
    let depth = match std::env::var_os(SESSION_DEPTH_ENV) {
        None => 0,
        Some(raw) => match raw.to_str().and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(value) => value,
            None => {
                tracing::warn!(
                    max_depth,
                    "malformed AMPLIHACK_SESSION_DEPTH; failing closed at the recursion limit (issue #964)"
                );
                max_depth
            }
        },
    };

    // Issue #1326, fail-closed. A nested run (`depth > 0`) whose ceiling cannot be
    // resolved from tree state is an incoherent state: the environment claims we
    // are inside a tree, but no tree vouches for the ceiling. Trusting the
    // environment here is exactly the bypass that let agents escalate
    // `AMPLIHACK_MAX_DEPTH` (observed ladder 5 -> 6 -> 7 -> 8 -> 9), because at the
    // root nothing seeds `AMPLIHACK_TREE_ID` and `sealed` is therefore `None` for
    // the whole chain. Refuse instead, consistent with how #964 already treats a
    // malformed `AMPLIHACK_SESSION_DEPTH`.
    let corroborated = tree_id.is_some() || has_orchestrator_ancestor();
    let depth = resolve_claimed_depth(depth, sealed, corroborated);
    if depth > 0 && sealed.is_none() {
        tracing::warn!(
            depth,
            has_tree_id = tree_id.is_some(),
            "nested recipe run with no sealed ceiling; failing closed (issue #1326)"
        );
        return Err(terminal_block(format!(
            "BLOCKED_TERMINAL orchestration_unavailable: nested run at depth {depth} has no \
             sealed recursion ceiling (issue #1326).\n\
             This is a POLICY decision, not an infrastructure fault. Retrying, switching \
             recipe, or setting AMPLIHACK_MAX_DEPTH will NOT change it.\n\
             DO: complete this step inline and return your result."
        )));
    }

    if depth >= max_depth {
        tracing::warn!(
            depth,
            max_depth,
            "recipe run blocked by recursion-depth guard; refusing to spawn a nested recipe-runner (issue #964)"
        );
        // Issue #1326: this refusal must not read as a transient fault. On the
        // affected host, agents parsed the previous wording as an infrastructure
        // failure, raised `AMPLIHACK_MAX_DEPTH`, and retried one level deeper
        // (observed ladder: 5 -> 6 -> 7 -> 8 -> 9). Say plainly that it is policy,
        // that retrying cannot change it, and what to do instead.
        return Err(terminal_block(format!(
            "BLOCKED_TERMINAL orchestration_unavailable: depth {depth} of max {max_depth} \
             (issue #964/#1326).\n\
             This is a POLICY decision, not an infrastructure fault. Retrying, switching \
             recipe, or setting AMPLIHACK_MAX_DEPTH will NOT change it -- the ceiling is \
             read from the session-tree state and the environment may only lower it.\n\
             DO: complete this step inline and return your result."
        )));
    }

    // Root seeding. Fail-closed above is only safe if a root actually establishes
    // a tree; otherwise the first nested run would be refused and nesting -- the
    // capability this guard exists to protect -- would be broken. The runner is
    // the single owner of the increment: it seals the ceiling here and hands the
    // child its identity and depth explicitly, so no descendant has to re-derive
    // either one.
    let tree_id = match tree_id {
        Some(id) => {
            validate_tree_id(&id).context("invalid AMPLIHACK_TREE_ID")?;
            id
        }
        None => new_tree_id(),
    };
    // Issue #1329: admit the node we are about to create, so the tree-global
    // session cap applies to every spawn and not only to callers that happen to go
    // through `session-tree register`. Debits under the tree lock, before the child
    // exists.
    let child_depth = depth.saturating_add(1);
    let session_id = new_tree_id();
    let max_sessions = std::env::var(MAX_SESSIONS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_SESSIONS);

    // Issue #1329: is another agent affordable at all? Nothing in the tree asked
    // this before; the host reached 247 GB with no component ever checking.
    if let Some((available, floor)) = memory_shortfall_mib() {
        tracing::warn!(
            available,
            floor,
            "spawn refused: available memory below floor"
        );
        return Err(terminal_block(format!(
            "BLOCKED_TERMINAL orchestration_unavailable: {available} MiB available, floor is \
             {floor} MiB (issue #1329).\n\
             This is a RESOURCE decision, not an infrastructure fault. Retrying will not \
             free memory.\n\
             DO: complete this step inline, or raise AMPLIHACK_MIN_AVAILABLE_MIB if you \
             know the floor is wrong for this host."
        )));
    }

    let admitted = match admit_session(&tree_id, &session_id, child_depth, max_depth, max_sessions)
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(%error, child_depth, max_sessions, "spawn refused by the tree budget");
            return Err(terminal_block(format!(
                "BLOCKED_TERMINAL orchestration_unavailable: {error} (issue #1329).\n\
                 This is a POLICY decision, not an infrastructure fault. Retrying, switching \
                 recipe, or changing AMPLIHACK_MAX_DEPTH / AMPLIHACK_MAX_SESSIONS will NOT \
                 change it -- both are read from the session-tree state.\n\
                 DO: complete this step inline and return your result."
            )));
        }
    };

    Ok(SpawnGuard {
        tree_id,
        child_depth,
        max_depth: admitted.ceiling,
        session_id,
    })
}

/// A fresh tree identifier for a root run. Matches `validate_tree_id`'s alphabet.
///
/// A collision would splice two unrelated trees together: they would share one
/// ceiling and one session budget, so an innocent root could be refused because a
/// stranger filled the tree. Truncating the timestamp to `u32` would wrap roughly
/// every 4.3 seconds and truncating the pid to `u16` discards most of its range,
/// which is more collision surface than this needs. Keep both whole.
pub(crate) fn new_tree_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    format!("{:016x}{:08x}", nanos, std::process::id())
}

/// Issue #1619: write this run's patched recipe copies, or `None` when no
/// per-step tier override is configured, the run has no durable directory, or
/// no recipe declares a named step.
///
/// Reported and never fatal: a tier is a preference about which model runs a
/// step, so a staging failure leaves the run on the recipes as written rather
/// than stopping a hundred agent steps.
fn stage_step_tier_overrides(
    run_dir: Option<&Path>,
    search_dirs: &[PathBuf],
    recipe: &Path,
) -> Option<super::step_tiers::Staged> {
    let (overrides, file_notices) = crate::commands::launch::model_tiers::step_tier_overrides();
    // Once per run, before anything is staged: this process is the only
    // reader of `[step_tiers]`, so a line it does not print is a line nobody
    // sees. The file's own diagnostics come first — they explain why a
    // mapping is not what the operator wrote.
    for notice in file_notices {
        eprintln!("{}", notice.render());
    }
    if overrides.is_empty() {
        return None;
    }
    // Not `run_dir?` above the emptiness check: an operator who configured
    // `[step_tiers]` and got nothing must be told why. Without this line a
    // run with no durable directory — a dry run — discards the whole table
    // with no `BadStepId`, no `StepTierUnmatched` and no line at all, which
    // is the silence `step_tiers` says a mistyped step id must never have.
    let Some(run_dir) = run_dir else {
        eprintln!(
            "amplihack: model tiers: per-step tier overrides are configured, but this \
             run has no durable directory to stage them in; the recipes run with the \
             tiers they declare."
        );
        return None;
    };
    match super::step_tiers::stage(run_dir, search_dirs, recipe, overrides) {
        Ok(outcome) => {
            for notice in &outcome.notices {
                eprintln!("{}", notice.render());
            }
            outcome.staged
        }
        Err(reason) => {
            eprintln!(
                "amplihack: model tiers: the per-step tier override could not be staged \
                 ({reason}); the recipes run with the tiers they declare."
            );
            None
        }
    }
}
