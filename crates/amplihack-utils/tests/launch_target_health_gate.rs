//! Integration tests for the launch-target health gate (issue #1266, Task A).
//!
//! These drive the real I/O shell against a temp-dir fixture, through the
//! `resolve_from_candidates` seam, so they never mutate process environment
//! (`std::env::set_var` is `unsafe` under edition 2024).
//!
//! The behaviour under test is the one that was missing on 2026-08-21, when
//! amplihack logged
//!
//! ```text
//! INFO launching claude binary=/home/azureuser/.npm-global/bin/claude version="unknown"
//! ```
//!
//! and then executed a 500-byte shell stub, producing
//! `Exec format error (os error 8)`. It had the signal and proceeded anyway.
//! Health is a filter, never an annotation.

#![cfg(unix)]

use amplihack_utils::launch_target::{
    InstallDecision, Rejection, TargetSource, decide_install, resolve_from_candidates,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The placeholder `@anthropic-ai/claude-code` leaves at `bin/claude.exe` when
/// its postinstall is suppressed: 500 bytes, ASCII, no shebang. Verified byte
/// for byte on the dev VM.
fn write_stub(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut body = b"echo \"Error: claude native binary not installed.\" >&2\nexit 1\n".to_vec();
    body.resize(500, b' ');
    fs::write(&path, body).unwrap();
    make_executable(&path);
    path
}

/// A binary that answers `--version` with a parseable semver.
///
/// Deliberately a small, ordinary shell script. It used to be padded with
/// 8 KiB of `#` so it would clear a pre-probe size threshold — a test that has
/// to pad past a production threshold to pass is reporting that the threshold
/// is wrong, and this one was: the same threshold rejected `@github/copilot`'s
/// real 1185-byte loader. Nothing in these fixtures may be sized to fit a gate.
fn write_healthy(dir: &Path, name: &str, version: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!("#!/bin/sh\necho '{version} (Claude Code)'\nexit 0\n"),
    )
    .unwrap();
    make_executable(&path);
    path
}

/// Exits non-zero on `--version`: present, executable, and useless.
fn write_broken_prober(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 3\n").unwrap();
    make_executable(&path);
    path
}

/// Answers `--version` with something that carries no semver.
fn write_unparseable(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\necho unknown\n").unwrap();
    make_executable(&path);
    path
}

/// Hangs forever on `--version`.
fn write_hanging(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nsleep 600\n").unwrap();
    make_executable(&path);
    path
}

fn make_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn rejection_for<'a>(rejected: &'a [(PathBuf, Rejection)], path: &Path) -> Option<&'a Rejection> {
    rejected.iter().find(|(p, _)| p == path).map(|(_, r)| r)
}

// ---------------------------------------------------------------------------
// The headline defect: never launch a stub
// ---------------------------------------------------------------------------

#[test]
fn a_stub_is_rejected_and_the_healthy_binary_behind_it_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let stub_dir = dir.path().join("npm-global-bin");
    let good_dir = dir.path().join("usr-bin");
    fs::create_dir_all(&stub_dir).unwrap();
    fs::create_dir_all(&good_dir).unwrap();

    let stub = write_stub(&stub_dir, "claude");
    let good = write_healthy(&good_dir, "claude", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (stub.clone(), TargetSource::AmplihackPrefix),
            (good.clone(), TargetSource::Path),
        ],
    );

    let target = resolution
        .target
        .expect("the healthy binary behind the stub must be found");
    assert_eq!(target.path, good);
    assert_eq!(target.version, "2.1.238");
    assert_eq!(target.source, TargetSource::Path);
    assert_eq!(
        rejection_for(&resolution.rejected, &stub),
        Some(&Rejection::PlaceholderStub)
    );
}

#[test]
fn a_stub_alone_yields_no_target_at_all() {
    // Not "a target with version: unknown". No target.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude");

    let resolution =
        resolve_from_candidates("claude", &[(stub.clone(), TargetSource::AmplihackPrefix)]);

    assert!(
        resolution.target.is_none(),
        "amplihack must not execute a binary it could not verify"
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &stub),
        Some(&Rejection::PlaceholderStub)
    );
}

