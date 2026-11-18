# Issue 2806: build and drive investigation

Implementation base: `3af9633bd9244169c16434a5fc59a88af3c9bb24` (main).
The implementation worktree starts at that commit. Read #2806, #2802,
#2785 and #2343, including all comments, on 18 November 2026 using `gh`.
No Gadugi scenario has been executed on this host.

## Verified interfaces

- `amplihack recipe run smart-orchestrator -w <checkout> -c key=value`
  runs classify, routing, reflection and validation subrecipes. Development
  routes through default-workflow; use workflow=default-workflow,
  force_single_workstream=true and no_merge=true.
- `amplihack recipe run auto-drive-to-merge` uses autodrive-build,
  autodrive-crusty-loop, autodrive-merge-loop and the final report. Entry
  evidence is gathered by three separate fast-tier agents. Build preflight
  explicitly requires an existing linked worktree. No replacement is created
  by that prompt. This is a source finding, not a behavioral reproduction.
- `run/execute.rs` launches the supported runner with the recipe path,
  --output-format json, -C <private-exec>, -R search roots and context.
  A step identifier is not delivered to the child (documented in that source).
  Fixtures must select by declared output contracts or task headings, never
  a supposed step-id environment variable or invocation number.
- The installed runner's cli_subprocess adapter supports Claude `-p <task>`
  and Codex `exec ... --output-last-message <path>` with task on stdin.
  Claude's default transport is plain stdout, not a fabricated JSON envelope.
  The repository runner pin is claude-plugin/recipe-runner.rev; CI must install
  that pin before staging executables. Installed local runner source is newer;
  transport differences must be treated as harness errors. Follow-up inspection
  of the actual pin (2833a0b) confirms Claude uses bare -p with task on stdin
  and --system-prompt-file for the system turn; the adapter supports that
  transport as well as the older inline transport. Codex remains stdin plus
  --output-last-message.
- `workflow_context.sh` snapshots AMPLIHACK_CONTEXT_FILE when sourced. If that
  variable exists, an unreadable or missing file does not fall back to aliases.
  Finalization collect and validate are separate operations on
  workflow_agentic_finalization.sh. The narrative is opaque text and cannot
  establish terminal success.
- workflow-finalize's validate-agentic-finalization emits typed workflow_result;
  non-success classifications exit 1. It may be wrapped by step recovery.
  Pending checks are not evidence that implementation did not happen (#2343).
- Crusty decisions use `orch helper autodrive-completion-write`. CLEAN with
  matching reviewed and recorded head and no commits completes; otherwise
  evaluate, including unreadable round evidence. Missing optional sidecars do
  not invalidate a CLEAN decision. Never compare to the base checkout's HEAD.
- Merge readiness is a posted write-up followed by an outcome word; the old
  parsed readiness API has been retired. Real round writers capture the
  assessed head. autodrive_merge.sh invokes gh pr view, gh pr checks --required,
  gh pr merge --match-head-commit, then merged readback. It never polls.
- Cleanup consumes run.json and owned-paths under the configured work root.
  RunTmp includes the short r/<id> runtime path; RunWorktree has independent
  dirty/unpushed checks. #2785 describes deletion through the RunTmp path.
  Behavior must be observed before an allowance can be justified.
- Install Smoke Test installs the tested checkout and pinned recipe runner.
  It has no pre-existing network namespace isolation for scenarios. A private
  Linux network namespace must be created before executing this driver; env -i
  alone is insufficient. Scenario assets must already be staged.
- CI's aggregate depends on install-smoke. Release admission requires successful
  push CI for the exact source SHA (with its documented release-stamp child).
  See docs/reference/ci-pipeline.md. No extra release bypass is appropriate.

## Evidence plan

Independent case identifiers: build, finalization-valid, finalization-missing,
finalization-unreadable, drive-control, drive-missing, cleanup-interrupted,
cleanup-other-stopped, cleanup-clean, cleanup-removal-failure. Failure in a case
must not stop other cases. Assert committed implementation, origin ref and PR
head independently. Record execution common dir, private repository, detached
launch path and registered linked worktree separately. Prepare a private PR
worktree only for the successful control; never repair the regression in an
agent fixture. Interrupted fixtures require both an unpushed commit and dirty
file; verify continuation before pushing. Synchronize by pipe events and child
completion, signal only a recorded live child PID.

Expected-failure allowances start empty. No assertion has baseline behavioral
proof yet. Source inspection and issue status do not justify an allowance.
CI must retain exact baseline failures, prerequisites and fingerprints before
allowing only #2802, #2785 or #2343. Unreachable behavior stays not executed;
newly passing assertions always pass even with a remaining allowance.

## First CI observation

Run 44369680421, Install Smoke Test job 135032745645, compiled the driver
and installed its prerequisites. Execution stopped before case setup because
/sys/class/net reflected the sysfs mount's original namespace after unshare.
This is a harness failure, not evidence for any regression allowance. The
correct namespace observation is /proc/self/net/dev. Persist the initial report
and prerequisite observations before enforcing network isolation so early
failures retain artifacts too. The expected-failure file remains empty.
