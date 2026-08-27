# NPE Hunting Reference

## Candidate Ledger

Use one row per dereference site. Shared lifecycle defects may link several rows.

| Field | Required content |
| --- | --- |
| ID | Stable identifier |
| Region | Subsystem or module |
| File and symbol | Exact source location |
| Null producer | Operation that can return or assign null |
| Consumer path | Entry point through dereference |
| Lifecycle transition | Initialization, replacement, teardown, or callback ordering |
| Guards | Checks and the exact property each establishes |
| Tests | Existing coverage and bypassing fixtures |
| False-positive case | Concrete condition that makes the path safe or unreachable |
| Skeptical verdict | Validated, reproduce, or reject with evidence |
| Formal decision | Required, useful, or inapplicable with reason |
| Validation | Trace, failing test, or model counterexample |
| Ownership | Files and interfaces assigned to a fix workstream |
| Final disposition | Fixed, rejected, or blocked |

## Process Journal Entry

Record each meaningful step:

```text
Context:
Evidence examined:
Hypothesis:
Probe or command:
Result:
Decision:
Next step:
Reusable heuristic:
Failure mode or dead end:
Verification:
```

The journal is chronological. The ledger is state-oriented. Keep both.

## Lifecycle Trace Checklist

For each nullable resource, answer:

1. Who creates it?
2. Can creation legitimately return null?
3. Who stores or discovers it?
4. What establishes readiness?
5. Can it be replaced while work is queued?
6. Who detaches, clears, releases, or invalidates it?
7. Which lists, listeners, queues, caches, or parents retain reachability?
8. Which thread or callback performs the dereference?
9. Is the guard checking the same value and generation that is later used?
10. Does cleanup use the identical resource acquired during setup?

## Evidence Gates

### Provisional candidate

All must be present:

- concrete null producer
- complete source-level path
- exact dereference
- plausible lifecycle transition
- falsifiable safe condition

### Retained after skeptical review

All must be present:

- `crusty-old-engineer` verdict
- contracts and guards challenged
- actual failure class identified
- smallest decisive reproduction described

### Validated bug

At least one:

- observed production stack trace at the site
- focused characterization test reproducing the failure
- executable formal counterexample for a modelable lifecycle violation

Source plausibility alone is not validation.

### Fix eligible

All must be present:

- validated bug
- characterization test in the affected repository
- intended behavior stated
- formal applicability decision recorded
- owned files identified

## False-Positive Filters

Reject or reclassify candidates when evidence shows:

- a lazy getter recreates the object instead of returning null
- short-circuit evaluation prevents the dereference
- the value is stale but non-null, producing a different bug class
- the null-producing teardown operation has no production caller
- release also unlinks the object before traversal can reach it
- a synchronous call-order invariant initializes the value first
- an earlier guard establishes the exact value and generation later used
- a test double bypasses the production path under investigation
- an assertion changes the exception only in assertion-enabled builds

Assertions do not count as production null guards. Adjacent readiness checks do
not establish the real precondition.

## Formal-Method Decision

### TLA+ is useful

Use it when a finite lifecycle model can expose ordering defects such as:

- schedule, detach, callback
- acquire on generation A, replace with B, cleanup
- register, release, retained listener
- stop, restart, queued work observes another generation

Model:

- lifecycle states and resource identity
- actions that add, remove, replace, schedule, and consume
- the current design
- the tempting minimal fix
- the proposed invariant-preserving fix

Typical safety invariants:

- no null dereference
- cleanup uses the resource acquired by setup
- closed or released state has no residual attachment
- queued work cannot observe another generation's resource

### TLA+ is inapplicable

Prefer a focused test when the defect is a direct sequential dereference with no
meaningful state machine. Formal syntax adds no confidence to `nullable lookup ->
unguarded call`.

### Proof reporting

Always report:

- TLC command and result
- counterexample trace
- state-space bound
- abstraction mapping to code
- atomicity assumptions
- safety versus liveness scope
- memory-model limitations

Do not call a bounded model check an unbounded proof.

## Parallel Fix Ownership

Before launching fixes, build this matrix:

| Workstream | Candidates | Owned files | Shared interfaces | Dependencies |
| --- | --- | --- | --- | --- |

Two workstreams are disjoint only if they do not edit the same files and do not
change a shared contract consumed by the other. If ownership is unclear, run
them sequentially.

Each `default-workflow` prompt must include:

- validated candidate IDs
- characterization tests
- intended behavior
- formal invariants and limits
- owned and excluded files
- targeted validation command

## Completion Checklist

- [ ] Seed stack trace and revision recorded
- [ ] Immediate null path confirmed
- [ ] Lifecycle transition explained or explicitly unresolved
- [ ] Structural signature documented
- [ ] Every candidate has a skeptical verdict
- [ ] Every retained candidate has a targeted reproduction
- [ ] Formal applicability recorded
- [ ] Every fixed bug has a characterization and regression test
- [ ] Parallel workstreams have disjoint ownership
- [ ] Final ledger and journal are complete
