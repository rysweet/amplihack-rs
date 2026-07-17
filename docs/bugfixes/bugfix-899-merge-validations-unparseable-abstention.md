# Bug Fix #899 — merge-validations treats an unparseable validator as an abstention

> **Issue:** [#899](https://github.com/rysweet/amplihack-rs/issues/899)

---

## Summary

The `quality-audit-cycle` recipe's `merge-validations` step no longer aborts the
whole audit when a single validator produces unparseable output. An unparseable
validator is recorded as an **abstention** (zero votes), its raw output is
preserved to a per-cycle artifact, a targeted `WARNING` is emitted, and the merge
**continues** — applying the configured confirmation threshold against the
validators that *did* parse.

The audit only fails closed when **no** validator parsed *and* at least one was
unparseable. When at least one validator parses, the cycle completes normally and
the audit can satisfy its required minimum number of cycles.

## Behavior

For a three-validator cycle, each validator output is classified as one of:

| Class | Meaning | Effect on merge |
| --- | --- | --- |
| `PARSED` | Valid structured verdicts | Counts toward `parsed_count`; its verdicts vote |
| `EMPTY` | Clean no-op (no findings, no error) | Silent; contributes no votes, no warning |
| `UNPARSEABLE` | Output could not be parsed as verdict JSON | **Abstention** — zero votes; `WARNING` emitted; raw output preserved |

### Abstention semantics

An `UNPARSEABLE` validator:

1. **Contributes zero votes.** The confirmation threshold is evaluated only
   against validators that parsed. A malformed validator can neither confirm nor
   veto a finding.
2. **Emits a targeted `WARNING`** to stderr naming the validator and the path to
   its preserved raw output:

   ```text
   [merge-validations] WARNING: validator v1 output unparseable; counting zero votes from it. Raw output preserved at: <OUTPUT_DIR>/cycle_<CYCLE>/validator_v1_raw.txt
   ```

   > The `WARNING` is emitted as a **single line** (`echo ... >&2`). It is
   > wrapped above only for readability — do not grep for a literal multi-line
   > string.

3. **Preserves the raw, unmodified output** to
   `<OUTPUT_DIR>/cycle_<CYCLE>/validator_<label>_raw.txt`. The artifact directory
   is created with `0700` and the artifact with `0600` permissions. The path is
   derived solely from trusted context variables (`OUTPUT_DIR`, `CYCLE`, and a
   fixed validator label) — never from validator content — so malformed payloads
   cannot influence the write location.

4. **Does not abort the cycle.** The merge proceeds using the surviving parsed
   validators.

### Fail-closed gate

The step exits `1` **only** when every validator failed to parse *and* at least
one was unparseable:

```text
parsed_count == 0 && unparseable_count >= 1  →  FATAL, exit 1
```

In that case a clear diagnostic (never a raw `jq` error) is emitted listing the
preserved raw artifacts:

```text
[merge-validations] FATAL: all validators produced unparseable output; cannot
merge any verdicts. Raw outputs preserved at: <artifact1>, <artifact2>, ...
```

An all-`EMPTY` cycle (`parsed_count == 0`, `unparseable_count == 0`) is a clean
audit and proceeds to produce zero confirmed findings — it does **not** fail.

## Configuration

The confirmation threshold is the number of parsed validators that must return a
`confirmed` (or `downgraded`) verdict for a finding to be recorded as
`confirmed`.

| Setting | Recipe context key | Default | Notes |
| --- | --- | --- | --- |
| Confirmation threshold | `validation_threshold` | `2` | Evaluated against **parsed validators only** |

Because abstentions contribute zero votes, running with one unparseable validator
at `validation_threshold=2` requires the two remaining validators to both confirm
a finding for it to survive. The threshold value is **not** relaxed to account for
the abstaining validator — abstention lowers the number of available voters, not
the bar for confirmation.

## Examples

### Two parsed + one unparseable at threshold 2 (the #899 scenario)

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path=src/stewardship \
  -c repo_path=. \
  -c min_cycles=3 \
  -c max_cycles=6 \
  -c validation_threshold=2
```

Given validators `v2` and `v3` parse and `v1` is unparseable:

- The cycle exits `0`.
- `merge-validations` writes valid merged `validated_findings` JSON.
- A finding is `confirmed` when **both** `v2` and `v3` confirm it (threshold 2 met
  by the two parsed validators); `v1` contributes zero votes.
- stderr contains the `WARNING` naming `v1` and its raw-artifact path.
- The malformed output is preserved at
  `<OUTPUT_DIR>/cycle_1/validator_v1_raw.txt`.
- The audit continues and can reach its required minimum three cycles.

Representative merged output:

```json
{
  "cycle": 1,
  "validated": [
    {
      "finding_id": "F-001",
      "verdict": "confirmed",
      "final_severity": "high",
      "votes": { "confirmed": 2, "false_positive": 0 },
      "reasoning": "..."
    }
  ],
  "confirmed_count": 1,
  "false_positive_count": 0,
  "false_positive_rate": "0%"
}
```

Note `votes.confirmed` reflects only the two parsed validators — the unparseable
validator does not appear as a vote. `final_severity` is the **minimum severity
across the confirming validators' verdicts** (via `min_by` over `new_severity`,
defaulting to `medium`), not a value copied from the original finding; `high`
here is illustrative.

### All three validators unparseable (fail-closed)

When no validator parses, the step exits `1` with the `FATAL` diagnostic above and
lists the preserved raw artifacts. This prevents silently merging an audit that
has no usable verdicts.

### Malformed payload containing shell metacharacters

A malformed validator payload such as `$(touch pwned)` or text containing
backticks is treated strictly as **data**. It is copied verbatim into the raw
artifact, contributes zero votes, and produces **no** side effects — no command
substitution, no phantom votes, and no file created outside the per-cycle
artifact directory.

## Regression coverage

The behavior is locked by an integration test that drives the shipped recipe body
through the byte-exact harness `tests/gadugi/run-merge-validations.sh`:

- **File:** `crates/amplihack-cli/tests/issue_899_merge_validations_abstention.rs`
- **Run:**

  ```bash
  cargo test -p amplihack-cli --test issue_899_merge_validations_abstention
  ```

The test asserts, for **2 parsed + 1 unparseable at threshold 2**:

- exit code `0`;
- stdout is valid merged `validated_findings` JSON;
- the unparseable validator contributes zero votes (confirmed count reflects the
  two parsed validators only);
- the `WARNING` naming the validator and raw-artifact path is emitted;
- the malformed raw output is preserved at
  `cycle_<N>/validator_<label>_raw.txt`.

Companion assertions cover:

- **all-unparseable → exit 1** (the fail-closed `parsed_count == 0 &&
  unparseable_count >= 1` gate still holds);
- **shell-metacharacter injection guard** — a malformed payload with shell
  metacharacters is preserved literally, casts zero votes, and creates no
  side-effect file.

The existing merge-validations regression tests remain green:
`issue_820_merge_validations_mixed_output`,
`issue_833_merge_validations_json_tolerance`, and
`issue_646_quality_audit_cycle_bugs`.

## Traceability

The `merge-validations` step in
`amplifier-bundle/recipes/quality-audit-cycle.yaml` carries an explicit `#899`
marker comment at the fail-closed guard tying the
`parsed_count == 0 && unparseable_count >= 1` gate to this issue, and clarifying
that any parsed survivor continues the merge with the unparseable validator
contributing zero votes. The implementation step adds this `#899` marker
alongside the existing D4 guard comment.

## Scope

This fix is **regression-hardening and traceability** only. The voting math,
verdict extraction, validator classification, the `jq` merge, threshold defaults,
and validator prompts are unchanged — the abstention semantics were already
correct in current source. The stale-installed-asset shadow that produced the
original #899 report is addressed separately by the install/update bundle
compatibility work (#888) and is out of scope here.
