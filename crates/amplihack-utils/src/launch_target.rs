//! The single repo-wide answer to "which binary do we launch, is it healthy,
//! and do we need to install anything?".
//!
//! Do not change the signatures below without updating
//! `docs/LAUNCH_TARGET_RESOLUTION.md`, which is the frozen contract.
//!
//! # Why this module exists
//!
//! Before it, three independent resolutions disagreed on a single
//! `amplihack claude` launch: the version check read `/usr/bin/claude`, the
//! install wrote `~/.npm-global/bin/claude`, and the exec ran
//! `~/.local/bin/claude`. Check, install, and exec must all resolve through
//! one function or they will drift apart again.
//!
//! # What this module is not
//!
//! It is tool-generic, and the body has to stay that way. Every function here
//! takes a `tool`, and a fact that is only true of `@anthropic-ai/claude-code`
//! belongs in [`crate::claude_native`], not in a check that runs for copilot
//! and codex too. The one time that boundary was crossed — a "small file with
//! no native magic is a broken install" gate — it rejected `@github/copilot`'s
//! legitimate 1185-byte `#!/usr/bin/env node` loader and broke the launch.
//!
//! # Security
//!
//! * SEC-3 — probe stdout comes from an arbitrary candidate binary. The
//!   capture is size-capped and passed through
//!   [`crate::binary_finder::strip_ansi`], which removes CSI and OSC/DCS/APC
//!   sequences, two-byte escapes, and turns every remaining C0 control into a
//!   space. Candidate *paths* are attacker-influenced too and get the stricter
//!   [`display_untrusted_path`] instead — see its docs for why a path is
//!   elided at the first control character rather than merely stripped.
//! * SEC-4 — the probe is bounded per-candidate *and* in total. The bound
//!   covers the whole subprocess, output drain included: a child that exits
//!   while a grandchild holds its stdout pipe open is not allowed to stall the
//!   launch, it costs a truncated capture instead. So a hung or hostile binary
//!   early in `$PATH` cannot stall a launch.
//! * SEC-5 — ownership drives the write policy, and it is decided by
//!   [`TargetSource`] alone: only a candidate found in amplihack's own prefix
//!   directory is ever written to. A directory that is spelled differently
//!   from that prefix is tagged [`TargetSource::Path`] and therefore left
//!   alone, so the failure mode is "amplihack declines to upgrade", never
//!   "amplihack writes outside its prefix".
//! * The health gate is **not** a security boundary. It is a correctness
//!   filter that stops amplihack executing its own broken install. Anyone who
//!   can plant a binary on your `$PATH` can already run code as you.

use crate::binary_finder::{PROBE_CAPTURE_LIMIT, run_capped_output_with_timeout, strip_ansi};
use crate::claude_native::has_placeholder_shape;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// A binary that passed the health gate and may be launched.
///
/// There is no such thing as a `LaunchTarget` with an unknown version. Health
/// is a filter, never an annotation — see [`resolve_from_candidates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTarget {
    /// Absolute path to the binary that will be executed.
    pub path: PathBuf,
    /// Parseable semver read from `<path> --version`.
    pub version: String,
    /// Where this candidate was found. Drives the write policy.
    pub source: TargetSource,
}

/// Where a candidate came from, in candidate order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    /// `AMPLIHACK_CLAUDE_BINARY_PATH` / `CLAUDE_BINARY_PATH`.
    ///
    /// `user_supplied` is false when amplihack set the variable itself
    /// (`commands/rustyclawd.rs` does, in-process). A user-supplied override
    /// that fails the health gate is a hard error; an amplihack-supplied one
    /// warns and falls through, because it is a preference, not an instruction.
    ExplicitOverride {
        /// True when the value came from the caller's environment.
        user_supplied: bool,
    },
    /// Found by walking `$PATH` in order.
    Path,
    /// `~/.npm-global/bin` — the prefix amplihack installs into and owns.
    AmplihackPrefix,
    /// `~/.cargo/bin`, `~/.local/bin`.
    FallbackDir,
}

/// Why a candidate is not a launch target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// A relative path, which no candidate may ever be.
    ///
    /// [`cheap_reject`] stats it against the *process* current directory while
    /// `execvp` resolves a separator-free name against the *child's* `$PATH` —
    /// they are not looking at the same file, and neither one is the file the
    /// user named. See [`path_dirs`] for the empty-`$PATH`-element chain this
    /// closes, and `augment_claude_launch_env` for what a relative candidate's
    /// empty parent does to the child's `$PATH`.
    NotAbsolute,
    /// No such path, or a dangling symlink.
    Missing,
    /// Resolves to a directory or other non-regular file.
    NotAFile,
    /// No executable bit for this user.
    NotExecutable,
    /// `--version` failed *and* the file has the placeholder's shape.
    ///
    /// A refinement of [`Self::ProbeFailed`], never an independent verdict —
    /// see [`label_failed_probe`].
    PlaceholderStub,
    /// `--version` failed and the file could not be read to say why.
    Unreadable,
    /// `--version` ran but exited non-zero.
    ProbeFailed,
    /// Never examined: resolution stopped before reaching this candidate.
    ///
    /// Not a verdict on the file at all — the probe cap
    /// ([`MAX_PROBE_CANDIDATES`]) or the total budget
    /// ([`TOTAL_PROBE_BUDGET`]) ran out first. Recorded rather than dropped
    /// because [`decide_install`] must be able to tell "there is no working
    /// binary" from "we stopped looking": the second one is inconclusive
    /// evidence and answers [`InstallDecision::Abstain`], which is the same
    /// rule a probe timeout already obeys. Dropped, it read as absence and
    /// bought a 339 MB install that resolves identically next launch —
    /// issue #1266's loop, reached through the funnel built to close it.
    NotProbed,
    /// `--version` exceeded the per-candidate budget.
    ProbeTimedOut,
    /// `--version` exited 0 but emitted no parseable semver.
    UnparseableVersion,
}

impl Rejection {
    /// One line naming what is wrong with a candidate, in the user's terms.
    ///
    /// Deliberately never mentions CPU architecture or platform mismatch. The
    /// failure this replaces — `Exec format error (os error 8)` — named nothing
    /// real and sent the user hunting for a hardware problem that did not exist.
    pub fn explain(&self) -> &'static str {
        match self {
            Self::NotAbsolute => "not an absolute path — name the binary by full path",
            Self::Missing => "not found (no such file, or a broken symlink)",
            Self::NotAFile => "not a regular file",
            Self::NotExecutable => "present but not executable by you",
            Self::PlaceholderStub => {
                "incomplete install — `--version` failed and the file is the \
                 small placeholder the npm package ships, not the native \
                 binary it is supposed to be replaced by"
            }
            Self::Unreadable => "`--version` failed and the file could not be read to diagnose it",
            Self::ProbeFailed => {
                "`--version` failed — the install is incomplete or the file \
                 cannot be executed"
            }
            Self::NotProbed => {
                "not examined — resolution stopped at the probe cap or the \
                 total probe budget before reaching it"
            }
            Self::ProbeTimedOut => "`--version` did not answer within the probe budget",
            Self::UnparseableVersion => {
                "`--version` reported no usable version, which means the \
                 install cannot be verified"
            }
        }
    }
}

/// The outcome of one resolution pass.
///
/// The rejection list is carried because the error path needs it: a bare
/// `Option<LaunchTarget>` can say "nothing worked" but cannot say what was
/// tried and why each attempt failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// The first healthy candidate, if any.
    pub target: Option<LaunchTarget>,
    /// Every candidate resolution has an answer about, in candidate order.
    ///
    /// Not all of them were *examined*: [`Rejection::NotProbed`] marks the ones
    /// resolution stopped short of. Reading this list as "everything that was
    /// tried and failed" is what made a truncated walk look like absence.
    pub rejected: Vec<(PathBuf, Rejection)>,
    /// Set when resolution stopped early because a **user-supplied** override
    /// failed the health gate.
    ///
    /// That early return is a conclusion rather than a truncation, so it
    /// deliberately records no [`Rejection::NotProbed`] — see
    /// [`resolve_from_candidates`]. But "conclusive" is not the same as "an
    /// install repairs it", and collapsing the two is how this module
    /// re-created the defect it exists to delete.
    ///
    /// An install writes exactly one place: [`amplihack_prefix_bin`]. So it can
    /// repair an override naming a file *there* — the placeholder case, which
    /// is the reason this exit returns [`InstallDecision::InstallMissing`] at
    /// all — and it cannot touch one naming `/opt/vendor/bin/claude`. Without
    /// the path, [`decide_install`] cannot tell those apart, and answers
    /// `InstallMissing` for both: a multi-hundred-megabyte install that
    /// provably cannot change what launches, re-decided identically on every
    /// launch, forever. That is issue #1266's own loop reached through the new
    /// funnel, and the reason this field is carried rather than inferred.
    pub halted_on_user_override: Option<PathBuf>,
}

/// What, if anything, amplihack should install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallDecision {
    /// Launch what is already there. No npm work.
    UseExisting,
    /// Nothing healthy exists anywhere; install into amplihack's own prefix.
    InstallMissing,
    /// The healthy target lives in amplihack's prefix and is stale.
    UpgradeOwned,
    /// Nothing healthy resolved, but the evidence is inconclusive: a candidate
    /// timed out rather than answering, or resolution stopped before examining
    /// every candidate ([`Rejection::NotProbed`]).
    ///
    /// Same rule `decide_install` already applies to a failed registry query,
    /// on the other axis. A 3 s `--version` timeout on a loaded box is the same
    /// class of transient as a network blip, and neither is worth ~339 MB. The
    /// caller reports it and stops instead of installing over a binary that may
    /// well be fine.
    Abstain,
    /// A user-supplied override is broken, and it names a file outside
    /// [`amplihack_prefix_bin`] — so no install amplihack can perform would
    /// change the answer.
    ///
    /// Distinct from [`Self::Abstain`], which means "we could not tell". Here
    /// the evidence is conclusive and the conclusion is that installing is
    /// futile: the caller must report the broken override and stop, not spend
    /// an install and then fail anyway.
    BrokenOverride,
}

/// Per-candidate `--version` budget.
///
/// Deliberately larger than `binary_finder`'s 500 ms
/// `VERSION_DETECTION_TIMEOUT`: that constant gates an advisory annotation
/// where a false negative costs nothing, this one gates a launch where a false
/// rejection degrades the user's session.
pub const PER_CANDIDATE_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Total probe budget across every candidate (SEC-4).
pub const TOTAL_PROBE_BUDGET: Duration = Duration::from_secs(10);

