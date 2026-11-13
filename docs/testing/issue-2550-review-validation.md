# Issue #2550 review validation

Recorded on 13 November 2026. **Required validation remains blocked.** The
composer regressions pass, but the required Clippy and workspace gates are red.
An earlier Mermaid probe differential did not reproduce on the final tree, but
its timing variation is not explained. These results do not establish delivery
readiness.

## Tested revisions and conditions

- Feature HEAD: `2668a592f73069673254c957ea0bdeefaf29b617`, with the review fixes.
- Full-comparison snapshot diff SHA-256:
  `325d659ff64600bf380e0e2c1dc84b65edaabe6d9c9bbaa4fcaa8ae712a1f19d`.
- Final implementation diff SHA-256:
  `ee25f2bebae38b6efd230746fb9b5c418305c414855d76d1b59640bb64397b9e`.
  These hash `git diff --binary` for the seven implementation, test, dependency
  and contract-document files, excluding this evidence note.
- Base: `4e0e6f8d40cedbcb5cd3253540cf6bf13695c53a`, the first feature commit's
  parent, checked out with `git worktree add --detach`.
- Rust `1.97.0 (2d8144b78 2026-07-07)`; Cargo
  `1.97.0 (c980f4866 2026-06-30)`; host `aarch64-apple-darwin`.
- Both full runs used the pinned repository configuration, locked dependencies,
  the same recipe TMPDIR and `RUST_TEST_THREADS=8`. The base had its own writable
  Cargo target directory, initialized with an APFS clone of the existing cache.
  Workspace path crates were rebuilt from the isolated base checkout.
- Full workspace runs executed sequentially on the shared host. The base tracked
  tree was clean after execution. Focused comparisons used the same workspace
  configuration and exact case filters on each revision.

Temporary probe diagnostics and an unsuccessful fixture experiment were reverted.
After the full runs, a benign-link regression showed that removing delimiters
could expose a word from a hidden URL as a closing keyword. The final refinement
checks raw source first and normalizes only parsed text views. Its source-imported
unit tests and all 50 affected tests passed again with the workspace configuration.
The entire workspace suite was not repeated after this refinement. Production
probe deadlines and the shared issue-keyword selector are unchanged.

## Passing affected checks

| Check | Result |
| --- | --- |
| Workflow unit tests | 19 passed |
| Issue-link integration tests | 19 passed |
| Readiness integration tests | 12 passed |
| The same affected tests inside the full-comparison snapshot | All 50 passed |
| Final workspace-configured focused rerun, including the benign URL regression | All 50 passed |
| Formatting, whitespace, YAML/TOML, toolchain references, recipe budgets and Git subprocess checks | Passed |
| Artifact guard, repeated against the settled files | Passed |
| Default-workflow skill mirror comparison | Passed |

The readiness tests cover both demonstrated inline bypasses, full/collapsed/
shortcut reference links, definitions in the published prefix, benign prose and
links, and preservation of existing published Markdown. The actual fetch-compose-
edit example invokes the Cargo-built composer. Rejected reports and invalid JSON
produce no composer body, submit no edit, preserve platform state and omit the
private marker from diagnostics.

## Required gate results

Clippy reported 27 library errors and 248 library-test errors on both revisions.
All 266 unique feature diagnostic message/location pairs were independently
reproduced among the base's 267 pairs. The additional base diagnostic was:

`bins/amplihack-hooks/tests/limit_resume_binary.rs:24:5`:
unused import `amplihack_hooks::limit_resume::RESUME_MSG`.

The original base Clippy command stopped at that independent hooks error.
A supplemental `--keep-going` run collected the remaining targets with the same
compiler, features and lint options. Baseline attribution does not satisfy the
failing required gate.

| Full-comparison workspace target | Feature failures | Base failures | Shared failing case names |
| --- | ---: | ---: | ---: |
| `amplihack --test integration` | 76 | 80 | 75 |
| `amplihack-cli --lib` | 101 | 97 | 97 |
| `amplihack-cli --test cli_integration` | 104 | 121 | 104 |
| `amplihack-remote --lib` | 1 | 1 | 1 |
| `amplihack-types --lib` | 3 | 3 | 3 |
| `amplihack-utils --lib` | 2 | 1 | 1 |
| Total | 287 | 303 | 281 |

Of the 281 shared failing cases, 278 also had the same first panic source location.
Three TUI cases failed at different assertions between runs: numeric tab hotkeys,
session search and force refresh. Their names alone do not establish an identical
failure mechanism. The base additionally failed 22 cases. Examples of reproduced
failures include the Bash syntax error in `workflow_hook_repair.sh:522` and
tracking-issue fixtures whose command lookup returned exit 127.

