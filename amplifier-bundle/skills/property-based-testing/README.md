# Property-Based Testing Skill

Expert guidance for **property-based** and **metamorphic** testing. Instead of
asserting one hand-picked example, you state a rule that must hold for *every*
input, and the library generates hundreds of inputs to try to break it. This
skill is the third leg of amplihack's formal-methods triad, alongside
`tla-plus-expert` (model checking) and `gherkin-expert` (BDD acceptance specs).

## Quick Start

```
User: I want to test that our redaction function can't leak a secret. How?

Claude: That's a property-based testing job — you can't enumerate every secret,
but you can state a rule. Use proptest (Rust), inside `cargo test`:

  proptest! {
      #[test]
      fn redacted_output_has_no_secret(
          secret in "[A-Za-z0-9]{8,32}", prefix in ".*", suffix in ".*"
      ) {
          let input = format!("{prefix}{secret}{suffix}");
          prop_assert!(!redact_secret(&input, &secret).contains(&secret));
      }
  }

This is a "no-secret-leak" safety invariant. Pair it with an idempotence
property, redact(redact(x)) == redact(x), for full coverage.
```

## When to Use

Reach for this skill when:

- Example tests keep missing edge cases (empty input, huge input, unicode,
  boundary numbers, duplicates, reordering).
- You can state a **rule** the code must obey but can't enumerate every input.
- You have a **reference implementation** (an older version, a slow-but-correct
  oracle, or a sibling implementation in another language) to compare against.

Prefer `tla-plus-expert` instead when the property is about concurrency,
ordering, or temporal/liveness behavior of a *design you can model*. Prefer
`gherkin-expert` when the value is a shared, human-readable acceptance spec.
These complement rather than replace each other — a mature component often has
all three.

## Features

- **Library selection per stack**: proptest/quickcheck (Rust), Hypothesis
  (Python), FsCheck/CsCheck (.NET), fast-check (JS/TS), jqwik (Java).
- **Property discovery**: invariants, round-trips, idempotence, commutativity,
  oracle/differential (incl. cross-language conformance), and metamorphic
  relations.
- **Shrinking & seeding guidance**: turn a failure into the smallest
  reproducible counterexample and pin it as a regression test.
- **Existing-runner integration**: property tests are ordinary tests — no
  separate harness.

## Activation

| Method   | Trigger                                                                                                   |
| -------- | --------------------------------------------------------------------------------------------------------- |
| Auto     | Phrases like "property-based testing", "metamorphic testing", "generative testing", "shrinking", "fuzz property", or a library name (proptest, quickcheck, hypothesis, fast-check, fscheck, jqwik). |
| Explicit | `/property-based-testing` or `/amplihack:property-based-testing`                                           |

No confirmation is required; explicit triggers skip confirmation entirely
(`confirmation_required: false`, `skip_confirmation_if_explicit: true`). The
skill's token budget is 3000.

## Library Selection

| Stack  | Library                                        | Runner it plugs into        |
| ------ | ---------------------------------------------- | --------------------------- |
| Rust   | **proptest** (preferred) or **quickcheck**     | `cargo test`                |
| Python | **Hypothesis**                                 | `pytest` / `unittest`       |
| .NET   | **FsCheck** (F#-friendly) / **CsCheck** (C#)   | `xunit` / `nunit`           |
| JS/TS  | **fast-check**                                 | `jest` / `vitest` / `mocha` |
| Java   | **jqwik**                                      | JUnit 5 platform            |

Every library above runs *inside* your existing test runner as ordinary test
cases. Do not build a separate harness.

## Property Families

| Family                | Rule                                   | Typical targets                       |
| --------------------- | -------------------------------------- | ------------------------------------- |
| Invariant             | Something always true of the output    | length bounds, permutation-preserving |
| Round-trip            | `decode(encode(x)) == x`               | serializers, parsers, config load/save|
| Idempotence           | `f(f(x)) == f(x)`                      | redaction, normalization, dedup       |
| Commutativity         | `merge(a, b) == merge(b, a)`           | set/CRDT merges, aggregation          |
| Oracle / differential | Matches a trusted reference            | old release, cross-language conformance|
| Metamorphic relation  | How output must *change* with input    | search, ranking, scaling              |

When you can't state an exact expected value, a metamorphic relation is usually
still available — that is the whole point of metamorphic testing.

## Worked Examples

The skill ships one worked example per stack, all drawn from amplihack's own
invariants:

- **Rust / proptest** — redaction idempotence and the no-secret-leak invariant.
- **Python / Hypothesis** — config bounds: `shard_jobs_max <= effective <= jobs_max`.
- **.NET / FsCheck** — coverage tally: `bundle >= in_scope` and
  `scanned + excluded == in_scope`.
- **JS/TS / fast-check** — telemetry totality/safety: `recordTelemetry` never
  throws and always returns a well-formed event.
- **Java / jqwik** — config encode/decode round-trip plus a differential oracle
  comparing a fast parser to a reference parser.

See [SKILL.md](./SKILL.md) for the full, runnable snippets.

## Cross-Language Conformance

For amplihack's multi-runtime components, run the *same* seeded corpus through
each language implementation and assert identical outputs. Generate the corpus
once with a fixed seed, feed it to every implementation, and diff the results.
Any divergence is a conformance bug — shrink it to the minimal input.

## Shrinking, Seeding, Reproducibility

- **Shrinking** reduces a failing input to the smallest counterexample. Never
  disable it — the minimal case *is* the bug report.
- **Seeding**: every run records a seed; re-run with it to reproduce
  deterministically. Commit the seed or the reduced counterexample as a
  regression example test so the bug stays fixed.
- **Budget**: start with the default case count (100–1000). Raise it in CI for
  critical invariants; keep it low locally for fast feedback.
- **Security note**: redact and seed shrink counterexamples so generative tests
  never leak sensitive data into CI logs.

## Workflow Integration

- **Step 11 (Write Tests)**: add properties for invariants example tests can't
  enumerate.
- **Step 12–13 (Run/Local Testing)**: property tests run in the existing runner
  via `smart-test` tiers — no separate harness.
- **After a failure**: pin the shrunk counterexample as a regression test.

## Related Skills

- `tla-plus-expert` — exhaustive model checking of designs (the high-cost leg).
- `gherkin-expert` — human-readable BDD acceptance scenarios.
- `test-gap-analyzer` — find *which* code lacks coverage; then add properties here.
- `smart-test` — run the resulting property tests through the existing runner.

---

See [SKILL.md](./SKILL.md) for complete documentation, runnable per-language
examples, and the built-in quiz.