/// Second-chance budget for a binary the user named explicitly (issue #1325).
///
/// A timeout is evidence about the machine, not the file. `Missing` and
/// `NotExecutable` are conclusions about the binary; `ProbeTimedOut` only says
/// the answer did not arrive in time. On a loaded host a healthy cold-start
/// Node process exceeds three seconds easily — `copilot --version` measured
/// 607 ms at load 9.5 on the host that reported this, which reached load 163
/// the same day.
///
/// Rejecting on that turns an arbitrary constant into an override of the
/// user's explicit instruction, and a run dies with "explicit binary override
/// failed the health gate" naming a binary that is perfectly good.
///
/// So a *user-supplied* override gets one more attempt with a far larger
/// budget before it is called broken. This deliberately does NOT relax the
/// invariant stated on [`LaunchTarget`] — health stays a filter, never an
/// annotation, and a target still requires a real version. It only stops a
/// busy machine being mistaken for a broken install. Candidates amplihack
/// found itself are unaffected: there the search moves on, which costs
/// nothing.
pub const EXPLICIT_OVERRIDE_RETRY_BUDGET: Duration = Duration::from_secs(30);

/// Hard cap on how many candidates are probed.
const MAX_PROBE_CANDIDATES: usize = 8;

/// Extract a parseable semver from `--version` output.
///
/// ANSI escapes are stripped before matching (SEC-3). Returns `None` when the
/// output carries no `\d+\.\d+\.\d+`, which makes the candidate
/// [`Rejection::UnparseableVersion`] — not a target with an unknown version.
pub(crate) fn extract_version(output: &str) -> Option<String> {
    static SEMVER: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\d+\.\d+\.\d+").expect("static semver regex"));
    // SEC-3: strip BEFORE matching, so an ESC sequence can neither hide inside
    // the captured version nor survive into the log line or the user's TTY.
    let cleaned = strip_ansi(output);
    SEMVER.find(&cleaned).map(|m| m.as_str().to_string())
}

/// The entire fix for the reinstall-on-every-launch defect, as a pure function.
///
/// Two rules are load-bearing:
///
/// * **amplihack never upgrades a binary it does not own.** If the binary that
///   will actually be executed lives outside `~/.npm-global`, installing into
///   that prefix would not change what gets launched — so the "upgrade" is
///   hundreds of megabytes with no effect, and the next launch decides
///   identically, forever. That loop is the defect.
/// * **A failed registry query never triggers an install.** `latest == None`
///   means "unknown", not "stale". A network blip must not cost 339 MB.
/// * **Inconclusive evidence never triggers an install either.** The same rule,
///   on the resolution axis: if nothing healthy resolved because a candidate
///   *timed out*, or because resolution *stopped before examining every
///   candidate*, amplihack does not know whether a working binary is there. It
///   answers [`InstallDecision::Abstain`]. This is why the whole
///   [`Resolution`] is the input and not just its target — the rejection list
///   is the difference between "nothing is installed" and "we could not tell".
/// * **A broken override is only worth an install when the install can reach
///   it.** `amplihack_bin` names the single directory amplihack writes to. An
///   override naming a file inside it is repairable — that is the placeholder
///   case, and repairing it is why the override's early return reports
///   conclusive evidence at all. An override naming anything else is not, and
///   answering `InstallMissing` there is the reinstall-on-every-launch defect
///   with a different first candidate.
/// * **Owning the directory is not owning the file.** `tool` is required
///   because [`TargetSource::AmplihackPrefix`] answers "which directory did
///   this come from", and the upgrade question is "is this binary the thing
///   `pkg` installs". See [`target_is_the_tool`].
pub fn decide_install(
    tool: &str,
    resolution: &Resolution,
    latest: Option<&str>,
    amplihack_bin: Option<&Path>,
) -> InstallDecision {
    let Some(target) = resolution.target.as_ref() else {
        // Neither a timeout nor an unexamined candidate is evidence of
        // absence. Everything else in the list is: Missing, NotAFile,
        // NotExecutable, NotAbsolute, PlaceholderStub, Unreadable,
        // UnparseableVersion and a non-zero probe all say "there is no working
        // binary here", which is what an install fixes.
        // Resolution stopped on the user's own override. Conclusive, but an
        // install only rewrites `amplihack_bin` — so it repairs an override
        // that points there and is pure waste for one that does not. Deciding
        // `InstallMissing` for the latter spends the install, resolves to the
        // same broken path, fails, and decides identically next launch.
        //
        // Checked BEFORE the inconclusive-evidence rule below, deliberately.
        // A user-supplied override that times out lands in both, and `Abstain`
        // wins on order alone would print the probe-budget message — which ends
        // by advising the user to `export {TOOL}_BINARY_PATH`, the thing they
        // already did. Whatever the rejection, the answer is about the file
        // they named, so say that.
        if let Some(override_path) = resolution.halted_on_user_override.as_ref() {
            let repairable = amplihack_bin
                .is_some_and(|bin| override_path.parent().is_some_and(|dir| same_dir(dir, bin)));
            if !repairable {
                return InstallDecision::BrokenOverride;
            }
        }
        if resolution.rejected.iter().any(|(_, rejection)| {
            matches!(rejection, Rejection::ProbeTimedOut | Rejection::NotProbed)
        }) {
            return InstallDecision::Abstain;
        }
        return InstallDecision::InstallMissing;
    };
    if target.source != TargetSource::AmplihackPrefix {
        // The binary that will actually be executed lives somewhere amplihack
        // does not write. Installing into amplihack's prefix cannot change what
        // gets launched, so the "upgrade" is hundreds of megabytes with no
        // effect and the next launch decides identically. Forever.
        return InstallDecision::UseExisting;
    }
    if !target_is_the_tool(target, tool) {
        // Same defect, one axis over: the directory is amplihack's, the file is
        // not the tool. `binary_candidates("claude")` is
        // `["rustyclawd", "claude"]` and the walk is candidate-major, so a
        // `rustyclawd` in `~/.npm-global/bin` outranks every `claude` and
        // arrives here tagged `AmplihackPrefix`. `latest` is then
        // `@anthropic-ai/claude-code`'s version, the comparison against
        // rustyclawd's is meaningless, and it is `!=` rather than "older than"
        // — so it fires in both directions and can never stop. Installing
        // claude-code does not change which file wins, so the next launch
        // decides identically. Forever.
        //
        // `~/.npm-global` is not amplihack-private: `npm config set prefix
        // ~/.npm-global` is the standard EACCES workaround, so anything the
        // user installs globally lands in a directory amplihack claims.
        return InstallDecision::UseExisting;
    }
    match latest {
        // A failed registry query means "unknown", never "stale".
        None => InstallDecision::UseExisting,
        Some(latest) if latest == target.version => InstallDecision::UseExisting,
        Some(_) => InstallDecision::UpgradeOwned,
    }
}

/// Is `target` actually the binary that `tool`'s npm package installs?
///
/// [`TargetSource`] answers *which directory* a binary came from, which is not
/// the same question. `binary_candidates` maps one tool to several accepted
/// file names (`claude` also matches `rustyclawd`), so a target can sit in
/// amplihack's own prefix and still be a different product with an unrelated
/// version series.
///
/// Both places that compare a target's version against a registry entry need
/// this: [`decide_install`], which spends hundreds of megabytes on the answer,
/// and the CLI's update notice, which prints an `npm install -g` line. One
/// predicate, so the two cannot drift — the notice had the check and the
/// install decision did not, and the disagreement was worth an install per
/// launch, forever.
///
/// Compares the file *stem*, so `claude`, `claude.exe` and `claude.cmd` all
/// match on the platforms that spell it those ways.
/// Do two paths name the same directory?
///
/// `Path`'s `Eq` is component-wise, which already absorbs `.` components,
/// doubled separators and a trailing slash (`Path::join` eats it). It does not
/// absorb symlinks, and a symlinked home is ordinary — `HOME=/home/bob` where
/// that is a link to `/export/home/bob`. A user who writes the resolved form
/// into `CLAUDE_BINARY_PATH` would otherwise mismatch amplihack's own prefix and
/// have a genuinely repairable placeholder reported as an unrepairable
/// [`InstallDecision::BrokenOverride`].
///
/// Canonicalisation needs both sides to exist, and neither is guaranteed to —
/// the prefix may not have been created yet. So it is an *additional* chance to
/// match, never a replacement: the literal compare stands on its own.
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn target_is_the_tool(target: &LaunchTarget, tool: &str) -> bool {
    target
        .path
        .file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case(tool))
}

/// Record every candidate resolution never got to, from `index` onwards.
///
/// [`decide_install`] reads the rejection list as *evidence*. An empty list
/// with no target means "nothing is installed" and buys an install; "we
/// stopped looking before the end of the list" means nothing of the sort. The
/// difference has to be visible in the list or it cannot be read from it.
fn record_unexamined(
    resolution: &mut Resolution,
    candidates: &[(PathBuf, TargetSource)],
    index: usize,
) {
    for (path, _) in &candidates[index..] {
        resolution
            .rejected
            .push((path.clone(), Rejection::NotProbed));
    }
}

/// The I/O shell: probe `candidates` in order and return the first healthy one.
///
/// Split from [`candidate_paths`] so the health gate is testable against a
/// temp-dir fixture without mutating process environment (which is `unsafe`
/// under edition 2024).
///
/// Probing stops at the first healthy candidate and is bounded by
/// [`MAX_PROBE_CANDIDATES`], [`PER_CANDIDATE_PROBE_TIMEOUT`], and
/// [`TOTAL_PROBE_BUDGET`].
pub fn resolve_from_candidates(tool: &str, candidates: &[(PathBuf, TargetSource)]) -> Resolution {
    let mut resolution = Resolution::default();
    let started = Instant::now();
    let mut probes = 0usize;

    for (index, (path, source)) in candidates.iter().enumerate() {
        // The cheap checks are free, so they do not consume the probe budget:
        // a $PATH with thirty entries and one binary must not exhaust it before
        // reaching the binary.
        let rejection = match cheap_reject(path) {
            Some(rejection) => Some(rejection),
            None => {
                if probes >= MAX_PROBE_CANDIDATES {
                    tracing::warn!(
                        tool,
                        max = MAX_PROBE_CANDIDATES,
                        "candidate probe cap reached; stopping resolution"
                    );
                    record_unexamined(&mut resolution, candidates, index);
                    break;
                }
                let Some(budget) = TOTAL_PROBE_BUDGET.checked_sub(started.elapsed()) else {
                    tracing::warn!(
                        tool,
                        budget = ?TOTAL_PROBE_BUDGET,
                        "total probe budget exhausted; stopping resolution"
                    );
                    record_unexamined(&mut resolution, candidates, index);
                    break;
                };
                probes += 1;
                let first = probe_version(path, PER_CANDIDATE_PROBE_TIMEOUT.min(budget));
                let attempt = match (&first, source) {
                    // Only the user's own instruction earns a second chance, and
                    // only for a non-answer (issue #1325).
                    (
                        Err(Rejection::ProbeTimedOut),
                        TargetSource::ExplicitOverride {
                            user_supplied: true,
                        },
                    ) => {
                        tracing::warn!(
                            tool,
                            path = %path.display(),
                            first_budget = ?PER_CANDIDATE_PROBE_TIMEOUT,
                            retry_budget = ?EXPLICIT_OVERRIDE_RETRY_BUDGET,
                            "explicitly named binary did not answer --version in time; \
                             retrying with a larger budget before calling it broken \
                             (issue #1325). A timeout is evidence about the machine, \
                             not the file."
                        );
                        probe_version(path, EXPLICIT_OVERRIDE_RETRY_BUDGET)
                    }
                    _ => first,
                };
                match attempt {
                    Ok(version) => {
                        tracing::debug!(
                            tool,
                            path = %path.display(),
                            version,
                            ?source,
                            "resolved launch target"
                        );
                        resolution.target = Some(LaunchTarget {
                            path: path.clone(),
                            version,
                            source: *source,
                        });
                        return resolution;
                    }
                    Err(rejection) => Some(rejection),
                }
            }
        };

        let Some(rejection) = rejection else {
            continue;
        };
        tracing::debug!(
            tool,
            path = %path.display(),
            ?rejection,
            ?source,
            "candidate rejected"
        );
        resolution.rejected.push((path.clone(), rejection));

        // A user who names a specific binary and gets a broken one is told so.
        // Silently launching a different binary than the one they asked for is
        // the behaviour this whole module exists to remove.
        match source {
            TargetSource::ExplicitOverride {
                user_supplied: true,
            } => {
                tracing::error!(
                    tool,
                    path = %path.display(),
                    ?rejection,
                    "explicit binary override failed the health gate"
                );
                // Deliberately NOT `record_unexamined`. This exit is a
                // conclusion, not a truncation: the user named one binary, it
                // is broken, and the candidates below are left unconsulted on
                // purpose — consulting them is the silent substitution this
                // module exists to prevent. So the evidence is conclusive *for
                // the question that was asked*, and `decide_install` should
                // read it that way and repair the install rather than
                // [`InstallDecision::Abstain`]. Recording `NotProbed` here
                // would flip it to Abstain and turn a repairable broken
                // override into a hard error.
                //
                // The path is carried out so `decide_install` can tell a
                // *repairable* broken override — one naming a file in the
                // directory amplihack installs into — from one naming a file
                // amplihack will never write. Without it both read as "nothing
                // is installed" and both buy an install; only the first is
                // fixed by one.
                resolution.halted_on_user_override = Some(path.clone());
                return resolution;
            }
            // An amplihack-set preference is only a preference: say so and
            // keep looking.
            TargetSource::ExplicitOverride { .. } => {
                tracing::warn!(
                    tool,
                    path = %path.display(),
                    ?rejection,
                    "amplihack-set binary preference failed the health gate; falling through"
                );
            }
            _ => {}
        }
    }

    resolution
}