#[test]
fn the_first_healthy_candidate_wins_not_the_first_found() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_healthy(dir.path(), "claude-first", "2.1.237");
    let second = write_healthy(dir.path(), "claude-second", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (first.clone(), TargetSource::Path),
            (second, TargetSource::AmplihackPrefix),
        ],
    );

    assert_eq!(resolution.target.unwrap().path, first);
}

// ---------------------------------------------------------------------------
// The regression that would have caught the copilot breakage
//
// `resolve` is tool-generic. A pre-probe rejection based on file size and the
// absence of native magic is a fact about `@anthropic-ai/claude-code`, and
// running it as a gate for every tool killed `amplihack copilot`: on this host
// `~/.npm-global/bin/copilot` is a 1185-byte `#!/usr/bin/env node` loader.
// Small is not broken.
// ---------------------------------------------------------------------------

/// A faithful copy of what `@github/copilot` actually installs: a tiny
/// `#!/usr/bin/env node`-style shim. Under 4 KiB, no native magic — the exact
/// shape the removed fast path rejected.
fn write_small_node_shim(dir: &Path, name: &str, version: &str) -> PathBuf {
    let path = dir.join(name);
    let body = format!(
        "#!/bin/sh\n\
         # npm-loader.js equivalent — a real one is 1185 bytes\n\
         echo '{version}'\n\
         exit 0\n"
    );
    assert!(
        body.len() < 4096,
        "fixture invariant: this shim must stay small enough to have been \
         rejected by the gate that broke copilot ({} bytes)",
        body.len()
    );
    fs::write(&path, body).unwrap();
    make_executable(&path);
    path
}

#[test]
fn a_small_node_shim_resolves_for_copilot() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_small_node_shim(dir.path(), "copilot", "0.0.415");

    let resolution =
        resolve_from_candidates("copilot", &[(shim.clone(), TargetSource::AmplihackPrefix)]);

    let target = resolution.target.expect(
        "@github/copilot ships a legitimate sub-4 KiB `#!/usr/bin/env node` \
         loader; rejecting it means `amplihack copilot` reinstalls on every \
         launch and then hard-fails",
    );
    assert_eq!(target.path, shim);
    assert_eq!(target.version, "0.0.415");
    assert!(resolution.rejected.is_empty());
}

#[test]
fn a_small_shim_resolves_for_every_tool_including_claude() {
    // Not a copilot special case — a tool-generic resolver has no business
    // judging file size for ANY tool, present or future.
    for tool in ["claude", "copilot", "codex", "some-future-tool"] {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_small_node_shim(dir.path(), tool, "1.2.3");
        let resolution = resolve_from_candidates(tool, &[(shim.clone(), TargetSource::Path)]);
        assert_eq!(
            resolution.target.map(|t| t.path),
            Some(shim),
            "a small healthy shim must resolve for {tool}"
        );
    }
}

#[test]
fn the_real_stub_is_still_rejected_and_still_named_correctly() {
    // The good diagnosis survives the fix. The 500-byte placeholder fails
    // `--version` on its own merits, and the report still calls it an
    // incomplete install rather than a generic probe failure.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude");

    let resolution =
        resolve_from_candidates("claude", &[(stub.clone(), TargetSource::AmplihackPrefix)]);

    assert!(resolution.target.is_none());
    assert_eq!(
        rejection_for(&resolution.rejected, &stub),
        Some(&Rejection::PlaceholderStub),
        "a failed probe on a placeholder-shaped file must be labelled as one, \
         not left as a bare ProbeFailed"
    );
}

#[test]
fn a_small_broken_shim_is_a_placeholder_and_a_large_one_is_a_probe_failure() {
    // The label is the only thing the shape decides. Both are rejected; they
    // are rejected because `--version` failed, not because of their size.
    let dir = tempfile::tempdir().unwrap();
    let small = write_broken_prober(dir.path(), "claude-small");
    let large = dir.path().join("claude-large");
    fs::write(&large, format!("#!/bin/sh\nexit 3\n{}\n", "#".repeat(5000))).unwrap();
    make_executable(&large);

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (small.clone(), TargetSource::Path),
            (large.clone(), TargetSource::Path),
        ],
    );

    assert_eq!(
        rejection_for(&resolution.rejected, &small),
        Some(&Rejection::PlaceholderStub)
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &large),
        Some(&Rejection::ProbeFailed)
    );
}

