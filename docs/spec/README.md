# OrchLedger — formal spec for nested-agent admission control

`OrchLedger.tla` models amplihack's admission control **across process boundaries**,
which is where the bug in issue #1326 lived. The ledger is a file; read and write are
separate steps, so time-of-check/time-of-use is expressible; any process may be
SIGKILLed between any two steps; and `flock` is released by the kernel on death.

Three design decisions are constants, so each can be shown *necessary* by ablation
rather than asserted:

| Config | shared ledger | flock | ceiling from ledger | Expected TLC result |
|---|:--:|:--:|:--:|---|
| `A_today` | ✗ | ✓ | ✗ | `NodeBudget` violated |
| `B_proposed` | ✓ | ✓ | ✓ | **no error** |
| `C_no_lock` | ✓ | ✗ | ✓ | `NodeBudget` violated |
| `D_env_ceiling` | ✓ | ✓ | ✗ | `CeilingMonotone` violated |

`B_proposed` is the configuration this repository implements. The other three are
regression oracles: if a refactor makes `A`, `C`, or `D` start passing, the spec has
stopped describing the hazard and the ablation is no longer meaningful.

## Running the gate

    scripts/check-spec.sh

Requires `java` and `tla2tools.jar` (set `TLA2TOOLS_JAR`, or place it at
`~/tla2tools.jar`). The script asserts the expected outcome of **all four** configs,
so a spec that no longer distinguishes the designs fails the gate.

## Conformance

The spec is not decoration. `crates/amplihack-cli/tests/spec_conformance.rs` enforces
each named invariant against the real implementation, and a drift test fails if an
invariant is added to a `.cfg` without a corresponding test. See that file for the
traceability table.