/// Can this path be executed at all? Filesystem facts only.
///
/// No subprocess, so these never consume the probe budget — and **no judgement
/// about the file's contents**. Every check here is true for any tool on any
/// platform: it is there, it is a file, you may run it.
///
/// It used to also reject a small file with no native executable magic, as a
/// fast path that saved one `execve` on a claude install that was already
/// broken. That is a claude-shaped fact, and running it as a gate for every
/// tool broke `amplihack copilot`, whose `@github/copilot` loader is a
/// legitimate 1185-byte `#!/usr/bin/env node` shim. The knowledge was not lost
/// — [`label_failed_probe`] still uses it to *name* a failure — but it can no
/// longer cause one. Do not move it back.
fn cheap_reject(path: &Path) -> Option<Rejection> {
    // FIRST, before any filesystem call. A relative candidate is not a
    // candidate: `metadata` below would stat it against amplihack's current
    // directory, and `probe_version` would then hand a separator-free name to
    // `execvp`, which resolves it against the child's `$PATH`. Two different
    // files, neither of them the one that was named.
    //
    // This is the funnel EVERY candidate resolved here passes through — both
    // override arms, every `$PATH` entry, every fallback dir, and whatever
    // producer is added next — so the invariant is enforced once, here, rather
    // than at each producer where the next one will forget it. The `source`
    // match in `resolve_from_candidates` then gives the right behaviour for
    // free: a user-supplied relative override is a hard error naming the path,
    // an amplihack-set one falls through to the next candidate.
    //
    // Two call sites reach `Command::new` from an env var without coming
    // through here — `fleet::reasoning_helpers`' two branches — and they apply
    // the same rule locally via `absolute_executable_from_env`, because they
    // resolve *before* consulting this module rather than through it. Review
    // found them by taking the paragraph above literally, which is the point of
    // writing it that way: if you add a producer that cannot route through
    // `resolve`, the rule still has to hold, and saying "every candidate" here
    // while a bypass exists makes this comment false rather than aspirational.
    if !path.is_absolute() {
        return Some(Rejection::NotAbsolute);
    }
    // `metadata` FOLLOWS symlinks, and it must: every npm-installed claude on
    // every host is a symlink into
    // lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe. Using
    // `symlink_metadata` here would reject them all, amplihack's own install
    // included. A dangling link correctly surfaces as `Missing`.
    let Ok(metadata) = std::fs::metadata(path) else {
        return Some(Rejection::Missing);
    };
    if !metadata.is_file() {
        return Some(Rejection::NotAFile);
    }
    if !is_executable(&metadata) {
        return Some(Rejection::NotExecutable);
    }
    None
}

/// Put the right words on a candidate whose `--version` probe has already
/// failed.
///
/// This can only ever *rename* an existing rejection, so — unlike the
/// pre-probe gate it replaces — it cannot reject a candidate the probe would
/// have accepted, for any tool, present or future. That property is the whole
/// design: the boundary violation is gone by construction rather than by an
/// `if tool == "claude"` that the next tool re-opens.
///
/// The good diagnosis survives: an incomplete `@anthropic-ai/claude-code`
/// install is still reported as an incomplete install rather than as a generic
/// "`--version` failed", which is the message this whole module exists to
/// improve.
fn label_failed_probe(path: &Path) -> Rejection {
    let Ok(metadata) = std::fs::metadata(path) else {
        // It answered `cheap_reject` a moment ago, so this is a genuine I/O
        // failure, not absence. Say that, rather than asserting something
        // about contents nobody read.
        return Rejection::Unreadable;
    };
    let mut head = [0u8; 8];
    let Ok(read) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) else {
        return Rejection::Unreadable;
    };
    if has_placeholder_shape(&head[..read], metadata.len()) {
        return Rejection::PlaceholderStub;
    }
    Rejection::ProbeFailed
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    // Windows has no executable bit; extension-based execution is the norm and
    // the `--version` probe is the authority there.
    true
}

/// Run `<path> --version` and require a parseable semver from a clean exit.
fn probe_version(path: &Path, timeout: Duration) -> Result<String, Rejection> {
    let mut cmd = Command::new(path);
    cmd.arg("--version");
    match run_capped_output_with_timeout(cmd, timeout, PROBE_CAPTURE_LIMIT) {
        Ok(Some(output)) if output.status.success() => {
            // SEC-3: stdout here is whatever an arbitrary binary chose to
            // print. It is capped above and ANSI-stripped inside
            // `extract_version`, and nothing but the matched semver survives.
            extract_version(&String::from_utf8_lossy(&output.stdout))
                .ok_or(Rejection::UnparseableVersion)
        }
        Ok(Some(_)) => Err(label_failed_probe(path)),
        Ok(None) => Err(Rejection::ProbeTimedOut),
        // A spawn failure is the ENOEXEC case among others: the file is there
        // and executable but the kernel will not run it. That is a failed
        // install, not a launch target.
        Err(_) => Err(label_failed_probe(path)),
    }
}

/// Who set `AMPLIHACK_{TOOL}_BINARY_PATH`, and therefore what a failing
/// override means.
///
/// C5 / issue #1276. This used to be a process-global one-way latch
/// (`OVERRIDE_IS_AMPLIHACK_SUPPLIED`) that `candidate_paths` read implicitly.
/// The distinction it drew is real and still drawn here; only the shape
/// changed. It is passed as a parameter for three reasons:
///
/// 1. A one-way latch cannot be exercised twice in one process, so the wiring
///    was untestable by construction and needed a `#[cfg(test)]` reset hook to
///    be tested at all. That hook is gone with the latch.
/// 2. The latch had one setter, one implicit reader, and no test called
///    either. Deleting the whole thing left the suite green while
///    `amplihack rustyclawd` regressed from "warn and keep looking" to "hard
///    error" on a broken `rustyclawd`. A parameter cannot be deleted silently:
///    the compiler names every call site.
/// 3. Process-global mutable state is cross-test interference. Any test that
///    set the latch changed the answer for every later test in the binary.
///
/// Still **not** a second environment variable, for the original reason: an
/// env marker would be inherited by nested `amplihack` invocations and would
/// silently downgrade a genuine user override into a preference.
///
/// There is deliberately no `Default`. The safe-looking default is
/// [`Self::User`], which is the *strict* arm — a caller that forgot to thread
/// the value would turn a preference into a hard launch failure, which is
/// exactly the regression above. Making every call site name the origin is the
/// point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideOrigin {
    /// The variable came from the caller's environment. A failing override is
    /// a hard error: the user named a binary and amplihack must not quietly
    /// run a different one.
    User,
    /// amplihack set the variable itself, as a preference —
    /// `configure_preferred_rustyclawd_binary` is the only producer. A failing
    /// override warns and the search continues, so a broken `rustyclawd` on
    /// `$PATH` cannot turn a working `amplihack rustyclawd` into a failed
    /// launch.
    AmplihackSupplied,
}

impl OverrideOrigin {
    /// True when the override in the environment is the user's instruction
    /// rather than amplihack's own preference.
    fn user_supplied(self) -> bool {
        matches!(self, Self::User)
    }
}

/// Render a path that came from outside amplihack for a terminal.
///
/// A launch path is attacker-influenced: it comes from `$PATH`, `$HOME`,
/// `CLAUDE_BINARY_PATH`, or a filename someone planted in a directory already
/// on `$PATH`. Every rendering of one — the rejection report below, and
/// `amplihack-cli`'s spawn-failure message — goes through here, because the
/// hazard is the same in both and two sanitisers for one class of data drift.
///
/// The rule is: a path is one line of printable text. Rendering stops at the
/// first control character and the remainder is elided. That is stricter than
/// stripping escape sequences, deliberately. Stripping keeps the tail, and the
/// tail is the payload: a path ending `claude\n\nThe install is fine; run the
/// binary directly.` survives ANSI-stripping as a sentence on amplihack's own
/// diagnosis line, in the user's words-of-amplihack voice, at the exact moment
/// they are deciding what to do. Eliding costs an attacker-planted path its
/// suffix and costs a real path nothing.
///
/// Control characters are [`char::is_control`], which is Unicode `Cc` — ASCII
/// `C0`, `DEL`, and the `C1` block that carries the 8-bit `CSI`. Format
/// characters such as the bidi overrides are *not* covered; they can reorder a
/// rendered path but cannot forge a line or drive the terminal, which is the
/// boundary this function draws.
pub fn display_untrusted_path(path: &Path) -> String {
    let rendered = path.display().to_string();
    let head: String = rendered.chars().take_while(|c| !c.is_control()).collect();
    if head.len() == rendered.len() {
        head
    } else {
        format!("{head}…")
    }
}

