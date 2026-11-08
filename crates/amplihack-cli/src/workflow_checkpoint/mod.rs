//! Publication checkpoint engine for the default workflow (#1554).
//!
//! The contract (actions, argument types, exit statuses, environment and the
//! owner-authorised differences from the Python it replaces) is written
//! once, in `docs/reference/workflow-checkpoint-cli.md`. This module
//! implements it.
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod actions;
mod advert;
mod authority;
mod history;
pub(crate) mod json;
mod json_errors;
mod num;
mod py_printable;
mod repo;
mod strict;

#[cfg(test)]
mod legacy_parity_tests;
#[cfg(test)]
mod spec_tests;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::num::NonZeroI32;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use crate::command_error::exit_error;

/// Repository selectors that can change the effective repository, object
/// store, index, configuration or pathspec interpretation. Verbatim from
/// `workflow_publish_checkpoint.sh:75-79` at 918c256cf. Every
/// `Selectors::Context` row refuses when one is set (an empty value counts),
/// before any Git child starts. The `Selectors::Ignored` rows never look at
/// them; the one that runs Git (`stage-strict`) passes them through, as 918c
/// did. None is ever unset by the refusing rows; their Git children are
/// built by `amplihack_git::command()`, which removes them.
pub const FORBIDDEN_SELECTORS: [&str; 17] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// Selector names that are not in 918c's refusal list, each with the reason
/// it is not refused. The bounded route accepts them, as 918c did; refusing
/// any of them would be a departure that needs bb's decision. Four of them
/// (`GIT_DISCOVERY_ACROSS_FILESYSTEM`, `GIT_INDEX_VERSION`, `GIT_PREFIX`,
/// `GIT_QUARANTINE_PATH`) are removed from bounded Git children by
/// `amplihack_git::command()`, whereas 918c's `git()` passed them through;
/// that difference is listed for bb under "Unresolved differences for owner
/// disposition".
pub const SELECTORS_NOT_REFUSED: [(&str, &str); 6] = [
    (
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "not in 918c's list; command() strips it",
    ),
    (
        "GIT_IMPLICIT_WORK_TREE",
        "printed by --local-env-vars; set internally by git",
    ),
    (
        "GIT_INDEX_VERSION",
        "not in 918c's list; command() strips it",
    ),
    (
        "GIT_NO_REPLACE_OBJECTS",
        "printed by --local-env-vars; 918c's git() set it itself",
    ),
    (
        "GIT_PREFIX",
        "printed by --local-env-vars; set by git for aliases and hooks",
    ),
    (
        "GIT_QUARANTINE_PATH",
        "not in 918c's list; command() strips it",
    ),
];

/// How an action takes its positional arguments, as the 918c Python read
/// `sys.argv`. No action refuses extra arguments unless its 918c code did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arity {
    /// `sys.argv` beyond the action is never read; extra arguments are
    /// ignored.
    Ignored,
    /// The first `n` arguments are indexed directly. A missing one was an
    /// uncaught `IndexError` (status 1) at the point the Python read it;
    /// extra arguments are ignored.
    Indexed(usize),
    /// The action checks its own argument count and refuses (status 2), as
    /// the 918c `require(len(sys.argv) ...)` did.
    Checked,
}

/// Whether the inherited Git selectors are refused before the action. The
/// column also decides the Git builder (`repo::GitEnv`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selectors {
    /// Refused (status 2) by `selectors()`, which the action reaches at the
    /// point 918c did: directly, or first thing in `context()`. Git children
    /// strip them (`GitEnv::Strip`).
    Context,
    /// Not inspected: the 918c snippet never looked at them, and any Git it
    /// ran inherited them (`GitEnv::Inherit`).
    Ignored,
}

/// How a 918c snippet decoded bytes it took from argv, the environment or
/// Git, and what the port does with the same bytes (decision 17.138 #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decoding {
    /// Kept: decoded with `surrogateescape` and written back as 918c wrote it.
    Surrogateescape,
    /// Refused with status 2 where 918c would have carried `\udcXX` through
    /// (17.138 #1 accepts this difference).
    RefusedNonUtf8,
}

