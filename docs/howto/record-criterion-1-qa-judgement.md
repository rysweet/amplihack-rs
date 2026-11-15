---
title: Record a Criterion-1 QA Judgement by Hand
description: Measure the QA facts for a PR, judge criterion 1 from them and the diff, and write the judgement up - what the criterion-1 agent step does inside an auto-drive round.
last_updated: 2026-11-15
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Record a Criterion-1 QA Judgement by Hand

Use this to reproduce what the criterion-1 step does in an auto-drive round:
checking a judgement a round recorded, working out why a blocker was named,
or keeping a record of a manual `/merge-ready` run. A manual run does not
require it. Field meanings are in
[Auto-drive QA](../reference/auto-drive-qa.md); the rules the judgement
follows are in
[criterion-1-qa-judgement.md](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md).

## Before you start

- A worktree at the PR head, and an installed `amplihack`.
- A private state directory: owned by you, mode `0700`.

```bash
cd ~/src/my-pr-worktree
export STATE="$HOME/.amplihack/work/qa-check"
mkdir -p "$STATE" && chmod 700 "$STATE"
export AUTODRIVE_QA_EVIDENCE="$STATE/qa-evidence.json"
```

## 1. Measure the facts

In a drive, the recipe sets the base and head from the observed PR. By hand,
you set them. `AUTODRIVE_QA_BASE` is the PR's base branch name without
`origin/`; the tool reads `refs/remotes/origin/<base>`, so fetch it first.
`AUTODRIVE_QA_HEAD` defaults to `HEAD`.

```bash
git fetch origin main
REPO_PATH="$PWD" AUTODRIVE_QA_BASE=main AUTODRIVE_ROUND_LABEL=round-1 \
  bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh" >/dev/null
jq '{qa_status, qa_reason, docs_only, qa_repo_type, qa_command_paths,
     gadugi_scenario_dir, gadugi_scenario_dir_state, qa_scenarios,
     gadugi_status, qa_command, qa_exit_code}' "$AUTODRIVE_QA_EVIDENCE"
```

Output for a PR that changes only `docs/install.md` in a Node repository:

```json
{
  "qa_status": "PASS",
  "qa_reason": "",
  "docs_only": "true",
  "qa_repo_type": "node",
  "qa_command_paths": "docs/install.md",
  "gadugi_scenario_dir": "tests/agentic",
  "gadugi_scenario_dir_state": "detected",
  "qa_scenarios": "install.yaml login.yaml",
  "gadugi_status": "FOUND",
  "qa_command": "npm test",
  "qa_exit_code": "0"
}
```

The tool runs the declared QA command. It runs no scenario, and it issues no
verdict you may rely on: `qa_status` describes the measurement only. With
`AUTODRIVE_QA_BASE` unset, or a base that is not fetched,
`qa_command_paths` is `(unknown)` and `docs_only` is `unknown`; see
[base and head](../reference/auto-drive-qa.md#base-and-head).

## 2. Judge whether the change is user-facing

Read the diff, not only the measurement:

```bash
git diff --stat "$(git merge-base origin/main HEAD)" HEAD
```

`docs_only` binds you one way only. `false` means you may not judge the PR
not user-facing. `true` is advisory: judge the change user-facing anyway
when it alters anything a user, operator or calling program sees, including
agent instructions and build inputs.

## 3. Exercise a user-facing change

Run only the scenarios this PR changed or added, in isolation:

```bash
export TMPDIR="$STATE/tmp" && mkdir -p "$TMPDIR"
gadugi-test run -d tests/agentic --scenario "upgrade from 1.2 to 1.3"
```

Give any `tmux` the scenario needs its own socket, with
`tmux -S "$STATE/tmux.sock" …`. Never run the whole suite on a shared host,
and in amplihack-rs never run `tests/gadugi/*` on the host: CI runs those.
When a scenario fails, classify the cause as `product`, `scenario` or
`environment`, fix the code or the scenario, commit, and run again. Never
weaken, skip or delete a failing scenario to reach a pass.

## 4. Write the judgement up

The judgement is prose, in the merge-ready report for a manual run or in the
PR comment for a drive. Keep a copy outside the repository so it is never
committed:

```bash
cat > "$STATE/judgement.md" <<'EOF'
Criterion 1: not user-facing.

The diff changes installation prose only: docs/install.md. No code,
configuration or build input changes, so no scenario exercises it.
Measured docs_only=true; the declared command `npm test` passed (exit 0).
EOF
```

A user-facing judgement names the journeys, the scenarios with their paths
and results, whether this PR wrote them and their commits, and each failure
with its cause and what you did:

```text
Criterion 1: user-facing. src/auth/login.rs changes the login flow.
tests/agentic/login.yaml (written in this PR, commit 3b9e2f1): pass.
Attempt 1 failed on a product cause - the token was not refreshed; fixed in
commit 7c41d0a and the scenario passed on attempt 2.
```

Leave raw command output, log tails, environment values, file contents from
outside the diff, `~/.claude` and `~/.amplihack` paths and email addresses
out of it. Name the scenario, the file and the outcome instead.

## 5. Hand off when only a person can act

A hand-off is the only criterion-1 path that ends a drive. Record the six
fields of [step failure recovery](../reference/step-failure-recovery.md):

```text
step_id: step-02-qa-team-scenarios
failure: cannot connect to the staging identity service
choice:  handoff
reason:  the login scenario needs the staging identity service, unreachable
         from CI and from isolation
owner:   platform team
needs:   restore the staging identity service, or name another endpoint for
         the login scenario
```

`owner` is a person or a team name, never an email address.

## 6. Read a named blocker

Where the measurement names a fact you have to work around, the judgement
names the blocker and carries on:

| Fact in the record | Blocker | What the judgement does |
| --- | --- | --- |
| `gadugi_scenario_dir_state` `operator-missing` | `qa-scenario-dir-missing:<path>` | Falls back to a tracked `tests/agentic`, `tests/gadugi/scenarios` or `scenarios` inside the repository, recording both paths; otherwise hands off; otherwise records not user-facing with cited files. |
| the record is absent, unparseable or carries an unknown value | `qa-measurement-unavailable` | Judges from the diff and the scenarios. |
| `gadugi_status` `NOT_INSTALLED`, `qa_reason` `qa-command-not-installed` | `qa-tool-not-installed:<tool>` | Uses other evidence, or hands off naming the tool. Installs nothing. |
| `qa_reason` `qa-command-failed` | `qa-command-failed:<name>` | Decides whether this PR caused the failure. |

None of them is a per-round QA failure, and none of them ends a run.

## 7. Clean up

```bash
rm -rf "$STATE/tmp"
rm -f "$STATE/judgement.md" "$STATE/qa-evidence.json" "$STATE/tmux.sock"
rmdir "$STATE"
```

Remove any `logs/` and `outputs/` directories a scenario run created in the
worktree, so the artifact guard does not stop your next commit.
