# Copy-Ready Templates

These match the repo's ACTUAL recipe shape (see `amplifier-bundle/recipes/
default-workflow.yaml` and `smart-classify-route.yaml`). Each protection is annotated
with its "why" so copiers do not strip it as boilerplate. Use `{{placeholders}}` —
never hardcode host-specific paths.

## Template A — Recipe skeleton (agentic step → tool writes typed record → thin-rail read)

```yaml
name: "{{workflow_name}}"
description: "{{one-line goal}}"
version: "1.0.0"
author: "{{author}}"
tags: ["{{tag}}"]

# NO-TIMEOUT POLICY: no per-step wall-clock `timeout:` on agent steps (AP-3).
# Idle/liveness detection guards agentic steps; shell `timeout` only guards
# external network commands inside bash steps.

recursion:
  max_depth: 6
  max_total_steps: 80

context:
  # Small, non-secret inputs only (AP-4: no large payloads via env/argv).
  repo_path: "."
  input_ref: ""          # path/id to a file the steps read; not the payload itself
  record_path: ""        # where the typed record is written/read

steps:
  # (a) AGENTIC STEP — judgment behind a prompt (Step 3a)
  - id: "decide"
    type: "agent"
    agent: "amplihack:core:architect"
    prompt: |
      === [RECIPE PROGRESS] Step: decide ===
      {{See Template B for the prompt body}}
      Emit ONLY the typed record described in Template C. Do not print prose.
    output: "decision_record"

  # (b) TOOL STEP — deterministic side effect: write the typed record 0o600 (AP-1)
  - id: "persist-record"
    type: "bash"
    command: |
      set -euo pipefail
      REC="${RECORD_PATH:?record_path required}"
      # WHY 0o600 at creation (not chmod-after): closes the TOCTOU window (AP-1).
      ( umask 177; : > "$REC" )          # create owner-only, atomically empty
      # WHY here-doc/stdin, not argv: avoids E2BIG + /proc leak (AP-4).
      cat > "$REC" <<'RECORD'
      {{typed record JSON — see Template C}}
      RECORD
    output: "record_written"

  # (c) THIN RAIL: verify the record and report what was found (Step 3c / Template D).
  # A later step routes on `route_result.routed`, and only when `integrity` is `ok`.
  - id: "route"
    type: "bash"
    command: |
      set -euo pipefail
      # Capture the reader's reason as evidence. Under `set -e` an unguarded call
      # would end the run, and the reason would survive only in the log, where the
      # recovery step cannot read it.
      ( umask 177; : > "${EVIDENCE_PATH:?}" )
      if decision=$("{{repo_path}}/scripts/read_record.sh" "${RECORD_PATH:?}" 2>>"$EVIDENCE_PATH"); then
        integrity="ok"
      else
        decision="unverified"; integrity="failed"
        echo "ERROR: record integrity check failed; evidence at $EVIDENCE_PATH" >&2
      fi
      jq -cn --arg d "$decision" --arg i "$integrity" --arg e "$EVIDENCE_PATH" \
        '{routed:$d, integrity:$i, evidence_path:$e}'
    output: "route_result"
    parse_json: true
```

## Template B — Inline prompt skeleton

The repo's canonical prompt shape is an **inline `prompt: |` block** inside the recipe
step (as in `smart-classify-route.yaml`). There is no separate `prompts/*.yaml` asset
format in this repo; keep prompts inline.

```yaml
    prompt: |
      === [RECIPE PROGRESS] Step: {{step_id}} ===

      You are performing ONE judgment step. Do NOT implement, build, or run tools.

      **INPUT**: {{input_ref}} (read the file at this path; it is not inlined here — AP-4)

      **TASK**: {{state the single decision — classify / prioritize / interpret}}

      **CLASSES / RULES**:
      - {{class or rule 1}}
      - {{class or rule 2}}

      **OUTPUT**: Emit ONLY a typed record matching this schema (no prose, no fences):
      {{Template C schema}}
```

> Aside: Simard sometimes stores prompts as separate asset files
> (`operator-liaison.yaml`, `merge-readiness-judge.yaml`). That variant is fine there,
> but this repo verifies only the inline form — use inline here.

## Template C — Typed-record schema

```json
{
  "schema": "{{workflow_name}}.decision",
  "schema_version": 1,
  "nonce": "{{run-unique nonce — freshness/replay guard, AP-1}}",
  "created_at": "{{RFC3339 timestamp}}",
  "decision": "{{one of the enumerated classes}}",
  "confidence": "{{low|medium|high}}",
  "rationale": "{{short, for observability only — never parsed for control flow}}"
}
```

Control flow reads ONLY typed fields (`decision`, `schema_version`, `nonce`). `rationale`
is human-facing and must never drive routing (that would re-introduce AP-1).

## Template D: Record-integrity reader (`scripts/read_record.sh`, the thin rail)

The rail validates ownership, freshness and parseability, and it never proceeds on a
record that fails one of those checks. That part does not soften. Its nonzero exit is a
report to the step that invoked the rail, not a gate that ends the run: the caller
captures the reason as evidence (Template A's `route` step) and an agent step reads that
evidence, decides what went wrong, and either fixes the cause and revalidates or says
plainly what is blocked. The reason must not be left only on stderr. A reader failure
that reaches the agent as a bare "unreadable" cannot be told apart from a replayed
nonce, a missing file, or a record owned by another run.

```bash
#!/usr/bin/env bash
# Thin rail: read a typed record and report on ANY doubt (AP-1).
set -euo pipefail
REC="${1:?record path required}"

# WHY each guard: any failure => report and hand to an agent step; never proceed on default data.
[ -f "$REC" ]                        || { echo "record missing: $REC" >&2; exit 1; }
perms=$(stat -c '%a' "$REC" 2>/dev/null || stat -f '%Lp' "$REC")  # GNU || BSD/macOS
[ "$perms" = "600" ]                 || { echo "record not 0o600 (got $perms)" >&2; exit 1; }
jq -e . "$REC" >/dev/null            || { echo "record not valid JSON" >&2; exit 1; }
ver=$(jq -r '.schema_version' "$REC")
[ "$ver" = "1" ]                     || { echo "unexpected schema_version: $ver" >&2; exit 1; }
[ "$(jq -r '.nonce' "$REC")" = "${EXPECTED_NONCE:?}" ] \
                                     || { echo "stale/replayed record (nonce mismatch)" >&2; exit 1; }

decision=$(jq -r '.decision' "$REC")   # route ONLY on the typed field
# Optional allowlist (defense-in-depth): when ALLOWED_DECISIONS is set (space-separated),
# report any decision outside it here, before the caller ever routes on it.
if [ -n "${ALLOWED_DECISIONS:-}" ]; then
  case " $ALLOWED_DECISIONS " in
    *" $decision "*) : ;;
    *) echo "decision not in allowlist: $decision" >&2; exit 1 ;;
  esac
fi
echo "$decision"
```

**PR-review checklist (paste into the PR):**
- [ ] No example shows a fail-OPEN handoff (proceeding on missing/partial record).
- [ ] Every reader failure reaches an agent step with its own reason as evidence, not
      as a bare "unreadable" and not only in the log.
- [ ] No example uses `chmod` after write (must be `0o600` at creation).
- [ ] No large/sensitive payload passed via argv or env (stdin/file only).
- [ ] No control flow keys off `rationale`/free-text (only typed fields).