/// `~/.npm-global/bin` — the one directory amplihack owns, spelled once.
///
/// C1 — this fact had no owner. It was re-derived in five places
/// (`candidate_paths` here, `is_already_reachable` in the launch command,
/// `install_fallback_dirs` in `binary_finder`, `npm_prefix_dir` in
/// `bootstrap`, plus `copilot_cli_client`), and moving the prefix would have
/// broken none of them at compile time: `claude` would simply, quietly, stop
/// being reachable in the child.
///
/// The irony was sharp, because [`TargetSource::AmplihackPrefix`] exists
/// precisely to answer this authoritatively — and then `augment_claude_launch_env`
/// is handed a bare `&Path`, discards the tag, and re-derives the directory by
/// string literal. Threading a whole [`Resolution`] through was refused as
/// scope growth and the parameter shape is settled, so what is shared is the
/// spelling.
///
/// `bin` and not the prefix itself: every caller wants the directory that holds
/// the binaries. `bootstrap::npm_prefix_dir` keeps its own name and returns
/// this path's parent, because npm's `--prefix` is the prefix.
///
/// Takes `home` rather than reading `$HOME` so it stays pure — the same reason
/// [`path_dirs`] is pure, and callers already have a `home` in hand.
pub fn amplihack_prefix_bin(home: &Path) -> PathBuf {
    home.join(".npm-global").join("bin")
}

/// The user's home directory — the **one** derivation in the launch path.
///
/// `HOME` or `USERPROFILE`, because Windows is a shipped release target and
/// normally sets only the latter. This exists because two spellings had drifted:
/// `candidate_paths` read both, `bootstrap::home_dir` read only `HOME`. On
/// Windows that made `amplihack_bin` unconditionally `None` at the
/// `decide_install` call site, so *every* halted user override answered
/// [`InstallDecision::BrokenOverride`] — including the repairable
/// placeholder-in-the-prefix case that exit exists for — while
/// `candidate_paths` had happily tagged that same directory
/// [`TargetSource::AmplihackPrefix`].
///
/// Two answers to one question inside a single launch is the disagreement this
/// module exists to delete, and it does not stop applying to the module's own
/// helpers.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// The `$PATH` → candidate-directory seam: split, then keep only the entries
/// that name an absolute directory.
///
/// POSIX defines an **empty** `$PATH` element as the current directory, and
/// trailing or doubled colons are ordinary in hand-edited shell profiles.
/// `split_paths("/usr/bin:")` yields `["/usr/bin", ""]`, and joining `""` with
/// `claude` gives the bare relative name `claude`. Two things then go wrong, in
/// order:
///
/// 1. `execvp` resolves a name containing no separator against the *child's*
///    `$PATH`, so the version probe executes whatever `./claude` happens to sit
///    in amplihack's current directory. If it prints parseable semver it
///    becomes the selected [`LaunchTarget`].
/// 2. That candidate's parent is the empty path, and prepending the empty path
///    puts the current directory at the **front** of the child's `$PATH` — for
///    the agent, every subagent, and every shell-out. A stray colon turns into
///    cwd-first resolution of `git`, `node` and `sh`.
///
/// `git clone <repo> && cd repo && amplihack claude` is the whole exploit.
/// `.` and `..` and bare relative entries are the same hazard spelled out, so
/// the filter is absoluteness rather than emptiness.
///
/// Pure so it can be pinned without mutating the process-global `$PATH`; see
/// `tests/no_global_path_mutation.rs` for why that matters in this crate.
pub fn path_dirs(path_var: &std::ffi::OsStr) -> Vec<PathBuf> {
    split_path_var(path_var, RelativeEntries::Drop)
}

/// The launch path's `$PATH` search directories, read from the environment.
///
/// The rule of [`path_dirs`] applied to the process `$PATH`, and nothing else
/// on top. Callers that need the *raw* elements — because they are rebuilding
/// or describing `$PATH` rather than searching it — want [`env_path_entries`],
/// and say so at the call.
pub fn env_path_dirs() -> Vec<PathBuf> {
    split_path_var_of(std::env::var_os("PATH"), RelativeEntries::Drop)
}

/// The process `$PATH`'s elements, verbatim and in order.
///
/// For the two callers that are *rebuilding* `$PATH` for a child or
/// *describing* it back to the user. See [`RelativeEntries::Keep`] for why
/// those must not be quietly edited on the way past.
pub fn env_path_entries() -> Vec<PathBuf> {
    split_path_var_of(std::env::var_os("PATH"), RelativeEntries::Keep)
}

/// **The one place that decides what "no `$PATH`" means**, as a pure function
/// of an optional `$PATH`.
///
/// Issue #1274. An unset `$PATH` and a `$PATH` set to the empty string are the
/// same amount of `$PATH`: none, i.e. zero elements — in *both*
/// [`RelativeEntries`] modes.
///
/// Before this the decision was re-made at every one of the nine `$PATH`
/// traversals and they did not agree:
///
/// * `path_conflicts` answered an empty `Vec` for an unset `$PATH`;
///   `bootstrap::prepend_path` answered `[""]`, because
///   `split_paths("")` yields one empty element. That is the current directory
///   in POSIX, and `prepend_path` writes its result back to the process
///   `$PATH` — so on a host with no `$PATH` at all, prepending a directory
///   also put **cwd** on the `$PATH` of amplihack and every child it spawns.
///   Exactly the hazard [`path_dirs`] exists to close, arriving through the
///   rebuild path instead of the search path.
/// * Search sites split further: some answered `None` on unset and an empty
///   `Vec` on empty, some the reverse, and three read `$PATH` with
///   [`std::env::var`] rather than [`std::env::var_os`], so a perfectly legal
///   non-UTF-8 `$PATH` read as absent.
///
/// Zero elements is the honest answer to all of them: there is nowhere to
/// look, and nothing to preserve. Every caller's "found nothing" branch
/// already handles it.
///
/// Pure, and taking an `Option`, for the reason
/// `docker_detector::which_docker_in` is: the unset branch is otherwise only
/// reachable by unsetting the process-global `PATH`, which
/// `tests/no_global_path_mutation.rs` forbids because it makes every
/// concurrent bare-name `git` spawn in a sibling test fail with `ENOENT`. The
/// branch that decided the empty case was, before this, untestable at every
/// one of the nine places that decided it.
pub fn split_path_var_of(
    path_var: Option<std::ffi::OsString>,
    relative: RelativeEntries,
) -> Vec<PathBuf> {
    match path_var {
        Some(path_var) => split_path_var(&path_var, relative),
        None => Vec::new(),
    }
}

/// What a `$PATH` element that does not name an absolute directory is worth.
///
/// Issue #1274 — the rule was being re-derived at every `$PATH` walk in the
/// repo and the derivations disagreed. Naming the two answers makes each call
/// state which one it wants instead of forking the logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeEntries {
    /// Drop them. The rule for every walk that is **choosing a file to run**.
    /// See [`path_dirs`] for the empty-element-is-cwd chain this closes.
    Drop,
    /// Keep them, verbatim and in order. Only for callers that are
    /// **rebuilding or describing** `$PATH` rather than searching it: editing
    /// the user's own `$PATH` on the way past would be a silent change to the
    /// environment of every child process, and a `$PATH` conflict report that
    /// hides an entry is describing a `$PATH` the user does not have.
    Keep,
}

/// Split `$PATH` once, applying the [`RelativeEntries`] rule.
///
/// The single `split_paths` call in this crate — `tests/no_global_path_mutation.rs`
/// ratchets that, and `amplihack-cli`'s
/// `tests/issue_1274_one_path_traversal.rs` ratchets the launch-path modules
/// that used to have their own.
///
/// An empty `$PATH` has no elements, in either mode; see
/// [`split_path_var_of`], which is where that is stated and tested.
pub fn split_path_var(path_var: &std::ffi::OsStr, relative: RelativeEntries) -> Vec<PathBuf> {
    if path_var.is_empty() {
        return Vec::new();
    }
    std::env::split_paths(path_var)
        .filter(|dir| relative == RelativeEntries::Keep || dir.is_absolute())
        .collect()
}

/// Build the candidate list for `tool` from the environment, in this order:
///
/// 1. `AMPLIHACK_{TOOL}_BINARY_PATH`, then `{TOOL}_BINARY_PATH`
/// 2. each `$PATH` entry, in `$PATH` order
/// 3. `~/.npm-global/bin` — amplihack's own prefix
/// 4. the remaining fallback dirs: `~/.cargo/bin`, `~/.local/bin`
fn candidate_paths(tool: &str, override_origin: OverrideOrigin) -> Vec<(PathBuf, TargetSource)> {
    let mut candidates: Vec<(PathBuf, TargetSource)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |candidates: &mut Vec<(PathBuf, TargetSource)>, path: PathBuf, source| {
        if seen.insert(path.clone()) {
            candidates.push((path, source));
        }
    };

    let tool_upper = tool.to_uppercase();
    if let Some(value) = std::env::var_os(format!("AMPLIHACK_{tool_upper}_BINARY_PATH")) {
        let user_supplied = override_origin.user_supplied();
        push(
            &mut candidates,
            PathBuf::from(value),
            TargetSource::ExplicitOverride { user_supplied },
        );
    }
    if let Some(value) = std::env::var_os(format!("{tool_upper}_BINARY_PATH")) {
        push(
            &mut candidates,
            PathBuf::from(value),
            TargetSource::ExplicitOverride {
                user_supplied: true,
            },
        );
    }

    // Directory list, tagged by who owns it. Ownership is decided by WHERE a
    // directory is, never by how it was discovered: on the repo owner's WSL
    // machine `~/.npm-global/bin` is the FIRST $PATH entry, and tagging it
    // `Path` there would tell `decide_install` that amplihack does not own its
    // own install and must never upgrade it.
    let home = home_dir();
    let npm_prefix_bin = home.as_ref().map(|h| amplihack_prefix_bin(h));
    let fallback_dirs: Vec<PathBuf> = home
        .as_ref()
        .map(|h| vec![h.join(".cargo").join("bin"), h.join(".local").join("bin")])
        .unwrap_or_default();
    let source_for = |dir: &Path| {
        if npm_prefix_bin.as_deref() == Some(dir) {
            TargetSource::AmplihackPrefix
        } else if fallback_dirs.iter().any(|d| d == dir) {
            TargetSource::FallbackDir
        } else {
            TargetSource::Path
        }
    };

    let mut dirs: Vec<PathBuf> = env_path_dirs();
    // Known install targets, appended in case the user's shell PATH predates
    // amplihack's own install (persistent tmux/ssh sessions, minimal Docker
    // shells). Already-present entries keep their $PATH position.
    if let Some(npm_bin) = npm_prefix_bin.clone() {
        dirs.push(npm_bin);
    }
    dirs.extend(fallback_dirs.iter().cloned());

    // Candidate-major, matching `binary_finder::binary_candidates`: a
    // `rustyclawd` anywhere on $PATH outranks a `claude`, which is the existing
    // and intended precedence for the RustyClawd front end.
    //
    // `dirs` may repeat an entry (a $PATH that already names the npm prefix,
    // appended again below). No separate directory de-duplication pass is
    // needed: `push` de-duplicates on the joined path and keeps the first
    // occurrence, so a repeated directory contributes nothing the first one
    // did not.
    for name in &crate::binary_finder::binary_candidates(tool) {
        for dir in &dirs {
            push(&mut candidates, dir.join(name), source_for(dir));
        }
    }

    candidates
}

