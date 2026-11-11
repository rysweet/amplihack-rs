---
title: "Issue #1390: validation and delivery evidence"
type: explanation
issue: 1390
last_updated: 2026-11-10
---

# Issue #1390: validation and delivery evidence

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
