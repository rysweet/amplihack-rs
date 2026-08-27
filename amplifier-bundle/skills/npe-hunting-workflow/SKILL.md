---
name: npe-hunting-workflow
description: Investigates and remediates NullPointerException (NPE) and null-dereference failures from a real stack trace. Use when tracing an NPE, finding structurally similar null bugs, filtering false positives, deciding whether TLA+ applies, or coordinating validated fixes.
metadata:
  version: "1.0"
  author: amplihack
---

# NPE Hunting Workflow

## Purpose

Turns one observed null failure into an evidence-driven search for related bugs.
It separates candidate generation, skeptical falsification, validation, formal
analysis, and remediation so plausible static matches are never treated as
confirmed defects.

## Required Outcomes

- Anchor the investigation to an observed stack trace and repository revision.
- Explain the complete lifecycle that can produce the null value.
- Record every candidate, including rejected and inconclusive candidates.
- Invoke `crusty-old-engineer` to falsify every candidate before validation.
- Assess formal methods for every surviving lifecycle or interleaving defect.
- Add a characterization test before changing production behavior.
- Fix only validated bugs through `default-workflow`.
- Run parallel fixes only when file and interface ownership is disjoint.

## Workflow

### 1. Initialize evidence tracking

Create a process journal and candidate ledger before searching. Use the formats
in [reference.md](reference.md). Record commands, hypotheses, failed approaches,
and decisions, not just successful findings.

Candidate states are:

`provisional -> skeptical-review -> reproduce -> validated -> fixed`

Terminal states are `rejected-false-positive`, `rejected-wrong-bug-class`, and
`blocked-insufficient-evidence`.

### 2. Anchor the seed failure

Start from a real stack trace or reproduce one before broad scanning.

If the trace text is absent, stop and obtain or reproduce it; never synthesize a trace.
If the failing revision is unknown, record the current checkout and mark line
mappings as conditional until history or build metadata identifies it.

1. Pin the source revision and map every stack frame to that revision.
2. Identify the exact null object. Do not infer field ownership from the JVM
   variable name.
3. Trace backward to the null producer and forward to the dereference.
4. Separate the immediate null path from the lifecycle transition that made the
   value absent.
5. State confirmed facts and hypotheses separately.

Do not widen the search until the immediate seed path is source-confirmed.

### 3. Trace the nullable lifecycle

For the seed object, enumerate:

- creation and initialization
- attachment or registration
- lookup and caching
- replacement and generation changes
- detachment, release, and clearing
- callbacks, animations, listeners, executors, and render traversals
- guards at scheduling time and guards at use time

Express the seed as a structural signature, for example:

`lifecycle-owned nullable resource + stale readiness check + deferred use + unguarded dereference`

### 4. Generate candidates structurally

Search for the signature, not for every nullable value. Partition large
codebases by subsystem so scans do not duplicate work.

Each provisional candidate must identify:

1. a concrete null producer
2. a reachable consumer path
3. the exact unguarded dereference
4. the lifecycle transition between producer and consumer
5. existing guards and tests
6. a concrete false-positive condition

Independently re-derive these facts from production source. Do not accept a
candidate writeup, scanner result, or test-double path as evidence by itself.

Stop broad searching when new regions yield only duplicate signatures or
rejected patterns. More matches are not progress by themselves.

### 5. Falsify every candidate

Invoke `crusty-old-engineer` with the complete ledger, or deterministic batches
when the ledger is too large. Require one verdict per candidate:

- `VALIDATED`: observed trace or characterization test proves the failure.
- `RETAIN_FOR_REPRODUCTION`: source-reachable, but runtime ordering is unproven.
- `REJECT_FALSE_POSITIVE`: a contract, guard, or unreachable transition blocks it.

The review must challenge the null producer, reachability, actual exception
type, short-circuit behavior, lazy recreation, call-order invariants, and
production teardown paths. Record the evidence behind every verdict.

No candidate may enter remediation directly from static analysis.

### 6. Validate and assess formal methods

Write the smallest targeted characterization test for each retained candidate.
The test must exercise the production method rather than a fixture override that
bypasses the suspect path.

Invoke `tla-plus-expert` when all of these are true:

- correctness depends on lifecycle state or operation ordering
- the relevant state and transitions can be finitely abstracted
- a safety invariant can distinguish the current design from a proposed fix
- testing would sample interleavings rather than cover them

For TLA+ work, require:

1. a counterexample for the current design
2. a check of the tempting minimal fix, such as a null check alone
3. a passing model for the proposed fix invariant
4. explicit abstraction, bound, atomicity, liveness, and memory-model limits

Model replacement as a new resource identity and in-place mutation as the same
identity. State which callbacks or operations are atomic in the model.

Skip formal modeling for a direct sequential missing guard when a focused test
proves the bug more clearly. Record that decision in the ledger.

### 7. Characterize before fixing

A bug is eligible for a fix only when:

- skeptical review retained it
- a trace, focused failing test, or formal counterexample validates it
- a characterization test captures current behavior
- the intended safe behavior is explicit

Preserve the characterization test, then change it or add a paired regression
test that asserts the corrected behavior.

### 8. Partition and fix

Create an ownership matrix for validated bugs. Group candidates by files and
tightly coupled interfaces.

- Launch parallel `default-workflow` workstreams only for disjoint groups.
- Run overlapping groups sequentially.
- Give each workstream its characterization tests, evidence, formal invariants,
  owned files, and excluded files.
- Integrate only after targeted tests and independent review pass.

Do not weaken formal invariants during implementation. A null check is not a
complete fix when acquisition and cleanup must use the same resource identity.

### 9. Close the investigation

Update every ledger entry with its final disposition, validation evidence,
formal-method decision, fix workstream, and test result. Record workflow
failures and fallbacks in the journal; infrastructure failure is process
evidence too.

## Supporting Material

- [reference.md](reference.md): ledger, journal, evidence gates, and formal checklist
- [examples.md](examples.md): representative activation and decision examples
