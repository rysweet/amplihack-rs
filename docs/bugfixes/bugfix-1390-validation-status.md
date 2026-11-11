---
title: "Issue #1390: validation and delivery evidence"
type: explanation
issue: 1390
last_updated: 2026-11-11
---

# Issue #1390: validation and delivery evidence

**Current Linux verification — 2026-11-11.** The
implementation below supersedes the earlier documentation-only status.
Historical failures remain recorded with their original revisions and
filesystem limitations. They are not final verification. The ordinary push
is the final delivery gate; its result and matching local/remote/PR head
receipt are recorded in the [PR description](https://github.com/rysweet/amplihack-rs/pull/2289).

## Current implementation and verification

Starting branch: `99ff1cab9cd06059f979b18f8fd82d24a7ac02b1`.
Fetched main: `bf64691d12ea5ce61b0616e65e14fbbdb5e80dda`.
Normal merge: `09b14f57df837b6b4ab2048c64ca618eb743fe36`.
The fetched revision is an ancestor of the merged branch. No rebase or hook
bypass was used. Pending changes were preserved as a patch and Git stash,
then reapplied after the merge. The original push log remains unchanged.

The integrated runner is pinned to
`90286bc50b84ded773795d2c8301b689f680db0b`. The fixture uses that dependency's
`Descriptor`, private-directory parser and `Store`, with Cargo's artifact
report selecting its library rather than guessing from stale build files.
Its writer opens before any invocation ledger, setup, or result output.
Probes remain side-effect-free. Both scenarios require real CLI admission
and finalization of a private durable `usage_limit` outcome, alongside the
existing classification and single-execution assertions. This is controlled
runner-result testing, not real agent execution or replay verification.

The tested fixture's SHA-256 is
`531e135c5b1d44160daf9b4f28017c91dfd3d2a3204a5a3cec430957b1aacf48`.
It is unchanged after the final passing behavioral run. The final CLI, classifier and
pattern logs have SHA-256 values
`085d4cbec1fc36cc8ac818da773715f2c66113b6cf596bb536eda7048404073c`,
`78b7d274e94156a1ff3259ef47d040a61a6bf01608d34ff60aceb72d565eb579`,
and `a009417dd506987c2d46a1020e62bbf349551707c8dfc1f7454a99331d56b869`.

Tests retain private HOME/configuration/profile/process roots, the clean
scratch-base guard, and both repository boundaries. Synthetic launcher
probes run without an agent override: ancestor context resolves before the
inner boundary and is excluded after it. No tests add sleeps, reset timers,
or wall-clock assertion windows. Production classification and cleanup are
unchanged.

Execution receipts and full logs for this implementation are retained under
`target/finish-1390/`. All commands below use Rust 1.97.0,
`CARGO_BUILD_JOBS=4`, empty `RUSTC_WRAPPER`, and
`TMPDIR=/tmp/issue-1390-compare-scratch`. The scratch helper correctly rejects
that base because `/tmp/.claude` exists, then uses writable `/var/tmp`.
No guard was relaxed. Build output is on XFS under
`/d0/ryan/cargo-targets/fix-2289`; scratch storage is on ext4. Shared Git
metadata is writable in this run. Earlier read-only constraints no longer
apply. `AMPLIHACK_TEST_BINARY` and `RUST_TEST_THREADS` are unset; the host
Cargo configuration supplies eight test threads.
The final CLI run explicitly used umask 022, matching the hook wrapper;
fixture directories and recovery documents enforce modes 0700 and 0600
independently of that umask.

| Command (Cargo is `/home/ryan/.cargo/bin/cargo`) | Exit | Result |
| --- | --- | --- |
| `env -u AMPLIHACK_TEST_BINARY cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit:: -- --nocapture` | 0 | Four passed, including both named CLI scenarios and both descriptor/probe contracts. Both scenarios reached all behavioral assertions and the synthetic launcher probes without an override. |
| `cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_failure_class:: -- --nocapture` | 0 | 52 passed: precedence, complete and absent reset hints, serialized evidence/attempt/action, bare 429, refusal/authentication, unrelated failures, transport retry eligibility. |
| `cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_limit_patterns:: -- --nocapture` | 0 | 14 passed. |
| `cargo fmt --all --check`; `git diff --check` | 0; 0 | Formatting and whitespace passed. |
| `pre-commit run --all-files` | 0 | Every configured pre-commit check passed, including artifact guard and `cargo clippy --locked --all-targets -- -D warnings`. |
| `cargo test --locked -p amplihack --test integration autodrive_pr_worktree:: -- --nocapture` | 0 | 48 passed with the enclosing recipe's work-root variables still present in the parent environment; sandbox commands remove them. |
| `cargo test --locked -p amplihack --test amplihack_bin issue_538_install_completeness:: -- --nocapture` | 0 | 10 passed with a private HOME/cwd repository boundary, controlled provider and fixture/system PATH (`install-boundary.{log,exit}`). |

The scenario assertions establish `usage_limit`, full `9:20am (UTC)`,
preserved error and stdout evidence, identical stdout/stderr verdicts, exit
1, terminal transport action on attempt 1, and exactly one runner execution.
The multi-step scenario also establishes completed setup and failed
`limited-agent` identity. The descriptor and tree identify the admitted run;
tree revision is at least two (runner writer publication plus CLI
finalization), with outcome `usage_limit`.

An intermediate fixture build failed because the explicit library link path
omitted Cargo's `deps/` directory. That was corrected before the passing run;
`cli-link-path-initial.{log,exit}` preserves the failed iteration. Earlier
fixture-creation failures, isolated passes, skipped scenarios and no-op pushes
do not satisfy these current checks.

The broader main workspace run exposed a separate test prerequisite: the
host wrapper prepended `/d0/ryan/runner-pins/90286bc`, whose copied executable
has no Cargo installation receipt at the location the publish tests require.
Seven publish-boundary tests failed before invoking the runner. The installed
`/home/ryan/.cargo/bin/recipe-runner-rs` has the real Cargo receipt naming
the exact pin. Both executables have SHA-256
`75d262bdb4e2a1b3702e3567ea0a48f5f9b4095b6470cb02ff128eea88563721`.
Selecting the installed runner with `RECIPE_RUNNER_RS_PATH` fixes the
prerequisite: all seven tests pass on main (exit 0,
`cargo test --locked -p amplihack --test integration workflow_publish_pr_metadata::runner:: -- --nocapture`).
No receipt was fabricated and no installed executable was changed.
The complete main workspace command returned 101 with exactly that one
failed target (`-p amplihack --test integration`), seven prerequisite
failures, and no cleanup failure. Every target ran because the command used
`--no-fail-fast`. Its log and exit receipt are
`main-workspace-final.{log,exit}`. The seven-test rerun establishes the
prerequisite correction only; it is not represented as a successful whole
workspace rerun. The final branch hook uses the installed runner explicitly.

### Required hook iteration and fixture corrections

The complete integrated pre-push preflight returned 1: its workspace Cargo
hook returned 101 with two failed targets. The binary target had 97 passes
and one failed install fixture; the root integration target had 1,755 passes,
eight failed resolver fixtures, eight ignored tests and 15 filtered tests.
Every remaining target ran, including the passing cleanup-lock test.
`pre-push-corrected.{log,exit}` preserves this failed iteration; it is not
reported as a successful required gate.

Execution demonstrated two additional fixture isolation gaps, justifying
two narrow Rust test-file changes beyond the original three-file design:

- `autodrive_pr_worktree_tests.rs` inherited `AMPLIHACK_WORK_ROOT` and
  `AMPLIHACK_WORK_ROOT_EXPLICIT`. Its expected repository-local worktrees
  were created in the enclosing recipe's external work root instead. The
  sandbox now removes both inherited values. All 48 resolver tests pass.
- `issue_538_install_completeness.rs` selected the host's Codex launcher
  context and probed native plugins, although its contract only concerns
  staged assets. The fixture now uses its private HOME as cwd, establishes
  a `.git` boundary there, selects Claude explicitly, and restricts Unix
  PATH to fixture tooling and system utilities. This preserves the real
  installation and source-derived asset assertions while excluding host
  account-dependent plugin discovery.

Only orphaned worktrees produced by these validation runs were removed:
their Git pointer had to name this run's scratch directory, its target had
to be absent, and their entries had to be only the synthetic fixture files.
Eighteen such orphans were removed; other worktrees were left alone.

The stricter malformed-descriptor assertions also exposed a fixture-mode
gap under umask 022: a mode-0755 parent was rejected before JSON decoding.
The issue #1390 fixture now explicitly sets its owned root to 0700 and its
malformed descriptor files to 0600, canonicalizes the existing descriptor
paths, and checks JSON/type decoder diagnostics. The initial strict failure
is retained in `cli-private-root-initial.{log,exit}`. The final registered
run (`cli-final.{log,exit}`) passes all four tests under umask 022, reaching
both negative decoder assertions and both CLI behavioral scenarios.

No tests or hook checks were removed or skipped to resolve these failures.
No production installer, resolver, classifier, retry, or cleanup behavior
was changed. The final ordinary commit and push run their configured hooks;
their receipts belong in the linked PR description rather than being
inferred from these targeted passes.

### Current cleanup comparison

The original failure is the post-release `Refused(InUse)` shown in the
preserved log below. Comparable worktrees were created at
`/tmp/issue-1390-start` (`99ff1cab9cd06059f979b18f8fd82d24a7ac02b1`) and
`/tmp/issue-1390-main` (`bf64691d12ea5ce61b0616e65e14fbbdb5e80dda`). Both
used separate XFS target directories under `/d0/ryan/cargo-targets/`,
the same tmpfs scratch directory `/tmp/issue-1390-compare-scratch`, Rust
1.97.0, four Cargo build jobs, empty `RUSTC_WRAPPER`, inherited umask 0077,
and the configured eight test threads. They ran sequentially; unrelated
host workloads were active and were not controlled.

| Workload | Starting branch exit/result | Fetched main exit/result |
| --- | --- | --- |
| `cargo test --locked -p amplihack-cli --lib commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted -- --exact --nocapture` | 0; one passed | 0; one passed |
| Whole CLI library with hook-equivalent exclusions, command below | 0; 5,620 passed, five ignored, 975 filtered | 0; 5,620 passed, five ignored, 975 filtered |

The exact library command was built and run as follows, using the host's
58 configured process-test exclusions and the same library-relevant install
and heavy-test exclusions as the hook wrapper:

```bash
args=(--skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu --skip commands::install:: --skip commands::update::)
while IFS= read -r name; do
  [ -n "$name" ] && args+=(--skip "$name")
done < /home/ryan/.amplihack/work/proc-skip-tests.txt
/home/ryan/.cargo/bin/cargo test --locked -p amplihack-cli --lib -- "${args[@]}"
```

The expanded command is retained in `library-direct-command.txt` with SHA-256
`27b31ff35d9d648910872bfe5732a74dea013371ad962cf87186040300b1652e`;
the skip file's SHA-256 is
`bf1f5b7d663a53d5a8ec3c0620a6ea1ce5e50ad32004a79a881b044b84de974b`.
Complete library logs have SHA-256
`7d493f9ae9e7e8b4a28498db622d276d0dd069a9299aeef0ba5fa065a94e2cda`
(branch) and
`641f3536b4a227f4a96aab54a3d576bc7327400e0cf4286c49dc1b65717ddfb2`
(main). These direct Cargo runs reproduced the failed library target's
selection without waiting behind unrelated host wrapper scheduling locks;
they do not replace the required normal push hook.

An initial workspace stress comparison used 72 threads. The starting branch
reached unrelated recipe shell tests that remained running and was manually
interrupted, so that run supplies no cleanup verdict or passing hook result.
The corresponding main attempt queued behind the host's first test slot.
Read-only descriptor inspection found orphaned sleepers from another PR
holding that slot after its original owner exited. No other task's process
was killed, no slot file was changed, and no hook was bypassed. A normal
retry admitted a main workspace run through the scheduler's available second
slot. That full main run completed with the separate seven-test provenance
prerequisite failure described above; the cleanup test passed. The integrated
branch's required hook must still pass with the corrected runner selection.

A separate single-threaded Rust investigation, synchronized by a Unix socket
pair and `waitpid`, demonstrated that a child inheriting a held `flock`
keeps it after the parent drops its handle. The independent probe then
acquired the lock only after child completion. This is a plausible mechanism
for a parallel-suite post-drop refusal; it does **not** identify the child
in the original failure. Closing on exec does not exclude the interval
between fork and exec. A genuine inherited holder must still prevent target
removal, so weakening production cleanup would be wrong.

**Unresolved reproduction limitation:** neither comparable library run
reproduced the original failure. Matching source and successful reruns do
not establish non-causation. No PR-caused cleanup defect was demonstrated,
and no cleanup production change or timing-based workaround was added.

## Contents

- [Current implementation and verification](#current-implementation-and-verification)
- [Review disposition](#review-disposition)
- [Evidence required before delivery](#evidence-required-before-delivery)
- [Historical push-log digest and sanitized excerpt](#historical-push-log-digest-and-sanitized-excerpt)
- [Step 16b: Outside-In Testing Results](#step-16b-outside-in-testing-results)
- [Step 13: Local Testing Results](#step-13-local-testing-results)
- [Historical step 11b results](#historical-step-11b-results)
- [Scratch storage gate](#scratch-storage-gate)
- [Validation before integration](#validation-before-integration)
- [Integration and delivery gates](#integration-and-delivery-gates)
- [Cleanup attribution](#cleanup-attribution)

## Review disposition

| Review finding | Disposition | Evidence or remaining requirement |
| --- | --- | --- |
| [Code review P1](https://github.com/rysweet/amplihack-rs/pull/2289#issuecomment-6773557323) and [security review P1](https://github.com/rysweet/amplihack-rs/pull/2289#issuecomment-6773578413): ancestor launcher state was not bounded | Resolved | Both registered CLI scenarios pass the synthetic ancestor probe without an override, then prove exclusion after the inner `.git` boundary. Outer `.git`, private environment/process roots and scratch safeguards remain. |
| Same reviews' P2: retained harness had absolute helper/binary paths and duplicated scenarios | Resolved | The duplicate executable documentation harnesses are no longer tracked. The one registered Rust module shares both scenarios and uses the current-checkout executable helper with no override. Cargo identifies the pinned recovery library by artifact report. |
| [Step 17b P1](https://github.com/rysweet/amplihack-rs/pull/2289#issuecomment-6784357600): executable behavior had not reached assertions | Resolved | Four registered tests now pass, including both complete CLI scenarios; writable clean scratch storage is available. |
| Step 17b integration/hook/delivery finding and subsequent philosophy review | Integration and affected validation resolved; final delivery receipt linked | Current main merged normally. The linked PR description records ordinary commit/push hook results and matching head checks. Earlier "Ready for Final Review" commentary and the failed preflight are not acceptance evidence. |
| Classification precedence, complete reset hints, and usage-limit exclusion from transport retries align with the design | Accepted; no classifier redesign | Current classifier suite (52), shared patterns (14), and the separately executed registered CLI scenarios pass. |
| Upstream runtime owns internal session and subagent lifecycle | Accepted scope | Reference retains upstream transcript, reasoning-context, and child-cleanup requirements without claiming that this change implements or validates them. |
| Durable recovery contract drift | Resolved | Pinned upstream descriptor/store APIs implement fixture admission; both CLI scenarios verify admission and finalization. Reference links the integrated recovery contract and distinguishes transport retry, durable recipe recovery, and upstream session lifecycle. |
| Historical push evidence was incomplete | Digest, sanitized excerpt, and library result recorded below | Original log remains unchanged. The hook failed with 5,626 library tests passed and one failed; this is not a successful push. |
| Cleanup failure attribution | Unresolved reproduction limitation retained | Comparable full CLI library workloads and exact tests pass on both starting branch and fetched main. Descriptor inheritance was demonstrated separately, but the historical child and cause remain unidentified; full-hook delivery remains required. |
| Final behavioral and integration evidence | Resolved | Current merged-checkout registered CLI tests (four), classifier tests (52), shared pattern tests (14), resolver tests (48), and install fixture tests (10) pass. Main ancestry is verified. The ordinary push and final head/clean-tree receipt are reported in the PR description. |

Issue #1390 and its maintainer comments, the PR body, all discussion comments,
and the formal-review and inline-comment collections were fetched again for
this implementation. The latter two collections were empty. No separate
Crusty review was available; none is inferred from other reviewers' comments.
The maintainer's [scope clarification](https://github.com/rysweet/amplihack-rs/issues/1390#issuecomment-5449708778)
governs the implementation. PR #2289 and issue #1390 stay open, with
`Refs #1390` rather than an automatic closing reference.

See the [failure classification and retry reference](../reference/transient-failure-retry.md)
for classification, integrated recipe recovery, and upstream ownership.

## Evidence required before delivery

This is the historical gate list from the documentation-only step. Its
dispositions describe that step; current implementation results above
supersede them.

Run final checks against the integrated revision, preserving exact commands,
source SHAs, environment and wrapper exclusions, complete logs, and numeric
exit receipts. Historical focused passes cannot satisfy these gates.

| Gate | Required evidence | Current disposition |
| --- | --- | --- |
| Single limited-agent CLI scenario | Pass `cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::a_single_limited_step_preserves_the_reset_hint -- --exact --nocapture` with the normal executable build helper and isolated scratch storage | Failed during fixture creation in the recorded runs; no behavioral pass. |
| Completed setup followed by a limited agent | Pass `cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::reported_limit_survives_the_cli_boundary_without_transport_retry -- --exact --nocapture`; establish step identities, complete hint, matching JSON/stderr, and exactly one runner execution | Failed before CLI assertions in the recorded runs. |
| Shared harness patterns | Pass `cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_limit_patterns:: -- --nocapture`, alongside final classifier tests | No final integrated-revision result recorded. The 52 classifier passes cover a different test module. |
| Comparable branch/main hook workloads | Run the required hooks on the integrated branch and current main with equivalent toolchain, environment, scratch isolation, test selection, wrapper exclusions, and separate build storage; compare target results and investigate the cleanup failure | Exact cleanup-test reruns and earlier library comparisons do not supply a comparable full-hook baseline or explain the original failure. |
| Durable recovery integration | Identify integrated CLI and upstream runner revisions; validate capability enforcement, descriptor propagation, durable admission, and replay eligibility; update both linked runner references | Pending implementation, integration, and validation. |
| Integration | Successful normal fetch and merge of current main, recorded integration commit, and main ancestry check | Recorded attempts failed on shared Git metadata permissions. |
| Delivery | Required checks pass; commit the final changes, push normally with hooks enabled, and verify matching local, remote, and PR tips plus current main ancestry | No successful new delivery established. `Everything up-to-date` does not validate hooks or deliver a new commit. |

## Historical push-log digest and sanitized excerpt

The original log is `/home/ryan/.amplihack/work/push-2289.log` (portable
location: `$HOME/.amplihack/work/push-2289.log`). Its SHA-256 was read during
this refinement without modifying the file:

```text
SHA-256: 44718665b5e4f99ff186a82772a734e30f520a0f646600c494c30cf516549475
Size: 1,486,729 bytes; 18,845 lines
```

Sanitized excerpt from lines 13–15, 10315–10326, and 18841–18845; omitted
lines are marked, the thread ID is redacted, and the absolute cache path is
replaced with `[build-cache]`:

```text
cargo test (workspace, skip heavy).......................................Failed
- hook id: cargo-test
- exit code: 101
[...]
---- commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted stdout ----

thread 'commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted' ([thread-id]) panicked at crates/amplihack-cli/src/commands/cleanup/tests_target_remove.rs:519:61:
called `Result::unwrap()` on an `Err` value: Refused(InUse)


failures:
    commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted

test result: FAILED. 5626 passed; 1 failed; 5 ignored; 0 measured; 975 filtered out; finished in 77.09s

error: test failed, to rerun pass `-p amplihack-cli --lib`
[...]
error: 1 target failed:
    `-p amplihack-cli --lib`
amplihack: build cache [build-cache] -- 69.7 GiB free on /.

error: failed to push some refs to 'https://github.com/rysweet/amplihack-rs.git'
```

The **5,626 passed / one failed** result belongs to `amplihack-cli --lib`,
not the whole workspace. The log records one failed target and a rejected
push. It also records the host Cargo wrapper skipping process-scanning and
install/deploy tests (lines 17–18), so this was not an unfiltered workspace
run. Its cleanup failure remains unexplained; later passes do not erase it.

## Step 16b: Outside-In Testing Results

**FAIL / delivery blocked — 2026-11-10.** Tested committed source
`99ff1cab9cd06059f979b18f8fd82d24a7ac02b1`. The worktree was clean on entry;
the earlier changes were already committed. The remote PR branch also points
to this SHA. No successful CLI behavioral result or new delivery is claimed.

Detected toolchains: Rust 1.97.0, edition 2024, Cargo workspace manifests and
`Cargo.lock`. All affected executable source is Rust. `bins/amplihack/src/main.rs`
is the executable entry point; `crates/amplihack-cli/tests/main.rs` registers
the scenarios in `cli_integration` (`autotests = false`). Root `package.json`
and `pyproject.toml` package unchanged npm and uvx wrappers; neither wrapper
is involved in this change. README and CONTRIBUTING document Cargo build,
test, format and clippy commands.

Strategy: apply qa-team's native Rust validation to the registered tests,
without an executable override, with private HOME/config/profile/process
roots and synthetic ancestor launcher state. Keep the existing scratch and
repository boundaries. Use the pinned Cargo directly, writable checkout-local
build storage and separate baseline build storage. Detached tmux uses a
private socket. No timers or wall-clock test windows were added.

Commands below use `/home/ryan/.cargo/bin/cargo`, `CARGO_TARGET_DIR="$PWD/target"`,
empty `RUSTC_WRAPPER`, and no `AMPLIHACK_TEST_BINARY`.

| Scenario | Command | Result and key output |
| --- | --- | --- |
| Simple: one limited agent preserves the full reset hint and structured verdict | `cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::a_single_limited_step_preserves_the_reset_hint -- --exact --nocapture` | FAIL, exit 101; fixture creation at line 130 returned errno 30 under `/var/tmp`, before invoking the CLI. |
| Integration: completed setup followed by a limited agent, matching JSON/stderr, exactly one runner execution | `cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::reported_limit_survives_the_cli_boundary_without_transport_retry -- --exact --nocapture` | FAIL, exit 101; same fixture-creation failure before any CLI assertion. |
| Classification and precedence | `cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_failure_class:: -- --nocapture` | PASS, exit 0; 52 passed, 0 failed. |
| Reported cleanup-lock failure on the PR branch | `cargo test --locked -p amplihack-cli --lib commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted -- --exact --nocapture` | PASS, exit 0; 1 passed. |
| Same cleanup-lock test on origin/main | Same test command with `--manifest-path "$TMPDIR/issue-1390-validation/main/Cargo.toml"` and separate `main-target` storage | PASS, exit 0; 1 passed. Source freshly extracted with `git archive origin/main`; main SHA `bd0df1988f9e9a7a354eb62ef49efb10f4ff8b57` matches `git ls-remote`. |
| Cleanup source comparison | `git diff --exit-code origin/main -- crates/amplihack-cli/src/commands/cleanup` | PASS, exit 0; identical source. |
| Formatting and whitespace | `cargo fmt --all --check`; `git diff --check` | PASS, both exit 0. |

Full output, commands, source hashes and numeric exit receipts are retained
under `target/review-1390-step16b/`. The original `push-2289.log` is unchanged.
Cleanup attribution remains unresolved: successful reruns and identical source
do not establish the cause of the original intermittent failure.

Fix count: **0 source fixes; one validation iteration**. Both writable roots
still fail the scratch helper's existing isolation contract; `/var/tmp` and
`/tmp` are read-only. Repeating the scenarios cannot supply eligible storage.
No guard was weakened and no real user configuration was accessed by them.

`git fetch origin main` failed opening read-only `FETCH_HEAD` (255), and
`git merge --no-edit origin/main` failed creating read-only `ORIG_HEAD.lock`
(128). Current main is outside branch ancestry. No rebase, force-push or
hook bypass was used. Integration and a new commit remain blocked by shared
Git metadata permissions. The earlier failing required-hook run remains
historical evidence below; focused passes do not waive delivery gates.

Artifact guard passed (exit 0). `git add -A` then failed creating read-only
`index.lock` (128), so no commit was attempted. A normal
`git push origin HEAD:feat/issue-1390-a-usage-limit-kills`, with hooks enabled,
reported `Everything up-to-date` (exit 0) plus a read-only local tracking-ref
update error. The remote tip was verified again as `99ff1cab9`; no new commit
was delivered. A push with no updated refs does not establish that the required
workspace-test hook passes. The evidence document remains uncommitted locally.

## Step 13: Local Testing Results

**FAIL — 2026-11-10.** Both current-source outside-in scenarios compiled and
executed, but failed during fixture creation, before invoking the CLI. Neither
scenario establishes a behavioral pass. Tested source: local HEAD
`6e69aec0a19e2f4fff674849799033fa37f75b8c` plus the uncommitted change splitting
the single-step and multi-step scenarios through one shared Rust fixture.
Full output, numeric exit receipts, source hashes and the exact patch are
retained in `target/review-1390-step13/`.

Detected toolchains:

- Rust/Cargo: root workspace `Cargo.toml`, `Cargo.lock`, and
  `rust-toolchain.toml` (Rust 1.97.0, edition 2024); all affected executable
  source is Rust. The `amplihack-cli` manifest sets `autotests = false` and
  registers `tests/main.rs` as `cli_integration`. The executable entry point
  is `bins/amplihack/src/main.rs`. README development commands document
  locked workspace builds and tests. Other language manifests belong to
  unrelated assets and distribution paths.

Chosen validation strategy:

- Use qa-team's native Rust CLI workflow: select the two registered scenarios
  individually, with `AMPLIHACK_TEST_BINARY` absent so the normal helper builds
  this checkout's executable. The single-step scenario checks the complete
  reset hint and structured failure. The multi-step scenario additionally
  checks completed setup, failed-agent identity, stderr/JSON agreement and
  exactly one runner execution. Both retain synthetic ancestor launcher
  configuration checks and temporary HOME/config/profile/process roots.
- Use `/home/ryan/.cargo/bin/cargo` with `CARGO_TARGET_DIR="$PWD/target"`,
  `RUSTC_WRAPPER` empty, and the recipe-provided `TMPDIR`. This avoids the
  host Cargo wrapper's test exclusions. Long commands run in a private-socket
  tmux session with `GIT_TERMINAL_PROMPT=0`, null stdin and a command-level
  timeout. Tests contain no timers or wall-clock assertion windows.

### Scenario 1 — Single limited agent preserves its full reset hint

Command: `/home/ryan/.cargo/bin/cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::a_single_limited_step_preserves_the_reset_hint -- --exact --nocapture`

Result: FAIL (exit 101)

Output: `test result: FAILED. 0 passed; 1 failed; 0 ignored`.
Fixture creation at `issue_1390_usage_limit.rs:130` failed with
`Read-only file system` (errno 30) under `/var/tmp/.tmpawHmgw`.
Full output: `target/review-1390-step13/simple.log`;
exit receipt: `target/review-1390-step13/simple.exit`.

### Scenario 2 — Completed setup followed by a limited agent is not transport-retried

Command: `/home/ryan/.cargo/bin/cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::reported_limit_survives_the_cli_boundary_without_transport_retry -- --exact --nocapture`

Result: FAIL (exit 101)

Output: `test result: FAILED. 0 passed; 1 failed; 0 ignored`.
Fixture creation at `issue_1390_usage_limit.rs:130` failed with
`Read-only file system` (errno 30) under `/var/tmp/.tmpfnuecw`.
Full output: `target/review-1390-step13/complex.log`;
exit receipt: `target/review-1390-step13/complex.exit`.

Root cause: the scratch helper rejects both writable roots because they are
below framework state. The clean `/var/tmp` candidate is read-only. No
isolation guard was weakened, and neither scenario reached real user state
or its CLI assertions. See the scratch storage gate below for the complete
constraint. Passing classifier or cleanup checks cannot replace these CLI
results.

Integration was also attempted freshly: `git fetch origin main` returned 255
opening read-only `FETCH_HEAD`; `git merge --no-edit origin/main` returned 128
creating read-only `ORIG_HEAD.lock`. Their complete output and exits are in
`target/review-1390-step13/{fetch,merge}.{log,exit}`. Current `origin/main` is
`bd0df1988f9e9a7a354eb62ef49efb10f4ff8b57`, outside local HEAD ancestry.

Staging the Rust scenario and this record returned 128 creating read-only
`index.lock` (`stage.log`, `stage.exit`). No commit or push was attempted
after that failure. No hooks were disabled or bypassed.

Additional current-source checks passed: all 52 failure-classification tests,
the exact reported cleanup-lock test on the branch, formatting and whitespace
checks (each exit 0). These results have separate `.log` and `.exit` pairs in
the same evidence directory. They do not change Step 13's failing status.

The exact cleanup-lock test also passed freshly on `origin/main`
`bd0df1988f9e9a7a354eb62ef49efb10f4ff8b57` (1 passed, exit 0). Its source
was extracted again with `git archive origin/main` into the existing isolated
baseline checkout; the baseline used its separate `main-target` build
directory and the same pinned toolchain. The actual command was
`/home/ryan/.cargo/bin/cargo test --manifest-path "$TMPDIR/issue-1390-validation/main/Cargo.toml" --locked -p amplihack-cli --lib commands::cleanup::sweep::target_remove::tests::held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted -- --exact --nocapture`.
Evidence: `main-source.txt`, `main-cleanup.log` and `main-cleanup.exit`.
`git diff --exit-code origin/main -- crates/amplihack-cli/src/commands/cleanup`
also returned 0 (`cleanup-source.log`, `cleanup-source.exit`). The original
intermittent failure remains unreproduced. Passing exact tests and identical
cleanup source do not establish its cause or waive the failing delivery gates.

## Historical step 11b results

Delivery of [PR #2289](https://github.com/rysweet/amplihack-rs/pull/2289)
remains **incomplete**. Step 11b rechecked local commit
`c5db3477f84f4005b6141fb0be55db16b50430ce`; no integration commit was
created. Production classification and the executable regression's isolation
checks remain unchanged.

## Scratch storage gate

`crates/amplihack-cli/tests/support/scratch_base.rs` supports `TMPDIR` and
then tries `/var/tmp` and `/tmp`. It rejects a base under the real
`$HOME/.claude` or `$HOME/.amplihack`, or below any ancestor containing
`amplifier-bundle`, `.claude`, or `src/amplihack/.claude`.

The execution sandbox exposes only two writable roots: this worktree and
the recipe's `runtime/tmp` directory under `$HOME/.amplihack`. The worktree
has `amplifier-bundle` above any fixture created inside it; the recipe
temporary directory lies under the real `$HOME/.amplihack`. Both fail the
existing isolation contract. Creating temporary directories directly in
`/tmp` and `/var/tmp` returned `Read-only file system` (errno 30).

Setting `TMPDIR` to either writable root cannot supply a clean scratch base.
Adding a writability check to candidate selection would still leave no
eligible base. No helper correction was justified: this environment cannot
provide writable storage satisfying the existing checks. No ancestor check
was removed or bypassed.

## Validation before integration

The previous implementation step's **52 focused passes** remain historical
evidence. Its retained logs are under the recipe run's
`runtime/tmp/issue-1390-validation/`, including `branch-classification.log`,
`branch-cli.log`, `branch-lib.log`, `main-lib.log`, and `hooks.log`.
The original `push-2289.log` was not modified.

Step 11b ran the commands below at the local commit above. These are fresh
results **before integration**, not checks against current main merged into
the branch. Command output, environment, commit, timestamps, and exit codes
are retained in `target/review-1390-step11b/*.log` and `*.json`.

The test commands used `/home/ryan/.cargo/bin/cargo`, with
`CARGO_TARGET_DIR="$PWD/target"`, `RUSTC_WRAPPER` empty, and
`AMPLIHACK_TEST_BINARY` absent. `TMPDIR` retained the recipe's temporary
directory. The registered target retained its normal executable build helper
and had no executable override. Scratch creation failed before that helper
or any CLI assertion executed.

| Command | Result |
| --- | --- |
| `cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit:: -- --nocapture` | Failed, exit 101: target compiled; fixture creation at line 119 failed under read-only `/var/tmp`. None of the boundary, setup, step-identity, diagnostic, preserved-error, or single-execution assertions ran. |
| `cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_failure_class::` | Passed: 52 passed, 0 failed. This repeats the focused evidence; it does not validate the executable scenario. |
| `git diff --check origin/main...HEAD` | Passed for the existing branch diff. |

The required hooks were invoked with a writable pre-commit cache using:

```bash
export CARGO_TARGET_DIR="$PWD/target"
export PRE_COMMIT_HOME="$PWD/target/review-1390-step11b/pre-commit-cache"
export CARGO=/home/ryan/.cargo/bin/cargo
export RUSTC_WRAPPER=
unset AMPLIHACK_TEST_BINARY
pre-commit run --hook-stage pre-push --files \
  crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs \
  crates/amplihack-cli/tests/issue_1390_usage_limit.rs \
  crates/amplihack-cli/tests/main.rs \
  docs/index.md \
  docs/reference/transient-failure-retry.md
```

Artifact guard, Git-command policy, formatting, clippy, merge-conflict,
whitespace, final-newline, and large-file checks passed. YAML, TOML,
toolchain-reference, and recipe-budget checks had no matching files.
The workspace-test hook completed with exit 101; pre-commit returned 1.
Writable build storage resolved the earlier hook build-cache blocker, but
the required hook gate still failed. The nine failing targets were:

| Target | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| `amplihack --test amplihack_bin` | 97 | 1 | 0 |
| `amplihack --test integration` | 1,649 | 71 | 8 |
| `amplihack-cli --lib` | 5,555 | 72 | 5 |
| `amplihack-cli --test cli_integration` | 1,512 | 169 | 0 |
| `amplihack-hooks --lib` | 742 | 41 | 0 |
| `amplihack-hooks --test tmux_socket_precedence` | 0 | 1 | 0 |
| `amplihack-types --lib` | 64 | 4 | 0 |
| `amplihack-utils --lib` | 877 | 1 | 0 |
| `amplihack-workflows --lib` | 51 | 1 | 0 |

The registered issue #1390 test again failed at scratch creation under
read-only `/var/tmp`. Other failures also report read-only paths; the full
output is retained in `required-hooks.log`, with per-target results in
`workspace-results.json`. This step did not run a new full-workspace main
baseline and does not attribute every failure to the environment or this PR.
The host Cargo wrapper also applied its existing process-scanning and
install/deploy test exclusions, recorded in the log; this was not an
unfiltered workspace run. No tests were newly excluded by this change.

## Integration and delivery gates

`git ls-remote origin refs/heads/main
refs/heads/feat/issue-1390-a-usage-limit-kills` and the REST response from
`gh api repos/rysweet/amplihack-rs/pulls/2289` rechecked the remote refs:

| Ref | Commit |
| --- | --- |
| Local HEAD | `c5db3477f84f4005b6141fb0be55db16b50430ce` |
| Remote branch and PR head | `8d98bd142e920958d7409fa7f40e9e938ebb2cdd` |
| Remote main and local `origin/main` | `bd0df1988f9e9a7a354eb62ef49efb10f4ff8b57` |

The tips do not match. `git merge-base --is-ancestor origin/main HEAD`
returned 1; the two main commits `3515dfed7` and `bd0df1988` are outside
branch ancestry. Remote ref inspection succeeds, but does not constitute a
successful fetch or integration.

| Command | Result |
| --- | --- |
| `git fetch origin main` | Failed, exit 255: shared worktree `FETCH_HEAD` is read-only. |
| `git merge --no-edit origin/main` | Failed, exit 128: cannot create shared worktree `ORIG_HEAD.lock` on the read-only filesystem. |
| `git add docs/bugfixes/bugfix-1390-validation-status.md` | Failed, exit 128: cannot create the shared worktree's `index.lock`; this evidence document remains uncommitted. |

The shared git directory is outside both writable roots. No normal push was
attempted because the required validation and integration gates have not
passed. No force-push, hook bypass, PR merge, or issue closure was performed.
There is no post-integration validation result or resulting integration
commit to report.

Completion requires an execution environment with writable shared git
metadata and a writable scratch base satisfying the helper's checks. Then
fetch and merge current main normally, pass the registered regression and
required hooks, commit the evidence update, push normally, and recheck main
ancestry and matching local, remote, and PR tips.

## Cleanup attribution

**Unresolved reproduction limitation.** The retained branch and main runs
both passed the exact cleanup-lock test and all 100 surrounding cleanup
tests. Their library suites had identical lists of 72 surrounding failures
(branch: 5,555 passed; main: 5,553 passed; both: 72 failed, 5 ignored).
Those passing reruns and matching surrounding failures do not explain the
original intermittent `Refused(InUse)` after dropping the held descriptor.
They establish neither its cause nor that it is unrelated to this PR. No
cleanup repair or separate cleanup reproduction run was introduced in step
11b.

The cleanup-lock test passed again within step 11b's workspace hook. That is
another passing observation, not a reproduction or explanation of the
original failure, and does not change the attribution above.
