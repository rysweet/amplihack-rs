# OrchLedger — formal spec for nested-agent admission control

`OrchLedger.tla` models amplihack's admission control **across process boundaries**,
which is where the bug in issue #1326 lived. The ledger is a file; read and write are
separate steps, so time-of-check/time-of-use is expressible; any process may be
SIGKILLed between any two steps; and `flock` is released by the kernel on death.

Three design decisions are constants, so each can be shown *necessary* by ablation
rather than asserted:

| Config | shared ledger | flock | ceiling sealed at root | Expected TLC result |
|---|:--:|:--:|:--:|---|
| `A_today` | ✗ | ✓ | ✗ | `NodeBudget` violated |
| `B_proposed` | ✓ | ✓ | ✓ | **no error** |
| `C_no_lock` | ✓ | ✗ | ✓ | `NodeBudget` violated |
| `D_unsealed` | ✓ | ✓ | ✗ | `CeilingMonotone` violated |

`B_proposed` is the configuration this repository implements. The other three are
regression oracles: if a refactor makes `A`, `C`, or `D` start passing, the spec has
stopped describing the hazard and the ablation is no longer meaningful.

## Running the gate

    scripts/check-spec.sh

Requires `java` and `tla2tools.jar` (set `TLA2TOOLS_JAR`, or place it at
`~/tla2tools.jar`). The script asserts the expected outcome of **every** config,
so a spec that no longer distinguishes the designs fails the gate.

## Conformance

The spec is not decoration. `crates/amplihack-cli/tests/spec_conformance.rs` enforces
each named invariant against the real implementation, and a drift test fails if an
invariant is added to a `.cfg` without a corresponding test. See that file for the
traceability table.

## Operational caveats

Honest limits of what the model proves and what the implementation can promise.

**This is bounded model checking, not a general proof.** TLC is exhaustive at
4 processes / 3 nodes / depth 2 (the `B_proposed` config). For all *N* an inductive invariant in TLAPS is needed;
`CeilingMonotone` and `NodeBudget` both look inductive and that is the natural next step.

**Not a security boundary.** A same-uid process can edit the tree state directly. The
observed failure was a well-behaved agent routing around what looked like a tooling
fault, and the design makes the correct path obvious and the incorrect path ineffective.
It does not defeat an adversary and must not be described as if it does.

**`flock` over NFS.** The tree lock uses `fs4`, i.e. `flock(2)`. On NFS, `flock` has
historically been unreliable or local-only ([`flock(2)` NOTES](https://man7.org/linux/man-pages/man2/flock.2.html)).
`NodeBudget` rests entirely on that lock: on an NFS home you are living in the
`C_no_lock` ablation, which the model says admits far more than the budget. Keep
`$HOME/.amplihack` on local storage, or set `AMPLIHACK_SESSION_TREE_DIR` to a local path.

**`HOME` unset.** Falls back to `/tmp/amplihack-session-trees` — durable enough within a
boot, but shared. Containers should set `HOME` or `AMPLIHACK_SESSION_TREE_DIR` explicitly.

**`HOME` rewritten mid-tree** splits the tree exactly as `TMPDIR` did. The runner pins
`AMPLIHACK_SESSION_TREE_DIR` for descendants to make this hard, but that variable is then
itself an inheritable override. This is a correctness boundary, not a sandbox.

**Mixed-version fleets.** A build predating this fix resolves the store from `TMPDIR` and
will not share a tree with a fixed build; both then under-count. `TreeState.writer_version`
records the sealing build and logs a warning on mismatch. It warns rather than refuses,
because refusing would break rolling upgrades.

**Retention.** The store is durable, so it needs an owner: `amplihack session-tree gc
--older-than-days N` (add `--dry-run` first). The old `TMPDIR` location got free cleanup;
that free cleanup is precisely what made the cap meaningless, so it is not coming back.

## Two gates, two failure shapes

`scripts/check-spec.sh` and `scripts/check-proofs.sh` answer different questions, and
the two real defects in this area were one of each kind.

**The spec gate (TLA+/TLC) covers ordering.** What happens when processes race, crash
mid-operation, or interleave badly. It caught the lost update (`C_no_lock`) and the
ceiling escalation (`D_unsealed`). It cannot say whether a single function computes the
right answer, because the model is a separate artifact from the Rust — which is exactly
how `B_proposed` stayed green over an implementation that had no sealed ceiling at all.

**The proof gate (Kani) covers logic.** It takes each decision function and asks a
solver whether *any* input breaks the stated claim. A pass holds for every input, not
for the inputs someone thought to test. It cannot reason about concurrency.

| | proves | blind to |
|---|---|---|
| `check-spec.sh` | orderings, races, crash points | one function's arithmetic |
| `check-proofs.sh` | every input to a decision | concurrency, timing |

### What is proved today

Seven harnesses over the pure decisions (`cargo kani -p amplihack-cli`):

- the environment can never raise a sealed ceiling, for any pair of values
- no input escapes `MAX_DEPTH_CEILING`
- a *lower* request is honoured — so the safety property cannot be satisfied by a
  function that always returns 0, which would satisfy safety and destroy nesting
- an unsealed tree falls back to the environment, still clamped
- an uncorroborated depth claim is discarded; a corroborated one stands
- a launch wave never exceeds its limit, and an unconfigured limit is never 0
- the ceiling function is total: no input panics or overflows

### What is not, and why

Anything touching the filesystem, `/proc`, process spawning, or agent behaviour. Those
are the environment rather than a function of their inputs, and no solver settles them.
They are covered by the process tests and by the spec's crash model instead.

Roughly 40% of the decision surface in this area is amenable to proof. The rest is I/O.
