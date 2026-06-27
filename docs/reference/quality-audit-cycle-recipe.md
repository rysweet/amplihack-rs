# Quality Audit Cycle Recipe Reference

Reference for the `quality-audit-cycle` recipe
(`amplifier-bundle/recipes/quality-audit-cycle.yaml`), version 4.0.0.

## Context Variables

| Variable               | Default                        | Description                                                   |
| ---------------------- | ------------------------------ | ------------------------------------------------------------- |
| `target_path`          | `src/amplihack`                | Directory to audit                                            |
| `repo_path`            | `.`                            | Repository root; sets `working_dir` for agent steps           |
| `min_cycles`           | `3`                            | Minimum audit cycles (always run at least this many)          |
| `max_cycles`           | `6`                            | Maximum cycles (safety valve)                                 |
| `validation_threshold` | `2`                            | Minimum validators that must agree (out of 3)                 |
| `severity_threshold`   | `medium`                       | Minimum severity to report (`low`/`medium`/`high`/`critical`) |
| `module_loc_limit`     | `300`                          | Flag modules exceeding this LOC count                         |
| `fix_all_per_cycle`    | `true`                         | Must fix ALL confirmed findings before next cycle (#2842)     |
| `categories`           | (all)                          | Comma-separated category filter                               |
| `output_dir`           | `./eval_results/quality_audit` | Where to write audit output files                             |

### Internal State Variables

These are managed by the recipe loop and should not be set manually:

`audit_findings`, `validation_agent_1`, `validation_agent_2`, `validation_agent_3`,
`validated_findings`, `fix_results`, `fix_verification`,
`cycle_number`, `cycle_history`, `recurse_decision`, `summary`,
`self_improvement_results`

## Steps

| Step ID                 | Type  | Purpose                                                            |
| ----------------------- | ----- | ------------------------------------------------------------------ |
| `seek`                  | agent | Scan codebase for quality issues (escalating depth)                |
| `validate-agent-1/2/3` | agent | Three independent validators confirm/reject findings               |
| `merge-validations`     | bash  | Merge validator outputs, require ≥`validation_threshold` agreement |
| `fix`                   | agent | Fix ALL confirmed findings (fix-all-per-cycle rule)                |
| `verify-fixes`          | bash  | Compare fix results against confirmed findings AND git diff        |
| `accumulate-history`    | bash  | Append cycle findings to history for next cycle's SEEK             |
| `recurse-decision`      | bash  | Decide CONTINUE or STOP based on cycle count and new findings      |
| `run-recursive-cycle`   | bash  | Re-invoke the recipe as a subprocess for the next cycle            |
| `summary`               | agent | Produce consolidated audit report                                  |
| `self-improvement`      | agent | Review the audit process itself for workflow improvements          |

### Loop Behavior

```
Cycle 1: seek → validate(×3) → merge → fix → verify → accumulate → decision
Cycle 2: seek(deeper) → validate(×3) → merge → fix → verify → accumulate → decision
Cycle 3+: seek(deepest) → validate(×3) → merge → fix → verify → accumulate → decision
```

- **Minimum cycles** always run regardless of findings.
- **Continue past minimum** if any high/critical findings or >3 medium findings
  emerged in the current cycle.
- **Stop** at `max_cycles` unconditionally.

### merge-validations: Validator-Output Normalization (#833)

The three validator agents are prompted to emit a JSON verdict object, but
agent output is free-form text: the JSON usually arrives wrapped in a
` ```json ` fence and may be preceded by reasoning prose, a log preamble, or
several concatenated JSON values. Feeding that raw text straight to
`jq --slurpfile` aborts the whole audit cycle with
`jq: Bad JSON in --slurpfile ... Invalid numeric literal at line 1, column 4`,
discarding the seek and validation work already done (#833).

Before merging, the step normalizes each validator's raw output to a single
JSON verdict object using a **self-contained** `extract_verdict()` shell
function. The function depends only on `jq` and `sed` — both already required
by the step — and on **no external binary**. (The earlier #820 attempt routed
extraction through `amplihack orch helper extract-json`; when that binary was
not on `PATH`, all three validators silently degraded to `{}`, masking real
findings. #833 removes that dependency entirely.)

#### Tiered extraction

`extract_verdict()` reads one validator's raw output and emits exactly one
normalized JSON object, trying each tier in order until one succeeds:

| Tier | Strategy                                                                                  | Result on success |
| ---- | ----------------------------------------------------------------------------------------- | ----------------- |
| 0    | Empty / whitespace-only input                                                             | `{}` (EMPTY)      |
| 1    | Strict parse of the whole input (`jq -c .`)                                               | object (PARSED)   |
| 2    | Strip ` ```json ` / ` ``` ` markdown fences, then re-parse                                 | object (PARSED)   |
| 3    | Trim to first `{` … last `}` and stream-scan candidates, **preferring the object that contains a `validated` key**; fall back to the last parseable object | object (PARSED)   |
| 4    | Nothing parseable                                                                          | `{}` (UNPARSEABLE)|

Tier 3 fixes a weakness in the earlier approach: when a validator prepends a
log line such as `{"level":"info","msg":"validating"}` before the real verdict,
the extractor selects the object carrying the `validated` key rather than the
first object it encounters. Each recovered candidate is re-validated by `jq`
before acceptance, so a slice that merely looks like JSON (for example, a `}`
inside a string) is rejected and the scan continues.

#### Per-validator classification

Each validator (`v1`, `v2`, `v3`) is classified independently:

- **PARSED** — a JSON verdict object was recovered. Counts toward the
  parsed-validator total and contributes its votes to the merge.
- **EMPTY** — the validator did not run or produced only whitespace. Normalizes
  to `{}`, contributes zero votes, and emits **no** warning. An all-EMPTY cycle
  is a clean audit and proceeds normally.
- **UNPARSEABLE** — the validator produced non-empty output but no JSON verdict
  object survived extraction. Emits a targeted stderr warning naming the
  validator, preserves the raw output as an artifact, normalizes to `{}`, and
  continues.

`EMPTY` and `UNPARSEABLE` are deliberately distinct: only `UNPARSEABLE`
indicates a malformed validator, and only `UNPARSEABLE` arms the fatal gate
described below.

#### Partial-failure tolerance

An `UNPARSEABLE` validator never aborts the merge. Instead the step writes a
warning to stderr and preserves the raw output under the cycle's output
directory:

```
[merge-validations] WARNING: validator v2 output unparseable; counting zero
votes from it. Raw output preserved at:
./eval_results/quality_audit/cycle_3/validator_v2_raw.txt
```

The raw artifact is written to
`${output_dir}/cycle_${cycle_number}/validator_vN_raw.txt`. The cycle directory
is created with mode `700` and the artifact file with mode `600`; if
`${output_dir}` is not writable the step falls back to `/tmp` (also `600`) and
the warning still names the fallback path. Artifact paths are built only from
trusted context variables (`output_dir`, `cycle_number`) and the fixed labels
`v1`/`v2`/`v3` — never from validator content — so a validator cannot influence
where its raw output is written.

The merge then proceeds with the validators that did parse, so a single
malformed validator can no longer take down the entire cycle.

#### All-unparseable fatal gate

The step tracks how many validators parsed. If **every** validator that
produced output was `UNPARSEABLE` (parsed count `0` and at least one
`UNPARSEABLE`), the step fails hard with a clear diagnostic that lists all
preserved raw artifacts — never a raw `jq` error:

```
[merge-validations] FATAL: all validators produced unparseable output; cannot
merge. Raw outputs preserved at:
  v1: ./eval_results/quality_audit/cycle_3/validator_v1_raw.txt
  v2: ./eval_results/quality_audit/cycle_3/validator_v2_raw.txt
  v3: ./eval_results/quality_audit/cycle_3/validator_v3_raw.txt
