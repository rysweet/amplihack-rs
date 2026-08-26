use super::correlation::{RecipeRunCorrelation, RecipeRunFinalStatus, known_log_paths};
use super::*;
use crate::env_builder::{EnvBuilder, active_agent_binary};
#[cfg(windows)]
use crate::util::run_with_timeout;
use crate::util::truncate_chars_with_notice;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

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

/// Threshold in bytes for total `--set` argument size before we switch
/// to passing context via a temp file. Well under the typical Linux
/// ARG_MAX (~2MB) to leave room for env vars and other args.
const CONTEXT_ARG_SIZE_THRESHOLD: usize = 128 * 1024;

/// Maximum byte length of a context value that we export as a single
/// environment variable. The kernel rejects any individual argv/envp
/// string longer than `MAX_ARG_STRLEN` (PAGE_SIZE * 32 = 131072 on Linux)
/// with `E2BIG`. We cap conservatively below that so a pathologically large
/// value cannot make the spawn fail. Over-limit values are still delivered
/// to the runner via `--set` / `--context-file` for `{{placeholder}}`
/// substitution; only the env mirror is skipped (issue #784, regression
/// guard for the E2BIG / `--context-file` path).
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
    "NODE_OPTIONS",
    "PERL5OPT",
    "RUBYOPT",
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