// ---------------------------------------------------------------------------
// SEC-4: the bound covers the drain, not just the wait
// ---------------------------------------------------------------------------

#[test]
fn a_candidate_that_leaves_a_grandchild_holding_its_pipe_still_resolves_promptly() {
    // Measured before the fix: 60.0 s against a 10 s budget. The shim exits
    // immediately, but the backgrounded `sleep` inherits its stdout, so the
    // drain thread never saw EOF and the unbounded join waited for the
    // grandchild. `launch_target` probes every candidate on `$PATH`, which is
    // precisely the threat SEC-4 names.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude");
    fs::write(
        &path,
        "#!/bin/sh\n/bin/sleep 30 &\necho '2.1.238'\nexit 0\n",
    )
    .unwrap();
    make_executable(&path);

    let started = std::time::Instant::now();
    let resolution = resolve_from_candidates("claude", &[(path.clone(), TargetSource::Path)]);
    let elapsed = started.elapsed();

    assert_eq!(
        resolution.target.map(|t| t.version),
        Some("2.1.238".to_string()),
        "the child exited cleanly and printed a version; that is a healthy binary"
    );
    assert!(
        elapsed < amplihack_utils::launch_target::TOTAL_PROBE_BUDGET,
        "resolution must stay within TOTAL_PROBE_BUDGET, took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Every rejection reason, end to end
// ---------------------------------------------------------------------------

#[test]
fn a_missing_path_is_rejected_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ghost = dir.path().join("claude");
    let resolution = resolve_from_candidates("claude", &[(ghost.clone(), TargetSource::Path)]);
    assert_eq!(
        rejection_for(&resolution.rejected, &ghost),
        Some(&Rejection::Missing)
    );
}

#[test]
fn a_dangling_symlink_is_missing_not_a_target() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("claude");
    std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
    let resolution = resolve_from_candidates("claude", &[(link.clone(), TargetSource::Path)]);
    assert_eq!(
        rejection_for(&resolution.rejected, &link),
        Some(&Rejection::Missing)
    );
}

#[test]
fn a_live_symlink_is_followed_not_rejected() {
    // Every npm-installed claude on every host is a symlink into
    // lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe. Using
    // symlink_metadata (or any is_file() derived from it) would reject them
    // all, including amplihack's own install.
    let dir = tempfile::tempdir().unwrap();
    let real = write_healthy(dir.path(), "claude.exe", "2.1.238");
    let link = dir.path().join("claude");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let resolution =
        resolve_from_candidates("claude", &[(link.clone(), TargetSource::AmplihackPrefix)]);
    let target = resolution
        .target
        .expect("a symlinked npm install is the normal case, not a rejection");
    assert_eq!(target.path, link);
    assert_eq!(target.version, "2.1.238");
}

#[test]
fn a_directory_is_rejected_as_not_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("claude");
    fs::create_dir(&subdir).unwrap();
    let resolution = resolve_from_candidates("claude", &[(subdir.clone(), TargetSource::Path)]);
    assert_eq!(
        rejection_for(&resolution.rejected, &subdir),
        Some(&Rejection::NotAFile)
    );
}

#[test]
fn a_non_executable_file_is_rejected_without_a_probe() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_healthy(dir.path(), "claude", "2.1.238");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o644);
    fs::set_permissions(&path, perms).unwrap();

    let resolution = resolve_from_candidates("claude", &[(path.clone(), TargetSource::Path)]);
    assert_eq!(
        rejection_for(&resolution.rejected, &path),
        Some(&Rejection::NotExecutable)
    );
}

#[test]
fn a_non_zero_version_probe_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_broken_prober(dir.path(), "claude");
    let resolution = resolve_from_candidates("claude", &[(path.clone(), TargetSource::Path)]);
    assert!(resolution.target.is_none());
    // The fixture is a small script, so the failed probe earns the sharper
    // `PlaceholderStub` label. Both are the same verdict — the probe failed —
    // and the label is decided after that, never instead of it. See
    // `a_small_broken_shim_is_a_placeholder_and_a_large_one_is_a_probe_failure`.
    assert_eq!(
        rejection_for(&resolution.rejected, &path),
        Some(&Rejection::PlaceholderStub)
    );
}

