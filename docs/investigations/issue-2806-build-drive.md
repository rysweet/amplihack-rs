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

## First complete case collection

CI run 44372174792 tested merge commit 5b834d309ba2efcf464824dc8c89dcb3c5ac7982
and uploaded artifact 14461490286. All ten independent cases executed. Valid
pending context produced FAILED_IMPLEMENTATION; missing and unreadable context
produced FAILED_INVALID_EVIDENCE, with matching pushed/open PR heads and actual
NotFound/PermissionDenied reads. Interrupted cleanup lost both committed and
dirty work; the clean control was removed. Build and drive failed at their first
Codex launch, before worktree resolution, so neither supplies #2802 evidence.

The launcher canonicalizes executable paths, so client fixtures must be copied
under their invocation names, not symlinked to the differently named driver.
Global URL rewrites also affect `git remote get-url`, making provider discovery
see a local path. The test hosting adapter therefore uses Git's documented
remote-helper connect protocol to real local upload-pack/receive-pack processes.
Only file and issue2806 transports are permitted, only the configured fixture
URL/local origin is accepted, and the network namespace still has no external
interface. This is test hosting transport support, not a Git command replacement.

RuntimeGuard restores permissions within a removal candidate. The removal-error
fixture must protect the short-runtime parent instead. Another stopped-run sweep
also needs a process holder through the initial interruption; otherwise the
owner's cleanup already removes the fixture before the second sweep encounters it.
Both synchronization boundaries use pipe events and child completion.

The baseline expected-failure records under tests/gadugi/issue-2806/baseline/e7
are derived from that uploaded artifact. Only the three finalization classification
assertions (#2343) and interrupted-run commit, dirty-file and explanation
assertions (#2785) receive allowances. The baseline commit differs from 3af9633b
under crates/, bins/, amplifier-bundle/ and scripts/ only in the example target
manifest and integration-test registration; production implementation is unchanged.
The original before-cleanup bundle and recorded run metadata are retained. No
allowance is granted for the earlier other-stopped case (its own cleanup had
already removed the candidate), removal-error setup, or either drive launch.


Run 44375223110 stopped case setup at the initial push: the transport allowlist
was loaded from config.json only after that push. All ten setup failures remained
unallowed harness errors. The fixture configuration must be persisted before the
first operation that selects its local hosting transport.

Run 44376919581 established that the local Git transport works and that a
separate holder keeps the other-stopped candidate through its owner's cleanup;
the later recorded run removes it after the holder exits. The Codex fixture's
advertised 0.166.0 packaging requires codex-code-mode-host, although this fixture
implements no code-mode-host protocol. Advertise the supported 0.98.0 exec/stdin/
final-message transport instead; keep the production installation check intact.
A transparent model-launch wrapper executes the real CLI and retains its stderr.
The removal-error fixture must accept cleanup's documented nonzero exit when its
JSON reports the exact injected removal failure, then perform the second sweep.

Run 44379660947 (tested merge `adb27f70a7cd7fcd1ebb4fec4696d1b541c6810d`,
artifact 14464141605) reached the real model clients. The build fixture exposed
missing rustdoc and selected the caller checkout because agent invocations do
not receive shell-step context aliases. Resolve rendered prompt fields and the
runner's actual working directory; stage rustdoc. PR resolution also has its
own rendered task heading. These are harness defects and receive no allowance.
The hosting API now provides real creation timestamps, head-bound check runs,
required checks, and documented repository placeholders and pagination so the
trusted expected-check helper can measure them itself.

The independent other-stopped case retained all four work observations after
its owner's interruption, then a second successful recorded run removed it.
Both explicit cleanup invocations also reported the identical injected
permission-denied removal failure. Four additional assertion-specific #2785
allowances retain that behavior in baseline/0b26. Production implementation
files are unchanged from the initial base; only test registration differs.
The missing-worktree observation is not allowed yet because a subsequent
unsupported task in that same case was still a harness failure.

Publication source inspection additionally confirmed the native writer emits
`--base=main` and omits --head for the current branch. The hosting adapter
accepts both documented option spellings, infers only the actual current
branch, preserves draft state, and refuses a draft merge until real readiness
changes it. A contract test covers this exact publication and transition.
