---
name: property-based-testing
version: 1.0.0
description: Property-based and metamorphic testing expert for choosing a library, finding good properties (invariants, round-trips, idempotence, oracle/differential, metamorphic relations), and wiring generative tests into the existing test runner.
auto_activates:
  - "property-based testing"
  - "property based test"
  - "metamorphic testing"
  - "generative testing"
  - "fuzz property"
  - "proptest"
  - "quickcheck"
  - "hypothesis testing library"
  - "fast-check"
  - "fscheck"
  - "jqwik"
  - "shrinking"
explicit_triggers:
  - /property-based-testing
  - /amplihack:property-based-testing
confirmation_required: false
skip_confirmation_if_explicit: true
token_budget: 3000
---

# Property-Based Testing Skill

Expert guidance for **property-based** and **metamorphic** testing: instead of
asserting one hand-picked example, you state a rule that must hold for *every*
input, and the library generates hundreds of inputs to try to break it.

## Where This Fits — the third leg of the formal-methods triad

amplihack ships two other formal-methods skills. Property-based testing is the
practical middle ground between them:

| Approach | Skill | What it checks | Cost |
| --- | --- | --- | --- |
| Example tests | (unit tests) | Behavior on inputs you thought of | Low |
| **Property-based / metamorphic** | **property-based-testing** | Invariants over *generated* inputs, at runtime, against real code | Medium |
| BDD acceptance | `gherkin-expert` | Human-readable behavior scenarios | Low–medium |
| Model checking / proof | `tla-plus-expert` | Exhaustive state-space or temporal properties of a *model* | High |

Reach for property-based testing when:

- Example tests keep missing edge cases (empty input, huge input, unicode,
  boundary numbers, duplicates, reordering).
- You can state a **rule** the code must obey but can't enumerate every input.
- You have a **reference implementation** (an old version, a slow-but-correct
  oracle, or a sibling implementation in another language) to compare against.

Prefer TLA+ instead when the property is about *concurrency, ordering, or
temporal/liveness* behavior of a design you can model. Prefer Gherkin when the
value is a shared, human-readable acceptance spec. These are complements, not
substitutes — a mature component often has all three.

## Pick the library for your stack