#[test]
fn version_unknown_is_a_rejection_not_an_annotation() {
    // This is the exact signal amplihack had on 2026-08-21 and ignored.
    let dir = tempfile::tempdir().unwrap();
    let path = write_unparseable(dir.path(), "claude");
    let resolution = resolve_from_candidates("claude", &[(path.clone(), TargetSource::Path)]);
    assert!(
        resolution.target.is_none(),
        "a binary that cannot report its version is never launched"
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &path),
        Some(&Rejection::UnparseableVersion)
    );
}

// ---------------------------------------------------------------------------
// SEC-4: bounded probing
// ---------------------------------------------------------------------------

#[test]
fn a_hanging_candidate_times_out_and_the_next_one_is_still_reached() {
    let dir = tempfile::tempdir().unwrap();
    let hang = write_hanging(dir.path(), "claude-hang");
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");

    let started = std::time::Instant::now();
    let resolution = resolve_from_candidates(
        "claude",
        &[
            (hang.clone(), TargetSource::Path),
            (good.clone(), TargetSource::FallbackDir),
        ],
    );
    let elapsed = started.elapsed();

    assert_eq!(resolution.target.expect("must fall through").path, good);
    assert_eq!(
        rejection_for(&resolution.rejected, &hang),
        Some(&Rejection::ProbeTimedOut)
    );
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "one hung candidate must not stall the launch; took {elapsed:?}"
    );
}

#[test]
fn the_total_probe_budget_bounds_a_path_full_of_hanging_binaries() {
    // SEC-4: eight candidates at the per-candidate timeout would be 24s of
    // foreground hang. The total budget is what makes that impossible.
    let dir = tempfile::tempdir().unwrap();
    let candidates: Vec<_> = (0..12)
        .map(|i| {
            (
                write_hanging(dir.path(), &format!("claude-{i}")),
                TargetSource::Path,
            )
        })
        .collect();

    let started = std::time::Instant::now();
    let resolution = resolve_from_candidates("claude", &candidates);
    let elapsed = started.elapsed();

    assert!(resolution.target.is_none());
    assert!(
        elapsed < std::time::Duration::from_secs(15),
        "total probe budget must bound the whole pass; took {elapsed:?}"
    );
}

#[test]
fn probing_stops_at_the_first_healthy_candidate() {
    // The common case must be one subprocess, not a full sweep: nothing after
    // the winner may appear in the rejection list.
    let dir = tempfile::tempdir().unwrap();
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");
    let hang = write_hanging(dir.path(), "claude-hang");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (good.clone(), TargetSource::Path),
            (hang.clone(), TargetSource::AmplihackPrefix),
        ],
    );

    assert_eq!(resolution.target.unwrap().path, good);
    assert!(
        rejection_for(&resolution.rejected, &hang).is_none(),
        "candidates after the winner must never be probed"
    );
}

// ---------------------------------------------------------------------------
// Explicit override
// ---------------------------------------------------------------------------

#[test]
fn a_broken_user_supplied_override_is_an_error_not_a_silent_demotion() {
    // If you point amplihack at a specific binary and it is broken, amplihack
    // says so rather than quietly launching a different one.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude-override");
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                stub.clone(),
                TargetSource::ExplicitOverride {
                    user_supplied: true,
                },
            ),
            (good, TargetSource::Path),
        ],
    );

    assert!(
        resolution.target.is_none(),
        "a broken user override must not silently fall through to another binary"
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &stub),
        Some(&Rejection::PlaceholderStub)
    );
}

#[test]
fn a_broken_user_supplied_override_records_the_halt_path() {
    // The early return is what makes the broken override a hard error rather
    // than a silent demotion, and it deliberately records no `NotProbed`. That
    // leaves `decide_install` with an empty-of-inconclusive-evidence rejection
    // list and no way to tell WHICH question was answered — so the path comes
    // out with it.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude-override");
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                stub.clone(),
                TargetSource::ExplicitOverride {
                    user_supplied: true,
                },
            ),
            (good, TargetSource::Path),
        ],
    );

    assert_eq!(
        resolution.halted_on_user_override.as_deref(),
        Some(stub.as_path()),
        "the override that stopped resolution must be named in the result"
    );
}

