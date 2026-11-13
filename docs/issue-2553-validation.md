# Issue #2553 validation

Fixes #2553.

The implementation separates administrative leaves from repository edits, keeps
tracking optional and assigns execution identities independently of issue numbers.
Classification fixtures supply explicit labels and demonstrate routing; they are
not evidence that a model interprets arbitrary prose correctly. The deterministic
classifier tests exercise the stated action-and-destination examples separately.

## Registered acceptance checks

```sh
cargo test --locked -p amplihack-cli --test cli_integration issue_2553_
cargo test --locked -p amplihack-cli --lib issue_2553_
cargo test --locked -p amplihack-workflows --lib issue_2553_
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
pre-commit run --all-files
```

The integration modules cover optional and malformed issues, identity, complete
plan validation, Operations routing, mixed requests, the pinned runner, literal
tracking bodies, durable mutation attempts, dependencies, completion validation,
EOF draining, failed persistence, ownership, resume and reconciliation.

Native process tests use a child process and pipe readiness. No test uses a
wall-clock success window, polling or timed retries. Fixtures isolate home,
configuration, profile, temporary and runtime roots. The process test clears its
environment and uses its own temporary home.

## macOS results

- The initial `cargo check -p amplihack-cli --tests` passed before the final
  delivery and migration adapters were added; this is not final validation.
- `check-yaml` passed after correcting completion-step indentation.
- Final acceptance, formatting, Clippy and pre-commit results remain pending.
  macOS has held generated build executables in a launched-but-suspended state,
  before their empty entry point runs. Interrupted infrastructure runs must not
  be reported as passing checks or as behavioural failures.
- The preceding recipe step reported macOS Clippy errors in unrelated
  Linux-specific profile and governor code. A fresh full hook run is required;
  the earlier report does not establish this change's final hook result.

## External integration follow-up (step 8b)

Both GitHub mutation adapters now share a bounded subprocess boundary. Added
checks cover exact service receipt parsing, output limits, literal bodies,
deadline termination and reaping, inherited host/repository overrides, assignment
arguments, sensitive diagnostics and refusal to replay invalid responses.

Formatting of the changed Rust files and compilation of an isolated harness
containing the production issue parser and GitHub adapter passed. That harness
does not establish whole-crate compilation or integration correctness. Its native
test executable produced no test output on this host; no behavioural pass is
claimed. Whole-crate checks and tracking acceptance checks are queued in the
private detached tmux session `external-integration`, with socket and logs in
`/Users/ryan/src/factory-home/.amplihack/work/r/d9bd8d534a7d/tmp/issue-2553-integration.NefmMj`.

Required commands for the follow-up are:

```sh
cargo check --locked -p amplihack-cli --tests --examples
cargo test --locked -p amplihack-cli --lib commands::multitask::github::tests
cargo test --locked -p amplihack-cli --test cli_integration issue_2553_tracking_resolution
```

The previously pending Clippy, pre-commit and configured hooks remain required.

## Exact Linux omissions

This lane cannot run the following Linux-only checks. Leave them visible for CI
and candide:

- `commands::multitask::process_scope::native_identity_tests::issue_2553_linux_start_ticks_follow_child_completion`
- `issue_1976_orphaned_nested_run::a_nested_run_whose_parent_is_already_dead_never_starts_the_runner`
- `issue_1976_orphaned_nested_run::a_reused_parent_pid_is_treated_as_gone`
- `issue_1976_orphaned_nested_run::a_run_exports_its_pid_and_start_time_to_the_runner`
- `issue_1976_orphaned_nested_run::invalid_parent_values_warn_and_never_stop_the_run`

The existing Linux pidfd and FIFO-pool suites remain CI responsibilities. This
change adds no pidfd or FIFO implementation. Do not substitute source-text rule
scans for any of the behavioural acceptance checks above.

## Handover

The implementation PR must include this validation record and `Fixes #2553`.
Candide owns the subsequent PR drive and merge. No hook bypass, force-push,
version change or execution of the motivating administrative request is part of
this lane.
