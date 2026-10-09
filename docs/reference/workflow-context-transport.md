---
title: Workflow context transport
description: Source authority for verifier, completion, documentation and finalization consumers.
type: reference
updated: 2026-10-09
---

# Workflow context transport

The shared transport implements source authority for implementation, verification,
documentation and finalization consumers. Direct consumer controls cover the
producers and downstream collection, validation and completion. Actual native
file spilling and whole-finalizer acceptance remain integration gates; direct
consumer tests alone do not establish those gates.
The contract applies to Codex, Claude and Copilot; each consumer retains its
existing decision policy.

## Contents

- [Source authority](#source-authority)
- [Fields and private reader](#fields-and-private-reader)
- [Structured JSON](#structured-json)
- [Implementation evidence](#implementation-evidence)
- [TDD verdict enforcement](#tdd-verdict-enforcement)
- [Verification evidence producer](#verification-evidence-producer)
- [Finalization consumers](#finalization-consumers)
- [Documentation checkpoint](#documentation-checkpoint)
- [Consumer and file inventory](#consumer-and-file-inventory)
- [Transport acceptance requirements](#transport-acceptance-requirements)
- [Integration and diagnostics](#integration-and-diagnostics)

## Source authority

Select one source for every field in the consumer's cohort:

| Consumer | Cohort |
| --- | --- |
| Implementation helper and TDD enforcer | `verdict_json`, `implementation`, `allow_no_op` |
| Verification evidence producer | `precommit_results`, `local_testing_gate`, `allow_no_op` |
| Finalization collection and validation | `implementation_terminal_evidence`, `verification_terminal_evidence`, `allow_no_op`, plus the deterministic assessment records listed below |
| Reporting-status producer | `agentic_finalizer_narrative` |
| Workflow completion | `workflow_result`; associated reporting metadata uses the same selected transport |
| Documentation checkpoint | `doc_review_feedback` |

| Priority | Source | Selection rule |
| --- | --- | --- |
| 1 | `AMPLIHACK_CONTEXT_FILE` | If the variable is set, its file is authoritative for every cohort. |
| 2 | Relevant `RECIPE_VAR_<key>` roots or `RECIPE_VAR_<key>__...` namespaces | Any supplied member selects canonical environment input for that entire cohort. |
| 3 | Fixed uppercase legacy scalars | Used only when the cohort has no canonical input and no context-file selector. |

Presence includes a variable set to an empty value. A supplied empty pathname,
missing or unreadable file, symlink, nonregular file, malformed JSON or non-object
file root suppresses fallback. The file contains exactly one top-level context
object. An empty object or a missing selected field is insufficient data within
that authoritative source.

Canonical roots provide complete values. Nested-only verdict or feedback fields
establish canonical presence but cannot reconstruct a missing object. For
deterministic completion and finalization records, retain supported fixed
`RECIPE_VAR_<record>__<field>` reads when no record root is supplied. A supplied
root, including an empty or malformed root, takes precedence over its nested
aliases; missing, false or negative root fields cannot be patched from them.
Unrelated canonical metadata, such as a task description, does not suppress a
legacy-only cohort.

Selection and reads use a coherent snapshot per command. A canonical verdict
cannot combine with a stale legacy no-op flag or implementation sentinel.
Collection and validation use the same completion resolver and authority rules.
Parse errors, absent fields and negative values never authorize a lower-priority
lookup. No-op and completion fields cannot be assembled across sources.

Each consumer selects its own private cohort. An inherited
`WORKFLOW_CONTEXT_COHORT` cannot change the implementation helper or enforcer's
authority. Scalar capture retains trailing newlines and newline-only data.
The enforcer and finalization resolver keep exact token comparisons; the
implementation and verification producers retain their existing case-folding
no-op normalization. Verification requires the strict structured receipts
described below.

## Fields and private reader

The private `workflow_context_read KEY` interface is implemented in
`amplifier-bundle/tools/workflow_context.sh`. Its implemented fixed inventory is:

| Key | Canonical value | Legacy scalar |
| --- | --- | --- |
| `verdict_json` | Complete Object; one serialized String compatibility decode | `VERDICT_JSON` |
| `implementation` | String | `IMPLEMENTATION` |
| `allow_no_op` | Boolean or String | `ALLOW_NO_OP` |
| `doc_review_feedback` | Complete Object; one serialized String compatibility decode | `DOC_REVIEW_FEEDBACK` |
| `precommit_results`, `local_testing_gate` | Native Object receipts; serialized String compatibility | `PRECOMMIT_RESULTS`, `LOCAL_TESTING_GATE` |
| `agentic_finalizer_narrative` | String | `AGENTIC_FINALIZER_NARRATIVE` |
| `implementation_terminal_evidence`, `verification_terminal_evidence`, `finalizer_step_status`, `finalization_evidence`, `workflow_result` | Complete Objects; one serialized String compatibility decode | Corresponding uppercase names |
| `worktree_setup`, `terminal_state`, `pr_publish_result`, `publish_terminal_evidence` | Complete Objects; one serialized String compatibility decode | Corresponding uppercase names |
| `repo_path`, `branch_name`, `base_ref`, `remote_host_type`, `pr_url`, `task_description` | String | Corresponding uppercase names |
| `pr_number`, `issue_number` | Number or String | `PR_NUMBER`, `ISSUE_NUMBER` |
| `publish_state_reached` | Boolean or String | `PUBLISH_STATE_REACHED` |

Record field access stays limited to the existing consumer schema; arbitrary
paths and environment names are not an API. Deterministic records require
complete objects or supported fixed nested fields, with Boolean/String
completion values. Preserve String-valued helper outputs and validate field
types before applying the consumer's token policy. Current step-12 and step-13
receipt producers use `parse_json: true` and return native Objects. A serialized
String containing one JSON Object is compatibility input, not their native type.

| Exit | Stdout | Meaning |
| --- | --- | --- |
| `0` | Raw String, lowercase Boolean token or compact object JSON | Selected field is usable; the caller applies its policy. |
| `2` | Empty | Field is absent in the selected source; authority remains unchanged. |
| `3` | Empty | Invalid key, transport, type, JSON or read; stderr carries a bounded fixed diagnostic. |

`false` is usable data. Empty String is usable for `implementation` and
`allow_no_op`, without authorizing completion. Null, arrays, numbers, booleans
and empty or malformed object text are invalid verdict/feedback values. `{}` is
a valid object with insufficient policy evidence.

The reader determines transport authority, without approving work, writing
files, altering inherited context or setting completion flags. Callers capture
exits `2` and `3` explicitly under `set -euo pipefail` and use conservative
defaults within the selected source.

## Structured JSON

Native `parse_json` outputs remain JSON objects, including evidence arrays and
unknown fields. A complete context object has this shape:

```json
{
  "verdict_json": {
    "verdict": "WORK_VERIFIED",
    "evidence": ["docs/howto/install-codex-plugin.md"],
    "rationale": "The installation guide describes native registration and explicit hook trust."
  },
  "implementation": "Updated the Codex installation guide.",
  "allow_no_op": false,
  "doc_review_feedback": {
    "status": "OK",
    "feedback": "The installation guide preserves user-controlled hook trust."
  }
}
```

This example specifies field shapes; the verifier supplies actual evidence for
each task. Its producer verdicts are `WORK_VERIFIED`, `HOLLOW_SUCCESS` or
`INSUFFICIENT_EVIDENCE`. Consumers preserve historical minimal-object and
synonym handling without adding an evidence-validation policy.

Canonical object parsing requires exactly one complete JSON document whose
value is an Object. One JSON-encoded compatibility String may decode to that object; recursive String
decoding, prose extraction and fenced JSON salvage are rejected. Compact output
must reparse equal to the input object, retaining arrays, nested values and
Unicode. Environment serialization lacks original scalar type tags; validation
uses each key's declared semantic type. Legacy scalar text retains each caller's
existing extraction rules.

The runner's existing serializer, spill and output budgets apply. Payloads are
not truncated, moved into command arguments or exported as global uppercase
object aliases. JSON-encoded String transport does not replace native object
outputs or file-only spill consumption.

## Implementation evidence

`workflow_implementation_evidence.sh` emits exactly four fields. Booleans remain
Strings in this response:

```json
{"implementation_completed":"true","terminal_no_op":"false","terminal_state":"IMPLEMENTATION_COMPLETED","terminal_reason":"step-08c work-verifier approved concrete implementation artifacts"}
```

Decision order is selected `allow_no_op`, selected implementation sentinel, then
selected verdict. No-op tokens are case-insensitive `true`, `1`, `yes` and `y`,
without whitespace trimming. The sentinel is the exact case-sensitive substring
`No files modified — orchestration task`, including the em dash.

| Decision | `implementation_completed` | `terminal_no_op` | `terminal_state` |
| --- | --- | --- | --- |
| Explicit no-op or exact sentinel | `"false"` | `"true"` | `ALLOW_NO_OP` |
| Exact positive verdict | `"true"` | `"false"` | `IMPLEMENTATION_COMPLETED` |
| Other, absent or invalid verdict | `"false"` | `"false"` | `IMPLEMENTATION_UNPROVEN` |

Positive tokens are exactly `WORK_VERIFIED`, `VERIFIED`, `SUCCESS`, `APPROVED`,
`PASS` and `PASSED`, case-sensitive. Legacy extraction selects the last matching
one-line verdict object; canonical objects are validated and compacted first.
All decision branches exit zero. Output-tool failures remain process errors.
Strict finalization still rejects insufficient implementation evidence.

The fixed no-op reasons are `allow_no_op was explicitly selected for a
non-code-change path` and `implementation output contained the explicit
orchestration no-op sentinel`. Unproven evidence uses `step-08c did not produce
WORK_VERIFIED implementation evidence`.

## TDD verdict enforcement

`workflow-tdd.yaml:step-08c-enforce-verdict` keeps its Bash type, resume condition
and `implementation_noop_guard` output. It checks the exact selected sentinel
before the selected no-op value, which must be literal `true`. Its no-op policy
differs from the evidence helper's boolish policy.

The existing `orch helper extract-json`, `extract-field --field verdict` and
`normalise-verdict` pipeline normalizes verdict tokens by trimming and uppercasing.

| Normalized verdict | Tokens | Result |
| --- | --- | --- |
| `WORK_VERIFIED` | `WORK_VERIFIED`, `VERIFIED`, `SUCCESS`, `APPROVED`, `PASS`, `PASSED` | Exit `0`; APPROVED diagnostic. |
| `HOLLOW_SUCCESS` | `HOLLOW`, `HOLLOW_SUCCESS`, `FAILED`, `FAIL`, `NO_WORK`, `NO_ARTIFACTS`, `EMPTY` | Exit `1`; rejection diagnostic. |
| `INSUFFICIENT_EVIDENCE` | Missing, malformed, unknown or other values | Exit `0`; warning, without approval. |
| Explicit no-op or exact sentinel | Existing opt-out rules | Exit `0`; opt-out diagnostic, without implemented-code evidence. |

`UNVERIFIED` and `NOT_APPROVED` remain insufficient. A zero process exit alone
does not establish approval or completed implementation.

## Verification evidence producer

`workflow-precommit-test.yaml:verification-terminal-evidence` is an inline Bash
producer. It must read selected `precommit_results`, `local_testing_gate` and
`allow_no_op`, including file-only inputs. Reading uppercase output aliases
alone cannot prove that canonical verification inputs reached this producer.

The native `step-12-run-precommit` and `step-13-local-testing` producers use
`parse_json: true` and return structured Object receipts. Selected serialized
String receipts remain supported for compatibility, including uppercase legacy
inputs. Both transports require exactly one complete JSON Object; malformed
input, embedded NUL, multiple documents (including failure followed by PASS),
non-object values and recursively encoded Strings cannot establish verification.

`precommit_results` requires `status: "PASS"`, numeric zero `exit_code`,
`precommit_exit_code` and `workspace_exit_code`,
`workspace_command: "cargo test --workspace --locked"`,
`validation_scope: "FULL_WORKSPACE"`, `test_threads: "DEFAULT"`, and a nonempty
`evidence` array referencing retained raw logs and actual exits. This means real
precommit execution and full locked workspace validation with default parallel
threads; subset tests or declared commands alone cannot supply that evidence.

`local_testing_gate` requires `status: "PASS"`, numeric zero `exit_code`, a
nonempty `evidence` array, and at least two actual outside-in `scenarios`, each
with `status: "PASS"` and numeric zero `exit_code`. Receipt evidence must bind the
executed validation to its current source and tools. Prose, nonempty reports,
failed runs and earlier source epochs cannot establish verification.

The separate explicit no-op path remains first: case-insensitive `true`, `1`,
`yes` and `y`. Normalization removes trailing LF through Bash command
substitution; other surrounding whitespace remains significant. The verification
producer emits a native
`parse_json` Object with four String fields:

| Path | Completed | No-op | State |
| --- | --- | --- | --- |
| Selected explicit no-op | `"false"` | `"true"` | `ALLOW_NO_OP` |
| Both selected receipts satisfy their strict schemas | `"true"` | `"false"` | `VERIFICATION_COMPLETED` |
| Either receipt absent, invalid or failing | `"false"` | `"false"` | `VERIFICATION_UNPROVEN` |

Here “Completed” means `verification_completed`, “No-op” means `terminal_no_op`;
the remaining fields are `terminal_state` and `terminal_reason`. The successful
reason is `current full workspace, pre-commit and outside-in structured receipts
report successful validation`; the unproven reason is `successful full workspace,
pre-commit or outside-in structured evidence is missing or invalid`. These
classification branches exit zero; output-tool failures remain process errors.
Controlled positive fixtures test the classifier and do not establish actual
workstream validation. Test the selected inputs, produced object and finalization
consumers together without inventing completion flags.

## Finalization consumers

The shared authority rule is implemented in all three modes of
`workflow_agentic_finalization.sh` and the inline producers in
`workflow-finalize.yaml`. Collection, validation and completion use the existing
context, metadata and mode-specific helper modules.

### Completion and no-op inputs

`collect` and `validate` must resolve the following fields identically from one
selected source. Existing legacy-only aliases remain supported in their current
order when canonical authority is absent:

| Typed context field | Legacy-only aliases, in precedence order |
| --- | --- |
| `implementation_terminal_evidence.implementation_completed` | `IMPLEMENTATION_COMPLETED`, `IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED` |
| `verification_terminal_evidence.verification_completed` | `VERIFICATION_COMPLETED`, `VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED` |
| `implementation_terminal_evidence.terminal_no_op` | `TERMINAL_NO_OP`, `IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP` |
| `allow_no_op` | `ALLOW_NO_OP` |

File-only complete records and canonical nested-only deterministic fields must
retain both positive and negative values. For example,
`RECIPE_VAR_implementation_terminal_evidence__implementation_completed=false`
cannot be overridden by stale `IMPLEMENTATION_COMPLETED=true`. An authoritative
file positive also cannot be suppressed by stale legacy negatives. The same
rule applies to explicit no-op; a stale true opt-out must not bypass a selected
false or absent opt-out.

Preserve the finalizer's existing boolish policy: exact lowercase `true`, `1`,
`yes` and `y` are true; other String values are false, without trimming. JSON
Booleans become their lowercase tokens. The verification producer's broader
case handling does not change this policy. Verification no-op output alone
cannot substitute for implementation no-op evidence plus `allow_no_op`.

### Collection, validation and completion

| Mode / recipe step | Transport responsibility | Preserved policy |
| --- | --- | --- |
| `collect` / `collect-finalization-evidence` | Read typed completion records, selected no-op and existing deterministic metadata; emit `finalization_evidence.completion` with String booleans. | Collection exit `0` means evidence was collected, not that finalization succeeded. |
| Inline `finalizer-step-status` | Read selected `agentic_finalizer_narrative`; emit native `{status, reporting_failure}`. | Narrative presence determines reporting availability; its prose never determines implementation success. |
| `validate` / `validate-agentic-finalization` | Read selected `finalization_evidence`, `finalizer_step_status` and completion records with the same resolver as collection. | Apply hard blockers before success; malformed evidence remains a failure. |
| `complete` / `workflow-complete` | Read the selected validated `workflow_result`; carry its fields into `workflow_completion`. | Report the validated result; never reclassify or promote missing/failed evidence to success. |

The fixed deterministic record inventory also includes `terminal_state`,
`pr_publish_result` and `publish_terminal_evidence`, and the existing repo,
worktree, branch/base and remote metadata used by collection. Their canonical
roots and supported nested fields follow the selected source, so stale aliases
cannot overwrite dirty-worktree, tooling, HOLLOW, prior-terminal, PR or reporting
signals. Preserve Git/tool probes and publish/PR ownership boundaries.

Before delimiter extraction, validation checks all five selected evidence fields:
`git.dirty_worktree`, `tooling.missing`, `tooling.gh_required`,
`prior_terminal_state.terminal_state` and `agent_outputs.hollow_success_signals`.
Strings and Booleans are supported; absent/null fields remain conservative.
Wrong field/container types, malformed selected transport and embedded NUL in any
extracted String produce `FAILED_INVALID_EVIDENCE`, `terminal_success="false"`,
`terminal_failure="true"`, `finalizer_output_valid="false"` and exit `1`. Invalid
field-reader results cannot be discarded or repaired from stale aliases;
positive completion and explicit no-op cannot override them. Newlines and other
non-NUL control characters remain data and cannot hide later blocker fields.

Validation preserves `FAILED_INVALID_EVIDENCE`, `FAILED_DIRTY_WORKTREE`,
`FAILED_MISSING_TOOLING`, blocked terminal states and `HOLLOW_SUCCESS` before
completion checks. Reporting failure remains `FAILED_REPORTING` when durable
implementation and verification are complete, otherwise `FAILED_IMPLEMENTATION`.
Only both completion fields true authorize `IMPLEMENTED_VERIFIED`; explicit
`allow_no_op` plus implementation `terminal_no_op` authorize `ALLOW_NO_OP`.
These paths remain subject to every existing hard blocker.

Validation emits one complete JSON Object with exactly these 19 String fields:
`terminal_success`, `terminal_state`, `terminal_reason`, `required_next_action`,
`hollow_success_detected`, `evidence_used`, `finalizer_schema_version`,
`finalizer_confidence`, `finalizer_output_valid`, `reporting_failure`,
`implementation_completed`, `verification_completed`, `publish_state_reached`,
`terminal_no_op`, `terminal_failure`, `pr_url`, `pr_number`, `observed_phases` and
`missing_evidence`. Boolean-like values and PR numbers remain Strings. It streams
scalar metadata and diagnostics through `jq --rawfile` and validates the assembled object, so large
file-backed PR values or tooling diagnostics never re-enter process arguments.
Values are preserved without truncation or a new size cap. Success exits `0`;
validation failures exit `1`. Missing jq retains the fixed failure response;
collection/completion tooling failures and invalid modes retain exit `2`.

Completion preserves `terminal_success`, `terminal_state`, `terminal_reason`,
`required_next_action`, `hollow_success_detected`, `evidence_used`,
`finalizer_schema_version`, `finalizer_confidence`, `finalizer_output_valid`,
`reporting_failure`, `terminal_failure` and `pr_url` from `workflow_result`.
Missing or invalid selected results remain conservative, never repaired from a
stale `WORKFLOW_RESULT_*` success alias. Legacy-only result aliases retain their
supported behavior. A successful formatting process is not terminal success.

### Strict exits and tooling

Transport changes preserve existing failure behavior. Validation emits its
structured failing result and exits `1` for non-success classifications,
including malformed evidence. Missing finalizer helpers at recipe call sites,
missing `jq` for collection/completion or `finalizer-step-status`, and invalid
helper modes retain error exit `2`. Validation's missing-`jq` path retains its
structured `FAILED_MISSING_TOOLING` result and exit `1`.

The reader's absent-field exit `2` and invalid-input exit `3` are separate from
these command exits. Consumers handle them explicitly, retain selected source
authority and use conservative evidence or structured failure. No error path
may inject completion flags, scrape narrative for approval, or relax strict
validation to make file transport appear successful.

## Documentation checkpoint

`workflow-design.yaml:step-06b-checkpoint-doc-review` reads its separate
documentation cohort. It extracts `status`, uppercases it and removes whitespace;
only normalized `OK` passes. Missing, invalid or negative canonical feedback
cannot revive a stale legacy `DOC_REVIEW_FEEDBACK` containing `OK`.

The checkpoint emits `DOC_REVIEW_CHECKPOINT: OK` or
`DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION`, with fixed reasons and warnings. It
retains structural exit `0`, nonfatal review/refinement and the existing durable
reference allowlist. Feedback remains data and is never evaluated or printed as
arbitrary checkpoint instructions. See the
[checkpoint reference](doc-review-non-fatal-checkpoint.md).

## Consumer and file inventory

These are the implementation boundaries:

| File | Required boundary |
| --- | --- |
| `amplifier-bundle/tools/workflow_context.sh` | Shared source selection, fixed keys/record fields, coherent snapshot and strict type/error handling. |
| `amplifier-bundle/tools/workflow_implementation_evidence.sh` | Selected verdict/no-op/sentinel to native implementation evidence. |
| `amplifier-bundle/recipes/workflow-tdd.yaml` | Actual enforcer and implementation evidence producer; preserve metadata and policies. |
| `amplifier-bundle/recipes/workflow-precommit-test.yaml` | Inline verification producer, its validation inputs and native output. |
| `amplifier-bundle/tools/workflow_agentic_finalization.sh` | Mode dispatch, dependency loading and strict command preconditions. |
| `amplifier-bundle/tools/workflow_finalization_context.sh`, `workflow_finalization_metadata.sh` | Shared fixed-field completion and metadata resolvers with selected source authority. |
| `amplifier-bundle/tools/workflow_finalization_collect.sh`, `workflow_finalization_validate.sh`, `workflow_finalization_complete.sh` | Deterministic collection, strict classification and complete result transport. |
| `amplifier-bundle/recipes/workflow-finalize.yaml` | Collect/validate/complete call sites and inline reporting-status producer. |
| `amplifier-bundle/recipes/default-workflow.yaml` | Completion record defaults and propagation across recipe boundaries. |
| `amplifier-bundle/recipes/workflow-design.yaml` | Separate nonfatal documentation feedback cohort. |

Consumer implementation is owned by `crates/amplihack-cli/Cargo.toml`. The
`issue_1538_context_transport` executable integration target is registered in
`bins/amplihack/Cargo.toml`; retain it and the existing verifier, reliability and
strict-finalization targets. Test the actual shell helper and rendered YAML commands, with separate verification and
finalization scenarios; parser-only tests cannot establish this contract.

## Transport acceptance requirements

Require direct semantic RED on unchanged consumers and GREEN on repaired
consumers for each changed boundary, plus native runner controls for typed
`parse_json` objects and file-only spill. RED means the expected behavior failed
in an executed consumer; tool/build errors or zero selected tests are not RED.
Preservation controls must remain GREEN where behavior already matches.

| Inputs | Required observations |
| --- | --- |
| Positive implementation and verification | Actual producers emit typed evidence; collect preserves both true values, validate classifies `IMPLEMENTED_VERIFIED`, complete preserves that result. |
| Negative, missing, `HOLLOW_SUCCESS` and insufficient verdicts | No implemented-code approval; HOLLOW enforcement and strict finalization remain failing where required. |
| Explicit no-op and false/absent opt-outs | Genuine no-op survives transport; false or absent selected fields cannot borrow stale true values. |
| Malformed, null, wrong-type, empty or unreadable selected input | No lower-tier fallback; conservative producer output or structured validation failure, with preserved exits. |
| File positives against canonical/legacy negatives; file/canonical negatives against legacy positives | Selected authority wins in both directions, including no-op, reporting and completion result fields. |
| Root records conflicting with nested aliases; missing fields and nested-only deterministic records | Root authority wins; supported nested-only records retain behavior without inventing verdict/feedback objects. |
| Legacy-only Claude/Copilot | Existing scalar token, sentinel, no-op, output schema and failure behavior remain supported. |
| Dirty worktree, missing tooling, blocked CI, malformed evidence and reporting failure | Existing strict blockers, reporting split and error-`2` preconditions remain intact. |

Bind real commands, selected counts, child exits, semantic results and
source/tool/fixture identities to each direct/native run. Exercise the inline
verification producer's output through the actual collector, validator and
completion reporter, rather than supplying caller-created true flags as proof.
A standalone collection result or hardcoded `observed_phases` list does not
establish a whole finalizer, actual phase execution or finding closure.
Whole-finalizer transport claims require verified native producer-to-completion
coverage of the matrix above.

## Integration and diagnostics

The evidence helper resolves the reader beside its own `BASH_SOURCE`. Recipe
consumers resolve libraries from their quoted owning repository/worktree paths.
An unavailable library is visible and conservative. No Python or Node runtime
dependency is required.

When evidence is insufficient, inspect the selected context source and the
consumer's diagnostics. Preserve the genuine verifier object and fix its
transport or missing fields. Setting completion flags, reviving stale aliases
or weakening [strict finalization](default-workflow-agentic-finalization.md)
does not establish verified work.

See [Codex usage](../howto/install-codex-plugin.md),
[implementation evidence](workflow-implementation-evidence.md) and
[runner validation](recipe-runner-validation.md).

Canonical shell consumers locate framework helpers through `AMPLIHACK_HOME`,
then the target `REPO_PATH`, cwd and private provider assets. An explicit
`AMPLIHACK_HOME` is authoritative even when its helper is missing. Target
repositories need not contain `amplifier-bundle`; paths remain quoted shell
values, including spaces, quotes and metacharacters. Documentation checkpoints
remain nonfatal and report `NEEDS_ATTENTION` when helpers are unavailable.