#[test]
fn an_amplihack_set_override_that_falls_through_records_no_halt() {
    // Only the hard-error exit is a halt. A preference that warns and keeps
    // looking has not stopped resolution, so recording it would make
    // `decide_install` refuse an install on a resolution that examined
    // everything.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude-preferred");
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                stub,
                TargetSource::ExplicitOverride {
                    user_supplied: false,
                },
            ),
            (good, TargetSource::Path),
        ],
    );

    assert!(
        resolution.halted_on_user_override.is_none(),
        "falling through is not halting"
    );
}

#[test]
fn a_broken_override_amplihack_cannot_reach_does_not_buy_an_install() {
    // Issue #1266's own loop, reached through the new funnel. `export
    // CLAUDE_BINARY_PATH=/opt/vendor/bin/claude` with a typo used to answer
    // `InstallMissing`: amplihack spent a multi-hundred-megabyte npm install,
    // re-resolved to the same broken override, failed — and decided
    // identically on the next launch, forever. An install only ever writes
    // `~/.npm-global/bin`, so it could never have changed this answer.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude-override");
    let amplihack_bin = dir.path().join("npm-global-bin");
    fs::create_dir_all(&amplihack_bin).unwrap();

    let resolution = resolve_from_candidates(
        "claude",
        &[(
            stub,
            TargetSource::ExplicitOverride {
                user_supplied: true,
            },
        )],
    );

    assert_eq!(
        decide_install("claude", &resolution, Some("2.1.239"), Some(&amplihack_bin)),
        InstallDecision::BrokenOverride,
        "an install that cannot reach the override must not be bought"
    );
}

#[test]
fn a_broken_override_inside_amplihacks_prefix_still_buys_the_repair() {
    // The other half, and the reason the exit reports conclusive evidence at
    // all: `CLAUDE_BINARY_PATH=~/.npm-global/bin/claude` pointing at the
    // 500-byte placeholder IS repairable, because that is the one directory an
    // install rewrites. Refusing here would turn the demonstrated repair path
    // into a hard error.
    let dir = tempfile::tempdir().unwrap();
    let amplihack_bin = dir.path().join("npm-global-bin");
    fs::create_dir_all(&amplihack_bin).unwrap();
    let stub = write_stub(&amplihack_bin, "claude");

    let resolution = resolve_from_candidates(
        "claude",
        &[(
            stub,
            TargetSource::ExplicitOverride {
                user_supplied: true,
            },
        )],
    );

    assert_eq!(
        decide_install("claude", &resolution, Some("2.1.239"), Some(&amplihack_bin)),
        InstallDecision::InstallMissing,
        "a placeholder in amplihack's own prefix is exactly what an install fixes"
    );
}

#[test]
fn a_broken_amplihack_set_override_falls_through() {
    // `configure_preferred_rustyclawd_binary` sets AMPLIHACK_CLAUDE_BINARY_PATH
    // in-process. That is a preference, not an instruction, so a broken value
    // must not turn a working `amplihack rustyclawd` into a hard failure.
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude-preferred");
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                stub.clone(),
                TargetSource::ExplicitOverride {
                    user_supplied: false,
                },
            ),
            (good.clone(), TargetSource::Path),
        ],
    );

    assert_eq!(
        resolution.target.expect("must fall through").path,
        good,
        "an amplihack-set preference that is broken warns and continues"
    );
}

// ---------------------------------------------------------------------------
// F-S5 / C3 / Issue 1 — the absoluteness invariant, at the one funnel every
// candidate producer passes through
//
// `path_dirs` filters `$PATH`-derived directories, but the two `ExplicitOverride`
// arms in `candidate_paths` push their value unfiltered, and a fourth producer
// added later would have to remember the rule all over again. `cheap_reject` is
// where the invariant lives now, so these assert it through
// `resolve_from_candidates` — the seam that receives whatever any producer
// pushes.
// ---------------------------------------------------------------------------