```

The diagnostic lists **only** the validators whose raw output was preserved —
that is, the `UNPARSEABLE` ones. In the mixed `EMPTY + UNPARSEABLE` (zero
`PARSED`) case the gate still fires, but an `EMPTY` validator produced no output
and therefore has no artifact, so it is **omitted** from the list; only the
unparseable validator(s) appear. The all-three example above is the pure
all-`UNPARSEABLE` case.

The step then exits `1`, halting the cycle before `fix` runs. An all-EMPTY
cycle (no validator produced output) is **not** fatal — it is treated as a
clean audit and proceeds.

#### Behavior summary

| v1 / v2 / v3 classification               | Outcome                                                  |
| ----------------------------------------- | -------------------------------------------------------- |
| All PARSED                                | Normal majority-vote merge                               |
| Mix of PARSED + EMPTY                     | Merge proceeds; EMPTY validators contribute zero votes   |
| Mix of PARSED + UNPARSEABLE               | WARN per unparseable validator + raw artifact; merge proceeds with parsed validators |
| Mix of PARSED + EMPTY + UNPARSEABLE       | WARN per unparseable validator + raw artifact; merge proceeds with the parsed validator(s) |
| All EMPTY                                 | Clean audit; merge yields zero confirmed findings; exit `0` |
| EMPTY + UNPARSEABLE, **no** PARSED        | FATAL diagnostic listing the unparseable artifact(s); exit `1` before `fix` |
| All UNPARSEABLE                            | FATAL diagnostic listing artifacts; exit `1` before `fix` |

The fatal gate is keyed on the **parsed count**, not on a literal "all three
unparseable" check: it fires whenever `parsed_count == 0` **and** at least one
validator is `UNPARSEABLE`. EMPTY validators are excluded from the gate (they
produced no output to parse), so an EMPTY + UNPARSEABLE mix with zero PARSED is
fatal, while an all-EMPTY cycle is not.

The deterministic majority-vote merge (`group_by` on `finding_id`, confirm when
`≥ validation_threshold` validators agree) is unchanged — it already tolerates
`{}` inputs via `?` — only the inputs are normalized.

#### Preserved security properties

The step retains all hardening introduced for earlier safety fixes:

- Validator content is captured via single-quoted, long-unique-delimiter
  heredocs (`<<'__AMPLIHACK_SAFE_HEREDOC_V1_TMPWRITE__'`), so it is never
  expanded, `eval`'d, or used in command substitution.
- Validator content reaches `jq`/`sed` only as file-path data
  (`--slurpfile`, `inputs`), never concatenated into a filter program or
  `--arg`, so it cannot inject `jq` filters.
- All temp files are created with `mktemp` and `chmod 600`; a `trap … EXIT`
  removes every temp file, including on the fatal exit path.
- A defensive input-size cap is applied before the Tier-3 brace scan to avoid
  pathological CPU/disk use on multi-megabyte blobs.

### verify-fixes: Git Diff Cross-Check (#646)

The `verify-fixes` step performs a two-layer verification:

1. **JSON-level check:** Parses the fix-agent's JSON output via `jq` to compare
   confirmed finding IDs against the list of applied fixes and skipped fixes.
   Any confirmed findings not accounted for trigger `VERIFY: FAIL` under the
   fix-all-per-cycle rule.

2. **Git diff cross-check:** After the jq parse, the step runs
   `git diff --quiet` to check for actual file modifications in the working
   tree. If the fix-agent claims `Fixed > 0` findings but `git diff --quiet`
   exits 0 (no changes), the step overrides the jq result with:

   ```
   VERIFY: FAIL — fix-agent claims N files fixed but git diff shows no file modifications
   ```

   This prevents a class of bugs where the fix-agent produces structurally
   valid JSON with `fixes_applied` entries but does not actually write changes
   to disk.

**Decision table:**

| jq: total_fixed | git diff --quiet exit | Result |
| ---------------- | --------------------- | ------ |
| 0                | 0 (clean)             | Normal jq-only evaluation (PASS if no unfixed, FAIL if unfixed exist) |
| > 0              | 0 (clean)             | `VERIFY: FAIL` — phantom fixes detected |
| > 0              | 1 (dirty)             | Normal jq-only evaluation (git confirms real changes) |
| 0                | 1 (dirty)             | Normal jq-only evaluation (changes from other sources ignored) |

### run-recursive-cycle: Subprocess Invocation (#646)

The `run-recursive-cycle` step re-invokes the quality-audit-cycle recipe for
the next cycle. It uses `type: bash` with a subprocess call instead of
`type: recipe` with `sub_context`.

**Why subprocess instead of `type: recipe`:**

The original `type: recipe` + `sub_context` dispatch did not propagate the
sub-recipe's output back to the parent recipe's context. The `output:` field
on a `type: recipe` step was silently ignored, causing the `final_report`
variable to remain empty. Converting to a `type: bash` step that invokes
`amplihack recipe run quality-audit-cycle` as a subprocess captures stdout
directly into `output: "final_report"`.

**Timeout guard:**

The subprocess call is wrapped in `timeout 900` (15 minutes) inside the bash
script to prevent unbounded recursion hangs. This is a **shell-level** timeout,
not a YAML-level `timeout:` field — the recipe does not use a YAML `timeout:`
on this step, which would violate the issue #439 no-per-step-timeout policy
(bash step YAML timeouts are restricted to network commands and must be ≥1800s).

On timeout, the subprocess receives SIGTERM, which the recipe runner handles
gracefully. The step exits non-zero and the recipe halts.

**cycle_history handling:**

The `cycle_history` context variable can contain large multi-line JSON. To avoid
shell injection and argument-length limits, the step writes `cycle_history` to
a temp file via a single-quoted heredoc, then passes it to the subprocess via
`-c "cycle_history=$(cat "$tmpfile")"`. The temp file is cleaned up via `trap`
on EXIT.

**Context variable propagation:**

All 13 context variables are forwarded to the subprocess via `-c key=value`
flags: `task_description`, `repo_path`, `target_path`, `min_cycles`,
`max_cycles`, `validation_threshold`, `severity_threshold`, `module_loc_limit`,
`fix_all_per_cycle`, `categories`, `output_dir`, `cycle_number`, and
`cycle_history`.

## Bash Step Safety

### The Problem

Bash steps receive context variables via `{{variable}}` template interpolation.
When a variable contains JSON (e.g., `{{validated_findings}}`), the raw JSON
is pasted into the bash script. Characters like `"`, `{`, `}`, `$`, and
backticks can be interpreted as bash syntax, causing errors like:

