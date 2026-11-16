---
title: Record a Criterion-1 QA Judgement by Hand
description: Measure the QA facts for a PR, judge criterion 1 from them and the diff, and write the judgement up - what the criterion-1 agent step does inside an auto-drive round.
last_updated: 2026-11-16
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

In a drive, `step-02a-qa-inputs` resolves the PR's own base: an operator's
`AUTODRIVE_QA_BASE` first, then the base branch the drive was launched
against, then `gh pr view <pr> --json baseRefName`. Where none of the three
answers it passes no base at all rather than guessing one, and the record says
which answered in `qa_base_source` — `operator`, `recipe`, `pr-base` or
`none`. It does not consult the clone's own default branch, which is the PR's
base only where the PR targets it. The order is in
[resolving the measurement base](../reference/auto-drive-qa.md#resolving-the-measurement-base).
By hand you set the base yourself, and you can simply get it right.
`AUTODRIVE_QA_BASE` is the PR's base branch name without `origin/`; the tool
reads `refs/remotes/origin/<base>`, so fetch it first. `AUTODRIVE_QA_HEAD`
defaults to `HEAD`.

```bash
git fetch origin main
REPO_PATH="$PWD" AUTODRIVE_QA_BASE=main AUTODRIVE_QA_BASE_SOURCE=operator \
  AUTODRIVE_ROUND_LABEL=round-1 \
  bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh" >/dev/null
jq '{qa_status, qa_reason, docs_only, qa_repo_type, qa_command_paths,
     gadugi_required, gadugi_scenario_dir, gadugi_scenario_dir_state,
     qa_scenarios, gadugi_status, qa_command, qa_exit_code}' \
  "$AUTODRIVE_QA_EVIDENCE"
```

Output for a Node PR that changes `src/auth/login.ts` and
`docs/install.md`:

```json
{
  "qa_status": "PASS",
  "qa_reason": "",
  "docs_only": "false",
  "qa_repo_type": "node",
  "qa_command_paths": "docs/install.md src/auth/login.ts",
  "gadugi_required": "true",
  "gadugi_scenario_dir": "tests/agentic",
  "gadugi_scenario_dir_state": "detected",
  "qa_scenarios": "tests/agentic/install.yaml tests/agentic/login.yaml",
  "gadugi_status": "FOUND",
  "qa_command": "npm test",
  "qa_exit_code": "0"
}
```

Node is the only repository type whose detection requires gadugi scenarios,
so this is the one shape in which the record names a detected scenario
directory. In amplihack-rs, and for any documentation-only PR, expect
`gadugi_required` `false`, `gadugi_status` `NOT_REQUIRED`, an empty
`gadugi_scenario_dir` and `gadugi_scenario_dir_state` `not-consulted`: the
tool looked for no directory, and you find it yourself.

Use the PR's own base, not `main`, for a PR into any other branch. Measured
against `main`, a PR into `release/1.2` records that release branch's commits
as its own change, reads `.amplihack/qa.toml` from the wrong merge base, and
reports a documentation-only flag covering commits the PR does not carry:

```bash
git fetch origin release/1.2
REPO_PATH="$PWD" AUTODRIVE_QA_BASE=release/1.2 AUTODRIVE_QA_BASE_SOURCE=operator \
  AUTODRIVE_ROUND_LABEL=round-1 \
  bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh" >/dev/null
```

The tool runs the declared QA command. It runs no scenario, and it issues no
verdict you may rely on: `qa_status` describes the measurement only. With
`AUTODRIVE_QA_BASE` unset, or a base that is not fetched,
`qa_command_paths` is `(unknown)` and `docs_only` is `unknown`. The record
names the base it used in `qa_base` and how it was reached in
`qa_base_source`; a base you set by hand with no source beside it records
`operator`, which is what you are. See
[operator environment](../reference/auto-drive-qa.md#operator-environment).

A record you find in a drive's state directory is not necessarily the latest
round's. Every round of a drive uses the same path, and only a round that
measured writes it: a round that could not measure prints its
`qa-measurement-unavailable` object and leaves the file untouched. So check
the round's own output before you trust the file's dates or contents as this
round's; see
[the record file](../reference/auto-drive-qa.md#the-record-file).

## 2. Judge whether the change is user-facing

Read the diff, not only the measurement:

```bash
git diff --stat "$(git merge-base origin/main HEAD)" HEAD
```

`docs_only` binds you one way only. `false` means you may not judge the PR
not user-facing. `true` is advisory: judge the change user-facing anyway
when it alters anything a user, operator or calling program sees, including
agent instructions and build inputs.

What a drive's round accepts afterwards follows from the measured value:

| Measured `docs_only` | A `not_user_facing` judgement |
| --- | --- |
| `true` | Met, where the reason cites files that appear in the diff. |
| `false` | Not met, whatever the reason cites. |
| `unknown` | Met only where the notes also name what the change set was judged from, such as the diff read against the recorded base. Nothing was measured, so the citation has nothing to be checked against. |
| absent | Read exactly as `unknown`. The `qa-measurement-unavailable` record carries no `docs_only` field, so the notes-naming rule applies there too and criterion 1 stays reachable. |

## 3. Exercise a user-facing change

Running a scenario runs the branch's own code on your host, so do this only
for a branch you own. A PR from a fork you do not control is the operator's
to judge, not yours.

Run only the scenarios this PR changed or added, one at a time, each staged
alone outside the repository. `gadugi-test --scenario` matches a substring
of the scenario's `name:` field, not a path, and concurrent scenarios in one
directory share a single CLI runner where the first to finish ends the
others' processes (gadugi-agentic-test #207) — so `-d` must point at the
staging directory, never at `tests/agentic` itself. Validate before you run:

```bash
export TMPDIR="$STATE/tmp" && mkdir -p "$TMPDIR"
stage="$(mktemp -d "$TMPDIR/scenario.XXXXXX")"
cp -- tests/agentic/login.yaml "$stage/"
gadugi-test validate -d "$stage"
gadugi-test run -d "$stage" --scenario "upgrade from 1.2 to 1.3"
rm -rf -- "$stage"
```

The `--scenario` argument is the scenario's own `name:` value, not its file
name. Give any `tmux` the scenario needs its own socket, with
`tmux -S "$STATE/tmux.sock" …`. Never run the whole suite on a shared host,
and in amplihack-rs never run `tests/gadugi/*` on the host: CI runs those.
When a scenario fails, classify the cause as `product`, `scenario` or
`environment` — a `gadugi-test validate` failure is a `scenario` cause — fix
the code or the scenario, commit, and run again. Never weaken, skip or
delete a failing scenario to reach a pass.

## 4. Write the judgement up

The judgement is prose, in the merge-ready report for a manual run or in the
PR comment for a drive. Keep a copy outside the repository so it is never
committed:

A user-facing judgement names the journeys, the scenarios with their paths
and results, whether this PR wrote them and their commits, and each failure
with its cause and what you did. For the PR measured above:

```bash
cat > "$STATE/judgement.md" <<'EOF'
Criterion 1: user-facing. src/auth/login.ts changes the login flow, so
docs_only=false binds.

tests/agentic/login.yaml (written in this PR, commit 3b9e2f1): pass, staged
alone and run by name. Attempt 1 failed on a product cause - the token was
not refreshed; fixed in commit 7c41d0a and the scenario passed on attempt 2.
The declared command `npm test` passed (exit 0).
EOF
```

A PR that changes documentation and nothing else is judged the other way.
Its record reads docs_only=true with gadugi_status=NOT_REQUIRED, and the
reason must cite files that appear in the diff:

```text
Criterion 1: not user-facing.

The diff changes installation prose only: docs/install.md. No code,
configuration or build input changes, so no scenario exercises it.
Measured docs_only=true; the declared command `npm test` passed (exit 0).
```

Leave raw command output, log tails, environment values, any credential,
token, key or connection string even where it looks already redacted, file
contents from outside the diff, `~/.claude` and `~/.amplihack` paths and
email addresses out of it. Name the scenario, the file and the outcome
instead.

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
| the record is absent, unparseable, empty of fields, or carries an unknown value; the resolved tools directory lies inside the worktree under review; or `qa_reason` `qa-command-missing` with `qa_command_paths` `(unknown)`, `docs_only` `unknown` and `qa_repo_type` `unknown` | `qa-measurement-unavailable` | Judges from the diff and the scenarios. The record carries no `docs_only`, so section 2's absent row applies. |
| `gadugi_status` `NOT_INSTALLED`, `qa_reason` `qa-command-not-installed` | `qa-tool-not-installed:<tool>` | Uses other evidence, or hands off naming the tool. Installs nothing. |
| `qa_reason` `qa-command-failed` | `qa-command-failed:<name>` | Decides whether this PR caused the failure. |

None of them is a per-round QA failure, and none of them ends a run.

Two record shapes are **not** blockers, however they look:

- `gadugi_scenario_dir_state` `not-consulted` with an empty
  `gadugi_scenario_dir`. This is the normal reading outside Node, including
  in amplihack-rs. Find the scenario directory yourself.
- `qa_reason` `qa-command-missing` where `qa_command_paths` holds real paths
  and `docs_only` is `true` or `false`. The change was measured; the token
  says only that this repository declares no suite command, or that
  `AUTODRIVE_QA_DIR` does not exist. `qa_summary` names which.

## 7. Tell the trust-root and record-shape causes apart

Several conditions end in `qa-measurement-unavailable`: an absent or
unparseable record, a value outside its set, the broken-scope signature in
the tables above, and the two checked here. A judgement naming the blocker
says which of them applies only once you have run the checks yourself. The
two below are the ones you can reproduce by hand.

The tool is pinned to the installed bundle, never the worktree. A drive
whose tools directory resolved inside the worktree it was measuring supplies
no record and no guidance path, because the PR under review would otherwise
be supplying the rules that judge it. Resolve both paths and compare them
the way the step does:

```bash
T=$( cd "$AMPLIHACK_HOME/amplifier-bundle/tools" && pwd -P )
W=$( pwd -P )
case "$T" in "$W" | "$W"/*) echo contained ;; *) echo separate ;; esac
# separate
```

`contained` is the trust-root cause. Run the measurement from the installed
bundle instead; a copy inside the worktree is never the one to use.

The record shape check requires an object with at least one field. Output of
`{}` counts as no record, rather than as a record whose fields all read as
absent. `jq -e` reports the shape through its exit status, so output that
does not parse at all gives `unavailable` rather than a parse error:

```bash
jq -e 'type == "object" and length > 0' "$AUTODRIVE_QA_EVIDENCE" \
  >/dev/null 2>&1 && echo usable || echo unavailable
# usable
```

A bundle installed before this guidance shipped lacks
`skills/merge-ready/criterion-1-qa-judgement.md`. A drive launched from it
names that missing file and starts nothing; `amplihack install` restores it.

## 8. Clean up

```bash
rm -rf "$STATE/tmp"
rm -f "$STATE/judgement.md" "$STATE/qa-evidence.json" "$STATE/tmux.sock"
rmdir "$STATE"
```

Remove any `logs/` and `outputs/` directories a scenario run created in the
worktree, so the artifact guard does not stop your next commit.
