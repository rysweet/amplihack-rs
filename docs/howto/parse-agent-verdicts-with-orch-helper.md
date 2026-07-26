---
title: Read Agent Verdicts with orch helper
last_updated: 2026-07-26
review_schedule: quarterly
owner: workflow-team
---

# Read Agent Verdicts with `orch helper`

How to read a verdict, intent, or outcome signal from agent output in a recipe
step without brittle bash text-scraping. Use this when adding a new gate,
migrating an old grep/awk/jq step, or debugging a gate that degrades to its
safe default.

## Before you start

- `amplihack` is installed (`amplihack --version` to confirm).
- Your agent step captures output with `output:` and, ideally, `parse_json: true`.
- See the [Structured Verdict & Intent Parsing Reference](../reference/structured-verdict-parsing.md)
  for the full contract.

## The rule

Never `grep`, `awk`, `jq`, or `case` over agent prose to derive a control
signal. Route every agent-emitted field through:

```bash
amplihack orch helper extract-json | amplihack orch helper extract-field --field FIELD --default SAFE_DEFAULT
```

For verdicts, add `| amplihack orch helper normalise-verdict`.

> **Security:** agent output is untrusted. Pipe it in as data with
> `printf '%s' "$RAW"` and branch on the canonical token the helper returns.
> Never `eval`/`source` agent output or expand it into a command position.

## Read a verdict from agent output

```bash
VERDICT=$(printf '%s' "$RAW_AGENT_OUTPUT" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field verdict --default INSUFFICIENT_EVIDENCE \
  | amplihack orch helper normalise-verdict)

case "$VERDICT" in
  WORK_VERIFIED)        echo "gate passed" ;;
  HOLLOW_SUCCESS)       echo "gate failed (no real work)"; exit 1 ;;
  *)                    echo "insufficient evidence — degrading safely" ;;
esac
```

`normalise-verdict` collapses exact synonym tokens (`APPROVED`, `PASS`,
`SUCCESS` → `WORK_VERIFIED`; `FAILED`, `EMPTY` → `HOLLOW_SUCCESS`) and maps
anything else — including negation-adjacent labels like `UNVERIFIED` or
`NOT_APPROVED` — to `INSUFFICIENT_EVIDENCE`. Matching is exact-token equality,
not substring, so a failure label can never collide with a pass token. See the
[canonical mapping](../reference/structured-verdict-parsing.md#canonical-mapping).

## Read a non-verdict field (status, tree_id, etc.)

Drop the normaliser and pick your own safe default:

```bash
STATUS=$(printf '%s' "$SESSION_INFO" \
  | amplihack orch helper extract-json \
  | amplihack orch helper extract-field --field status --default unknown)
[ "$STATUS" = "ok" ] && echo "allowed"
```

## Move intent into a structured field

When a signal reflects *intent* (should we merge? is the goal met?), do not
guess it from prose with a regex. Have the upstream agent emit it as a
`parse_json` field and read it in an engine condition.

1. Extend the agent's JSON contract, e.g. the classifier emits:

   ```json
   { "task_type": "Development", "no_merge": true }
   ```

2. Read it with a `condition:` instead of a bash regex:

   ```yaml
   condition: "no_merge == true"
   ```

For loop control on a status field:

```yaml
condition: "reflection_1.goal_status == 'PARTIAL' or reflection_1.goal_status == 'NOT_ACHIEVED'"
```

Use `==` equality, never substring `in`, so a stray word in narrative prose
cannot false-trigger the branch.

## Emit JSON from `session-tree register`

If you need `tree_id` / `depth` in a script, request JSON and reuse the same
pipeline instead of scraping the text line:

```bash
INFO=$(amplihack session-tree register --json)
TREE_ID=$(printf '%s' "$INFO" | amplihack orch helper extract-field --field tree_id --default "")
DEPTH=$(printf '%s'  "$INFO" | amplihack orch helper extract-field --field depth   --default 0)
```

The default text output (`TREE_ID=… DEPTH=…`) is unchanged for existing
consumers; `--json` is opt-in.

## Verify your change

Run the CLI helper tests and the recipe's own test after each migration:

```bash
cargo test -p amplihack-cli
# then the recipe's shell/integration test, e.g.
amplifier-bundle/recipes/tests/test-issue-962-step17a-testing-evidence-gate.sh
```

## Keep fail-safes intact

Migrating the parsing mechanism must not change behaviour:

- Keep the same safe default the old code used (`INSUFFICIENT_EVIDENCE`,
  `NEEDS_ATTENTION`, `unknown`, `no_merge=false`).
- Keep every `WARNING` stderr message and `# issue #NNN` defensive branch.
- Keep documented prose fail-safe tokens where a test requires them (for
  example the prose `VERDICT: FAILED` fatal token in
  `workflow-pr-review.yaml`, issue #962).

## Do NOT convert these

These are correct already — leave them alone:

- `jq` over `gh --json` output.
- SHA / ref-format / charset validation regex.
- Greps of `gh` / `git` **stderr** for rate-limit / auth / transient errors.
- Token-redaction `sed`, slug-building `tr`, the `grep -qF` sentinel.

## Related

- [Structured Verdict & Intent Parsing Reference](../reference/structured-verdict-parsing.md)
- [`amplihack orch run` and helpers](../reference/orch-run-command.md)
- [Run a Recipe End-to-End](run-a-recipe.md)
