# Verus vs Lean 4 vs TLA+ for Rust

A durable reference for choosing a formal-methods tool when verifying Rust
systems code. It compares **Verus**, **Lean 4**, and **TLA+**, states what each
tool verifies and what it costs, and gives concrete guidance for applying Verus
to Simard's Rust. This document is reference guidance, not a point-in-time
report: it describes the tools' verification models, which are stable, rather
than transient benchmark numbers.

The three tools are **complementary, not competitors**. They operate at
different layers: TLA+ verifies a *design*, Verus verifies the *Rust
implementation*, and Lean 4 provides a maximal-expressiveness *proof* backstop
for the cases the other two cannot reach.

## What each tool verifies, and its cost model

### Verus -- implementation-level Rust correctness

Verus verifies the correctness of code written in Rust. Developers write
specifications of what the code should do, and Verus statically checks that the
executable Rust will always satisfy them for all executions. Verus adds no
run-time checks; it discharges proof obligations with the Z3 SMT solver, in the
tradition of Dafny, Boogie, F*, VCC, Prusti, and Creusot. It targets full
functional correctness of low-level systems code and supports a growing subset
of Rust. (Sources: Verus README and Guide overview,
https://github.com/verus-lang/verus and https://verus-lang.github.io/verus/guide/.)

- **What it verifies:** function contracts (`requires`/`ensures`), loop
  invariants with termination (`decreases`), data-structure invariants, and --
  automatically -- absence of arithmetic overflow on concrete `exec` integer
  operations and provable in-bounds indexing (panic-freedom for those
  operations). Verification is modular: callers rely only on a callee's
  `ensures`, so functions verify independently.
- **Cost model:** low-to-moderate proof burden. Much is automated by Z3; the
  developer's work is writing specs and, where the solver needs help, loop
  invariants, `assert ... by`, and small `proof fn` lemmas. Ghost `spec`/`proof`
  annotations are erased before compilation, so there is no run-time cost.
  Typical friction is overflow obligations, loop invariants (loops verify in
  isolation and do not inherit surrounding preconditions by default), and
  quantifier triggers/timeouts.
- **Automation assist:** auto-verus (`microsoft/verus-proof-synthesis`) can
  propose proof annotations. **AutoVerus** (arXiv:2409.13082) uses a three-phase
  inference/refinement/repair pipeline for small algorithmic code; **VeruSAGE**
  (arXiv:2512.18436) uses an agentic loop for repository-level systems code. Both
  are proposers behind the verifier -- Verus remains the arbiter.

### Lean 4 -- maximal-expressiveness interactive proof

Lean 4 (https://github.com/leanprover/lean4) is an interactive theorem prover
based on dependent type theory, with tactic-based proofs and the `mathlib`
library. It can express and prove essentially arbitrary mathematics and
whole-program properties, at the cost of substantial manual proof effort.

- **What it verifies:** arbitrary mathematical statements and program properties
  expressible in dependent type theory; proofs are checked by the small,
  trusted Lean kernel.
- **Rust angle:** Rust is not verified in Lean directly. An experience report on
  a Rust-to-Lean pipeline (arXiv:2605.30106) describes lifting production Rust
  into Lean 4 via symbolic extraction tools (Charon/Aeneas or Hax), targeting
  formal specification libraries, and closing proof obligations with AI provers;
  every proof is checked by the Lean kernel, so prover output cannot compromise
  soundness. The same report documents real engineering gaps (toolchain drift,
  extraction limits, missing lemmas, tactic gaps).
- **Cost model:** highest expressiveness, highest manual burden. Appropriate
  when a property genuinely exceeds what SMT-automated tools can prove (deep
  mathematics, cryptographic primitives), and when the team can invest in
  interactive proof and extraction tooling.

### TLA+ -- design-level concurrent/distributed correctness

TLA+ specifies systems as state machines and checks safety and liveness
properties, typically with the TLC model checker. It reasons about *designs and
protocols*, especially concurrency and distribution, before or alongside
implementation. This repository already uses TLA+ (see the `tla-plus-expert`
skill and the TLA+ experiment infrastructure).

- **What it verifies:** protocol-level invariants and temporal properties
  (safety and liveness) of a design, over an explicitly modeled state space.
- **Cost model:** moderate, at the design layer. The state space must be bounded
  for model checking, and a TLA+ spec is an abstraction of -- not the same
  artifact as -- the running code, so it does not by itself guarantee the
  implementation matches the design.
- **AI caveat:** SysMoBench (arXiv:2509.23130) evaluates AI-generated formal
  models (using TLA+) against automated correctness metrics -- syntactic and
  runtime correctness, conformance to system code, and invariant correctness --
  reinforcing that the checker, not the model author (human or LLM), is ground
  truth.

## Use X when

| Goal                                                                   | Reach for | Why                                                                 |
| ---------------------------------------------------------------------- | --------- | ------------------------------------------------------------------- |
| Prove a specific Rust function is panic-free and overflow-free          | Verus     | Automatic overflow/bounds checking on `exec` code                   |
| Attach a functional postcondition to real, compiled Rust                | Verus     | `requires`/`ensures` verified against the actual implementation     |
| Prove data-structure invariants (indexes, offsets, monotone counters)  | Verus     | Modular, SMT-automated, low proof burden                            |
| Carry a TLA+-established design invariant down into the code            | Verus     | Verus verifies the implementation that realizes the design          |
| Verify a concurrent/distributed protocol or state-machine design       | TLA+      | Model checks safety/liveness over the design state space            |
| Explore design alternatives before writing code                        | TLA+      | Cheap to model and re-check at the design layer                     |
| Prove deep mathematics or a property beyond SMT automation              | Lean 4    | Dependent types + tactics give maximal expressiveness               |
| Verify cryptographic primitives with kernel-checked proofs             | Lean 4    | Rust-to-Lean extraction + kernel-checked proofs (arXiv:2605.30106)  |

## Applying Verus to Simard's Rust

The following targets come from the context of issue #4610 (formal verification
for Simard's Rust). They are named here as **candidate categories**; the point is
the *class of invariant* each represents and why it is Verus-amenable, not any
internal implementation detail. The concrete Phase-2 scoping happens against the
actual code.

- **Concurrency/durability invariants (cost-ledger concurrent append).** The
  property "no appended entry is lost, duplicated, or interleaved incorrectly"
  is a durability/consistency invariant. Verus can verify the synchronous core
  of an append operation -- bounds, capacity, offset monotonicity, and a
  functional postcondition relating input to the recorded entry.
- **State-transition safety (OODA state machine, self-deploy gate).** Predicates
  that decide whether a transition or a deploy is permitted are exactly the kind
  of small, mostly-synchronous logic Verus verifies well: encode the legal
  transitions as a `spec` relation and prove the `exec` predicate implements it,
  with panic/overflow freedom for free.
- **Index/CSR consistency (lbug store).** Invariants over indexes and
  compressed/structured representations (offsets in range, sorted or monotone
  keys, sizes agreeing) are classic data-structure invariants that unit tests can
  only sample; Verus proves them for all inputs.
- **Panic/overflow freedom on hot or critical paths.** Independent of functional
  specs, Verus can prove that a chosen function cannot panic on arithmetic
  overflow or out-of-bounds indexing -- a high-value, low-spec-effort guarantee.
- **Design-to-code invariant transfer (epoch-fencing applier).** Where a design
  invariant is already established in TLA+, Verus can carry that invariant into
  the Rust that implements it, closing the gap between a verified design and the
  running code.

### What Verus cannot practically cover

Be honest about the boundaries -- these are evidence-based limits, not
aspirations:

- **Heavy `async`/`await` and complex trait/generic machinery** outside Verus's
  supported Rust subset. Verus supports a growing subset of Rust; code that lives
  mostly in async runtimes or advanced type machinery is a poor early target.
- **FFI and `unsafe` boundaries.** Behavior that crosses into C or raw
  system calls lives outside the Rust semantics Verus reasons about.
- **Large legacy modules.** Where specification effort dwarfs the risk reduction,
  the return on verification is low; prefer small, critical, well-understood
  functions first.

## Recommendation: a phased path for issue #4610 Phase-2+

Adopt Verus incrementally, letting measured proof burden -- not ambition -- drive
breadth:

1. **Pick 1-2 small, critical, mostly-synchronous functions** (for example, a
   cost-ledger append or a deploy-gate predicate). Prefer code with clear inputs,
   few dependencies, and real consequences if wrong.
2. **Prove the cheap, high-value properties first:** panic-freedom and
   arithmetic-overflow-freedom, plus **one** functional postcondition. This
   establishes the toolchain and a repeatable pattern without a large spec
   commitment.
3. **Measure the proof burden** honestly: time to first green verification,
   invariant/lemma count, and verification time. This is the real input to any
   breadth decision.
4. **Evaluate auto-verus on that same target.** Use AutoVerus/VeruSAGE to propose
   invariants and lemmas, and record how much it closed automatically -- always
   behind the verifier, with a cheat checker and no `assume`/`admit`.
5. **Only then decide breadth.** Expand to the next invariant category
   (concurrency/durability, index/CSR consistency, state-transition safety) if
   and only if steps 1-4 show acceptable cost.
6. **Keep Lean 4 as a research track.** Reach for Lean only if a specific target
   needs proof power Verus lacks (deep mathematics or cryptographic primitives),
   accepting the extraction and interactive-proof cost documented in
   arXiv:2605.30106.

Throughout, hold the line that the **verifier is ground truth**: a specification
or proof counts only once Verus (or, for Lean, the Lean kernel) accepts it.

## References

- Verus repository: https://github.com/verus-lang/verus
- Verus Guide (tutorial and reference): https://verus-lang.github.io/verus/guide/
- Verus standard library (`vstd`): https://verus-lang.github.io/verus/verusdoc/vstd/
- auto-verus (proof synthesis): https://github.com/microsoft/verus-proof-synthesis
- AutoVerus (arXiv:2409.13082): https://arxiv.org/abs/2409.13082
- VeruSAGE (arXiv:2512.18436): https://arxiv.org/abs/2512.18436
- Lean 4: https://github.com/leanprover/lean4
- Rust-to-Lean verification pipeline with AI provers (arXiv:2605.30106): https://arxiv.org/abs/2605.30106
- SysMoBench (arXiv:2509.23130): https://arxiv.org/abs/2509.23130
- Verus Expert skill (this repo): [claude/skills/verus-expert/SKILL.md](../claude/skills/verus-expert/SKILL.md)
- TLA+ Expert skill (this repo): [claude/skills/tla-plus-expert/SKILL.md](../claude/skills/tla-plus-expert/SKILL.md)
- Issue #4610: formal verification for Simard's Rust (Verus / auto-verus / Lean 4 vs TLA+)