/// 17.138 #1 audit: every site where 918c decoded bytes with
/// `surrogateescape`, and the port's behaviour for non-UTF-8 input there.
/// `surrogateescape_audit_matches_decision_17_138_1` pins each row.
pub const SURROGATEESCAPE_AUDIT: [(&str, Decoding); 8] = [
    (
        "cleanup-skip-result reason (argv)",
        Decoding::Surrogateescape,
    ),
    (
        "worktree-path RECIPE_VAR_worktree_setup",
        Decoding::Surrogateescape,
    ),
    (
        "admission-refused-result RECIPE_VAR_worktree_setup",
        Decoding::Surrogateescape,
    ),
    (
        "advertised-ref status (argv): int() ValueError, status 1",
        Decoding::Surrogateescape,
    ),
    ("text action arguments (argv)", Decoding::RefusedNonUtf8),
    ("RECIPE_VAR_* environment text", Decoding::RefusedNonUtf8),
    ("Git output compared as text", Decoding::RefusedNonUtf8),
    ("private paths written to JSON", Decoding::RefusedNonUtf8),
];

/// What a malformed advertisement means for this action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdvertStatus {
    /// `CHECKPOINT_PY` `destination_tip`: a `require` refusal, status 2.
    Refusal,
    /// `ORIGIN_REF_PY`: `sys.exit(78)` with nothing on stderr.
    Exit78,
    /// The action reads no advertisement.
    None,
}

/// When stdout is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StdoutPolicy {
    /// Only after the action finished with status 0.
    OnSuccess,
    /// The delegated verifier's stdout is copied through on every status,
    /// as 918c `verify-pushed-ref` did before `sys.exit(returncode)`.
    Verifier,
}

/// One row of the action table. Each column cites the 918c256cf Python it
/// keeps.
#[derive(Debug)]
pub struct ActionSpec {
    pub name: &'static str,
    pub arity: Arity,
    pub selectors: Selectors,
    pub advert: AdvertStatus,
    pub stdout: StdoutPolicy,
    /// Where the replaced Python lives at 918c256cf.
    pub source: &'static str,
}

const fn spec(
    name: &'static str,
    arity: Arity,
    selectors: Selectors,
    advert: AdvertStatus,
    source: &'static str,
) -> ActionSpec {
    ActionSpec {
        name,
        arity,
        selectors,
        advert,
        stdout: StdoutPolicy::OnSuccess,
        source,
    }
}

const CP: &str = "amplifier-bundle/tools/workflow_publish_checkpoint.sh";