```
/bin/bash: line 3: crates/: Is a directory
/bin/bash: line 14: json: command not found
```

### Safe Pattern: Temp Files + Quoted Heredocs

Write JSON to temp files via single-quoted heredocs (`<<'EOF'`) so bash never
interprets the content, then read from the file in Python:

```yaml
- id: "verify-fixes"
  type: "bash"
  command: |
    _VALIDATED_TMPFILE=$(mktemp)
    _FIX_RESULTS_TMPFILE=$(mktemp)
    trap 'rm -f "$_VALIDATED_TMPFILE" "$_FIX_RESULTS_TMPFILE"' EXIT
    cat > "$_VALIDATED_TMPFILE" <<'__VALIDATED_EOF__'
    {{validated_findings}}
    __VALIDATED_EOF__
    cat > "$_FIX_RESULTS_TMPFILE" <<'__FIX_RESULTS_EOF__'
    {{fix_results}}
    __FIX_RESULTS_EOF__

    export VALIDATED_FILE="$_VALIDATED_TMPFILE"
    export FIX_RESULTS_FILE="$_FIX_RESULTS_TMPFILE"

    python3 - <<'PYEOF'
    import os
    from amplihack_utils.defensive import parse_llm_json
    with open(os.environ['VALIDATED_FILE']) as f:
        validated = parse_llm_json(f.read())
    # ... process safely in Python ...
    PYEOF
```

