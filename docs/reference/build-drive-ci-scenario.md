---
title: Build and Auto-drive CI Scenario
description: Coverage, fixtures, isolation, and release evidence for the complete build and merge workflows.
doc_type: reference
last_updated: 2026-11-18
review_schedule: as-needed
---

# Build and Auto-drive CI Scenario

> [Home](../index.md) > [CI Pipeline](ci-pipeline.md) > Build and Auto-drive

The qa-team scenario `tests/gadugi/scenarios/issue-2806-build-drive.yaml`
runs a complete `smart-orchestrator` build and a complete
`auto-drive-to-merge` through the supported recipe runner. It supplies required
release evidence for [issue #2806](https://github.com/rysweet/amplihack-rs/issues/2806).
It also exercises the worktree, finalization, and cleanup failures described in
[#2802](https://github.com/rysweet/amplihack-rs/issues/2802),
[#2343](https://github.com/rysweet/amplihack-rs/issues/2343), and
[#2785](https://github.com/rysweet/amplihack-rs/issues/2785), including their comments.

The scenario executes **in CI only**. Never run it, its driver, or other Gadugi
scenarios on a factory host. Local unit and integration tests remain available.

## Contents

- [CI execution and evidence](#ci-execution-and-evidence)
- [Coverage and acceptance](#coverage-and-acceptance)
- [Fixture configuration](#fixture-configuration)
- [Isolation and lifecycle](#isolation-and-lifecycle)
- [Assertion outcomes](#assertion-outcomes)
- [Release dependency](#release-dependency)

## CI execution and evidence

`Install Smoke Test` in `.github/workflows/ci.yml` runs the scenario driver after
installing the binary built from the checkout and the pinned recipe runner.
The scenario has no path filter or optional opt-in. Missing prerequisites fail
the job. Existing job timeouts remain unchanged.

The qa-team YAML entry point and CI invoke the same complete Rust driver and
assertions. The driver is an example target of `amplihack-cli`, with its source
under `tests/gadugi/issue-2806/fixture/`. It adds no shipped executable or public
production API. CI stages the driver, fixture assets, and required tools before
entering the isolated execution environment; scenario execution downloads nothing.

Each case executes independently and retains its own observations. A build failure
does not stop the remaining drive, finalization, or cleanup cases from executing
and retaining evidence. CI uploads all case results and evidence on both success
and failure. Evidence lives outside folders subject to cleanup and includes:

- The tested commit, binary identity, and pinned runner revision.
- Workflow stdout, stderr, terminal outcomes, and recorded run metadata.
- Fixture invocations, selected response contracts, and hosting state transitions.
- Repository identities, commit and origin refs, Git common directories, worktree
  registrations, and execution directories.
- Isolation observations, lifecycle events, and individual assertion outcomes.
- For an expected failure, its baseline reproduction and issue attribution;
  for unexecuted behavior, the blocking assertion and reason.

For example, this read-only command lists recent push CI evidence on `main`:

```bash
gh run list --repo rysweet/amplihack-rs --workflow ci.yml \
  --branch main --event push --limit 5 --json headSha,conclusion,url
```

The output is a JSON array with `headSha`, `conclusion`, and `url` for each run.
Each run page contains the `Install Smoke Test` logs and evidence artifacts.
This lookup uses GitHub outside the scenario; scenario processes have no network
access. A green run supplies evidence only for its recorded tested commit.

## Coverage and acceptance

### Complete build

The build starts from an issue served by the hosting fixture in a temporary
repository with a local bare origin. Classification selects one development
workstream using `default-workflow` with no-merge intent.

Agent fixtures write the requested implementation and run its checks before
returning valid receipts. Real workflow steps perform orchestration, Git
operations, publishing, finalization, reflection, and summarization.
Acceptance requires independent observations of all three states:

| State | Required evidence |
| --- | --- |
| Local repository | Implementation content is committed beyond the baseline. |
| Bare origin | The implementation branch exists at the committed head. |
| Hosting fixture | Exactly one corresponding PR is open at that pushed head. |

The workflow also records terminal success. A fixture's success response alone
does not establish completion. After execution, retained run folders and branches
are inspected for their committed work.

### Pending-check finalization

Three independent cases finalize an already published implementation whose PR
checks remain pending: valid authoritative context, missing context, and
unreadable context. Fault injection occurs through an explicit event after
publication and before context consumption. The unreadable case records an
actual read failure under the identity executing finalization.

Each case re-reads the pushed origin branch and open PR and checks their heads.
The typed terminal outcome and exit result must describe the observed state,
including `BLOCKED_CI` when CI is the remaining gap. Pending checks or unavailable
context alone must not produce `FAILED_IMPLEMENTATION`. The hosting fixture keeps
checks pending until the case explicitly transitions them to green.

### Private-copy drive and worktree resolution

Each drive control and regression case independently seeds its own local bare
origin, committed and pushed implementation branch, and corresponding open PR in
its private hosting state. These inputs do not depend on the build case's output
or success. Each case launches from its own factory-style per-PR clone containing
a linked `worktrees/pr-N` checkout. Evidence establishes that execution enters the
runner-created private repository, recording its bare repository, detached launch
checkout, common directories, and registered worktrees separately from the caller.

| Case | Private-repository setup | Required behavior |
| --- | --- | --- |
| Successful control | After private-copy creation and before preflight, the fixture explicitly prepares a valid PR worktree there. | The complete drive reaches review, readiness, merge finalization, and merged readback. |
| Missing-worktree regression | The private repository initially has no suitable PR worktree; fixtures do not repair it. | Production orchestration deterministically resolves an existing valid worktree or creates one at the observed PR head, reaches the build phase, and completes the merge. |

The detached launch checkout cannot stand in for a PR worktree. Resolution leaves
dirty work intact. Agent fixtures cannot silently create the regression case's
missing worktree or report success for work that did not execute.

Review and readiness use the supported production evidence writers. A clean
review names the observed PR head and makes no commits. Before merge completion,
checks explicitly transition to green. The real finalizer invokes
`gh pr merge --match-head-commit`; the hosting fixture requires agreement between
the requested SHA, PR head, and actual local-origin head before recording a merge.
The scenario then reads back the merged state and inspects retained work.

If #2802 blocks worktree resolution, the failed resolution assertion remains
visible and downstream assertions are recorded as not executed. The successful
control still supplies complete private-copy drive-and-merge evidence.

### Interrupted work and cleanup

Cleanup cases use actual recorded runs with a checkout inside the run folder.
Before interruption, the fixture creates both an unpushed commit and an
uncommitted file, then emits a structured readiness event. The controller stops
only its recorded child PID and waits for process completion before inspection.

End-of-run cleanup and the supported cleanup command are both exercised.
For the other-stopped-run case, a separately recorded child holds the runtime
as its cwd during interruption. After the recipe exits, an explicit pipe release
and child completion remove that in-use protection. The next recorded run's sweep
then encounters an existing stopped runtime containing the original work. No
repository is copied back and no run record is fabricated.
Acceptance covers these distinct cases:

| Case | Required observation |
| --- | --- |
| Interrupted run | Its unpushed commit, uncommitted content, and usable branch survive; cleanup explains preservation. |
| Another stopped run encountered by the sweep | The same preservation rules apply even though a different run initiated cleanup. |
| Clean, fully pushed control | Cleanup removes the eligible run folder. |
| Deterministic removal failure | A second cleanup invocation does not repeat the same removal-failure report. |

Preserved runs can continue on the same branch. Continuation is demonstrated
before retained work is published. Evidence records dirty state, remote state,
run metadata, and observed cleanup ownership or locking state so preserved and
removed runs can be compared. Cleanup checks inspect behavior regardless of the
folder's cleanup category.

## Fixture configuration

Configuration is test-only and stored under `tests/gadugi/issue-2806/`:

| Path | Role |
| --- | --- |
| `fixture/main.rs` | Explicit driver and fixture entry points, verified client transports. |
| `fixture/driver.rs` | Isolated case setup and real workflow/lifecycle execution. |
| `fixture/responses.rs` | Rendered-task adaptation and action-backed responses. |
| `fixture/environment.rs` | Private environments, confined paths, and sanitized Git observations. |
| `fixture/agent.rs` | Claude and Codex adapters and response-contract validation. |
| `fixture/hosting.rs` | Typed persistent hosting state and invocation records. |
| `fixture/lifecycle.rs` | Pipe events, tracked processes, interruption, and cleanup. |
| `fixture/evidence.rs` | Independent assertions and retained observations. |
| `fixture/tests.rs` | Unit tests that do not execute the scenario. |
| `responses/contracts.json` | Declared response contracts used to select fixture behavior. |
| `responses/build.json`, `responses/drive.json`, `responses/cleanup.json` | Deterministic case responses and required actions. |
| `expected-failures.json` | Assertion-specific, evidence-backed allowances for the three referenced regressions. |

Both model clients are replaced with fixtures. Selection uses declared output
contracts in runner-delivered tasks or verified, supported runner metadata.
Invocation order and invented step identifiers are not dispatch contracts.
Claude responses use its supported output transport; Codex responses use the
validated runner-provided final-message path. Unsupported or ambiguous requests,
malformed output, and contradictory state fail the harness.

Receipts follow completed implementation, validation, review, or readiness
actions. The hosting adapter implements only verified `gh` operations, validates
PR identity against persistent state and real origin refs, and records calls.
Neither adapter forwards commands to live services.

## Isolation and lifecycle

Each case has a private temporary root with mode `0700`, its own HOME,
configuration directories, work root, temporary files, and local bare origin.
Commands execute under `env -i` with an allowlisted environment and explicitly
resolved executables. `gh`, `claude`, and `codex` resolve exclusively to fixtures.
Required workflow assets are staged into the private environment.

The environment excludes hosting tokens, model credentials, SSH agents, inherited
Git configuration, and credential helpers. Git permits only local files and the fixture hosting transport. Git's standard
remote-helper `connect` protocol binds the logical GitHub URL to real local
`git-upload-pack` and `git-receive-pack` processes; it opens no network socket.
This preserves provider discovery through `git remote get-url`: a URL rewrite
would make that command report a local path and select the non-GitHub workflow.
The helper accepts only this case's configured repository identity and local
bare origin, records every connection, and refuses other commands and URLs. CI network isolation is established and verified before orchestration;
environment scrubbing alone is insufficient. An unavailable isolation prerequisite
is a harness failure. Real user configuration, including `~/.claude` and
`~/.amplihack`, is never accessed by scenario execution.

Paths are confined to designated roots, including response files and cleanup
candidates, with traversal and symlink escapes rejected. Evidence records only
allowlisted paths and observations; it does not dump inherited environments.

Synchronization uses explicit pipe or process events and process completion.
There are no timers, sleeps, retries, raised timeouts, source-text tests, or new
production guard checks. Only exact PIDs started and recorded by the case can be
signaled; process-name and pattern-based stops are prohibited.

## Assertion outcomes

| Outcome | Meaning |
| --- | --- |
| `PASS` | The original assertion executed and passed. |
| `FAIL` | The assertion failed without an applicable, substantiated allowance. |
| `XFAIL` | An independently observed failure matches an allowance naming #2802, #2785, or #2343 and its baseline evidence. |
| Not executed | A prerequisite assertion blocked execution; the report names that assertion and the reason. |

Allowances are established from behavior on the implementation base. An open
issue or source inspection alone is insufficient evidence. Each allowance keeps
the original assertion and is limited to the affected behavior. A newly passing
assertion reports `PASS` even if its allowance remains in the file.

Unrelated failures, missing prerequisites, isolation breaches, malformed fixture
responses, and harness errors cannot receive `XFAIL`. Unreachable downstream
behavior never counts as passed or as a reproduced failure. Only downstream
behavior within the same case blocked by an evidenced, allowed regression can
remain unexecuted while the independent successful control supplies the required
complete workflow coverage. A failed case cannot justify skipping another case;
the remaining cases execute and retain their own evidence before the aggregate
result is reported. Other failures and unexplained omissions fail CI.

Deterministic fixtures establish workflow behavior for the prescribed contracts.
They do not prove how every live model interprets worktree instructions.

## Release dependency

This scenario is part of required `Install Smoke Test` coverage. Every subsequent
release depends on a successful CI run containing its results, with only the
issue-scoped expected failures described above permitted.

The existing [exact-commit release admission](ci-pipeline.md#a-release-follows-green-ci-on-the-same-commit)
provides the dependency: Auto Release consumes successful push CI on `main` and
builds its recorded tested SHA; manual dispatch also requires successful push CI
on that exact SHA. A green run for an older commit cannot admit a different tree.
The [release stamp commit](ci-pipeline.md#release-tags-point-at-a-stamp-commit)
is the documented version-stamping child of that tested source commit. No
separate scenario release bypass or live-model credentials are required.

## Recorded baseline allowances

CI [run 44372174792](https://github.com/rysweet/amplihack-rs/actions/runs/44372174792)
at tested merge commit `5b834d309ba2efcf464824dc8c89dcb3c5ac7982` supplies the
records in `tests/gadugi/issue-2806/baseline/e7/`:

| Issue | Allowed assertions | Observed behavior |
| --- | --- | --- |
| #2343 | The three `finalization-*.classification` assertions | Valid pending context returns `FAILED_IMPLEMENTATION`; missing/unreadable context returns `FAILED_INVALID_EVIDENCE` despite matching pushed/open PR heads. |
| #2785 | `cleanup-interrupted.commit`, `.dirty`, `.explanation` | Both kinds of unpublished work disappear after interruption; cleanup does not explain preservation. |

The remaining assertions have no allowance from this run. Its build and drive
launch failures are harness errors, its other-stopped sweep lacked the required
existing candidate, and its removal-failure setup did not produce an access error.

CI [run 44379660947](https://github.com/rysweet/amplihack-rs/actions/runs/44379660947)
at `adb27f70a7cd7fcd1ebb4fec4696d1b541c6810d` supplies four further #2785
records in `baseline/0b26/`: the other-stopped case's commit, dirty-content, and
explanation assertions, plus repeated removal-failure reporting. The stopped
runtime was observed intact after interruption and before the second recorded
run's sweep. The removal case retains both real permission-denied reports and
both exit codes. These allowances preserve each original assertion; passing
behavior takes precedence over an allowance.

CI [run 44382405727](https://github.com/rysweet/amplihack-rs/actions/runs/44382405727)
at `97d56c6194071548551470bd0fc837be7609595c` supplies
`baseline/97/drive-missing.worktree.json` for #2802. The private repository has
only its bare registration and detached launch checkout, and every entry gate
reports no-worktree without a fixture error. Only the resolution assertion is
allowed; review, retention and merge downstream remain explicitly unexecuted.

## Successful whole-workflow evidence

CI [run 44386929063](https://github.com/rysweet/amplihack-rs/actions/runs/44386929063)
at `e6ae5f81cf477050a5776d62b4ca43bb159e5e0a` produced artifact
`14466583260`. Both `build.complete` and `drive-control.complete` are PASS:
the real smart-orchestrator reports success, committed answer content matches
the bare-origin branch and one open PR, and the private-copy drive records the
production matched-head merge followed by a fresh MERGED readback.

The report contains 15 PASS, 11 issue-scoped XFAIL, six explicitly unexecuted
assertions, and zero harness errors. The allowances comprise three #2343
finalization classifications, one #2802 worktree-resolution assertion, and seven
#2785 preservation/reporting assertions. The unexecuted assertions are the four
missing-worktree downstream checks and the two cleanup continuation checks;
they supply no passing coverage. Independent successful controls remain mandatory.

Local verification includes 31 unit and fixture-integration tests, formatting,
all-target Clippy with warnings denied, and pre-commit on all files. No Gadugi
scenario or complete driver was run on the implementation host. The final
reflection, outside-in validation and summary use actual Git/hosting observations
and retained command exit receipts; they do not fabricate downstream completion.

When running the YAML entry point in an already prepared CI namespace, stage the
example executable on PATH and supply its inputs through
`AMPLIHACK_2806_ASSETS`, `AMPLIHACK_2806_AMPLIHACK`, `AMPLIHACK_2806_RUNNER`,
`AMPLIHACK_2806_CARGO`, `AMPLIHACK_2806_RUSTC`, `AMPLIHACK_2806_PRECOMMIT`, and
`AMPLIHACK_2806_TESTED_SHA`. The unconditional CI step supplies the equivalent
explicit arguments. All executable inputs are resolved before isolation;
the driver downloads and installs nothing.
