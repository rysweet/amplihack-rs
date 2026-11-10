# Issue #1390 validation and proposed PR evidence

Status: incompletely verified. Production code and existing regression tests
are unchanged. No test-before-implementation claim is made.

Tested source: `266f57e9b41d063ff061f0ee11ccdf54ce022b89`, clean checkout
before this evidence document was added. Platform: Darwin 27.2.0, arm64.
Linux, Docker, and Podman execution were unavailable in this session.
There is no PR for the current branch (`gh pr view` reported none); this file
provides the validation text for its eventual PR body.

## Reproduced passes

All Cargo commands below use `CARGO_TARGET_DIR=/private/tmp/issue-1390-target`.

```sh
cargo test --locked -p amplihack-cli --lib commands::recipe::run::tests_failure_class -- --nocapture
git diff --check HEAD^ HEAD
```

The focused run passed 52 tests, zero failures, 6299 filtered out. The whitespace
check passed. Earlier reports of 52 passes were supplied evidence until this
rerun reproduced them. The earlier temporary CLI harness remains supplied
evidence only and does not establish that the registered regression passed.

## Hook results

```sh
PRE_COMMIT_HOME=/private/tmp/issue-1390-precommit \
CARGO_TARGET_DIR=/private/tmp/issue-1390-target pre-commit run --files \
  crates/amplihack-cli/src/commands/recipe/run/tests_failure_class.rs \
  crates/amplihack-cli/tests/issue_1390_usage_limit.rs \
  crates/amplihack-cli/tests/main.rs docs/reference/transient-failure-retry.md
```

Passed: check-merge-conflict, trailing-whitespace, end-of-file-fixer,
check-added-large-files, artifact-guard, check-git-command-sanitised, cargo-fmt.
Not applicable to these paths: check-yaml, check-toml, check-toolchain-refs,
check-brick-budget. Failed: cargo-clippy (`cargo clippy --locked --all-targets
-- -D warnings`), including existing macOS unused imports in
`amplihack-hooks-bin/tests/limit_resume_binary.rs` and 248 CLI library-test
warnings promoted to errors. No unrelated warning fixes were made.

## Blocked registered and workspace execution

```sh
cargo test --locked -p amplihack-cli --test cli_integration issue_1390_usage_limit::reported_limit_survives_the_cli_boundary_without_transport_retry -- --exact --nocapture
cargo test --workspace --locked --no-fail-fast -- --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu
```

Both commands exited 101 before test execution: E0432 in
`issue_1954_first_install_restager.rs:29` imports `proc_link`, which is defined
only on Linux. The writable target directory resolved the earlier filesystem
blocker, but cannot resolve this unrelated platform compilation blocker.

The exact pending issue regression is
`issue_1390_usage_limit::reported_limit_survives_the_cli_boundary_without_transport_retry`.
Its fixture and assertions cover CLI exit 1, parsed usage-limit classification,
reset hint, matching parsed stderr diagnostic, and exactly one runner invocation,
with existing temporary home/configuration/profile isolation. These assertions
have **not** been observed passing through the registered target in this session.

No tests were selectively removed or disabled. The workspace command's four
repository-prescribed skip values above are substring filters, not exact test
names; compilation failed before discovery, so no exhaustive per-test skipped
inventory is available. All workspace execution beyond the focused 52 tests is
blocked/unverified, not a pass. The specific issue test has no inherent Linux
dependency (it is Unix-gated); the consolidated binary currently requires Linux
because of the unrelated import above. The adjacent Linux-dependent test is
`issue_1954_first_install_restager::the_new_binary_restages_on_the_first_update_and_a_live_run_survives`.

Required follow-up: existing `.github/workflows/ci.yml` job `test` (display name
`Test`, `ubuntu-latest`) runs `cargo nextest run --workspace --locked` without
these skip filters and includes the registered regression. Job `check` (`Lint &
Format`, Ubuntu) must also pass. No CI change is necessary to enable coverage.
Record the Linux run URL, tested SHA, exact discovered omissions (if any), and
the registered test result in the PR before declaring verification complete.
The exact Cargo regression command above should also be run on Linux.

## Prescribed outside-in bootstrap attempt

This Rust repository does support `uvx`: `pyproject.toml` packages the shell
bootstrap `uvx/amplihack`. The prescription therefore applies.

```sh
UV_TOOL_DIR=/private/tmp/issue-1390-uv-tools \
UV_TOOL_BIN_DIR=/private/tmp/issue-1390-uv-bin \
UV_CACHE_DIR=/private/tmp/issue-1390-uv-cache \
CARGO_TARGET_DIR=/private/tmp/issue-1390-target \
uvx --from git+https://github.com/rysweet/amplihack-rs@feat/issue-1390-a-usage-limit-kills amplihack --version
```

Failed, exit 127: bootstrap line 99 uses `readarray`, unavailable in the default
macOS Bash. Package installation succeeded but resolved remote commit
`def287b8222c7a1df014e9397cf961d6536d8185`, not the local tested checkpoint.
An initial attempt with only UV_CACHE_DIR also failed to write the default uv
tools directory; the writable tool directories above resolved that restriction.
This smoke attempt does not validate the usage-limit workflow. The registered
CLI regression is the outside-in behavioral check still required on Linux;
it invokes the real CLI with a synthetic runner without consuming quota.