#[test]
fn a_relative_user_supplied_override_fails_loudly_instead_of_launching_something_else() {
    // `CLAUDE_BINARY_PATH=claude`. `cheap_reject` stats it against amplihack's
    // cwd; `execvp` would resolve it against the child's `$PATH`. Neither file
    // is the one the user named, so the only honest answer is to stop and say
    // so — the same contract as
    // `a_broken_user_supplied_override_is_an_error_not_a_silent_demotion`,
    // which this must not be allowed to bypass.
    let dir = tempfile::tempdir().unwrap();
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");
    let relative = PathBuf::from("claude");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                relative.clone(),
                TargetSource::ExplicitOverride {
                    user_supplied: true,
                },
            ),
            (good, TargetSource::Path),
        ],
    );

    assert!(
        resolution.target.is_none(),
        "a relative user override must not silently fall through to another \
         binary; got {:?}",
        resolution.target
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &relative),
        Some(&Rejection::NotAbsolute)
    );
    let report = resolution.rejection_report("claude", "@anthropic-ai/claude-code");
    assert!(
        report.contains("absolute"),
        "the report must tell the user what to do about it:\n{report}"
    );
}

#[test]
fn a_relative_amplihack_set_override_falls_through_like_any_other_bad_preference() {
    let dir = tempfile::tempdir().unwrap();
    let good = write_healthy(dir.path(), "claude-good", "2.1.238");
    let relative = PathBuf::from("./claude");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (
                relative,
                TargetSource::ExplicitOverride {
                    user_supplied: false,
                },
            ),
            (good.clone(), TargetSource::Path),
        ],
    );

    assert_eq!(
        resolution.target.expect("must fall through").path,
        good,
        "a preference is a preference even when it is unusable"
    );
}

#[test]
fn a_relative_path_derived_candidate_is_never_probed() {
    // The `$PATH` half: an empty element joined with `claude` is the bare name
    // `claude`. It must be rejected on filesystem facts alone, before
    // `probe_version` spawns anything — a hostile `./claude` that prints
    // parseable semver would otherwise become the selected `LaunchTarget`.
    let dir = tempfile::tempdir().unwrap();
    let good = write_healthy(dir.path(), "claude", "2.1.238");
    let relative = PathBuf::from("claude");

    let resolution = resolve_from_candidates(
        "claude",
        &[
            (relative.clone(), TargetSource::Path),
            (good.clone(), TargetSource::Path),
        ],
    );

    assert_eq!(
        resolution.target.expect("the absolute candidate wins").path,
        good
    );
    assert_eq!(
        rejection_for(&resolution.rejected, &relative),
        Some(&Rejection::NotAbsolute),
        "and the relative one is recorded as such, not as a failed probe"
    );
}

// ---------------------------------------------------------------------------
// The error surface (Defect 3)
// ---------------------------------------------------------------------------

#[test]
fn the_rejection_report_explains_a_total_failure_without_naming_architecture() {
    let dir = tempfile::tempdir().unwrap();
    let stub = write_stub(dir.path(), "claude");
    let resolution =
        resolve_from_candidates("claude", &[(stub.clone(), TargetSource::AmplihackPrefix)]);

    assert!(resolution.target.is_none());
    let report = resolution.rejection_report("claude", "@anthropic-ai/claude-code");
    assert!(
        report.contains(&stub.display().to_string()),
        "the report must name what it tried:\n{report}"
    );
    let lower = report.to_lowercase();
    assert!(
        lower.contains("npm install"),
        "the report must state a remedy:\n{report}"
    );
    for forbidden in ["exec format error", "os error 8", "architecture"] {
        assert!(
            !lower.contains(forbidden),
            "the report must not contain {forbidden:?}:\n{report}"
        );
    }
}

#[test]
fn an_empty_candidate_list_is_not_a_panic() {
    let resolution = resolve_from_candidates("claude", &[]);
    assert!(resolution.target.is_none());
    assert!(resolution.rejected.is_empty());
    let _ = resolution.rejection_report("claude", "@anthropic-ai/claude-code");
}