/// Validate a single recipe context entry for env export (issue #784 / #1023),
/// independent of any budget accounting. Returns the uppercased env name on
/// success, or a static, name-only skip `reason` on rejection. The filters run
/// in the fixed order documented on [`context_env_pairs`].
fn validate_context_entry(key: &str, value: &str) -> Result<String, &'static str> {
    let name = key.to_ascii_uppercase();
    if !is_valid_env_identifier(&name) {
        return Err("invalid_identifier");
    }
    if name.starts_with("AMPLIHACK_") || RESERVED_ENV_DENYLIST.contains(&name.as_str()) {
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

/// Total bytes the current process environment will consume as it passes
/// THROUGH to the spawned recipe-runner (and is re-exported to every bash
/// step). Summed with the same per-entry accounting used for the mirror.
fn inherited_env_bytes() -> usize {
    std::env::vars_os()
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
///    `E2BIG`); they remain available via the recipe context file.
///
/// Aggregate budget (issue #1023): the total bytes of the exported mirror must
/// stay within `budget` so the environment inherited by every bash step cannot
/// cross the kernel's ARG_MAX. Surviving entries are partitioned into ESSENTIAL
/// keys (see [`is_essential_context_key`]) — always exported, even if that
/// alone exceeds the budget (a loud warning is logged) — and non-essential
/// keys, which fill the remaining budget SMALLEST-FIRST so the maximum number
/// of useful vars survive. A non-essential entry that does not fit is dropped
/// from the mirror ONLY; it is still delivered to the runner via
/// `--set` / `--context-file` (see [`pass_context`]) for `{{placeholder}}`
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

pub(super) fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    _verbose: bool,
    working_dir: &Path,
    search_dirs: &[PathBuf],
    step_timeout: Option<u64>,
) -> Result<RecipeRunResult> {
    // Issue #964: fail-closed recursion-depth guard. Refuse to spawn a nested
    // recipe-runner once the session has reached the configured maximum depth,
    // so a failing / misbehaving orchestration can never recursively re-enter
    // the orchestrator and fork-bomb the host. Runs BEFORE any work (binary
    // lookup, temp dirs, spawn) so no descendant is ever created past the limit.
    let guard = enforce_recursion_depth_guard()?;

    let binary = super::binary::find_recipe_runner_binary()?;
    let recipe_name = recipe_name_for_correlation(recipe_path);
    let correlation =
        RecipeRunCorrelation::new(recipe_name, working_dir, context, binary.as_path());
    let mut command = Command::new(&binary);
    command
        .arg(recipe_path)
        .arg("--output-format")
        .arg("json")
        .arg("-C")
        .arg(working_dir);

    // Issue #494: forward sub-recipe search dirs as -R flags so
    // recipe-runner-rs can resolve sub-recipes the same way amplihack
    // resolves top-level recipes. One -R per non-empty entry, in order.
    for dir in search_dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        command.arg("-R").arg(dir);
        tracing::debug!(dir = %dir.display(), "forwarding -R to recipe-runner-rs");
    }

    if dry_run {
        command.arg("--dry-run");
    }

    // Pass context as a file when the total size would risk E2BIG (os error 7).
    // The temp file is kept alive until the recipe runner child completes.
    let _context_file = pass_context(&mut command, context)?;

    // Issue #784 / #4583: export recipe context as environment variables so
    // bash steps (and nested sub-recipes, via OS inheritance) can read
    // $TASK_DESCRIPTION / $REPO_PATH under `set -u`. Applied at the LOWEST
    // precedence — written BEFORE EnvBuilder and the run-id below — so every
    // amplihack-managed/protective variable deterministically wins over any
    // colliding context key. Reserved/dangerous names are dropped upstream in
    // `context_env_pairs` (they are not EnvBuilder-managed). The aggregate byte
    // budget (#1023) keeps the total mirrored env under the kernel's ARG_MAX so
    // late bash steps cannot fail with "Argument list too long".
    command.envs(context_env_pairs(context, resolve_context_env_budget()));

    let runtime_dir = tempfile::Builder::new()
        .prefix("amplihack-workflow-")
        .tempdir()
        .context("failed to create isolated workflow runtime directory")?;
    let artifact_dir = runtime_dir.path().join("artifacts");
    let tmp_dir = runtime_dir.path().join("tmp");
    std::fs::create_dir_all(&artifact_dir)
        .context("failed to create isolated workflow artifact directory")?;
    std::fs::create_dir_all(&tmp_dir)
        .context("failed to create isolated workflow tmp directory")?;

    let env_builder = EnvBuilder::new()
        .with_agent_binary(active_agent_binary())
        .with_session_tree_context()
        .with_amplihack_home_from(working_dir)
        .with_asset_resolver()
        .with_pager_safe_defaults()
        .with_python_sanitization()
        .unset("CLAUDECODE")
        .set("AMPLIHACK_NONINTERACTIVE", "1")
        .with_project_graph_db(working_dir)?;

    // Issue #439: propagate --step-timeout as AMPLIHACK_STEP_TIMEOUT env var.
    // When Some(n), the child process sees AMPLIHACK_STEP_TIMEOUT=n (0 = disable).
    // When None, the env var is not injected (parent-inherited values flow through).
    let env_builder = match step_timeout {
        Some(seconds) => env_builder.set("AMPLIHACK_STEP_TIMEOUT", seconds.to_string()),
        None => env_builder,
    };

    env_builder.apply_to_command(&mut command);
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
    command.env("AMPLIHACK_RECIPE_RUN_ID", correlation.run_id());
    command.env("AMPLIHACK_WORKFLOW_RUNTIME_DIR", runtime_dir.path());
    command.env("AMPLIHACK_RUNTIME_ROOT", runtime_dir.path());
    command.env("AMPLIHACK_WORKFLOW_ARTIFACT_DIR", &artifact_dir);
    command.env("TMPDIR", &tmp_dir);

    // Issue #964: snapshot the caller checkout's git state BEFORE spawning so a
    // runner that corrupts it (e.g. flips `core.bare=true`, which breaks
    // `git status`) can be repaired on a terminal failure — leaving the caller's
    // checkout usable while preserving any durable child worktrees.
    let caller_git = CallerGitState::snapshot(working_dir);

    let result =
        spawn_with_streaming_stderr(command, correlation, recipe_path, recipe_runner_timeout());
    // Issue #964: restore on ANY terminal failure, not just a spawn/parse `Err`.
    // A runner that completes and emits a structured result reporting failure
    // (`Ok(RecipeRunResult { success: false, .. })`) is still a terminal failure
    // and is in fact the more likely path to leave the caller checkout corrupted
    // (it did real work before failing). Restoring only on `Err` would miss it.
    let terminal_failure = match &result {
        Err(_) => true,
        Ok(run_result) => !run_result.success,
    };
    if terminal_failure {
        caller_git.restore_on_failure();
    }
    result
}