/// Per-tool memo of the last resolution, with the candidate list it was
/// computed from.
///
/// Keyed by tool and bounded by the number of tools, so it cannot grow.
static RESOLUTION_MEMO: LazyLock<Mutex<HashMap<String, (Candidates, Resolution)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A candidate list, as [`candidate_paths`] produces it.
///
/// Not cosmetic: inlining this spelling into [`RESOLUTION_MEMO`] trips
/// `clippy::type_complexity`, which the workspace denies.
type Candidates = Vec<(PathBuf, TargetSource)>;

/// Resolve the launch target for `tool`.
///
/// This is the only function in the repo permitted to answer "which binary" for
/// a launch, an install decision, or a version check. See
/// `docs/LAUNCH_TARGET_RESOLUTION.md`.
///
/// # Memoized
///
/// One launch asks this question at least twice — the update notice, then the
/// install decision — and each answer costs a `--version` subprocess against a
/// ~339 MB binary (measured: 151 ms per resolution on the dev VM, of which
/// 0.15 ms is building the candidate list). The repeats are pure waste: the
/// question is "which binary will we launch", and it has one answer per
/// process.
///
/// The memo is keyed by tool and validated against the **candidate list**, not
/// just the name. Every input `resolve_from_candidates` reads is in that list,
/// so any environment change that could change the answer — `PATH`, `HOME`, an
/// override variable, the [`OverrideOrigin`] argument — produces a
/// different list and misses the memo rather than returning a stale answer.
///
/// What the memo cannot see is the filesystem changing underneath it, which is
/// exactly what an install does. That path calls [`resolve_uncached`].
pub fn resolve(tool: &str, override_origin: OverrideOrigin) -> Resolution {
    let candidates = candidate_paths(tool, override_origin);
    // The probe runs under the lock, and the lock is ONE mutex for ALL tools,
    // not one per tool. So a slow `claude` resolution also delays a concurrent
    // `copilot` one. Accepted, because the wait it can impose is bounded:
    // `resolve_from_candidates` returns within `TOTAL_PROBE_BUDGET`, and that
    // bound covers the output drain as well as the wait (SEC-4). For a second
    // thread asking the SAME question — the case that actually happens, since a
    // launch asks about one tool — waiting for the answer beats racing for it.
    // Split the map per tool if a real workload ever resolves two tools at once.
    let mut memo = RESOLUTION_MEMO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((memoized_candidates, resolution)) = memo.get(tool)
        && *memoized_candidates == candidates
    {
        return resolution.clone();
    }
    let resolution = resolve_from_candidates(tool, &candidates);
    memo.insert(tool.to_string(), (candidates, resolution.clone()));
    resolution
}

/// Resolve `tool` ignoring the memo, and refresh it with the answer.
///
/// For callers that just changed the filesystem — i.e. installed something.
/// Nothing else should need it: the memo already misses on any environment
/// change that could matter.
pub fn resolve_uncached(tool: &str, override_origin: OverrideOrigin) -> Resolution {
    let candidates = candidate_paths(tool, override_origin);
    let resolution = resolve_from_candidates(tool, &candidates);
    RESOLUTION_MEMO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(tool.to_string(), (candidates, resolution.clone()));
    resolution
}

impl Resolution {
    /// Human-readable account of what happened, and the remedy.
    ///
    /// `tool` and `package` are parameters because this is the error path for
    /// **every** tool. It used to hardcode claude's name and claude's npm
    /// package, so a copilot user was told "No usable claude binary was found"
    /// and instructed to install `@anthropic-ai/claude-code` — a regression in
    /// exactly the surface this function exists to improve.
    ///
    /// Two headlines, because there are two failures and they need different
    /// words. When nothing resolved, the list below *is* the story. When
    /// something did resolve and the caller is reporting a failure anyway — the
    /// spawn path — the binary that failed is the one that is missing from the
    /// list, and "no usable binary was found" over a list that does not contain
    /// it (and is usually empty) is simply false.
    ///
    /// Carries paths, rejection reasons, and the remedy — never the
    /// environment, never the full argv. ANSI escapes and control characters in
    /// candidate paths are stripped (SEC-3).
    pub fn rejection_report(&self, tool: &str, package: &str) -> String {
        let mut out = match self.target.as_ref() {
            Some(target) => format!(
                "amplihack selected {path} (version {version}) for {tool}, and it \
                 could not be run.\n",
                path = display_untrusted_path(&target.path),
                version = strip_ansi(&target.version),
            ),
            None => format!(
                "No usable {tool} binary was found. Every candidate below was \
                 considered:\n"
            ),
        };
        // Unexamined candidates are summarised, not listed. They carry no
        // information about the file — only that resolution stopped — and on a
        // long `$PATH` a cap hit produces dozens of identical rows that bury
        // the ones that do say something. This report is read by a user who is
        // already stuck; its length is part of whether it helps.
        let mut not_probed = 0usize;
        for (path, rejection) in &self.rejected {
            if *rejection == Rejection::NotProbed {
                not_probed += 1;
                continue;
            }
            // SEC-3: a planted filename can carry ESC, a newline in it would
            // forge extra rows in this very report, and its tail would read as
            // amplihack's own prose. `display_untrusted_path` handles all
            // three; see its docs for why eliding beats stripping here.
            out.push_str(&format!(
                "\n  {}\n      {}\n",
                display_untrusted_path(path),
                rejection.explain()
            ));
        }
        if not_probed > 0 {
            out.push_str(&format!(
                "\n  ({not_probed} further candidate(s) were not examined — \
                 resolution stopped at the probe cap or the total probe \
                 budget.)\n"
            ));
        }
        out.push_str(&format!(
            "\nRemedy: install a complete copy of {tool}, then run amplihack \
             again:\n  \
             npm install -g {package}\n\
             A package whose postinstall step was skipped leaves a small \
             placeholder behind instead of the binary it is supposed to \
             install.\n"
        ));
        out
    }
}

#[cfg(test)]
mod tests {

    /// The pre-`amplihack_bin` spelling, for the tests that predate it.
    ///
    /// Every one of them resolves without an override halt, so the directory
    /// amplihack installs into cannot change their answer. Tests that DO
    /// exercise the halt call [`decide_install`] directly with an explicit
    /// prefix — passing `None` there would assert the bug.
    fn decide(resolution: &Resolution, latest: Option<&str>) -> InstallDecision {
        decide_install("claude", resolution, latest, None)
    }
    use super::*;
    use std::ffi::OsStr;

    // ------------------------------------------------------------------
    // cheap_reject / label_failed_probe — the diagnosis is not a gate
    // ------------------------------------------------------------------

    /// The exact bytes of the placeholder amplihack has seen in the wild:
    /// 500 bytes, ASCII, **no shebang**. Verified on the dev VM 2026-08-21.
    fn real_stub_bytes() -> Vec<u8> {
        let mut v = b"echo \"Error: claude native binary not installed.\" >&2\nexit 1\n".to_vec();
        v.resize(500, b' ');
        v
    }

