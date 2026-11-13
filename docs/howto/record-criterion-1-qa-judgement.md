---
title: Record a Criterion-1 QA Judgement by Hand
description: Produce qa-inputs.json, write a judgement and record it with qa-record, as the criterion-1 agent step does.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Record a Criterion-1 QA Judgement by Hand

Use this to reproduce what the criterion-1 agent step does in an auto-drive
round, for example when checking a refused record or a hand-off, or to keep
a record of a manual `/merge-ready` run. A manual run does not require it. Field
meanings are in [Auto-drive QA](../reference/auto-drive-qa.md).

## Before you start

- A worktree at the PR head, and an installed `amplihack`.
- A private state directory: owned by you, not writable by group or others.

```bash
cd ~/src/my-pr-worktree
export STATE="$HOME/.amplihack/work/qa-check"
mkdir -p "$STATE" && chmod 700 "$STATE"
export AUTODRIVE_QA_EVIDENCE="$STATE/qa-evidence.json"
```

## 1. Measure the inputs

In a drive, the recipe sets the base and head from the observed PR. By hand,
you set them. `AUTODRIVE_QA_BASE` is the PR's base branch name without
`origin/`; the script reads `refs/remotes/origin/<base>`, so fetch it first.
`AUTODRIVE_QA_HEAD` defaults to `HEAD`.

```bash
git fetch origin main
REPO_PATH="$PWD" AUTODRIVE_QA_BASE=main AUTODRIVE_ROUND_LABEL=round-1 \
  bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh"
jq '{repo_type, docs_only, changed_files, declared_qa: (.declared_qa | del(.log_tail))}' \
  "$STATE/qa-inputs.json"
```

Output for a PR that changes only `docs/install.md` in a Node repository:

```json
{
  "repo_type": "node",
  "docs_only": "true",
  "changed_files": ["docs/install.md"],
  "declared_qa": {"source": "detected", "dir": ".", "command": "npm test",
    "status": "ran", "exit_status": 0, "cause": "", "candidates": "", "ci_skip": "no-match"}
}
```

The script runs the declared command; it does not run any scenario. With
`AUTODRIVE_QA_BASE` unset, or a base that is not fetched, `changed_files` is
empty and `docs_only` is `unknown`. See
[base and head](../reference/auto-drive-qa.md#base-and-head).

## 2. Write the judgement

For a documentation-only change:

```bash
cat > "$STATE/judgement.json" <<'EOF'
{
  "qa_status": "PASS",
  "qa_judgement": "not_user_facing",
  "qa_judgement_reason": "Only installation prose changes; no code, configuration or build input.",
  "qa_cited_files": ["docs/install.md"],
  "qa_scenarios": [],
  "qa_iterations": [],
  "qa_handoff": null
}
EOF
```

For an unreachable service, a hand-off:

```json
{
  "qa_status": "HANDOFF",
  "qa_judgement": "user_facing",
  "qa_judgement_reason": "The login journey needs the staging identity service, which refuses connections.",
  "qa_cited_files": ["src/auth/login.rs"],
  "qa_scenarios": [{"path": "tests/agentic/login.yaml", "written_in_pr": true, "commit": "3b9e2f1", "result": "fail"}],
  "qa_iterations": [{"attempt": 1, "failing_scenario": "tests/agentic/login.yaml", "cause": "environment", "fix_commit": null, "result": "fail"}],
  "qa_handoff": {"step_id": "step-02-qa-team-scenarios", "failure": "connection refused: identity service",
    "choice": "handoff", "reason": "service unreachable from CI and isolation",
    "owner": "repository maintainer", "needs": "restore the staging identity service or name another endpoint"}
}
```

When the declared QA command failed and the PR caused it, record the
attempt with `failing_scenario` set to `null`:

```json
{"attempt": 1, "failing_scenario": null, "cause": "product", "fix_commit": "7c41d0a", "result": "pass"}
```

For a user-facing PR, a passing declared command is not enough for `PASS`:
at least one scenario must have `result` `pass`, or the record is refused
with `user-facing-needs-passing-scenario`.

## 3. Record it

```bash
amplihack orch helper autodrive qa-record \
  --inputs "$STATE/qa-inputs.json" --repo "$PWD" --out "$AUTODRIVE_QA_EVIDENCE" \
  < "$STATE/judgement.json"
echo "exit $?"
jq -r '.qa_status, .qa_reason, .head_sha' "$AUTODRIVE_QA_EVIDENCE"
```

Output for the documentation-only judgement:

```text
exit 0
PASS

9f2c1e4b7a0d3f6e8c5b2a1d4e7f0a3c6b9d2e5f
```

`qa_reason` is empty for `PASS`. `head_sha` is the worktree's `HEAD`.

## When the record is refused

Exit 2 means nothing was written. The message names the rule and the field:

```text
qa-record: refused: handoff-incomplete at qa_handoff.owner
exit 2
```

Correct that field and record again. The rules are listed under
[shape rules](../reference/auto-drive-qa.md#shape-rules).

Exit 1 is an environment problem: a missing or symlinked inputs file, a
repository with no `HEAD`, or a state directory writable by group or
others. Fix the environment; the judgement is not at fault.

## Clean up

```bash
rm -f "$HOME/.amplihack/work/qa-check/judgement.json" \
  "$HOME/.amplihack/work/qa-check/qa-inputs.json" \
  "$HOME/.amplihack/work/qa-check/qa-evidence.json"
rmdir "$HOME/.amplihack/work/qa-check"
```