## Exact-case follow-up

Six cases failed only in the feature's full run. Paired checks ran those exact
names with eight workers, then one worker, without changing production deadlines.

| Case | Feature/base, eight workers | Feature/base, one worker |
| --- | --- | --- |
| TUI detail/escape | Fail / Fail | Fail / Fail |
| Mermaid already present | Fail / Fail | Fail / Pass |
| Mermaid install failure | Fail / Pass | Fail / Pass |
| Mermaid install/reprobe hook | Fail / Pass | Pass / Pass |
| Profile reread budget | Fail / Pass | Fail / Fail |
| Codex handshake deadline | Pass / Pass | Pass / Pass |

Exact identities:

- `no_python_probe::tc25_fleet_tui_enter_opens_detail_and_escape_returns_to_fleet_without_python`
- `commands::install::tests::mermaid_cli_tests::ensure_mermaid_cli_skips_when_mmdc_already_present`
- `commands::install::tests::mermaid_cli_tests::ensure_mermaid_cli_install_failure_does_not_propagate`
- `commands::install::tests::mermaid_cli_tests::ensure_mermaid_cli_with_hooks_every_child_through_install_and_reprobe`
- `commands::profile::rotate::tests_issue_1870::all_rereads_of_one_gate_share_one_time_limit`
- `codex_capacity::tests::the_deadline_covers_the_handshake`

A further single-case comparison still failed on the feature (0.59 seconds) and
passed on the base (0.33 seconds) for the Mermaid install-failure case. Temporary
instrumentation confirmed that its private fake `npm --version` probe exceeded
the existing 500 ms deadline. The cause of this revision differential has not
been established. It must not be described as a proven baseline failure.
The final tree's isolated recheck passed in 0.35 seconds with the same 500 ms
deadline. That passing recheck does not establish the cause of the earlier
variation or satisfy a passing full-workspace gate.
The profile test also uses a 300 ms budget with 200 ms simulated reads; the
observed timing variation is a limitation of these host results.

## Commands and retained evidence

Affected commands, run from the feature checkout:

```bash
cargo test --locked -p amplihack-cli --lib commands::workflow::
cargo test --locked -p amplihack --test integration workflow_issue_link::
cargo test --locked -p amplihack --test integration workflow_readiness_description::
pre-commit run --all-files
pre-commit run artifact-guard --all-files
```

The final affected-test rerun retained the workspace feature configuration:

```bash
scripts/git-hook-cargo.sh cargo test --workspace --locked --no-fail-fast -- \
  commands::workflow:: workflow_issue_link:: workflow_readiness_description:: \
  --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu
```

The workspace gate below ran from each checkout, with separate
`CARGO_TARGET_DIR` values and the same `RUST_TEST_THREADS=8`:

```bash
scripts/git-hook-cargo.sh cargo test --workspace --locked --no-fail-fast -- \
  --skip fleet_probe --skip kuzu --skip fleet::fleet_local --skip memory::kuzu
```

Base Clippy commands:

```bash
scripts/git-hook-cargo.sh cargo clippy --locked --all-targets -- -D warnings
scripts/git-hook-cargo.sh cargo clippy --locked --all-targets --keep-going -- -D warnings
```

Paired rechecks used that workspace command with the six exact names after
`-- --exact`. Serial checks additionally passed `--test-threads 1`. Single-case
checks used only the Mermaid install-failure name. The final all-file
pre-commit rerun passed every non-Clippy hook and again reported the 27 library
and 248 library-test errors. All 266 final diagnostic message/location pairs
were present in the independently executed base check.

The private evidence bundle is `${TMPDIR}/review-2550.rTaX3s`. It contains the
full case inventories, panic locations, diagnostic identities, command lines,
exit statuses and logs. The normalized `validation-inventory.json` SHA-256 is
`367340b31458adcef10c229c635e019795f9915f94f365f25e400fdd34308255`.
Final-tree logs use the `final-affected`, `final-precommit` and `final-mermaid`
prefixes. The earlier full-comparison and separate targeted-test results are
retained; earlier compile attempts are not the final results.

No Linux binaries, Linux Clippy/workspace run or CI run against this working
tree were executed in this step. Integration services were isolated fakes;
authenticated GitHub auto-closing behavior was not exercised. Linux CI against
the final tree and passing required gates are still needed before delivery
sign-off. The unexplained probe timing variation remains a validation limitation.