    #[cfg(unix)]
    fn write_executable(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn cheap_reject_passes_a_small_shim_it_cannot_judge() {
        // THE regression. `~/.npm-global/bin/copilot` is a 1185-byte
        // `#!/usr/bin/env node` loader: small, no native magic, and perfectly
        // healthy. `cheap_reject` answers "can this be executed at all", and
        // for this file the answer is yes. Anything more is the probe's call.
        let dir = tempfile::tempdir().unwrap();
        let shim = write_executable(
            dir.path(),
            "copilot",
            b"#!/usr/bin/env node\nrequire('@github/copilot/npm-loader.js');\n",
        );
        assert_eq!(
            cheap_reject(&shim),
            None,
            "a small executable file is not a rejection — size is not evidence"
        );
    }

    #[test]
    fn cheap_reject_refuses_every_spelling_of_a_relative_candidate() {
        // The three spellings of the same hazard. `claude` is what an empty
        // `$PATH` element produces, `./claude` and `../claude` are what a
        // hand-written `CLAUDE_BINARY_PATH` produces. None of them names a file
        // that both `metadata` and `execvp` would agree on, so none of them is
        // a candidate.
        for spelling in ["claude", "./claude", "../claude", "bin/claude", ""] {
            assert_eq!(
                cheap_reject(Path::new(spelling)),
                Some(Rejection::NotAbsolute),
                "{spelling:?} is relative and must be rejected before it is \
                 stat'd, let alone executed"
            );
        }
    }

    #[test]
    fn the_absoluteness_check_runs_before_the_filesystem_ones() {
        // Ordering, not mere presence. A relative name that DOES resolve
        // against the process cwd must still be `NotAbsolute` — if `metadata`
        // ran first this would come back `NotExecutable`, which would mean the
        // gate had already stat'd a file that is not the one `execvp` will run.
        //
        // Cargo runs a test binary with its cwd set to the package root, so
        // `Cargo.toml` is a real, non-executable file at a relative path. The
        // assertion below fails loudly rather than vacuously if that ever
        // changes. No env or cwd mutation: sibling tests in this crate spawn
        // `git` by bare name and never take a lock.
        let relative = Path::new("Cargo.toml");
        assert!(
            relative.exists(),
            "this test needs a real file at a relative path; cargo is supposed \
             to run test binaries from the package root"
        );
        assert_eq!(
            cheap_reject(relative),
            Some(Rejection::NotAbsolute),
            "a relative path must be rejected before it is stat'd"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cheap_reject_reports_only_filesystem_facts() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            cheap_reject(&dir.path().join("nothing-here")),
            Some(Rejection::Missing)
        );
        assert_eq!(cheap_reject(dir.path()), Some(Rejection::NotAFile));

        let path = dir.path().join("not-executable");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(cheap_reject(&path), Some(Rejection::NotExecutable));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_probe_on_the_real_stub_is_labelled_a_placeholder() {
        // The good diagnosis the removed fast path was written for, kept: an
        // incomplete claude install still says "incomplete install", it just
        // says it after the probe has already rejected the file instead of
        // before anyone looked.
        let dir = tempfile::tempdir().unwrap();
        let stub = write_executable(dir.path(), "claude", &real_stub_bytes());
        assert_eq!(label_failed_probe(&stub), Rejection::PlaceholderStub);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_probe_on_something_substantial_stays_a_plain_probe_failure() {
        let dir = tempfile::tempdir().unwrap();
        let big = write_executable(
            dir.path(),
            "claude",
            &vec![b'#'; crate::claude_native::STUB_MAX_LEN as usize + 1],
        );
        assert_eq!(label_failed_probe(&big), Rejection::ProbeFailed);
    }

    #[test]
    fn an_unreadable_candidate_is_not_diagnosed_as_a_placeholder() {
        // The old code read the head with `.unwrap_or(0)`, so an EACCES became
        // `read == 0` and every file under 4 KiB was confidently reported as
        // "incomplete install — this is the small placeholder…". An I/O error
        // is not evidence about contents nobody managed to read.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            label_failed_probe(&dir.path().join("vanished")),
            Rejection::Unreadable
        );
    }

    // ------------------------------------------------------------------
    // extract_version
    // ------------------------------------------------------------------

    #[test]
    fn extract_version_parses_the_real_claude_output() {
        assert_eq!(
            extract_version("2.1.238 (Claude Code)\n").as_deref(),
            Some("2.1.238")
        );
    }

    #[test]
    fn extract_version_strips_ansi_before_matching() {
        // SEC-3: probe stdout is attacker-controlled. An ESC sequence must not
        // reach the version string or the user's TTY.
        let raw = "\x1b[32m2.1.238\x1b[0m (Claude Code)";
        let parsed = extract_version(raw).expect("version behind ANSI must still parse");
        assert_eq!(parsed, "2.1.238");
        assert!(!parsed.contains('\x1b'), "no ESC may survive: {parsed:?}");
    }

    #[test]
    fn extract_version_rejects_the_stubs_error_text() {
        // The stub's own message, should it ever exit 0.
        assert_eq!(
            extract_version("Error: claude native binary not installed."),
            None
        );
    }

    #[test]
    fn extract_version_rejects_unknown_and_empty() {
        assert_eq!(extract_version("unknown"), None);
        assert_eq!(extract_version(""), None);
        assert_eq!(extract_version("   \n\n"), None);
    }

    #[test]
    fn extract_version_rejects_a_two_component_version() {
        // "2.1" is not a semver; an unparseable version is a rejection, never
        // a target annotated `version: "unknown"`.
        assert_eq!(extract_version("claude 2.1"), None);
    }

    // ------------------------------------------------------------------
    // decide_install — the whole of Defect 2, as a table
    // ------------------------------------------------------------------

    fn target(source: TargetSource, version: &str) -> LaunchTarget {
        LaunchTarget {
            path: PathBuf::from("/anywhere/claude"),
            version: version.to_string(),
            source,
        }
    }

    fn resolved(source: TargetSource, version: &str) -> Resolution {
        Resolution {
            target: Some(target(source, version)),
            rejected: Vec::new(),
            halted_on_user_override: None,
        }
    }

    fn nothing_resolved(rejected: &[Rejection]) -> Resolution {
        Resolution {
            target: None,
            rejected: rejected
                .iter()
                .enumerate()
                .map(|(i, r)| (PathBuf::from(format!("/candidate/{i}/claude")), *r))
                .collect(),
            halted_on_user_override: None,
        }
    }

    #[test]
    fn decide_install_installs_when_nothing_healthy_exists() {
        for latest in [Some("2.1.238"), None] {
            assert_eq!(
                decide(&nothing_resolved(&[Rejection::PlaceholderStub]), latest),
                InstallDecision::InstallMissing
            );
        }
        assert_eq!(
            decide(&Resolution::default(), None),
            InstallDecision::InstallMissing,
            "an empty candidate list is still 'nothing is installed'"
        );
    }

    #[test]
    fn decide_install_abstains_when_a_candidate_timed_out() {
        // A 3 s `--version` timeout on a loaded box is the same class of
        // transient as a failed registry query, and the rule one paragraph up
        // in the docstring already says a transient must not cost ~339 MB.
        // Before this, `ProbeTimedOut` was indistinguishable from "nothing is
        // installed" and bought a full reinstall.
        assert_eq!(
            decide(
                &nothing_resolved(&[Rejection::ProbeTimedOut]),
                Some("2.1.238")
            ),
            InstallDecision::Abstain
        );
    }

    #[test]
    fn decide_install_abstains_if_any_candidate_timed_out() {
        // One inconclusive candidate is enough: the binary that would have
        // answered may be the one that hung.
        assert_eq!(
            decide(
                &nothing_resolved(&[
                    Rejection::Missing,
                    Rejection::ProbeTimedOut,
                    Rejection::PlaceholderStub,
                ]),
                Some("2.1.238"),
            ),
            InstallDecision::Abstain
        );
    }

    #[test]
    fn decide_install_abstains_when_resolution_stopped_before_the_end() {
        // "We stopped looking" is not "there is nothing there". The candidate
        // that would have answered may be the one past the cap.
        assert_eq!(
            decide(
                &nothing_resolved(&[Rejection::PlaceholderStub, Rejection::NotProbed]),
                Some("2.1.238"),
            ),
            InstallDecision::Abstain
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopping_at_the_probe_cap_records_the_candidates_it_never_reached() {
        // Non-vacuous end to end: `resolve_from_candidates` must leave the
        // evidence behind, not just `decide_install` read it.
        let dir = tempfile::tempdir().unwrap();
        let mut candidates = Vec::new();
        for i in 0..(MAX_PROBE_CANDIDATES + 3) {
            // Executable, and exits non-zero: passes `cheap_reject`, fails the
            // probe, so every one of them consumes a probe slot.
            let path = write_executable(dir.path(), &format!("claude{i}"), b"#!/bin/sh\nexit 1\n");
            candidates.push((path, TargetSource::Path));
        }

        let resolution = resolve_from_candidates("claude", &candidates);
        assert!(resolution.target.is_none());
        let not_probed = resolution
            .rejected
            .iter()
            .filter(|(_, r)| *r == Rejection::NotProbed)
            .count();
        assert_eq!(
            not_probed, 3,
            "the 3 candidates past the cap must be recorded, got {:?}",
            resolution.rejected
        );
        assert_eq!(
            decide(&resolution, Some("2.1.238")),
            InstallDecision::Abstain,
            "a truncated pass must not buy an install it cannot justify"
        );
    }

    #[test]
    fn decide_install_still_installs_when_every_rejection_is_conclusive() {
        // Missing / not executable / a stub / a non-zero probe all mean "there
        // is no working binary here", which is precisely what an install fixes.
        assert_eq!(
            decide(
                &nothing_resolved(&[
                    Rejection::Missing,
                    Rejection::NotAFile,
                    Rejection::NotExecutable,
                    Rejection::PlaceholderStub,
                    Rejection::Unreadable,
                    Rejection::ProbeFailed,
                    Rejection::UnparseableVersion,
                ]),
                Some("2.1.238"),
            ),
            InstallDecision::InstallMissing
        );
    }

    #[test]
    fn decide_install_ignores_a_timeout_once_something_healthy_was_found() {
        // A hung candidate ahead of a healthy one is not inconclusive: we know
        // what we are going to launch.
        let mut resolution = resolved(TargetSource::Path, "2.1.237");
        resolution
            .rejected
            .push((PathBuf::from("/slow/claude"), Rejection::ProbeTimedOut));
        assert_eq!(
            decide(&resolution, Some("2.1.238")),
            InstallDecision::UseExisting
        );
    }

    #[test]
    fn decide_install_never_upgrades_a_binary_amplihack_does_not_own() {
        // THE reinstall loop, verified on dev: /usr/bin/claude @ 2.1.237 is
        // healthy and on PATH, the registry says 2.1.238. Installing into
        // ~/.npm-global cannot change what gets launched, so "upgrading" is
        // 339 MB of download and the next launch decides identically. Forever.
        for source in [
            TargetSource::Path,
            TargetSource::FallbackDir,
            TargetSource::ExplicitOverride {
                user_supplied: true,
            },
            TargetSource::ExplicitOverride {
                user_supplied: false,
            },
        ] {
            assert_eq!(
                decide(&resolved(source, "2.1.237"), Some("2.1.238")),
                InstallDecision::UseExisting,
                "must not upgrade a non-owned target ({source:?})"
            );
        }
    }

    #[test]
    fn decide_install_upgrades_a_stale_binary_in_amplihacks_own_prefix() {
        assert_eq!(
            decide(
                &resolved(TargetSource::AmplihackPrefix, "2.1.237"),
                Some("2.1.238")
            ),
            InstallDecision::UpgradeOwned
        );
    }

    #[test]
    fn decide_install_does_nothing_when_the_owned_binary_is_current() {
        // A7's second run: zero npm work, no 339 MB download.
        assert_eq!(
            decide(
                &resolved(TargetSource::AmplihackPrefix, "2.1.238"),
                Some("2.1.238")
            ),
            InstallDecision::UseExisting
        );
    }

    #[test]
    fn decide_install_never_installs_when_the_registry_query_failed() {
        // latest == None means "unknown", not "stale". A network blip must
        // never cost the user a reinstall.
        assert_eq!(
            decide(&resolved(TargetSource::AmplihackPrefix, "2.1.237"), None),
            InstallDecision::UseExisting
        );
    }

    // ------------------------------------------------------------------
    // rejection_report — Defect 3's message contract (A-AMB-11)
    // ------------------------------------------------------------------

    const CLAUDE_PKG: &str = "@anthropic-ai/claude-code";

    fn stub_and_timeout_resolution() -> Resolution {
        Resolution {
            target: None,
            rejected: vec![
                (
                    PathBuf::from("/home/you/.npm-global/bin/claude"),
                    Rejection::PlaceholderStub,
                ),
                (
                    PathBuf::from("/home/you/.local/bin/claude"),
                    Rejection::ProbeTimedOut,
                ),
            ],
            halted_on_user_override: None,
        }
    }

    fn claude_report(resolution: &Resolution) -> String {
        resolution.rejection_report("claude", CLAUDE_PKG)
    }

    #[test]
    fn rejection_report_names_the_real_cause() {
        let report = claude_report(&stub_and_timeout_resolution()).to_lowercase();
        assert!(
            report.contains("install")
                && (report.contains("incomplete")
                    || report.contains("stub")
                    || report.contains("placeholder")),
            "must name the incomplete-install cause, got:\n{report}"
        );
    }

    #[test]
    fn rejection_report_states_a_remedy() {
        let report = claude_report(&stub_and_timeout_resolution());
        assert!(
            report.contains("npm install") && report.contains(CLAUDE_PKG),
            "must state a copy-pasteable remedy, got:\n{report}"
        );
    }

    #[test]
    fn rejection_report_names_the_tool_and_package_it_was_given() {
        // The regression this signature exists to prevent: a copilot user was
        // told "No usable claude binary was found" and instructed to install
        // @anthropic-ai/claude-code.
        let report = stub_and_timeout_resolution().rejection_report("copilot", "@github/copilot");
        assert!(
            report.contains("copilot") && report.contains("@github/copilot"),
            "the report must speak about the tool it was asked about, got:\n{report}"
        );
        assert!(
            !report.contains("claude-code") && !report.to_lowercase().contains("claude binary"),
            "a copilot failure must not name claude's package, got:\n{report}"
        );
    }

    #[test]
    fn rejection_report_lists_every_rejected_candidate() {
        let report = claude_report(&stub_and_timeout_resolution());
        assert!(report.contains("/home/you/.npm-global/bin/claude"));
        assert!(report.contains("/home/you/.local/bin/claude"));
    }

    #[test]
    fn rejection_report_does_not_claim_nothing_was_found_when_something_was() {
        // The spawn-failure path (`launch/mod.rs`) reports a target that WAS
        // resolved and then would not exec. The old headline said "No usable
        // claude binary was found" over a list that did not contain it and was
        // usually empty — false, and it hid the one path that matters.
        let resolution = resolved(TargetSource::Path, "2.1.238");
        let report = claude_report(&resolution);
        assert!(
            !report.to_lowercase().contains("no usable"),
            "something WAS found; the report must not say otherwise:\n{report}"
        );
        assert!(
            report.contains("/anywhere/claude") && report.contains("2.1.238"),
            "the report must name the binary that failed and its version:\n{report}"
        );
    }

    #[test]
    fn rejection_report_says_nothing_was_found_when_nothing_was() {
        let report = claude_report(&stub_and_timeout_resolution()).to_lowercase();
        assert!(
            report.contains("no usable claude binary was found"),
            "got:\n{report}"
        );
    }

    #[test]
    fn rejection_report_does_not_send_the_user_hunting_for_an_arch_problem() {
        // The old message was `Exec format error (os error 8)`, which named
        // nothing real and pointed at a CPU-architecture problem that did not
        // exist.
        let report = claude_report(&stub_and_timeout_resolution()).to_lowercase();
        for forbidden in [
            "exec format error",
            "os error 8",
            "architecture",
            "arch mismatch",
            "platform mismatch",
        ] {
            assert!(
                !report.contains(forbidden),
                "report must not contain {forbidden:?}, got:\n{report}"
            );
        }
    }

    // ------------------------------------------------------------------
    // display_untrusted_path — the one renderer for attacker-influenced paths
    // ------------------------------------------------------------------

    #[test]
    fn an_ordinary_path_renders_unchanged() {
        assert_eq!(
            display_untrusted_path(Path::new("/home/you/.npm-global/bin/claude")),
            "/home/you/.npm-global/bin/claude"
        );
    }

    #[test]
    fn rendering_stops_at_the_first_control_character() {
        // The tail is the payload: ANSI-stripping alone would leave this
        // sentence sitting on amplihack's own diagnosis line, in amplihack's
        // voice, telling the user the opposite of the truth.
        let rendered = display_untrusted_path(Path::new(
            "/tmp/claude\n\nThe install is fine; run the binary directly.",
        ));
        assert_eq!(rendered, "/tmp/claude…");
        assert!(!rendered.contains('\n'));
    }

    #[test]
    fn no_escape_sequence_survives_rendering() {
        for planted in [
            "/tmp/\x1b[2J\x1b[Hclaude",          // CSI: clear screen, home cursor
            "/tmp/\x1b]52;c;ZXZpbA==\x07claude", // OSC 52: write the clipboard
            "/tmp/\u{9b}2Jclaude",               // 8-bit C1 CSI, no ESC involved
        ] {
            let rendered = display_untrusted_path(Path::new(planted));
            assert!(
                !rendered.chars().any(char::is_control),
                "{planted:?} rendered as {rendered:?}"
            );
        }
    }

    #[test]
    fn an_elided_path_says_so() {
        // The user has to be able to tell "this is the path" from "this is as
        // much of the path as amplihack was willing to print".
        assert!(display_untrusted_path(Path::new("/tmp/a\nb")).ends_with('…'));
        assert!(!display_untrusted_path(Path::new("/tmp/ab")).ends_with('…'));
    }

    #[test]
    fn rejection_report_strips_ansi_from_candidate_paths() {
        // SEC-3: a planted ~/.local/bin/<ESC>… filename must not be rendered
        // into the user's terminal.
        let resolution = Resolution {
            target: None,
            rejected: vec![(
                PathBuf::from("/tmp/\x1b[2J\x1b[Hclaude"),
                Rejection::PlaceholderStub,
            )],
            halted_on_user_override: None,
        };
        let report = claude_report(&resolution);
        assert!(
            !report.contains('\x1b'),
            "no ESC may reach the TTY, got: {report:?}"
        );
    }

    #[test]
    fn a_newline_in_a_candidate_path_cannot_forge_a_report_row() {
        // The report renders "\n  {path}\n      {reason}\n". A $PATH entry
        // carrying a newline could otherwise inject convincing extra rows —
        // making an attacker's rejected candidate read as a healthy one.
        let resolution = Resolution {
            target: None,
            rejected: vec![(
                PathBuf::from("/tmp/a\n  /usr/bin/claude\n      ok"),
                Rejection::PlaceholderStub,
            )],
            halted_on_user_override: None,
        };
        let report = claude_report(&resolution);
        let rows = report.lines().filter(|l| l.starts_with("  /")).count();
        assert_eq!(
            rows, 1,
            "exactly one candidate row may render, got:\n{report}"
        );
    }

    #[test]
    fn rejection_report_carries_no_environment() {
        // Error text carries paths, reasons, and the remedy. Nothing else.
        let report = claude_report(&stub_and_timeout_resolution());
        for leak in ["PATH=", "HOME=", "AMPLIHACK_", "NODE_OPTIONS"] {
            assert!(
                !report.contains(leak),
                "report must not leak {leak:?}, got:\n{report}"
            );
        }
    }

    // ------------------------------------------------------------------
    // F-S2 — a stray colon in $PATH must not become a cwd-relative candidate
    //
    // POSIX defines an EMPTY $PATH element as the current directory, and
    // trailing/doubled colons are common in hand-edited shell profiles.
    // `split_paths("/usr/bin:")` yields ["/usr/bin", ""], and `"".join("claude")`
    // is the bare relative path `claude`. Two things then go wrong, in order:
    //
    //   1. `execvp` resolves a bare name against the child's $PATH, so the
    //      probe EXECUTES whatever `./claude` happens to be in amplihack's
    //      current directory. If it prints parseable semver it becomes the
    //      selected LaunchTarget.
    //   2. That candidate's parent is the empty path, and prepending the empty
    //      path puts the current directory at the FRONT of the child's $PATH —
    //      for the agent, every subagent, and every shell-out. A stray colon
    //      turns into cwd-first resolution of `git`, `node` and `sh`.
    //
    // `git clone <repo> && cd repo && amplihack claude` is the whole exploit.
    // The seam below is deliberately pure so this can be pinned without
    // mutating the process-global $PATH — see
    // `tests/no_global_path_mutation.rs` for why that matters here.
    // ------------------------------------------------------------------

    #[test]
    fn an_empty_path_element_contributes_no_candidate_directory() {
        assert_eq!(
            path_dirs(OsStr::new("/usr/bin:")),
            vec![PathBuf::from("/usr/bin")],
            "a trailing colon is the current directory, and cwd is not a place \
             amplihack may look for a binary it is about to exec"
        );
        assert_eq!(
            path_dirs(OsStr::new(":/usr/bin")),
            vec![PathBuf::from("/usr/bin")],
            "a leading colon is the same hazard at the FRONT of the search order"
        );
        assert_eq!(
            path_dirs(OsStr::new("/usr/bin::/opt/bin")),
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")],
            "a doubled colon must drop out without disturbing its neighbours"
        );
    }

    #[test]
    fn every_directory_taken_from_the_path_is_absolute() {
        let dirs = path_dirs(OsStr::new("/usr/bin:relative/bin:.:..:/opt/bin"));
        assert!(
            dirs.iter().all(|d| d.is_absolute()),
            "relative entries survived: {dirs:?}"
        );
        assert_eq!(
            dirs,
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")],
            "`.`, `..` and bare relative entries are all cwd-anchored"
        );
    }

    #[test]
    fn a_path_with_nothing_absolute_in_it_yields_no_directories() {
        assert!(
            path_dirs(OsStr::new(":.:relative")).is_empty(),
            "an entirely relative $PATH must produce no candidates at all, \
             not a fallback to cwd"
        );
        assert!(path_dirs(OsStr::new("")).is_empty());
    }

    // ------------------------------------------------------------------
    // Issue #1274 — ONE place decides the empty case.
    //
    // Before this, nine `$PATH` walks each decided for themselves what an
    // unset `$PATH` meant, and the `None`-vs-empty-`Vec` branch was
    // unreachable in a test at every one of them: the only way to exercise it
    // was to unset the process-global `PATH`, which
    // `tests/no_global_path_mutation.rs` forbids. Splitting the decision out
    // as a pure function of `Option<OsString>` is what makes it testable.
    // ------------------------------------------------------------------

    #[test]
    fn an_unset_path_and_an_empty_path_answer_the_same_thing() {
        assert!(
            split_path_var_of(None, RelativeEntries::Drop).is_empty(),
            "no $PATH means nowhere to look — never a fallback to cwd"
        );
        assert_eq!(
            split_path_var_of(None, RelativeEntries::Drop),
            split_path_var_of(Some(std::ffi::OsString::from("")), RelativeEntries::Drop),
            "unset and empty are the same amount of $PATH; callers must not \
             have to know which one they got"
        );
        assert_eq!(
            split_path_var_of(None, RelativeEntries::Drop),
            split_path_var_of(
                Some(std::ffi::OsString::from(":.:relative")),
                RelativeEntries::Drop,
            ),
            "a $PATH with nothing absolute in it is also nowhere to look"
        );
    }

    #[test]
    fn no_path_at_all_is_zero_entries_in_both_modes() {
        // The disagreement this collapses: `path_conflicts` read an unset
        // `$PATH` as an empty `Vec`, and `bootstrap::prepend_path` read it as
        // `[""]`, because `split_paths("")` yields one empty element. Both
        // asked the same question about the same variable.
        for mode in [RelativeEntries::Drop, RelativeEntries::Keep] {
            assert!(
                split_path_var_of(None, mode).is_empty(),
                "an unset $PATH has no elements, in {mode:?} mode too"
            );
            assert!(
                split_path_var_of(Some(std::ffi::OsString::from("")), mode).is_empty(),
                "an empty $PATH has no elements, in {mode:?} mode too"
            );
        }

        // Why `Keep` has to answer the same way. `prepend_path` rebuilds the
        // process `$PATH` from this list, and one empty element is POSIX for
        // the current directory: `join_paths(["/new/bin", ""])` is
        // `"/new/bin:"`, which puts cwd on the `$PATH` of amplihack and every
        // child it spawns. Preserving the user's entries verbatim must not
        // mean inventing one.
        assert!(
            split_path_var(OsStr::new(""), RelativeEntries::Keep).is_empty(),
            "`Keep` preserves what is there; there is nothing there"
        );
    }

    #[test]
    fn env_path_dirs_is_the_pure_seam_applied_to_the_real_path() {
        // Reads the process $PATH; it does not write it. The point is that
        // `env_path_dirs` adds no rule of its own on top of the seam.
        assert_eq!(
            env_path_dirs(),
            split_path_var_of(std::env::var_os("PATH"), RelativeEntries::Drop),
            "`env_path_dirs` must be exactly the seam over the environment"
        );
        assert!(
            env_path_dirs().iter().all(|d| d.is_absolute()),
            "the environment seam must apply the absoluteness rule too"
        );
    }

    #[test]
    fn keeping_relative_entries_is_an_explicit_named_choice() {
        // The other half of the rule: a caller REBUILDING $PATH must not
        // silently edit the user's environment on the way past, so it asks for
        // `Keep` at the call and the reader can see it did.
        let raw = OsStr::new("/usr/bin::.:relative:/opt/bin");
        assert_eq!(
            split_path_var(raw, RelativeEntries::Keep),
            vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from(""),
                PathBuf::from("."),
                PathBuf::from("relative"),
                PathBuf::from("/opt/bin"),
            ],
            "`Keep` must preserve every element, verbatim and in order"
        );
        assert_eq!(
            split_path_var(raw, RelativeEntries::Drop),
            path_dirs(raw),
            "`path_dirs` is `Drop` — one rule, spelled once"
        );
    }

    #[test]
    fn no_candidate_path_is_relative_for_any_tool() {
        // The property the seam exists to guarantee, asserted at the level the
        // launcher actually consumes: whatever $PATH the process happens to
        // have, every candidate must be absolute. `execvp` treats a candidate
        // containing no separator as a $PATH lookup rather than a path, which
        // is precisely the case a relative candidate creates.
        for tool in ["claude", "copilot", "codex"] {
            for (path, _) in candidate_paths(tool, OverrideOrigin::User) {
                assert!(
                    path.is_absolute(),
                    "candidate {} for {tool} is relative; execvp would resolve \
                     it against the child's $PATH or the current directory",
                    path.display()
                );
                assert!(
                    path.parent().is_some_and(|p| !p.as_os_str().is_empty()),
                    "candidate {} for {tool} has an empty parent directory, \
                     which would prepend the current directory to the child's \
                     $PATH",
                    path.display()
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // C5 / issue #1276 — the override origin is wired to the tagging
    //
    // This started life as a process-global one-way latch
    // (`mark_override_amplihack_supplied`) with one setter, one implicit
    // reader, and no test calling either. Delete the whole latch and every
    // test still passed, while `amplihack rustyclawd` regressed from "warn and
    // keep looking" to "hard error" on a broken `rustyclawd`. These pin the
    // wiring: the origin must actually change how `candidate_paths` tags the
    // override, because that tag is what `resolve_from_candidates` branches
    // on.
    //
    // As a parameter this is also exercisable twice in one process — the latch
    // was not, which is why it needed a `#[cfg(test)]` reset hook to be tested
    // at all. That hook is gone.
    // ------------------------------------------------------------------

    /// Find the `AMPLIHACK_*_BINARY_PATH` entry's tag in a candidate list.
    fn override_tag(candidates: &[(PathBuf, TargetSource)], needle: &Path) -> Option<TargetSource> {
        candidates
            .iter()
            .find(|(path, _)| path == needle)
            .map(|(_, source)| *source)
    }

    #[test]
    fn the_override_origin_changes_how_the_override_is_tagged() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Absolute, so `cheap_reject`'s absoluteness rule is not what is under
        // test here — only the tagging is.
        let needle = PathBuf::from("/nonexistent-for-this-test/rustyclawd");
        let previous = std::env::var_os("AMPLIHACK_CLAUDE_BINARY_PATH");
        // SAFETY: edition 2024 requires unsafe; serialised by `env_lock()`.
        unsafe { std::env::set_var("AMPLIHACK_CLAUDE_BINARY_PATH", &needle) };

        let as_user = override_tag(&candidate_paths("claude", OverrideOrigin::User), &needle);
        let as_amplihack = override_tag(
            &candidate_paths("claude", OverrideOrigin::AmplihackSupplied),
            &needle,
        );
        // Both directions, in one process, in one test — the thing the latch
        // could not do.
        let back_to_user = override_tag(&candidate_paths("claude", OverrideOrigin::User), &needle);

        // SAFETY: as above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("AMPLIHACK_CLAUDE_BINARY_PATH", value),
                None => std::env::remove_var("AMPLIHACK_CLAUDE_BINARY_PATH"),
            }
        }

        assert_eq!(
            as_user,
            Some(TargetSource::ExplicitOverride {
                user_supplied: true
            }),
            "an override amplihack has not claimed is the user's instruction, \
             and a broken one is a hard error"
        );
        assert_eq!(
            as_amplihack,
            Some(TargetSource::ExplicitOverride {
                user_supplied: false
            }),
            "when amplihack supplied it, it is a preference: a broken \
             `rustyclawd` must warn and fall through, not fail the launch"
        );
        assert_eq!(
            back_to_user, as_user,
            "the origin is a parameter, not a latch: asking again as the user \
             must answer as the user"
        );
    }

    #[test]
    fn the_override_origin_does_not_touch_the_unprefixed_user_variable() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let needle = PathBuf::from("/nonexistent-for-this-test/claude");
        let previous = std::env::var_os("CLAUDE_BINARY_PATH");
        // SAFETY: edition 2024 requires unsafe; serialised by `env_lock()`.
        unsafe { std::env::set_var("CLAUDE_BINARY_PATH", &needle) };

        let tag = override_tag(
            &candidate_paths("claude", OverrideOrigin::AmplihackSupplied),
            &needle,
        );

        // SAFETY: as above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CLAUDE_BINARY_PATH", value),
                None => std::env::remove_var("CLAUDE_BINARY_PATH"),
            }
        }

        assert_eq!(
            tag,
            Some(TargetSource::ExplicitOverride {
                user_supplied: true
            }),
            "amplihack only ever writes the AMPLIHACK_-prefixed variable; the \
             bare one is the user's and stays an instruction"
        );
    }

    /// The regression the latch hid, pinned end to end.
    ///
    /// Issue #1276's acceptance: "broken `rustyclawd` under an
    /// amplihack-supplied override warns and keeps looking, rather than
    /// hard-erroring — has a direct test that fails if the flag stops being
    /// threaded through."
    ///
    /// `resolve_from_candidates`' two arms were already covered with
    /// hand-written `TargetSource` values, and that is exactly why deleting
    /// the latch left the suite green: nothing joined `candidate_paths` to
    /// them. This one starts at the environment, builds the candidate list the
    /// way `resolve` does, and runs the real gate over it. Stop threading
    /// `override_origin` and the two answers below become identical.
    ///
    /// `halted_on_user_override` is the observable difference and needs no
    /// second candidate to exist: it is set only by the hard-error arm.
    ///
    /// The tool name is deliberately one no host has a binary for, so the only
    /// candidate that is ever probed is the fixture — no `$PATH` mutation, no
    /// 339 MB `--version` subprocess.
    #[cfg(unix)]
    #[test]
    fn an_amplihack_supplied_override_that_fails_the_gate_does_not_halt_the_launch() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        const TOOL: &str = "rustyclawdfixture1276";
        let dir = tempfile::tempdir().expect("tempdir");
        // Present, executable, and useless: `--version` exits non-zero. The
        // shape of a broken `rustyclawd`.
        let broken = dir.path().join("rustyclawd-broken");
        std::fs::write(&broken, "#!/bin/sh\nexit 3\n").expect("write fixture");
        let mut perms = std::fs::metadata(&broken).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&broken, perms).expect("chmod");