| Stack | Library | Runner it plugs into |
| --- | --- | --- |
| Rust | **proptest** (preferred) or **quickcheck** | `cargo test` |
| Python | **Hypothesis** | `pytest` / `unittest` |
| .NET | **FsCheck** (F#-friendly) or **CsCheck** (C#-friendly) | `xunit` / `nunit` |
| JS/TS | **fast-check** | `jest` / `vitest` / `mocha` |
| Java | **jqwik** | JUnit 5 platform |

**Do not build a separate harness.** Every library above runs *inside* your
existing test runner as ordinary test cases. A property test is just a test
that loops over generated inputs.

## Finding good properties

A "property" is a rule that holds for all valid inputs. The reliable families:

- **Invariant** — something always true of the output (e.g. output length ≤
  input length; a sorted list is still a permutation of the input).
- **Round-trip** — `decode(encode(x)) == x`. Great for serializers, parsers,
  compressors, config load/save.
- **Idempotence** — applying twice equals applying once: `f(f(x)) == f(x)`.
  Redaction, normalization, dedup, formatting.
- **Commutativity / associativity** — order of operations doesn't change the
  result: `merge(a, b) == merge(b, a)`.
- **Oracle / differential** — compare against a trusted reference: a naive
  slow implementation, the previous release, or **a sibling implementation in
  another language** (cross-language conformance).
- **Metamorphic relation** — you don't know the exact output, but you know how
  the output must *change* when the input changes. E.g. adding an item to a set
  can never shrink its size; re-scaling all weights by k scales the total by k;
  searching a superset returns at least as many hits.

When you can't state an exact expected value, a metamorphic relation is usually
still available. That is the whole point of metamorphic testing.

## Shrinking, seeding, reproducibility

- **Shrinking**: when a property fails, the library automatically reduces the
  failing input to the *smallest* counterexample (e.g. from a 400-char string
  to `"a"`). Never disable shrinking — the minimal case is the bug report.
- **Seeding**: every run prints/records a seed. On failure, re-run with the
  same seed to reproduce deterministically. Commit the seed (or the reduced
  counterexample as a regression example test) so the bug stays fixed.
- **Budget**: start with the default case count (usually 100–1000). Raise it in
  CI for critical invariants; keep it low locally for fast feedback.

## Worked examples (concrete property families)

The snippets below use amplihack's own invariants as worked examples.

### Rust — proptest (redaction idempotence + no-secret-leak)

```rust
use proptest::prelude::*;

proptest! {
    // redact(redact(x)) == redact(x)
    #[test]
    fn redact_is_idempotent(s in ".*") {
        let once = redact(&s);
        let twice = redact(&once);
        prop_assert_eq!(once, twice);
    }

    // output never contains the secret
    #[test]
    fn redacted_output_has_no_secret(secret in "[A-Za-z0-9]{8,32}", prefix in ".*", suffix in ".*") {
        let input = format!("{prefix}{secret}{suffix}");
        let out = redact_secret(&input, &secret);
        prop_assert!(!out.contains(&secret));
    }
}
```

### Python — Hypothesis (config bounds invariant)

```python
from hypothesis import given, strategies as st

# shard_jobs_max <= effective <= jobs_max
@given(
    jobs_max=st.integers(min_value=1, max_value=256),
    shard_jobs_max=st.integers(min_value=1, max_value=256),
    requested=st.integers(min_value=0, max_value=512),
)
def test_effective_jobs_respects_bounds(jobs_max, shard_jobs_max, requested):
    shard_jobs_max = min(shard_jobs_max, jobs_max)
    effective = compute_effective_jobs(requested, shard_jobs_max, jobs_max)
    assert shard_jobs_max <= effective <= jobs_max
```

### .NET — FsCheck (coverage tally invariants)

```csharp
using FsCheck;
using FsCheck.Xunit;

public class CoverageProperties
{
    // bundle >= in_scope  AND  scanned + excluded == in_scope
    [Property]
    public Property CoverageTallyHolds(PositiveInt inScope, PositiveInt extra, NonNegativeInt excluded)
    {
        int in_scope = inScope.Get;
        int bundle   = in_scope + extra.Get;              // bundle is a superset
        int exc      = Math.Min(excluded.Get, in_scope);
        int scanned  = in_scope - exc;

        bool bundleSuperset = bundle >= in_scope;
        bool tallyBalances  = scanned + exc == in_scope;
        return (bundleSuperset && tallyBalances).ToProperty();
    }
}
```

### JS/TS — fast-check (telemetry totality / safety)

```ts
import fc from "fast-check";
import { recordTelemetry } from "../src/telemetry";

// totality: recordTelemetry never throws and always returns a well-formed event
test("telemetry is total and safe", () => {
  fc.assert(
    fc.property(fc.anything(), fc.string(), (payload, name) => {
      const event = recordTelemetry(name, payload); // must not throw
      expect(event.name).toBe(name);
      expect(typeof event.timestamp).toBe("number");
      expect(typeof event.dropped).toBe("boolean"); // always a boolean, never undefined
    }),
  );
});
```

### Java — jqwik (encode/decode round-trip + differential oracle)

```java
import net.jqwik.api.*;

class ConfigRoundTripProperties {

    // decode(encode(x)) == x  (round-trip)
    @Property
    boolean encodeDecodeRoundTrips(@ForAll("configs") Config c) {
        return Config.decode(Config.encode(c)).equals(c);
    }

    // oracle/differential: fast parser agrees with reference parser
    @Property
    boolean parsersAgree(@ForAll("configText") String text) {
        return FastParser.parse(text).equals(ReferenceParser.parse(text));
    }

    @Provide Arbitrary<Config> configs() { return Arbitraries.of(Config.defaults()); }
    @Provide Arbitrary<String> configText() { return Arbitraries.strings().ofMaxLength(64); }
}
```

## Cross-language conformance (oracle in practice)

For amplihack's multi-runtime components, run the *same* generated inputs
through each language implementation and assert identical outputs. Generate the
corpus once (with a fixed seed), feed it to every implementation, and diff the
results. Any divergence is a conformance bug — shrink it to the minimal input.

## Quick quiz

Check your understanding (answers below).

1. A function `normalize_path` is claimed to be safe to call more than once.
   Which property family proves that, and how do you write it?
2. You are testing a JSON serializer in TypeScript and want to catch encoding
   bugs. Which library and which property family fit best?
3. You can't predict the exact ranking a search function returns, but you know
   adding a matching document must never *reduce* the number of results. What
   kind of property is this?
4. Which stack does **jqwik** target, and which runner does it plug into?
5. A redaction function must satisfy two rules. Name them and classify each.

<details>
<summary>Answers</summary>

1. **Idempotence**: assert `normalize_path(normalize_path(p)) == normalize_path(p)`
   over generated paths.
2. **fast-check**, using a **round-trip** property: `decode(encode(x)) == x`.
3. A **metamorphic relation** — you constrain how the output must change, not
   its exact value.
4. **Java**, plugged into the **JUnit 5** platform.
5. **Idempotence** (`redact(redact(x)) == redact(x)`) and a **no-secret-leak
   invariant** (the output never matches the secret). The first is an
   idempotence property; the second is a safety invariant.

</details>

## Related Skills

- `tla-plus-expert` — exhaustive model checking of designs (the high-cost leg).
- `gherkin-expert` — human-readable BDD acceptance scenarios.
- `test-gap-analyzer` — find *which* code lacks coverage; then add properties here.
- `smart-test` — run the resulting property tests through the existing runner.