/// The closed set of actions. The wrappers call each of them; nothing else
/// is dispatched. An unknown name is `Refusal('unknown private checkpoint
/// operation')` (`workflow_publish_checkpoint.sh:992`).
pub const ACTIONS: [ActionSpec; 16] = [
    // :764 `selectors()`.
    spec(
        "selectors",
        Arity::Ignored,
        Selectors::Context,
        AdvertStatus::None,
        CP,
    ),
    // :766 `context(sys.argv[2])`.
    spec(
        "capture",
        Arity::Indexed(1),
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :782 `phase = sys.argv[2]`.
    spec(
        "admit",
        Arity::Indexed(1),
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :847 `check()`.
    spec(
        "check",
        Arity::Ignored,
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :847-855 `check()` then `pending()`.
    spec(
        "paths",
        Arity::Ignored,
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // workflow_stage_changes.sh:69 STRICT_PATHS_PY `sys.argv[1]`.
    spec(
        "stage-strict",
        Arity::Indexed(1),
        Selectors::Ignored,
        AdvertStatus::None,
        "amplifier-bundle/tools/workflow_stage_changes.sh",
    ),
    // :856 `require(len(sys.argv) >= 3 ...)`.
    spec(
        "before",
        Arity::Checked,
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :898 `require(len(sys.argv) == 3 ...)`.
    spec(
        "sync-fetch-mode",
        Arity::Checked,
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :940-946 `sys.argv[2]`, then `sys.argv[3]`, `int(sys.argv[4])`.
    spec(
        "after",
        Arity::Indexed(3),
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :822 `check()`.
    spec(
        "destination-is-head",
        Arity::Ignored,
        Selectors::Context,
        AdvertStatus::Refusal,
        CP,
    ),
    // :829-831 `check()`, then `require(len(sys.argv) == 5 ...)`.
    ActionSpec {
        name: "verify-pushed-ref",
        arity: Arity::Checked,
        selectors: Selectors::Context,
        advert: AdvertStatus::Refusal,
        stdout: StdoutPolicy::Verifier,
        source: CP,
    },
    // workflow_pr_head_sync.sh:83 ORIGIN_REF_PY `sys.argv[1..3]`.
    spec(
        "advertised-ref",
        Arity::Indexed(3),
        Selectors::Ignored,
        AdvertStatus::Exit78,
        "amplifier-bundle/tools/workflow_pr_head_sync.sh",
    ),
    // :1010 inline `hashlib.sha256(sys.stdin.buffer.read())`.
    spec(
        "sha256",
        Arity::Ignored,
        Selectors::Ignored,
        AdvertStatus::None,
        CP,
    ),
    // :1155 and :1184 ADMISSION_RESULT_PY.
    spec(
        "admission-refused-result",
        Arity::Ignored,
        Selectors::Ignored,
        AdvertStatus::None,
        CP,
    ),
    // workflow_cleanup_checkpoint.sh:17 CLEANUP_SKIP_JSON `sys.argv[1]`.
    spec(
        "cleanup-skip-result",
        Arity::Indexed(1),
        Selectors::Ignored,
        AdvertStatus::None,
        "amplifier-bundle/tools/workflow_cleanup_checkpoint.sh",
    ),
    // workflow-publish.yaml:137,179 and workflow-pr-review.yaml:204,246.
    spec(
        "worktree-path",
        Arity::Ignored,
        Selectors::Ignored,
        AdvertStatus::None,
        "amplifier-bundle/recipes/workflow-publish.yaml",
    ),
];
/// Why an action stopped. Each variant has exactly one exit status.
#[derive(Debug)]
pub(crate) enum Fail {
    /// A checked condition failed: a 918c `Refusal` or a caught
    /// `OSError`/`ValueError`/`KeyError`/`TypeError`. Exit 2 with one
    /// refusal line.
    Refusal(String),
    /// A malformed advertisement. Its status is the action's
    /// [`AdvertStatus`] column; see [`advert_fail`].
    Advert(advert::AdvertError),
    /// A Git command exited 79. Passed through unchanged; never a refusal.
    Terminal79,
    /// The delegated verifier's own non-zero status. Only
    /// `verify-pushed-ref` produces this.
    Passthrough(NonZeroI32),
    /// What 918c left uncaught (a missing `sys.argv` item, an exception
    /// outside the `except` list, any error in a snippet with no handler).
    /// CPython exits 1 with a traceback; this exits 1 with one line.
    Crash(String),
}

/// Result of an exact ref lookup in a validated advertisement.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// Exit 0.
    Present,
    /// Exit 3: the exact ref is absent from a fully validated advertisement.
    Absent,
}

pub(crate) type Res<T> = Result<T, Fail>;

pub(crate) fn refuse(ok: bool, reason: &str) -> Res<()> {
    if ok {
        Ok(())
    } else {
        Err(Fail::Refusal(reason.to_owned()))
    }
}

/// Convert an I/O error at one site into a refusal that names the site.
pub(crate) fn io_refusal(what: &'static str) -> impl Fn(std::io::Error) -> Fail {
    move |error| Fail::Refusal(format!("{what}: {error}"))
}

/// The one place an advertisement error becomes a status: the action's
/// [`AdvertStatus`] column decides.
pub(crate) fn advert_fail(spec: &ActionSpec, error: advert::AdvertError) -> Fail {
    match spec.advert {
        AdvertStatus::Exit78 => Fail::Passthrough(advert::EXIT_78),
        AdvertStatus::Refusal | AdvertStatus::None => Fail::Refusal(error.checkpoint_reason()),
    }
}

/// The status-bearing error `run` returns for a failure. Only a refusal and
/// a crash write to stderr here; the other statuses have already said why.
pub(crate) fn to_error(fail: Fail, prefix: &str) -> anyhow::Error {
    match fail {
        Fail::Refusal(reason) => {
            eprintln!("{prefix}{reason}");
            exit_error(2)
        }
        Fail::Advert(error) => {
            eprintln!("{prefix}{}", error.checkpoint_reason());
            exit_error(2)
        }
        Fail::Terminal79 => exit_error(79),
        Fail::Passthrough(code) => exit_error(code.get()),
        Fail::Crash(reason) => {
            eprintln!("{CRASH_PREFIX}{reason}");
            exit_error(1)
        }
    }
}

pub(crate) const REFUSAL_PREFIX: &str = "checkpoint scope refusal: ";
const STRICT_PREFIX: &str = "stage_workflow_changes: strict refusal: ";
pub const CRASH_PREFIX: &str = "checkpoint internal error: ";
/// The panic hook's one fixed line. No location, payload or backtrace.
pub const PANIC_MESSAGE: &str = "checkpoint internal error: unexpected panic";

/// Replace the default panic hook for `internal workflow-checkpoint`: one
/// fixed line on stderr, so a panic never prints source paths or payloads.
/// `run` then exits 1, as an uncaught Python exception did.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(std::io::stderr(), "{PANIC_MESSAGE}");
    }));
}

/// State one invocation keeps between its checks. Built only by
/// [`Ctx::for_row`], so the Git builder always comes from the action row.
pub(crate) struct Ctx {
    /// The owned worktree top, once `context` has verified it.
    pub root: PathBuf,
    /// Advertisements read in this process, by push-URL digest. Never
    /// written anywhere; a push's before check reuses the one actual read.
    pub advertisements: BTreeMap<String, advert::AdvertMap>,
    /// The publication view from the latest scope check.
    pub checked_publication: Option<serde_json::Map<String, serde_json::Value>>,
    /// Private authority directory, set only after the authority record has
    /// been loaded and verified (admission).
    pub observation_directory: Option<PathBuf>,
    /// The Git builder for this action, from its row. Set once by
    /// [`Ctx::for_row`]; handlers read it, never set it.
    pub git_env: repo::GitEnv,
}

impl Ctx {
    /// The empty state for one invocation of `spec`, with the Git builder
    /// taken from the row's `Selectors` column.
    pub(crate) fn for_row(spec: &ActionSpec) -> Self {
        Self {
            root: PathBuf::new(),
            advertisements: BTreeMap::new(),
            checked_publication: None,
            observation_directory: None,
            git_env: repo::GitEnv::from(spec.selectors),
        }
    }
}

/// Run `body`, mapping a panic to status 1, the status of an uncaught
/// Python exception, rather than an abort or 101.
pub(crate) fn catch_panic<T>(
    action: &str,
    body: impl FnOnce() -> Res<T>,
) -> anyhow::Result<Res<T>> {
    catch_unwind(AssertUnwindSafe(body)).map_err(|_| {
        let _ = action;
        exit_error(1)
    })
}

/// 17.138 #5: pick the action row from raw argv before anything reads a
/// flag. Pure: no environment, no I/O.
///
/// - empty argv: 918c `sys.argv[1]` IndexError, status 1;
/// - a name that is empty, not UTF-8, starts with '-' (`--help`, `-h`, `-V`,
///   `-x`, `--`, `-`), or is not in [`ACTIONS`]: the reference
///   'unknown private checkpoint operation' refusal, status 2;
/// - otherwise the row, with the arguments after it unchanged.
pub(crate) fn gate(argv: &[OsString]) -> Result<(&'static ActionSpec, &[OsString]), Fail> {
    let Some((first, rest)) = argv.split_first() else {
        return Err(Fail::Crash("IndexError: list index out of range".into()));
    };
    let unknown = || Fail::Refusal("unknown private checkpoint operation".into());
    let name = first.to_str().ok_or_else(unknown)?;
    if name.is_empty() || name.starts_with('-') {
        return Err(unknown());
    }
    let spec = ACTIONS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(unknown)?;
    Ok((spec, rest))
}

/// Run one checkpoint invocation from raw argv: everything after
/// `internal workflow-checkpoint`. `amplihack`'s `main` calls this before
/// clap parses anything, so no token can be read as a flag.
pub fn run_argv(argv: &[OsString]) -> anyhow::Result<()> {
    match gate(argv) {
        Ok((spec, args)) => run_spec(spec, args),
        Err(fail) => Err(to_error(fail, REFUSAL_PREFIX)),
    }
}

/// The action name and its arguments, for in-process callers. It goes
/// through the same [`gate`].
pub fn run(action: &OsStr, args: Vec<OsString>) -> anyhow::Result<()> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(action.to_os_string());
    argv.extend(args);
    run_argv(&argv)
}

/// Run one action row. Stdout is written only when the action succeeds,
/// except for the verifier's own output (`StdoutPolicy::Verifier`).
fn run_spec(spec: &'static ActionSpec, args: &[OsString]) -> anyhow::Result<()> {
    let prefix = if spec.name == "stage-strict" {
        STRICT_PREFIX
    } else {
        REFUSAL_PREFIX
    };
    let mut ctx = Ctx::for_row(spec);
    let outcome = catch_panic(spec.name, || actions::dispatch(&mut ctx, spec.name, args))?;
    let outcome = outcome.map_err(|fail| match fail {
        Fail::Advert(error) => advert_fail(spec, error),
        other => other,
    });
    match outcome {
        Ok(actions::Done::Absent) => Err(exit_error(3)),
        Ok(actions::Done::Stdout(stdout)) => write_stdout(&stdout),
        Ok(actions::Done::Verifier(stdout, status)) => {
            write_stdout(&stdout)?;
            match status {
                Some(code) => Err(exit_error(code.get())),
                None => Ok(()),
            }
        }
        Err(fail) => {
            if let Fail::Refusal(reason) = &fail {
                // Evidence only, and only after admission. A failed write
                // must not change the refusal.
                let _ = authority::observation(
                    &ctx,
                    &serde_json::json!({"boundary": "refusal", "reason": reason}),
                );
            }
            Err(to_error(fail, prefix))
        }
    }
}

fn write_stdout(stdout: &[u8]) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(stdout)
        .and_then(|()| out.flush())
        .map_err(|e| anyhow::anyhow!("checkpoint stdout: {e}"))
}