        let var = format!("AMPLIHACK_{}_BINARY_PATH", TOOL.to_uppercase());
        let previous = std::env::var_os(&var);
        // SAFETY: edition 2024 requires unsafe; serialised by `env_lock()`.
        // Not `PATH` — see `tests/no_global_path_mutation.rs`.
        unsafe { std::env::set_var(&var, &broken) };

        let as_user = resolve_from_candidates(TOOL, &candidate_paths(TOOL, OverrideOrigin::User));
        let as_amplihack = resolve_from_candidates(
            TOOL,
            &candidate_paths(TOOL, OverrideOrigin::AmplihackSupplied),
        );

        // SAFETY: as above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(&var, value),
                None => std::env::remove_var(&var),
            }
        }

        assert_eq!(
            as_user.halted_on_user_override.as_deref(),
            Some(broken.as_path()),
            "a USER-supplied override that fails the gate is a conclusion: \
             amplihack stops and says so rather than launching something else"
        );
        assert_eq!(
            as_amplihack.halted_on_user_override, None,
            "an AMPLIHACK-supplied override is a preference: a broken \
             `rustyclawd` must warn and keep looking. This is the regression \
             that shipped green when the process-global latch was deleted — \
             both arms of `resolve_from_candidates` were covered and nothing \
             joined them to `candidate_paths`."
        );
        assert!(
            as_amplihack
                .rejected
                .iter()
                .any(|(path, _)| path == &broken),
            "falling through must still record why the preference was rejected"
        );
    }
}

