---
name: verus-expert
version: 1.0.0
description: Verus deductive-verification expert for proving Rust functional correctness, panic-freedom, and arithmetic-overflow-freedom with SMT-backed specs and LLM-assisted proof synthesis
activation_keywords:
  - "Verus"
  - "verus!"
  - "formal verification"
  - "deductive verification"
  - "proof synthesis"
  - "auto-verus"
  - "AutoVerus"
  - "requires ensures"
  - "loop invariant"
  - "decreases"
  - "SMT"
  - "Z3"
  - "panic-free"
  - "overflow-free"
  - "refinement"
  - "ghost code"
  - "spec function"
agent: amplihack:specialized:verus-expert
---

# Verus Expert Skill

## Purpose

Provides expert-level Verus assistance for deductive verification of Rust:
writing specifications, proving panic-freedom and arithmetic-overflow-freedom,
authoring loop invariants and proofs, debugging failed verification, and using
LLM proof synthesis (auto-verus) safely behind the verifier.

## When This Skill Activates

- User asks to write or review Verus specifications (`requires`, `ensures`,
  `invariant`, `decreases`) for Rust code
- User wants to prove panic-freedom or arithmetic-overflow-freedom on a function
- User needs help authoring loop invariants, assertions, or `proof fn` lemmas
- User is debugging a failed Verus verification (failed pre/postcondition,
  overflow, timeout, quantifier trigger issues)
- User asks about Verus modes (`spec`/`proof`/`exec`), ghost/tracked code, or
  `int`/`nat` reasoning
- User wants to apply LLM proof synthesis (auto-verus / AutoVerus / VeruSAGE) as
  a proposer behind the Verus checker
- User is deciding between Verus, Lean 4, and TLA+ for a Rust verification goal

## How It Works

This skill delegates to the `verus-expert` agent, which has evidence-grounded
knowledge of:

1. **Verus specification language** -- `verus! { ... }`, `requires`/`ensures`,
   spec functions (`open`/`closed`/`pub`), and the modular verification protocol
2. **The three modes** -- `spec`, `proof`, and `exec`; ghost code and its erasure
   before compilation; `ghost`/`tracked` state
3. **Panic and overflow freedom** -- automatic overflow checking on `exec`
   arithmetic; `checked_*` and `CheckedU*`; provable index bounds
4. **Loops, invariants, and termination** -- loop invariants, modular loop
   verification, `#[verifier::loop_isolation(...)]`, and `decreases`
5. **SMT/Z3 solving model** -- reading Verus errors, `assert ... by`, lemmas,
   quantifier triggers, and diagnosing timeouts
6. **auto-verus** -- AutoVerus (three-phase LLM proof construction) and VeruSAGE as LLM
   proof-synthesis proposers, always checked by Verus
7. **LLM guardrails** -- verifier is ground truth; provide `vstd` and a Verus
   binary; use a cheat checker; forbid `assume`/`admit`

## Integration with Existing Infrastructure

This is Phase-1 of the formal-verification effort tracked in issue #4610: a
reusable skill plus a durable applicability assessment. It complements the
existing `tla-plus-expert` skill -- TLA+ verifies concurrent/distributed
*designs*, while Verus verifies the *Rust implementation* that realizes them.

No Verus toolchain or CI wiring is introduced in this phase. Whether and where to
adopt Verus in the build (annotating specific functions, adding a verification
gate) is a Phase-2 decision, gated on the assessment's recommendation. The
durable comparison and phased plan live in the formal-methods assessment:
[Verus vs Lean 4 vs TLA+ for Rust](https://github.com/rysweet/amplihack-rs/blob/main/docs/formal-methods/verus-vs-lean-vs-tla-for-rust.md).

## Usage Examples

```
# Add a machine-checked contract to a function
/verus-expert Add requires/ensures to this ledger-append function and prove it cannot overflow

# Author a loop invariant
/verus-expert Help me write the loop invariant and decreases clause for this scan loop

# Debug a failed verification
/verus-expert Verus reports "possible arithmetic overflow" here -- how do I discharge it?

# Use auto-verus safely
/verus-expert Propose loop invariants with auto-verus, then confirm Verus accepts them

# Decide if Verus is the right tool
/verus-expert Should I use Verus, Lean 4, or TLA+ to verify this state-machine transition?
```

## Key Resources

- Verus repository: https://github.com/verus-lang/verus
- Verus Guide (tutorial and reference): https://verus-lang.github.io/verus/guide/
- Verus standard library (`vstd`): https://verus-lang.github.io/verus/verusdoc/vstd/
- auto-verus (proof synthesis): https://github.com/microsoft/verus-proof-synthesis
- Formal-methods assessment: [docs/formal-methods/verus-vs-lean-vs-tla-for-rust.md](https://github.com/rysweet/amplihack-rs/blob/main/docs/formal-methods/verus-vs-lean-vs-tla-for-rust.md)
- Issue #4610: formal verification for Simard's Rust (Verus / auto-verus / Lean 4 vs TLA+)