/// The `n`th action argument, read where the 918c Python indexed
/// `sys.argv`: a missing one is that `IndexError` (status 1).
pub(crate) fn indexed(args: &[OsString], n: usize) -> Res<&OsString> {
    args.get(n)
        .ok_or_else(|| Fail::Crash("IndexError: list index out of range".into()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod selector_set_tests {
    use super::{FORBIDDEN_SELECTORS, SELECTORS_NOT_REFUSED};
    use std::collections::BTreeSet;
    use std::sync::OnceLock;

    /// 918c's `selectors()` list, in its order
    /// (`workflow_publish_checkpoint.sh:75-79` at 918c256cf).
    #[test]
    fn forbidden_selectors_are_exactly_918c_in_order() {
        assert_eq!(
            FORBIDDEN_SELECTORS,
            [
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_COMMON_DIR",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_CONFIG",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_PARAMETERS",
                "GIT_NAMESPACE",
                "GIT_SHALLOW_FILE",
                "GIT_GRAFT_FILE",
                "GIT_REPLACE_REF_BASE",
                "GIT_CEILING_DIRECTORIES",
                "GIT_GLOB_PATHSPECS",
                "GIT_NOGLOB_PATHSPECS",
                "GIT_ICASE_PATHSPECS",
            ]
        );
    }

    /// The two assignment lists never overlap, so a name is either refused
    /// or not refused, never both, and every unrefused name has a reason.
    #[test]
    fn the_two_selector_lists_are_disjoint() {
        let refused: BTreeSet<&str> = FORBIDDEN_SELECTORS.into_iter().collect();
        let accepted: BTreeSet<&str> = SELECTORS_NOT_REFUSED.iter().map(|(n, _)| *n).collect();
        assert!(
            refused.is_disjoint(&accepted),
            "{:?}",
            refused.intersection(&accepted)
        );
        assert_eq!(
            refused.len(),
            FORBIDDEN_SELECTORS.len(),
            "duplicate refusal"
        );
        assert_eq!(
            accepted.len(),
            SELECTORS_NOT_REFUSED.len(),
            "duplicate assignment"
        );
        assert!(SELECTORS_NOT_REFUSED.iter().all(|(_, why)| !why.is_empty()));
    }

    /// Assigned names that `git rev-parse --local-env-vars` does not print,
    /// each with the reason it is assigned anyway. The reverse drift check
    /// accepts only these.
    const NOT_PRINTED_BY_GIT: [(&str, &str); 8] = [
        (
            "GIT_NAMESPACE",
            "in 918c's list; git treats it as a ref prefix, not a local selector",
        ),
        (
            "GIT_CEILING_DIRECTORIES",
            "in 918c's list; bounds repository discovery",
        ),
        (
            "GIT_GLOB_PATHSPECS",
            "in 918c's list; changes pathspec interpretation",
        ),
        (
            "GIT_NOGLOB_PATHSPECS",
            "in 918c's list; changes pathspec interpretation",
        ),
        (
            "GIT_ICASE_PATHSPECS",
            "in 918c's list; changes pathspec interpretation",
        ),
        ("GIT_DISCOVERY_ACROSS_FILESYSTEM", "command() strips it"),
        ("GIT_INDEX_VERSION", "command() strips it"),
        ("GIT_QUARANTINE_PATH", "command() strips it"),
    ];

    /// The host's `git rev-parse --local-env-vars`, queried once per run.
    fn host_local_env_vars() -> &'static BTreeSet<String> {
        static HOST: OnceLock<BTreeSet<String>> = OnceLock::new();
        HOST.get_or_init(|| {
            let out = amplihack_git::command()
                .args(["rev-parse", "--local-env-vars"])
                .stdin(std::process::Stdio::null())
                .output()
                .expect("git rev-parse --local-env-vars runs");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        })
    }

    fn assigned() -> BTreeSet<&'static str> {
        FORBIDDEN_SELECTORS
            .into_iter()
            .chain(SELECTORS_NOT_REFUSED.iter().map(|(n, _)| *n))
            .collect()
    }

    /// Drift check against the host's git: a name the host calls local that
    /// is in neither list fails, with its name.
    #[test]
    fn every_host_local_env_var_is_assigned() {
        let assigned = assigned();
        let unassigned: Vec<&String> = host_local_env_vars()
            .iter()
            .filter(|n| !assigned.contains(n.as_str()))
            .collect();
        assert!(
            unassigned.is_empty(),
            "unassigned selector(s): {unassigned:?}"
        );
    }

    /// Reverse drift check: an assigned name the host does not print must be
    /// a documented exception, and every exception must be assigned.
    #[test]
    fn every_assigned_name_is_printed_by_git_or_a_documented_exception() {
        let assigned = assigned();
        let host = host_local_env_vars();
        let exceptions: BTreeSet<&str> = NOT_PRINTED_BY_GIT.iter().map(|(n, _)| *n).collect();
        assert!(NOT_PRINTED_BY_GIT.iter().all(|(_, why)| !why.is_empty()));
        let undocumented: Vec<&str> = assigned
            .iter()
            .copied()
            .filter(|n| !host.contains(*n) && !exceptions.contains(n))
            .collect();
        assert!(
            undocumented.is_empty(),
            "assigned but neither printed by git nor documented: {undocumented:?}"
        );
        let stray: Vec<&str> = exceptions.difference(&assigned).copied().collect();
        assert!(stray.is_empty(), "exceptions not assigned: {stray:?}");
    }
}