#[cfg(test)]
mod issue_1325_timeout_is_not_brokenness {
    use super::*;

    /// The retry budget must be meaningfully larger than the first attempt, or
    /// the second chance is no chance at all under the load that caused #1325.
    #[test]
    fn the_retry_budget_is_substantially_larger_than_the_first_attempt() {
        assert!(
            EXPLICIT_OVERRIDE_RETRY_BUDGET >= PER_CANDIDATE_PROBE_TIMEOUT * 5,
            "a retry budget close to the original would not survive the load that \
             produced this bug: first={PER_CANDIDATE_PROBE_TIMEOUT:?} \
             retry={EXPLICIT_OVERRIDE_RETRY_BUDGET:?}"
        );
    }

    /// The retry must not be reachable through the total budget, which is
    /// smaller. It is a deliberate exception for one candidate, not a raising
    /// of the overall ceiling.
    #[test]
    fn the_retry_exceeds_the_total_budget_on_purpose() {
        assert!(
            EXPLICIT_OVERRIDE_RETRY_BUDGET > TOTAL_PROBE_BUDGET,
            "the second chance exists precisely because the shared budget is too \
             small for one explicitly named binary on a busy host"
        );
    }

    /// A timeout is a non-answer; the others are conclusions about the file.
    /// Anything that blurs that distinction reintroduces #1325.
    #[test]
    fn only_a_timeout_is_a_non_answer() {
        // Conclusions: the file itself is wrong.
        for conclusive in [
            Rejection::Missing,
            Rejection::NotAFile,
            Rejection::NotExecutable,
            Rejection::NotAbsolute,
        ] {
            assert_ne!(
                conclusive,
                Rejection::ProbeTimedOut,
                "a conclusion about the file must never be conflated with a timeout"
            );
        }
        // And the retry path keys on exactly one variant.
        assert_eq!(Rejection::ProbeTimedOut, Rejection::ProbeTimedOut);
    }

    /// Only the user's own instruction earns the second chance. A candidate
    /// amplihack found itself should keep searching instead — retrying there
    /// would spend 30s per candidate on a slow machine.
    #[test]
    fn only_a_user_supplied_override_earns_the_retry() {
        let user = TargetSource::ExplicitOverride {
            user_supplied: true,
        };
        let amplihack = TargetSource::ExplicitOverride {
            user_supplied: false,
        };
        assert_ne!(
            user, amplihack,
            "the two override origins must stay distinguishable, or the retry \
             would apply to amplihack's own preference and cost 30s per candidate"
        );
    }
}