/// Spawn the runner with stdout captured (we need to parse JSON from it)
/// and stderr "teed": each line is forwarded live to our own stderr AND
/// captured in a buffer so the error path can still surface a meaningful
/// stderr tail. (#357)
fn spawn_with_streaming_stderr(
    mut command: Command,
    correlation: RecipeRunCorrelation,
    recipe_path: &Path,
    timeout: Duration,
) -> Result<RecipeRunResult> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `pre_exec` runs after fork and before exec. `setsid` is
        // async-signal-safe and lets timeout cleanup terminate the recipe tree.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    correlation.emit_early();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _summary = correlation.emit_final(
                RecipeRunFinalStatus::SpawnFailure,
                None,
                None,
                known_log_paths(None),
            );
            return Err(error).context("failed to spawn recipe-runner-rs");
        }
    };
    let child_pid = Some(child.id());

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

    let status = match wait_for_recipe_runner(&mut child, timeout)
        .context("failed to wait for recipe-runner-rs")?
    {
        Some(status) => status,
        None => {
            let pid = child.id();
            terminate_recipe_runner(&mut child)?;
            let _summary = correlation.emit_final(
                RecipeRunFinalStatus::Failure,
                child_pid,
                None,
                known_log_paths(None),
            );
            anyhow::bail!(
                "recipe-runner-rs timed out after {:?} (pid {}, recipe {}, working dir {})",
                timeout,
                pid,
                recipe_path.display(),
                correlation.cwd()
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
    reap_recipe_runner_group(child.id(), recipe_runner_teardown_grace());

    let stdout_buf = stdout_rx
        .recv_timeout(RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT)
        .with_context(|| {
            format!(
                "recipe-runner-rs stdout did not close within {:?} after process exit",
                RECIPE_RUNNER_PIPE_DRAIN_TIMEOUT
            )
        })?
        .context("failed to read recipe-runner-rs stdout")?;
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
            let _summary = correlation.emit_final(
                final_status,
                child_pid,
                status.code(),
                known_log_paths(None),
            );
            Err(error).with_context(|| {
                format!(
                    "recipe-runner-rs exited with {}",
                    exit_status_label(&status)
                )
            })
        }
    }
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

/// Pass context key-value pairs to the command. When total serialised size
/// is small, uses `--set key=value` CLI args. When large, writes a JSON
/// file and passes `--context-file <path>` to avoid E2BIG (issues #209, #211).
///
/// Returns an `Option<tempfile::NamedTempFile>` that must be kept alive
/// until the child process has finished reading the file.
pub(super) fn pass_context(
    command: &mut Command,
    context: &BTreeMap<String, String>,
) -> Result<Option<tempfile::NamedTempFile>> {
    if context.is_empty() {
        return Ok(None);
    }

    let total_bytes: usize = context
        .iter()
        .map(|(k, v)| "--set".len() + k.len() + 1 + v.len())
        .sum();

    if total_bytes <= CONTEXT_ARG_SIZE_THRESHOLD {
        for (key, value) in context {
            command.arg("--set").arg(format!("{key}={value}"));
        }
        return Ok(None);
    }

    // Write context as JSON to a temp file.
    let mut tmp =
        tempfile::NamedTempFile::new().context("failed to create temp file for recipe context")?;
    serde_json::to_writer(&mut tmp, context)
        .context("failed to serialize recipe context to temp file")?;
    tmp.flush()
        .context("failed to flush recipe context temp file")?;

    command.arg("--context-file").arg(tmp.path());

    Ok(Some(tmp))
}

fn exit_status_label(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {} ({})", signal_name(signal), signal);
        }
    }

    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    match signal {
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
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

fn recipe_runner_timeout() -> Duration {
    std::env::var(RECIPE_RUNNER_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(RECIPE_RUNNER_DEFAULT_TIMEOUT)
}

fn wait_for_recipe_runner(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        // Single clock read per poll: derive the remaining budget once and reuse
        // it for both the deadline check and the sleep cap. A zero (or elapsed)
        // remaining is the timeout.
        let remaining = match timeout.checked_sub(started.elapsed()) {
            Some(remaining) if !remaining.is_zero() => remaining,
            _ => return Ok(None),
        };
        thread::sleep(RECIPE_RUNNER_POLL_INTERVAL.min(remaining));
    }
}

fn terminate_recipe_runner(child: &mut Child) -> Result<()> {
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
        let grace = recipe_runner_teardown_grace();
        let pgid = pid as libc::pid_t;
        signal_process_group(pgid, libc::SIGTERM);
        wait_until_exited(grace, || matches!(child.try_wait(), Ok(None)));
        signal_process_group(pgid, libc::SIGKILL);
    }
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
/// `RECIPE_RUNNER_POLL_INTERVAL` steps (floored at 1ms, never past the remaining
/// grace) so a zero grace escalates almost immediately. Shared by both teardown
/// paths so the SIGTERM -> grace -> SIGKILL timing policy lives in one place.
#[cfg(unix)]
fn wait_until_exited(grace: Duration, mut still_running: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while still_running() {
        let elapsed = started.elapsed();
        if elapsed >= grace {
            return false;
        }
        let remaining = grace.saturating_sub(elapsed);
        thread::sleep(
            RECIPE_RUNNER_POLL_INTERVAL
                .min(remaining)
                .max(Duration::from_millis(1)),
        );
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
/// SIGKILL contract, scoped strictly to `-pgid`.
#[cfg(unix)]
fn reap_recipe_runner_group(pgid_pid: u32, grace: Duration) {
    let pgid = pgid_pid as libc::pid_t;
    // `kill(-pgid, 0)` probes group liveness without delivering a signal.
    let group_alive = || unsafe { libc::kill(-pgid, 0) } == 0;
    // Fast path: the group is already empty (well-behaved runner, no orphans).
    if !group_alive() {
        return;
    }
    signal_process_group(pgid, libc::SIGTERM);
    if !wait_until_exited(grace, group_alive) {
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
///   `MAX_DEPTH_CEILING`, and an unset / malformed value falls back to
///   `DEFAULT_MAX_DEPTH`;
/// * below the limit the caller spawns normally (no over-blocking).
///
/// Logs numeric depth/limit fields only — never env-var *values*, which may
/// carry session tokens or secrets.
fn enforce_recursion_depth_guard() -> Result<SpawnGuard> {
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
        .and_then(|value| value.trim().parse::<u32>().ok());
    let tree_id = std::env::var(TREE_ID_ENV)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
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

/// Pre-run snapshot of the caller checkout's git state needed to keep it usable
/// after a terminal recipe-runner failure (issue #964).
///
/// The observed corruption was a runner flipping the caller checkout's
/// `core.bare` to `true`, which makes `git status` fail with
/// `this operation must be run in a work tree`. We snapshot `core.bare` before
/// spawning and, on any terminal failure, restore the pre-run value so the
/// caller's checkout is left usable. This is intentionally scoped to the single
/// `core.bare` key on the caller checkout: it never touches the work tree, and
/// never deletes or unregisters durable child worktrees produced by the run.
struct CallerGitState {
    /// The caller checkout (the runner's working directory).
    dir: PathBuf,
    /// `true` only if `dir` was a usable git work tree before the run. When
    /// `false` there is nothing to restore and we must never "fix" a non-repo
    /// into a repo.
    was_git_checkout: bool,
    /// Pre-run value of `core.bare` (`None` == unset). A `None` snapshot is
    /// never restored as an explicit `false`; absence is preserved as absence.
    core_bare: Option<String>,
}

impl CallerGitState {
    /// Capture the caller checkout's `core.bare` before spawning. Returns a
    /// snapshot with `was_git_checkout = false` (a no-op restore) when `dir` is
    /// not a git work tree or git is unavailable.
    fn snapshot(dir: &Path) -> Self {
        let was_git_checkout =
            git_capture(dir, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true");
        let core_bare = if was_git_checkout {
            git_capture(dir, &["config", "--local", "--get", "core.bare"])
        } else {
            None
        };
        Self {
            dir: dir.to_path_buf(),
            was_git_checkout,
            core_bare,
        }
    }

    /// Best-effort restore of the caller checkout to the snapshotted `core.bare`,
    /// run only after a terminal failure. Never bails; a restore failure is
    /// surfaced via structured `tracing` (issue #964 requirement 5) and never
    /// masks the original error. Preserves durable child worktrees (config-only).
    fn restore_on_failure(&self) {
        if !self.was_git_checkout {
            return;
        }
        let current = git_capture(&self.dir, &["config", "--local", "--get", "core.bare"]);
        if current == self.core_bare {
            return; // caller checkout untouched — nothing to restore.
        }

        let restored = match &self.core_bare {
            Some(value) => git_run(&self.dir, &["config", "--local", "core.bare", value]),
            // Was unset before the run: unset whatever the runner left behind.
            None => git_run(&self.dir, &["config", "--local", "--unset", "core.bare"]),
        };

        if restored {
            tracing::warn!(
                dir = %self.dir.display(),
                "restored caller checkout core.bare after terminal recipe-runner failure (issue #964)"
            );
        } else {
            tracing::error!(
                dir = %self.dir.display(),
                "failed to restore caller checkout git state after terminal recipe-runner failure (issue #964)"
            );
        }
    }
}

/// Run `git -C dir <args>` and return trimmed stdout, or `None` if git is
/// unavailable or the command failed (e.g. `--get` of an unset key exits
/// non-zero, which we map to `None` == "unset").
fn git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `git -C dir <args>` for side effects; `true` on success. `--unset` of an
/// already-absent key exits non-zero (code 5) — treated as a non-fatal no-op so
/// restoring "was unset, still unset" is not reported as a failure.
fn git_run(dir: &Path, args: &[&str]) -> bool {
    match std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
    {
        Ok(status) if status.success() => true,
        // `git config --unset` of a missing key -> exit code 5; benign here.
        Ok(status) if args.contains(&"--unset") && status.code() == Some(5) => true,
        _ => false,
    }
}
