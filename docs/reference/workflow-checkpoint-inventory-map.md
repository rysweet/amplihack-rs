---
title: Workflow checkpoint inventory map
type: reference
issues: [1554, 1914]
decisions: [17.126, 17.138, 17.146]
---

# Workflow checkpoint inventory map

`crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml`
records how every behaviour of the Python checkpoint helpers is kept in Rust.
Each obligation in cc's reference inventory maps to one Rust test that runs in
the ordinary workspace test, or to one owner-approved disposition. While the
port is in progress, an obligation whose Python file is still tracked may
instead be **pending** with that file (decision 17.146-2). Two test files
check the map: the validator in `workflow_checkpoint_inventory.rs` and the
honesty guards in `workflow_checkpoint_map_honesty.rs`.

The reference IDs and assertion counts come only from the pinned
inventory, never from Python sources. The 19 Python files are removed one per
commit, in the [port order](#port-order). Every commit on the way is green:
the guards check that the tracked `.py` files are exactly the test files that
still have pending rows, plus the helpers those files still use. PR #2020 merges
only when no row is pending and no `.py` file is tracked.

To port the next file, follow
[Port a checkpoint Python test file to Rust](../howto/port-a-checkpoint-python-test-file.md).

## Contents

- [Files](#files)
- [Obligations](#obligations)
- [Map schema](#map-schema)
- [Pending rows](#pending-rows)
- [Port order](#port-order)
- [Test target forms](#test-target-forms)
- [Dispositions](#dispositions)
- [The role rows and the source-level removal control](#the-role-rows-and-the-source-level-removal-control)
- [What the validator refuses](#what-the-validator-refuses)
- [The tracked-Python guards](#the-tracked-python-guards)
- [What the honesty guards refuse](#what-the-honesty-guards-refuse)
- [The Rust phase projection](#the-rust-phase-projection)
- [The Rust Git fixture](#the-rust-git-fixture)
- [Running the checks](#running-the-checks)
- [Changing the map](#changing-the-map)

## Files

All paths are under `crates/amplihack-cli/tests/fixtures/workflow_checkpoint/`.

| File | Contents | Edited by hand |
| --- | --- | --- |
| `inventory.json.part-1`, `inventory.json.part-2` | cc's reference inventory, split at a line boundary to stay under the 500 KB file limit. Their concatenation has SHA-256 `0c3e19b0c72c92a3bd2a140acedcf03f311cb11e9b2c336abf8d3797cc6c788f`. | never |
| `inventory_ids.json` | the IDs, summary counts and source digest derived from the pinned parts | never |
| `inventory_map.toml` | one row per obligation, the eight dispositions and the provenance row. It was first written with every row pending, by the `jq` command recorded in the commit that created it | yes |
| `corpus/<action>.toml` | byte-exact cases for each `workflow-checkpoint` action; a case a map row targets names that row in `inventory_ref` | `inventory_ref` only |

## Obligations

The map has 445 coverage rows:

| Table | Rows | Reference ID form | Example ID |
| --- | --- | --- | --- |
| `[[python_test]]` | 190 | `<file>::<Class>.<method>` | `amplifier-bundle/recipes/tests/test-canonical-staging-harness.py::HarnessControls.test_all_original_prompt_scalars_preserve_fields` |
| `[[procedural]]` | 207 | `<file>::<function>@L<line>` | `amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py::restored_auxiliary_command@L47` |
| `[[role]]` | 41 | `<asset>::<step_id>` | `workflow-prep::step-02-clarify-requirements` |
| `[[site]]` | 7 | `<asset>::<step_id>` | `workflow-finalize::step-20b-push-cleanup` |

The 41 role rows are the 40 authorised roles plus
`workflow-terminal-adjudication::terminal-state-adjudicator`. That role is
not authorised. Its row targets a test that asserts it is refused.

Reference IDs keep the `.py` paths of the removed files because they name
cc's reference, not files in this repository.

## Map schema

### `[meta]`

| Key | Value |
| --- | --- |
| `inventory_source_sha256` | the pinned digest, `0c3e19b0…6c788f` |

The map holds no summary counts. The counts come from `inventory_ids.json`.

### Coverage rows

Every row in `[[python_test]]`, `[[procedural]]`, `[[role]]` and `[[site]]`
may hold only these keys:

| Key | Required | Meaning |
| --- | --- | --- |
| `ref_id` | yes | an ID from `inventory_ids.json`, exactly once per table |
| `rust_test` | one of three | the Rust test that keeps the behaviour (see [Test target forms](#test-target-forms)) |
| `pending_file` | one of three | the tracked `.py` file whose port will map this row (see [Pending rows](#pending-rows)) |
| `disposition` | one of three, never in this map | accepted by the validator's grammar, but no coverage row can use it (see below) |
| `assertions_python` | `python_test` rows with `rust_test` | the length of the method's `assertions` array in the pinned parts |
| `assertions_rust` | `python_test` rows with `rust_test` | the checks the target body makes; at least `assertions_python`, at most the count in the body |
| `note` | when required | why one target covers many rows, or how a loop or `subTest` assertion maps |

Each row holds exactly one of `rust_test`, `pending_file` and `disposition`.
A row with two of them is refused with `MultipleColumns`, and a row with none
with `NeitherColumn`.

A note is required on every row whose target is named by more than eight
rows, and on rows for Python loop or `subTest` assertions.

A coverage row may cite a disposition only if that disposition's entry in
`APPROVED_ROWS` lists the row's `ref_id` in `covers`. `covers` exists only
in `APPROVED_ROWS` in `workflow_checkpoint_inventory.rs`, not in the map, and
it is empty for all eight rows, because the owner has approved no omission of
a reference obligation. Any coverage row with `disposition` is therefore
refused with `UnrecordedDisposition`. An ID is added to `covers` only with
the owner's written record.

A `[[python_test]]` row. The reference ID and the count of 4 are real and
come from the pinned parts. The target is the name the port convention gives
in the module for the harness's `test_` methods (see [Port order](#port-order));
the function does not exist yet:

```toml
# [PLANNED] rust_test names the port of this method; validate() reports
# NoSuchTest until the function exists. Before the map is committed, this
# example must match a row in inventory_map.toml whose target runs.
[[python_test]]
ref_id = "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py::HarnessControls.test_all_original_prompt_scalars_preserve_fields"
rust_test = "crates/amplihack-cli/tests/checkpoint_port_canonical_staging_harness.rs::all_original_prompt_scalars_preserve_fields"
assertions_python = 4
assertions_rust = 4
```

A target is never in `canonical_staging_runner_test.rs`: its runner tests
are `#[ignore]`d, and the validator refuses an ignored target with
`IgnoredTarget`.

Ported tests keep the Python method name without its `test_` prefix. A
renamed Rust test is allowed. The real `cargo test` run proves that it
executes.

### `[[disposition]]`

| Key | Meaning |
| --- | --- |
| `id` | one of the eight approved row IDs |
| `decision` | `17.126` or `17.138` |
| `doc_section` | a heading in [`workflow-checkpoint-cli.md`](workflow-checkpoint-cli.md) |
| `pinning_test` | the test that pins the behaviour, as `<repo path>::<fn>`; looked up by that exact key, so a bare function name is refused with `NoSuchTest` |

### `[[provenance]]`

One row records where the inventory came from: `sha256` is the pinned digest,
and the description says the rows were extracted from the pinned parts with
`jq` and that the assertion counts come from each definition's `assertions`
array. A provenance row has no `ref_id` and never counts as coverage. Audit
receipts, if any, go in `[[audit_receipt]]` under the same rule.

## Pending rows

A pending row names the Python file that still holds the behaviour. It is a
statement that the port is not done. It is not a disposition, and it never
counts as coverage:

```toml
[[python_test]]
ref_id = "amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py::ConservativeBoundaryTests.test_same_bytes_replacement_record_cannot_replace_file_identity"
pending_file = "amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py"
```

Rows are written by `jq` from `inventory_ids.json`, never typed by hand.

`pending_file` must satisfy all of these:

- It is a repository-relative, normalised UTF-8 path: not absolute, with no
  `..` or `.` segment, no backslash and no NUL. It is compared byte for byte
  with `git ls-files -z` output and never canonicalised.
- It is tracked at the commit under test (else `PendingFileNotTracked`).
- It is the file the row's `ref_id` names (else `PendingFileMismatch`). The
  file part of a `ref_id` is everything before the first `::`, or before the
  last `@L<digits>`, and must match `^[A-Za-z0-9_./-]+\.py$`. A `ref_id` that
  does not parse is refused.

The two table kinds whose `ref_id` names no `.py` file use fixed rules:

| Table | Pending with | Mapped when |
| --- | --- | --- |
| `[[role]]` | `amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py` | the [source-level removal control](#the-role-rows-and-the-source-level-removal-control) exists; from then on a pending role row is refused |
| `[[site]]` `autodrive-crusty-round::step-04-address-concerns`, `workflow-tdd::checkpoint-after-implementation`, `workflow-refactor-review::checkpoint-after-review-feedback`, `workflow-publish::step-16b-outside-in-fix-loop` | `amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py` | that file is ported |
| `[[site]]` `workflow-publish::step-15-commit-push`, `workflow-pr-review::step-18c-push-feedback-changes`, `workflow-finalize::step-20b-push-cleanup` | `amplifier-bundle/recipes/tests/test-checkpoint-scope.py` | that file is ported |

Each site is pending with the last file in the [port order](#port-order) that
exercises it; the files are found with `git grep -l -F -e <site id>`. This
mapping is a design decision, not data from the inventory. The validator
holds it as a constant table, so it cannot drift. A site row may be mapped
before its file is ported only if a Rust test that is not `#[ignore]`d covers
the whole site.

A Python file is **accounted for** when it is in the
[expected tracked set](#the-expected-tracked-set). A tracked `.py` file that
is not accounted for is refused with `UnaccountedPython`. A helper's own
pending rows do not account for the helper: an unused helper is refused even
while rows are pending with it.

### Helpers

Five tracked files hold shared Python code rather than only tests.
`HELPERS` in `workflow_checkpoint_inventory.rs` pins exactly these five:

| Helper | Python users | Rows of its own |
| --- | --- | --- |
| `crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py` | `test-checkpoint-scope.py` | none |
| `crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py` | `test-checkpoint-scope.py` | procedural |
| `amplifier-bundle/recipes/tests/canonical_staging_publication.py` | `test-pragmatic-checkpoints.py`, `test-pragmatic-evidence.py`, `test-minimalist-auto-stage.py`, `test-checkpoint-publication-boundaries.py`, `test-checkpoint-scope.py`, and the helper `test-canonical-staging-harness.py` | none |
| `amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py` | `test-checkpoint-review-boundaries.py`, `test-pragmatic-auto-stage.py`, `test-minimalist-auto-stage.py`, `test-checkpoint-scope.py` | procedural |
| `amplifier-bundle/recipes/tests/test-canonical-staging-harness.py` | `test-checkpoint-scope.py` | `test_` methods and procedural |

A helper's users are found with `git grep -l -F -e <helper stem>` and checked
in both directions: every file the table lists as a user names the helper,
and every tracked `.py` file that names the helper is in the table. Every
helper's last user, directly or through the harness, is
`test-checkpoint-scope.py`, so all five are deleted in commit 14 of the
[port order](#port-order).

A helper's own rows are pending with the helper until that commit, which
ports them into the modules named in the port order. The same commit updates
the Rust tests that read the helpers and removes the helpers' entries from
`coverage.json` and `maintenance_paths.json`. Those tests are:

- `crates/amplihack-cli/tests/workflow_checkpoint_static_guards.rs`
- `crates/amplihack-cli/tests/workflow_checkpoint_legacy_guards.rs`
- `crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs`
- `crates/amplihack-workflows/tests/canonical_staging_portability_test.rs`
- `crates/amplihack-workflows/tests/support/canonical_staging.rs`

### The expected tracked set

`expected_tracked_python()` is the single rule every guard uses:

```text
tests    = { pending_file of every pending row } \ HELPERS
expected = tests ∪ { helper | helper is used by a file in expected }
```

The second line is applied until nothing changes, so helper chains are
followed: while `test-canonical-staging-harness.py` is expected, so is
`canonical_staging_publication.py`, which it uses. A pending row whose
`pending_file` is a helper adds nothing to the set. A helper is expected
only while some expected file uses it; once its last user is gone it is
refused with `UnaccountedPython`, which forces its deletion even though its
own rows are still pending.

The result must be a subset of `PINNED_19`, the 19 `.py` paths tracked when
the map was created. A new `.py` file is therefore refused even if a row
names it.

## Port order

Decision 17.146 ports one Python file per commit, smallest first. Each
commit ports the file's behaviour, negative cases and mutant sensitivity,
replaces its pending rows with mapped rows, deletes the file with `git rm`,
and is green under `cargo test --workspace --locked` before it is pushed.

| # | File under `amplifier-bundle/recipes/tests/` | Lines | Rust module under `crates/amplihack-cli/tests/` |
| --- | --- | --- | --- |
| 1 | `test-checkpoint-conservative-boundaries.py` | 72 | `checkpoint_port_conservative_boundaries.rs` |
| 2 | `test-pragmatic-crusty-directories.py` | 102 | `checkpoint_port_pragmatic_crusty_directories.rs` |
| 3 | `test-checkpoint-rebase-identity.py` | 106 | `checkpoint_port_rebase_identity.rs` |
| 4 | `test-checkpoint-value-transport.py` | 117 | `checkpoint_port_value_transport.rs` |
| 5 | `test-checkpoint-scope-history.py` | 161 | `checkpoint_port_scope_history.rs` |
| 6 | `test-checkpoint-refusal-evidence.py` | 174 | `checkpoint_port_refusal_evidence.rs` |
| 7 | `test-checkpoint-synthesis-alias.py` | 208 | `checkpoint_port_synthesis_alias.rs` |
| 8 | `test-checkpoint-review-boundaries.py` | 283 | `checkpoint_port_review_boundaries.rs` |
| 9 | `test-pragmatic-checkpoints.py` | 356 | `checkpoint_port_pragmatic_checkpoints.rs` |
| 10 | `test-pragmatic-auto-stage.py` | 395 | `checkpoint_port_pragmatic_auto_stage.rs` |
| 11 | `test-pragmatic-evidence.py` | 457 | `checkpoint_port_pragmatic_evidence.rs` |
| 12 | `test-minimalist-auto-stage.py` | 543 | `checkpoint_port_minimalist_auto_stage.rs` |
| 13 | `test-checkpoint-publication-boundaries.py` | 1593 | `checkpoint_port_publication_boundaries.rs` |
| 14 | `test-checkpoint-scope.py` | 2017 | `checkpoint_port_scope.rs` |

Commit 14 also deletes the five [helpers](#helpers) and ports their own rows:

| Helper rows | Rust module under `crates/amplihack-cli/tests/` |
| --- | --- |
| `test-canonical-staging-harness.py` (`test_` methods and procedural rows) | `checkpoint_port_canonical_staging_harness.rs` |
| `canonical_staging_checkpoint_audit.py` (procedural rows) | `checkpoint_port_checkpoint_audit.rs` |
| `audit_receipts.py` (procedural rows) | `checkpoint_port_audit_receipts.rs` |

`private_adapter.py` and `canonical_staging_publication.py` have no rows of
their own. The files with no `test_` methods (the pragmatic and minimalist
files) have procedural rows only; their procedural assertions become
`#[test]` functions or corpus cases.

If a port shows that the Rust engine behaves differently from 918c, that
file is not committed. The input, 918c's output and the port's output go to
the owner, the engine is not changed, and the port continues with the next
file only if that file does not depend on the open case.

## Test target forms

| Form | Example | Resolves when |
| --- | --- | --- |
| `<repo path>::<fn>` | `bins/amplihack/tests/workflow_checkpoint_exit79.rs::site_1_reports_a_terminal_79_from_the_destination_read_as_79` | the file defines that `#[test]` function |
| `<repo path>::corpus_<action>#<case>` | `crates/amplihack-cli/tests/workflow_checkpoint_corpus.rs::corpus_admission_refused_result#unset` | the corpus runner exists, the case is in `corpus/<action>.toml`, and the case's `inventory_ref` equals this row's `ref_id` |

The corpus link works in both directions. Each map row that targets a corpus
case is named by that case's `inventory_ref`, and each `inventory_ref` names a
row that targets its case:

```toml
[[case]]
name = "unset"
inventory_ref = "<ref_id of the row that targets corpus_admission_refused_result#unset>"
argv = []
setup = "none"
expect_status = 0
```

Every target must be compiled by a Cargo test target. It must not be
`#[ignore]`d, gated by `cfg` or an environment variable, or contain a
`return;` before its last assertion. It must not read a `.py` file or name
`python3`.

## Dispositions

The map has exactly eight `[[disposition]]` rows. They match the allowlist in
the validator and the tables in
[Authorised changes (17.126)](workflow-checkpoint-cli.md#authorised-changes-17126)
and [Decision 17.138](workflow-checkpoint-cli.md#decision-17138).

| `id` | `decision` | `doc_section` | `pinning_test` in the map |
| --- | --- | --- | --- |
| `17.138-1` | 17.138 | Decision 17.138 | `crates/amplihack-cli/src/workflow_checkpoint/spec_tests.rs::surrogateescape_audit_matches_decision_17_138_1` |
| `17.138-2` | 17.138 | Decision 17.138 | `bins/amplihack/tests/workflow_checkpoint_restorations.rs::owner_17_138_2_after_refuses_nd_digits_and_values_outside_i64` |
| `17.138-3` | 17.138 | Decision 17.138 | `bins/amplihack/tests/workflow_checkpoint_restorations.rs::owner_17_138_3_a_top_level_lone_surrogate_worktree_path_exits_1` |
| `17.138-4` | 17.138 | Decision 17.138 | `crates/amplihack-cli/src/workflow_checkpoint/spec_tests.rs::records_refuse_a_field_containing_nul` |
| `17.138-6` | 17.138 | Decision 17.138 | `crates/amplihack-cli/src/workflow_checkpoint/legacy_parity_tests.rs::a_panic_maps_to_status_1_like_an_uncaught_python_exception` |
| `17.126-dest-read-79-site-1` | 17.126 | Authorised changes (17.126) | `bins/amplihack/tests/workflow_checkpoint_exit79.rs::site_1_reports_a_terminal_79_from_the_destination_read_as_79` |
| `17.126-dest-read-79-site-2` | 17.126 | Authorised changes (17.126) | `bins/amplihack/tests/workflow_checkpoint_exit79.rs::site_2_reports_a_terminal_79_from_the_destination_read_as_79` |
| `17.126-stage-strict-79` | 17.126 | Authorised changes (17.126) | `bins/amplihack/tests/workflow_checkpoint_p1_selectors.rs::stage_strict_passes_a_real_git_79_through` |

`validate()` looks `pinning_test` up by that exact `<repo path>::<fn>` key in
its index of `#[test]` functions. A bare function name finds nothing and is
refused with `NoSuchTest`.

The allowlist itself is `APPROVED_ROWS` in `workflow_checkpoint_inventory.rs`.
Its `pin_test` field names the same test in module form, and must resolve to
exactly one runnable `#[test]` (else `NoSuchTest` or `AmbiguousPinTest`):

| Module form | Resolves to |
| --- | --- |
| `amplihack_cli::<a>::<b>::<fn>` | `<fn>` in `crates/amplihack-cli/src/<a>/<b>.rs` or `<a>/<b>/mod.rs` |
| `<stem>::<fn>` | `<fn>` in a file `<dir>/tests/<stem>.rs` |

| `id` | `APPROVED_ROWS.pin_test` |
| --- | --- |
| `17.138-1` | `amplihack_cli::workflow_checkpoint::spec_tests::surrogateescape_audit_matches_decision_17_138_1` |
| `17.138-2` | `workflow_checkpoint_restorations::owner_17_138_2_after_refuses_nd_digits_and_values_outside_i64` |
| `17.138-3` | `workflow_checkpoint_restorations::owner_17_138_3_a_top_level_lone_surrogate_worktree_path_exits_1` |
| `17.138-4` | `amplihack_cli::workflow_checkpoint::spec_tests::records_refuse_a_field_containing_nul` |
| `17.138-6` | `amplihack_cli::workflow_checkpoint::legacy_parity_tests::a_panic_maps_to_status_1_like_an_uncaught_python_exception` |
| `17.126-dest-read-79-site-1` | `workflow_checkpoint_exit79::site_1_reports_a_terminal_79_from_the_destination_read_as_79` |
| `17.126-dest-read-79-site-2` | `workflow_checkpoint_exit79::site_2_reports_a_terminal_79_from_the_destination_read_as_79` |
| `17.126-stage-strict-79` | `workflow_checkpoint_p1_selectors::stage_strict_passes_a_real_git_79_through` |

Row 17.138-5 is not a disposition: the leading-dash behaviour was restored,
so its obligations map to ordinary tests. A disposition ID that is not on
the allowlist is refused, including `pending` or `unresolved-*` placeholders.

An obligation the port cannot keep is not given a made-up disposition. It is
reported to the owner and listed under
[Unresolved differences for owner disposition](workflow-checkpoint-cli.md#unresolved-differences-for-owner-disposition),
and the map is not committed until the owner decides.

## The role rows and the source-level removal control

Decision 17.146-1 lets the 41 role rows rest on the source-policy tests in
`crates/amplihack-workflows/tests/canonical_staging_policy_test.rs`, but only
if a removal control runs in ordinary CI:

| Rows | `rust_test` |
| --- | --- |
| the 40 authorised roles | `crates/amplihack-workflows/tests/canonical_staging_policy_test.rs::every_authorised_agent_explicitly_defers_staging` |
| `workflow-terminal-adjudication::terminal-state-adjudicator` | `crates/amplihack-workflows/tests/canonical_staging_policy_test.rs::nested_adjudicator_is_a_visible_scope_exclusion` |

The control is
`removing_any_authorised_opt_out_is_named_by_the_policy_check`, a plain
`#[test]` with no `#[ignore]`. It reads "native removal control" as a
source-level control that needs only the recipe files and a temporary
directory. The runner-backed removal runs (the 25 native runs in
`canonical_staging_runner_test.rs`) stay `#[ignore]`d with their reason and
remain outstanding; this control does not replace them. If the owner rejects
this reading, the role rows return to pending and nothing else changes.

The policy check is the function `policy_failures(&RecipeSet) -> Vec<String>`,
which `every_authorised_agent_explicitly_defers_staging` also calls. The
control checks:

1. **Negative control.** The unmodified recipes give an empty result.
2. **Each authorised role.** For each of the 40 roles, the recipe bytes are
   copied into a `TempDir`, that step's `auto_stage: false` line is removed as
   text, and the recipes are parsed again. The result must be exactly
   `["<asset> / <step_id>: omitted or enabled auto_stage"]`. The control also
   asserts that the edit removed exactly one line, inside the target step,
   and that every other step parses the same as before.
3. **Adjudicator mutant.** Removing the adjudicator's opt-out leaves the
   result empty, and the adjudicator stays in the scope-exclusion list.
4. **Sensitivity.** A check that ignores one step leaves exactly one case
   undetected, so the control would notice a weakened check.

The older `checkpoint_delta_adds_four_separately_counted_opt_out_controls`
covers four steps; its closing comment points to this control and keeps its
note that the native runs are outstanding.

The validator refuses a pending role row once this control exists in its
index of test functions. Its index covers
`crates/amplihack-workflows/tests/` as well as the `amplihack-cli` and
`amplihack` test files, and a meta-test proves that a workflows test resolves.

## What the validator refuses

`validate()` in `crates/amplihack-cli/tests/workflow_checkpoint_inventory.rs`
returns one violation per problem:

| Violation | Cause |
| --- | --- |
| `WrongSha` | `meta.inventory_source_sha256` is not the pinned digest |
| `Missing`, `Invented`, `Duplicate` | a reference ID is absent, unknown, or listed twice |
| `MultipleColumns`, `NeitherColumn` | a row has more than one of `rust_test`, `pending_file` and `disposition`, or none |
| `PendingFileNotTracked` | `pending_file` is not tracked at this commit, or is not a normalised repository path |
| `PendingFileMismatch` | `pending_file` is not the file the row's `ref_id` names, or not the fixed file for a role or site row |
| `PendingRoleAfterControl` | a role row is pending although the source-level removal control exists |
| `UnaccountedPython` | a tracked `.py` file is not in the [expected tracked set](#the-expected-tracked-set): a test file with no pending row, a helper no expected file uses (even if rows are pending with it), or a file not in `PINNED_19` |
| `NoSuchTest`, `IgnoredTarget`, `CfgGated`, `EnvGatedEarlyReturn` | the target does not resolve, or does not run normally |
| `NoSuchCorpusCase`, `CorpusRefMismatch` | the corpus case is missing, or its `inventory_ref` names another row |
| `AssertionCount` | `assertions_rust` is missing or below `assertions_python` |
| `UnknownDisposition`, `UnrecordedDisposition` | the disposition is not one of the eight, or does not cover this obligation |
| `MissingDocSection` | `doc_section` is not one of the four allowed headings, or the heading is missing from the CLI reference |
| `SharedPinTest`, `AmbiguousPinTest` | two dispositions share a pin, or a pin matches more than one test |
| `ProvenanceCounted` | a provenance or audit-receipt row has a `ref_id` |
| `Cardinality` | the IDs are not 190 Python tests, 207 procedural rows, 41 roles (40 authorised) and 7 sites |

`pending_file` is never treated as a disposition. `UnrecordedDisposition`,
the meta-test that refuses invented disposition IDs, and the eight
`APPROVED_ROWS` are the same with or without pending rows. The validator
reads the tracked files with `git ls-files -z` through
`amplihack_git::command_in`, and a meta-test refuses a `pending_file` that
names an untracked file.

## What the honesty guards refuse

`crates/amplihack-cli/tests/workflow_checkpoint_map_honesty.rs` adds checks
that `validate()` does not make:

- `assertions_python` must equal the length of the definition's `assertions`
  array in the pinned parts;
- `assertions_rust` must not exceed the checks the target body makes (for a
  corpus case: status, stdout, stderr and each expected file);
- a target named by more than eight rows needs a `note` on every one of them;
- a coverage row may hold only the seven keys listed in [Coverage rows](#coverage-rows);
- the corpus runner must define `corpus_<action>` for every corpus file;
- every target is in a compiled test file, has no early `return;` and reads
  no `.py` file;
- `crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs`
  declares the `checkpoint_projection` module and calls
  `checkpoint_projection::project(`.

The scanners are structural. They do not prove that a test runs. The proof is
the real test run: every distinct `rust_test` must appear in
`cargo test -- --list` and be reported `ok`. See
[Matching a target to the test list](#matching-a-target-to-the-test-list).

The honesty guards and the owner-control checks in
`workflow_checkpoint_legacy_guards.rs` run on a partial map. An owner-control
row may be mapped, or pending with its own file. A Python method counts as
covered by `mapping_table_covers_every_python_method` when its row is mapped
or pending.

While no row names a target, the guards that need mapped rows check the
pending state instead: every coverage row is pending, the two destination-read
79 tests stay pinned by their 17.126 dispositions, the corpus and the map
name each other zero times, and the 41st role is pending only while
`REMOVAL_CONTROL` does not exist.

## The tracked-Python guards

These guards check the state of each commit, not only the final one. None of
them is `#[ignore]`d, and none returns early.

| Guard | Asserts |
| --- | --- |
| `no_python_file_is_tracked` (`pr2020_python_removal.rs`), `no_v23_python_file_is_tracked` (`workflow_checkpoint_static_guards.rs`) | the tracked `.py` set from `git ls-files -z` equals `expected_tracked_python()`; with no pending rows both are empty |
| `check_no_python_assets_script_passes` (`pr2020_python_removal.rs`), `check_no_python_assets_script_is_byte_identical_to_base_and_passes` (`workflow_checkpoint_static_guards.rs`) | the script is byte-identical to the base at every commit. It then runs with a cleared environment plus `PATH=/usr/bin:/bin` and `LC_ALL=C`. With no pending rows it exits 0 with empty stderr. Otherwise it exits non-zero and names exactly the expected set, which is also compared with `git ls-files`, so a tracked file missing on disk cannot hide |
| `the_fixture_installs_no_python_adapter` (`crates/amplihack-workflows/tests/canonical_staging_portability_test.rs`) | no adapter is installed unless `private_adapter.py` is in the expected set; a mutant check proves the guard fails when an adapter appears |
| `HELPERS` and `PINNED_19` meta-tests | `HELPERS.len() == 5`; the expected set is a subset of `PINNED_19` |
| `no_test_spawns_python3_or_includes_a_py_fixture` (`workflow_checkpoint_static_guards.rs`) | no checkpoint test spawns `python3`; an `include_str!` or `include_bytes!` of a `.py` file, and any literal naming one of the `PINNED_19` files, must name a file in the expected set. The inventory and guard modules that hold these paths as data may never include one |

So `scripts/check-no-python-assets.sh` passes only on the commit that deletes
the last `.py` file. Until then its failure is expected, and the guard checks
that it fails for exactly the right files.

## The Rust phase projection

`tests/integration/checkpoint_projection.rs` is the Rust replacement for the
Python `PhaseProjection`. Both test crates include it with `#[path]`:

```rust
#[path = "../../../tests/integration/checkpoint_projection.rs"]
mod checkpoint_projection;

let result = checkpoint_projection::project(&request, &env);
```

| Item | Purpose |
| --- | --- |
| `project(req, env) -> ShellResult` | runs one checkpoint phase from the source recipe against a fixture |
| `project_1671(…)` | the same projection for the issue #1671 bridge |
| `projected_json_output(raw) -> Result<Ctx, String>` | parses a phase's JSON output |
| `ProjectionRequest::branch_key` | the `worktree_setup` key that names the task branch: `local_branch` for `Owner::project`, `branch_name` for the bounded checkpoint steps, as `test-pragmatic-checkpoints.py` read it |

The module uses only the standard library and `serde_json`, so neither test
crate needs a `regex` or `base64` dependency:

- `{{name}}` templates, `  - id: "…"` block lines and `    key: value`
  lines are read by line and character scanners. Trailing whitespace is
  trimmed by exactly Python's `str.isspace` set, and an end of line matches
  only before `\n`. A key containing a newline is matched literally, as the
  escaped pattern matched it.
- Path bytes are encoded and decoded with a standard-alphabet base64 codec in
  the module. The decoder accepts and refuses the same inputs as the
  `base64` crate's `GeneralPurpose` engine configured as the projection used
  it, with the same messages: bad padding and symbols outside the standard
  alphabet are refused, and non-canonical trailing bits are accepted, as
  CPython's `b64decode(validate=True)` accepts them.
- Malformed input returns an error. Nothing in the module panics on it.

`crates/amplihack-cli/tests/checkpoint_projection_parity.rs` proves that the
scanners and the codec behave the same as the code they replaced. It compiles
the original pattern literals with `Regex::new` and runs the `base64` crate
as the reference, then compares results and refusals on adversarial inputs:
an unclosed `{`, nested and tripled braces, trailing tabs, `\x0b` and `\x0c`,
multi-byte UTF-8, CRLF, a last line without a line feed and a key containing
a newline. The template renderer, the block reader and the decoder are also
compared on every short input built from a small set of symbols or tokens.
The guard in `checkpoint_projection_unit.rs` requires the parity file to hold
the four original pattern literals and the whitespace class, to run each
parity check as a plain `#[test]`, and to be registered in `tests/main.rs`.
It still pins the SHA-256 of the retained pre-delta assets.

`project_renders_branch_name_for_bounded_step` in
`checkpoint_projection_unit.rs` runs `project` on a one-step bounded recipe
in an ordinary test, without the authorised binary the `#[ignore]`d
end-to-end tests need.

### Step-wise projection

`PhaseProjection` in the same module ports the Python class of that name for
tests that drive one checkpoint phase at a time:

| Method | Runs |
| --- | --- |
| `capture_scope(…)` | the phase's scope-capture step |
| `verdict(…)` | the phase's verdict step |
| `finish(…)` | the phase's finishing step |

Each method builds a `ProjectionRequest` and calls `project()`. The phases
come from a Rust `PHASES` table copied from the pinned values. Entries are
added when a ported file first needs them.

## The Rust Git fixture

`tests/integration/checkpoint_git_fixture.rs` ports the Python `GitFixture`.
`crates/amplihack-cli/tests/main.rs` mounts it with `#[path]`; it is not a
separate `[[test]]` target.

| Item | Provides |
| --- | --- |
| `GitFixture::new()` | in a test-owned `TempDir` (mode 0700): the caller repository, a linked worktree on `task/checkpoint`, a bare remote with `origin/HEAD`, the `private/`, `bin/`, `home/` and `hooks/` directories with `core.hooksPath` set, and the four base files |
| `env()` | the only environment a child gets, built from an empty allowlist: `PATH=<fixture>/bin:/usr/bin:/bin`, `HOME`, `TMPDIR`, `LC_ALL=C`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=<fixture file>`, `GIT_TERMINAL_PROMPT=0`, `GIT_ALLOW_PROTOCOL=file` |
| `git(args)` | `amplihack_git::command_in(root)` with `env()` applied |
| `stage(paths)` | runs `bash --noprofile --norc -c '. "$1"; stage_workflow_changes --strict --path-list -'` with the wrapper path as `$1`, `BASH_ENV` and `ENV` unset, the paths on stdin, then `wait_with_output` |
| `snapshot()` | a `Snapshot` (`PartialEq`, `Debug`) with separate index, HEAD and refs, and worktree fields; the worktree walk uses `lstat` and never follows symlinks |
| `hook(name, body)` | writes `#!/bin/sh` hooks under `<fixture>/hooks` with mode 0755 |
| stub actors in `bin/` | `amplihack` (run through `isolated(..)`), `gh` (records its argv only, never the environment or stdin), and `cargo`, `ssh`, `curl`, `docker`, `kubectl`, `claude`, `copilot`, `python`, `python3` and `pip`, which exit 97 |

The fixture has no timers. The Python `timeout=25` is not ported: tests wait
for exit statuses. Helpers such as `remote_reject`, `history`, `own` and
`sync_context` are added with the first ported file that needs them.

Its self-test sets hostile `BASH_ENV`, `ENV`, `GIT_CONFIG_PARAMETERS`,
`GIT_DIR` and `GIT_SSH_COMMAND` values that would write a marker file, and
asserts that the marker never appears and that
`git config --show-origin --list` shows only fixture files.

A grep guard refuses `inherited_command`, `bash -c` scripts built by string
formatting, and `python3` in `checkpoint_git_fixture.rs` and every
`checkpoint_port_*.rs` file.

## Running the checks

Run them from the feature worktree, with `RUST_TEST_THREADS=8` exported.

Validator and honesty guards:

```sh
export RUST_TEST_THREADS=8
cargo test --locked -p amplihack-cli --test cli_integration workflow_checkpoint_inventory
cargo test --locked -p amplihack-cli --test cli_integration workflow_checkpoint_map_honesty
```

Projection parity:

```sh
cargo test --locked -p amplihack-cli --test cli_integration checkpoint_projection_parity
```

Proof that every target runs:

```sh
cargo test --workspace --locked -- --list > list.txt 2>&1
cargo test --workspace --locked > run.txt 2>&1
```

Every distinct `rust_test` in the map must appear in `list.txt` as
`<name>: test` and in `run.txt` as `test <name> ... ok`, under the
`Running` line of the right test binary. Cargo prints the `Running` lines to
stderr, hence `2>&1`. Keep both files outside the repository.

### Matching a target to the test list

The map names a test by file path; `cargo test` names it by module path
inside one test binary. Convert as follows:

| Map target | Binary (`Running` line) | Name in the list |
| --- | --- | --- |
| `crates/<crate>/src/<a>/<b>.rs::<fn>` (or `<a>/<b>/mod.rs`) | `src/lib.rs` of `<crate>` | `<a>::<b>::<fn>` |
| `crates/amplihack-cli/tests/<stem>.rs::<fn>` | `tests/main.rs` of `amplihack-cli` | `<stem>::<fn>` |
| `bins/amplihack/tests/<stem>.rs::<fn>` | `tests/main.rs` of `amplihack` | `<stem>::<fn>` |
| `crates/amplihack-workflows/tests/<stem>.rs::<fn>` | `tests/<stem>.rs` | `<fn>` |
| `<path>::corpus_<action>#<case>` | as for `<path>::corpus_<action>` | `<stem>::corpus_<action>` |

`amplihack-cli` and `amplihack` set `autotests = false` and compile their
test files into one binary through `#[path] mod <stem>;` in `tests/main.rs`,
so the module name is the file stem. `amplihack-workflows` keeps Cargo's
default, so each test file is its own binary and its tests have no prefix.
A test inside a nested `mod` in the file gains that module in its name. A
corpus case has no list entry of its own: the `corpus_<action>` runner runs
every case, so the runner's `ok` covers the case.

To see which files the port still has to do:

```sh
git ls-files '*.py'
grep -h '^pending_file' crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml | sort | uniq -c
```

Once the last file is ported, no Python is left:

```sh
git ls-files '*.py'                 # prints nothing
scripts/check-no-python-assets.sh   # exits 0
```

## Changing the map

1. Find the reference ID in `inventory_ids.json`. Never type an ID or count
   by hand: take `assertions_python` from the pinned parts with `jq`.
2. Read the target's body and set `assertions_rust` to what it checks. If it
   checks less than the Python method did, write a pin test first. Register a
   new file in `crates/amplihack-cli/tests/main.rs` with `#[path] mod`.
3. For a corpus target, add the matching `inventory_ref` to the case.
4. Run the validator, the honesty guards and the runs-normally proof.

If an obligation cannot be kept, stop and ask the owner. Do not add a
disposition, a placeholder row or an `#[ignore]`. A row may stay pending only
while its file is still tracked; pending is not a way to defer an obligation
past the deletion of its file.

## Related

- [Port a checkpoint Python test file to Rust](../howto/port-a-checkpoint-python-test-file.md) - the per-file procedure that turns pending rows into mapped rows

- [`amplihack internal workflow-checkpoint` reference](workflow-checkpoint-cli.md) - the actions these tests cover, and the 17.126 and 17.138 tables
- [Canonical staging fixtures](../../crates/amplihack-workflows/tests/fixtures/canonical_staging/README.md) - the fixtures, evidence directories and portable controls
- [Canonical staging test contract](../../amplifier-bundle/recipes/tests/canonical-staging.md) - scope and delivery requirements