**Why this works:** The `<<'__VALIDATED_EOF__'` (single-quoted delimiter) prevents
bash from expanding `$variables` and backticks inside the heredoc. The JSON is
written to a temp file — never assigned to a shell variable — so special
characters like `{`, `}`, `$`, and backticks are inert. The `trap` ensures
cleanup on exit.

### Unsafe Pattern: Direct Interpolation in Bash

```yaml
# UNSAFE — template variables expand as bash code
- id: "bad-step"
  type: "bash"
  command: |
    FINDINGS={{validated_findings}}  # JSON becomes bash syntax
    echo "$FINDINGS" | python3 -c "import sys, json; ..."
```

This fails because `{{validated_findings}}` expands to raw JSON like
`{"validated": [...]}`, which bash interprets as command groups.

### Alternative: Inline Python via Quoted Heredoc

For simpler steps that only need one variable:

```yaml
- id: "recurse-decision"
  type: "bash"
  command: |
    _TMPFILE=$(mktemp)
    trap 'rm -f "$_TMPFILE"' EXIT
    cat > "$_TMPFILE" <<'__EOF__'
    {{validated_findings}}
    __EOF__

    python3 - <<'PYEOF'
    from amplihack_utils.defensive import parse_llm_json
    with open("'$_TMPFILE'") as f:
        data = parse_llm_json(f.read())
    PYEOF
```

> **Note:** The heredoc delimiter **must** be single-quoted (`<<'__EOF__'`).
> An unquoted delimiter (`<<__EOF__`) allows bash to expand `$` and backticks
> inside the heredoc, re-introducing the injection vulnerability.

## Invocation

Use `amplihack recipe run` from the CLI:

```bash
amplihack recipe run quality-audit-cycle \
  -c target_path="src/amplihack" \
  -c repo_path="." \
  -c min_cycles="3" \
  -c max_cycles="6" \
  --verbose
```

> **Do not use** `amplihack recipe execute` — this CLI form is deprecated.
> `amplihack recipe run` is the canonical invocation, consistent with
> `dev-orchestrator` and all other recipe workflows.

## See Also

- [How to Run a Quality Audit](../howto/run-quality-audit.md) — task-focused guide
- [Quality Audit Skill](../../amplifier-bundle/skills/quality-audit/SKILL.md) — skill
  activation and detection categories
