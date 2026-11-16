---
title: Auto-drive QA (Criterion 1)
description: How criterion 1 is judged inside auto-drive - the measured QA record, the declared QA command, the named recoverable blockers, and the recorded judgement the merge round reads.
last_updated: 2026-11-15
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive QA (Criterion 1)

Inside auto-drive, criterion 1 of [merge-ready](../../amplifier-bundle/skills/merge-ready/SKILL.md)
is a judgement an agent step makes and records, not a table of hard stops.
A trusted shell tool measures facts about the PR and writes them to the QA
record. The agent reads those facts with the PR diff, the linked issue and
the existing scenarios, decides whether the PR changes behaviour a user
sees, exercises the change outside-in where it does, loops on failures, and
writes the judgement up in prose. Nothing parses that write-up into a
criterion-1 verdict.

QA runs against the selected PR head in its linked worktree. The written
guidance the judgement follows is
[criterion-1-qa-judgement.md](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md),
read only from the pinned bundle, never from the worktree under review: a PR
cannot change the guidance that judges it.

The launch checks that guidance file into its trusted assets alongside the
merge-ready skill and the auto-drive tools, so a bundle installed before the
guidance existed names the missing file at launch and starts no drive that
would judge criterion 1 without it. Reinstalling with `amplihack install`
is the remedy; nothing falls back to the repository's own copy.

For the evidence and finalization contracts shared by all criteria, see
[current behaviour and authority](auto-drive-to-merge.md#current-behavior-and-authority).
This reference does not establish execution coverage for the
[target host/runtime matrix](auto-drive-to-merge.md#target-compatibility-matrix).

## Contents

- [How a round judges criterion 1](#how-a-round-judges-criterion-1)
  - [Resolving the measurement base](#resolving-the-measurement-base)
  - [The record file](#the-record-file)
  - [Light drives](#light-drives)
- [What the judgement records](#what-the-judgement-records)
- [Named recoverable blockers](#named-recoverable-blockers)
- [The measured record](#the-measured-record)
  - [The status values](#the-status-values)
    - [Why `FAIL` kept its name](#why-fail-kept-its-name)
    - [Telling the three `qa-command-missing` causes apart](#telling-the-three-qa-command-missing-causes-apart)
- [Source precedence](#source-precedence)
- [Operator environment](#operator-environment)
- [Repository configuration](#repository-configuration)
  - [Declaring how criterion 1 is met](#declaring-how-criterion-1-is-met)
    - [How merge-ready agents use the declaration](#how-merge-ready-agents-use-the-declaration)
- [CI selection](#ci-selection)
- [Manifest detection and scope](#manifest-detection-and-scope)
- [Scenario judgement](#scenario-judgement)
- [Documentation-only measurement](#documentation-only-measurement)
- [The record hash](#the-record-hash)
- [How the merge round reads the judgement](#how-the-merge-round-reads-the-judgement)
- [Retired stop reasons](#retired-stop-reasons)
- [Evidence](#evidence)

## How a round judges criterion 1

`autodrive-merge-evidence.yaml` runs two steps for criterion 1, in this
order:

| Step | Type | What it does |
| --- | --- | --- |
| `step-02a-qa-inputs` | bash | Resolves the pinned tools directory from `autodrive_tools_dir` or `$AMPLIHACK_HOME`, never through the git toplevel and never from the worktree under review. It resolves that directory and the worktree with `pwd -P` and checks that the first does not lie inside the second, so a PR carrying its own `amplifier-bundle/tools` cannot supply the tool or the guidance that judges it. It then resolves a measurement base — the operator's `AUTODRIVE_QA_BASE`, else the clone's own `origin/HEAD`, else the literal `main`, as [resolving the measurement base](#resolving-the-measurement-base) sets out — and runs the trusted `autodrive_qa_evidence.sh` with `AUTODRIVE_QA_BASE` set to it. That tool selects and runs the declared QA command, measures the changed files, the documentation-only flag, the repository type and the scenario directory with its tracked scenarios, and writes [the measured record](#the-measured-record). The step checks the shape of what the tool wrote before it uses it: output that is not an object carrying at least one field is treated as no record at all. It runs no scenario and issues no verdict any consumer conditions on. The step prints that record with one field added, `qa_guidance`: the absolute path of the pinned bundle copy of `criterion-1-qa-judgement.md`, checked to be a regular file beside the resolved tools directory. That is the only copy the judgement may read its rules from, so a PR cannot rewrite them. |
| `step-02-qa-team-scenarios` | agent | Reads the guidance from the pinned bundle, the diff, the linked issue, the measured record and the existing scenarios. Judges, writes or extends this PR's scenarios, runs them, fixes and re-runs, and returns the judgement. |

`step-02a-qa-inputs` cannot end a round. The measurement cannot run when a
tools directory, a tool or a guidance file is absent, is not a regular file
or is not one plain absolute line; when the resolved tools directory lies
inside the worktree under review; when the worktree is not a git repository;
when the tool wrote no record; or when what it wrote is not an object
carrying at least one field. In each case the step prints an object built
from its own literals, naming `qa-measurement-unavailable`, and exits 0:

```json
{"qa_status": "BLOCKED", "qa_reason": "qa-measurement-unavailable",
 "qa_summary": "autodrive_qa_evidence.sh is not a regular file under the auto-drive tools directory",
 "qa_guidance": ""}
```

Every `qa_summary` the step can print there is one of its own literals, and
the object carries no interpolated command output: the step is
`parse_json: true` and has no sanitiser of its own. An empty `qa_guidance`
says the judgement has no pinned copy of the guidance to read, and must say
so rather than fall back to the worktree's. That record carries four fields
and no `docs_only`; an absent `docs_only` is read as `unknown`, under
[how the merge round reads the judgement](#how-the-merge-round-reads-the-judgement).

A record of `{}`, or any output with no `"field":` pair in it, reaches that
same path. The shape check runs before the step appends `qa_guidance`, so a
degenerate record is reported by name rather than handed on as an empty set
of facts that each read as absent.

The agent loop inside `step-02-qa-team-scenarios`:

1. Decide `user_facing` or `not_user_facing`, citing the changed files that
   support the decision. Every cited file appears in the diff.
2. For `not_user_facing` (documentation, configuration with no behaviour
   change), record the reason and the cited files.
3. For `user_facing`, name the journeys this PR's own change affects and
   find their outside-in scenarios. Write or extend the missing ones in the
   scenario directory and commit them to the PR branch. Cross-repository
   user journeys belong to product management; the drive neither runs nor
   requires them.
4. Run only the scenarios this PR changed or added, in CI or isolation, with
   a private `TMPDIR` and a private `tmux` socket. Never the full suite on a
   shared host.
5. On a failure, classify its cause as `product`, `scenario` or
   `environment`, fix it, commit the fix and run again.
6. Record a hand-off only when a person must act.

The record is spliced into the agent step's prompt as data, inside a fence,
with the step saying so at the point of the splice. Two of its fields hold
free text from the branch under review: `qa_summary` carries the tail of the
declared command's output, and `qa_scenarios` carries path names from the
worktree. The measurement sanitises both so the JSON stays valid and the
terminal is unharmed, which protects the data channel and not the
instruction channel; a line in either field that reads like an instruction
is a string the PR author wrote, and the judgement treats it as one. The
same holds for the diff, the issue, scenario files and commit messages.

No missing scenario, repository type, absent tool, non-existent operator
scenario directory or failing declared command ends a drive, and none of
them yields a criterion-1 outcome a later round cannot clear. Only a
recorded hand-off hands the drive to a person.

### Resolving the measurement base

The base decides what the measurement describes: which files the PR changed,
whether the change is documentation-only, and which merge-base blob
`.amplihack/qa.toml` is read from. A PR into a branch other than the
repository's default, measured against the default anyway, records the base
branch's own commits as the PR's change, selects the suite the default branch
declares, and reads a documentation-only flag covering commits the PR does not
carry.

`step-02a-qa-inputs` takes the base from the first of these three sources that
holds a value:

| Order | Source | Where it comes from |
| --- | --- | --- |
| 1 | `AUTODRIVE_QA_BASE` in the step's environment | An operator override for the run, used exactly as given. A variable that is set but empty counts as no answer and falls through to row 2. |
| 2 | `git symbolic-ref -q --short refs/remotes/origin/HEAD`, with the leading `origin/` removed | The clone's own default branch, read in the worktree under review. A clone where that ref was never set gives no answer. |
| 3 | the literal `main` | The last resort, and the reason the step always has a base to export. |

The step consults nothing else. It does not read the base branch the drive
observed for the PR, it does not ask the hosting interface for `baseRefName`,
and it does not check the value it took against `git check-ref-format` or
against the branch-name rules the measurement tool applies for itself. Because
row 3 always answers, the step exports `AUTODRIVE_QA_BASE` on every path, and a
round never reaches the preflight described below.

Validation happens one layer down: `autodrive_qa_scope.sh` requires the base to
pass `git check-ref-format --branch` and treats a name beginning with `-` as
invalid. An override that is not a branch name therefore reaches the tool and
makes the measurement unknown rather than being passed over for another source,
so a mistyped override records `(unknown)` paths and an `unknown` `docs_only`,
which the judgement must name, instead of a measurement against a base the
operator did not choose.

Row 3 is the hazard this section opens with, left standing rather than solved.
Where the clone has no `refs/remotes/origin/HEAD` — a single-branch fetch, a
mirror, a clone whose remote HEAD was never set — and the PR's base is not
`main`, the round measures the PR against `main` and records the result as
fact. Row 2 has the same shape wherever a PR targets a branch other than the
clone's default: it answers with the default, not with the PR's base. An
operator who knows the PR's base lies elsewhere should set `AUTODRIVE_QA_BASE`
for the run, because that override is the only source in this order that can
name a base the clone does not already default to. Reading the PR's own base,
validating what is read, and recording the base in the measurement are not
part of this change.

The measurement needs `refs/remotes/origin/<base>` in the worktree. Where the
round has not fetched that branch — including the `main` of row 3 in a clone
that never fetched it — the merge base cannot be resolved, and the record
carries `(unknown)` paths, an `unknown` `docs_only` and
`qa_criterion1_declaration` `none`. That is a recorded unknown rather than a
silent fallback, and the judgement says so. A base that resolves but is the
wrong base is the opposite case: it records as fact, which is why row 3 is a
hazard and an unfetched base is not.

A preflight is the measurement tool's behaviour with no `AUTODRIVE_QA_BASE` in
its environment at all, which is reached by running
`autodrive_qa_evidence.sh` by hand rather than from a round. It describes the
head commit alone: `qa_command_paths` is `(unknown)` and `docs_only` is
`unknown`, and the declaration is read from the head tree rather than a merge
base, so a preflight record can carry a declaration no base branch ever
committed. What a reader does with that is set out under [how merge-ready
agents use the declaration](#how-merge-ready-agents-use-the-declaration).

The record carries no field naming the base it was measured against, so
neither the judging agent nor the round's assessment can confirm from the
record which base produced it. The assessment's rule for a merge-base
declaration — that it counts only in evidence measured against the PR's actual
base — therefore rests on the order above holding, not on anything the
assessment can read. The criterion 1 prompt states the rule rather than asking
the agent to verify it.

### The record file

The file named by the `autodrive_qa_evidence` recipe variable has exactly one
writer: `autodrive_qa_evidence.sh`, at the end of a measurement that ran. It
writes under `umask 077`, so the file is mode `0600`, and it removes whatever
is at the path before writing, so a planted symlink there is unlinked rather
than written through. `step-02a-qa-inputs` neither composes the record nor
rewrites it; it runs the tool and then checks the shape of the line the tool
printed. The judging agent reads the file and never writes it, and the round
hashes it (see [the record hash](#the-record-hash)).

The path where no measurement could run does not touch the file at all. The
step prints its `qa-measurement-unavailable` object to stdout, exits 0, and
leaves whatever was at the path in place. One drive uses one evidence path for
every round, so a round that could not measure leaves the previous round's
record sitting there. The judgement is not misled by that — the assessment
reads the step's printed object, not the file — but two readers are: the hash
`step-00d-qa-evidence-hash` records is the hash of a file this round did not
write, and anyone who opens the file by hand is reading an earlier round's
measurement. Removing or replacing the file on that path is not part of this
change.

The record names the round and the head it measured, so a reader can tell the
two apart without that. Both the criterion-1 assessment and the expected-checks
step take a merge-base declaration from the record only where its `head_sha` is
the head under review and its `qa_round` is this round; where either differs, or
where this round's record is the `qa-measurement-unavailable` blocker, the
declaration is `none`. That direction is fail-safe: it withholds a declaration
rather than granting one, so it can create no hold a later round cannot clear.
Without it a round that could not measure would read an earlier round's
`qa_criterion1_declaration` and `scenarios_ci_job` name as though freshly
measured, and `qa_evidence_sha` would not notice, because the file is
byte-identical to the one it hashed last round.

Where no measurement has ever written the path, the file is absent. That is
not a named blocker on its own: the blocker reaches its readers through the
object the step prints, and the absent file shows up at
[the record hash](#the-record-hash) as an empty `qa_evidence_sha` beside the
tool's `WARNING:` line.

### Light drives

`auto-drive-light.yaml` checks the change before the PR exists, so it has its
own steps, in this order after `light-implement`, under the same condition
as before, `not pr_number and not existing_branch`.

| Step | Type | What it does |
| --- | --- | --- |
| `light-qa-inputs` | bash | Runs the same measurement tool with `AUTODRIVE_QA_EVIDENCE` fixed at `$QUICK_CHANGE_STATE_DIR/qa/qa-evidence.json`, `AUTODRIVE_QA_BASE` set to the clone's `origin/HEAD` with the same literal `main` fallback the round uses, and `AUTODRIVE_QA_HEAD` set to the worktree's `HEAD` commit. There is no PR yet, so no hosting interface is consulted for the base. That measurement base is not the commit `light-base-pin` pinned: the pin is a SHA of the worktree's `HEAD` at the start of the change, recorded as `light_base` and used for the change set `light-qa` reads, while the measurement base is a branch name resolved independently of it. |
| `light-qa` | agent | Follows the same guidance and returns the judgement as `light_checks`. |

`light-prereqs` reports whether `gadugi-test` is on the path and exits 0.
`gadugi_test` is a closed set of two literals:

```json
{"gadugi_test": "installed"}
```

```json
{"gadugi_test": "not-installed"}
```

An absent `gadugi-test` is a fact for the judgement, so nothing there ends
the drive, and the repository type is not consulted. A value outside the two
literals, or an absent `gadugi_test` key, is read as `not-installed`: the
step reports a fact and may not stop the drive by being unreadable.

Before the PR exists the change is uncommitted, and the tool diffs committed
history only: `qa_command_paths` and `docs_only` do not describe the change.
`light-qa` takes the change set from `git diff <the recorded base>` plus
`git status --porcelain --untracked-files=all`, which lists the new files
`git diff` leaves out, and treats those lines as data rather than passing
them to a shell.

The light judgement is a pre-PR check. It does not meet criterion 1: once
the PR is open, `light-merge-loop` runs `autodrive-merge-loop.yaml`, and each
round measures and judges again against the PR head.

Its hand-off is advisory for the same reason. A `qa_handoff` that `light-qa`
records stays in the step's own `light_checks` output for the run's write-up;
nothing writes it to the step-recovery store of
[step failure recovery](step-failure-recovery.md), and no later step reads it
as a recorded recovery. Read the light judgement, its named blockers and its
hand-off as advice to the run, and expect the merge round to judge criterion 1
again from its own measurement.

## What the judgement records

`step-02-qa-team-scenarios` returns the measured fields it read, plus:

| Field | Value |
| --- | --- |
| `qa_judgement` | `user_facing` or `not_user_facing`. |
| `qa_judgement_notes` | The judgement in prose: the reason with its cited diff files, the scenarios written, extended or run with their results, each failure with its cause and what was done, any named blocker and how it was cleared, and any fallback with the operator path it replaced. |
| `qa_handoff` | `null`, or `{step_id, failure, choice, reason, owner, needs}` with `choice` equal to `handoff`. The names match the arguments of `orch helper step-recovery record`; see [step failure recovery](step-failure-recovery.md). |

The step's output stays `parse_json: true` because the same step collects
the measured facts criteria 3, 6 and 8 read. Its criterion-1 content is
facts plus the judgement; no consumer turns it into a verdict. The
assessment step in `autodrive-merge-round.yaml` writes the judgement up in
prose and posts it as the PR comment, which nothing parses.

`qa_judgement_notes` reaches a public PR comment. It carries no raw command
output, log tail or environment value, no file contents from outside the
diff, no path under `~/.claude` or `~/.amplihack`, and no email address;
`qa_handoff.owner` names a person or a team.

The agent writes nothing to `AUTODRIVE_QA_EVIDENCE`: the measurement owns
that file, and the round hashes it (see [the record hash](#the-record-hash)).

## Named recoverable blockers

The judgement names a blocker from this closed list, derived from the
measured facts. Each one is an input it carries on from, not an outcome.

| Blocker | Derived from | What the judgement does |
| --- | --- | --- |
| `qa-scenario-dir-missing:<path>` | `gadugi_scenario_dir_state` is `operator-missing` | Falls back to the repository's own tracked scenario directory, recording the fallback and the operator path it replaced; otherwise hands off naming the path and the operator; otherwise records `not_user_facing` with cited diff files. Never a per-round QA failure. |
| `qa-measurement-unavailable` | the record is absent, unparseable, incomplete or not an object carrying at least one field; the pinned tools directory resolved inside the worktree under review; the record carries a value outside its set; or it carries the broken-scope signature: `qa_reason` `qa-command-missing` together with `qa_command_paths` `(unknown)`, `docs_only` `unknown` and `qa_repo_type` `unknown` (see [telling the three causes apart](#telling-the-three-qa-command-missing-causes-apart)) | Judges from the diff and the scenarios; hands off only when it cannot. A broken bundle install surfaces by name; it neither passes silently nor ends the run. `gadugi_required` `true` is not this signature: it is the normal value for a Node repository and for any operator-set scenario directory. |
| `qa-tool-not-installed:<tool>` | `gadugi_status` `NOT_INSTALLED`, or `qa_reason` `qa-command-not-installed` | Meets criterion 1 by other evidence — the declared command, the repository's own tests, or a scenario the available tools run — or hands off naming the tool and its owner. Nothing is ever installed automatically. |
| `qa-command-failed:<name>` | `qa_reason` `qa-command-failed` with `qa_exit_code` | Finds out whether the PR caused the failure and fixes it like any product failure if it did. |

The fallback directory for `qa-scenario-dir-missing` must be one of
`tests/agentic`, `tests/gadugi/scenarios` or `scenarios`, must resolve
inside the repository root after `pwd -P`, and must be tracked in Git.
Nothing else is a fallback, and both paths are recorded.

## The measured record

`autodrive_qa_evidence.sh` writes the record at `AUTODRIVE_QA_EVIDENCE`
itself, not at a sibling path, so the round's hash of that path stays
meaningful. The write is `( umask 077; rm -f -- "$EVIDENCE"; printf '%s' …
> "$EVIDENCE" )`: mode 0600, and never through a planted symlink. The same
JSON is printed on standard output.

The tool exits 1, writing nothing, only when it cannot enter `REPO_PATH` —
measuring another directory would record that directory's files and head as
the PR's — or when the scope helper reports a broken install. Every other
condition is recorded inside the record. Every value is a string, so a
reader never has to guess a type.

Two examples, with every field coherent. First, a Node PR that changes code
and one documentation file, in a repository with three tracked scenarios.
Node is the only repository type whose detection requires gadugi scenarios,
so the scenario directory was consulted:

```json
{
  "qa_status": "PASS", "qa_reason": "", "qa_repo_type": "node",
  "qa_dir": ".", "qa_command": "npm test", "qa_command_source": "detected",
  "qa_criterion1_declaration": "none",
  "qa_command_paths": "docs/install.md src/auth/login.ts",
  "qa_candidates": "", "qa_ci_skip": "no-match",
  "qa_suite_commands_count": "1",
  "qa_scenarios": "tests/agentic/install.yaml tests/agentic/login.yaml tests/agentic/upgrade.yaml",
  "qa_exit_code": "0", "qa_summary": "Tests: 42 passed, 42 total",
  "qa_round": "round-1",
  "head_sha": "9f2c1e4b7a0d3f6e8c5b2a1d4e7f0a3c6b9d2e5f",
  "docs_only": "false", "gadugi_required": "true", "gadugi_status": "FOUND",
  "gadugi_scenario_dir": "tests/agentic",
  "gadugi_scenario_dir_state": "detected",
  "gadugi_scenario_count": "3",
  "gadugi_validate_exit_code": "", "gadugi_run_exit_code": "",
  "gadugi_scenarios_validated": "0", "gadugi_scenarios_run": "0",
  "gadugi_scenarios_passed": "0", "gadugi_scenarios_failed": "0",
  "gadugi_failed_scenarios": "", "gadugi_scenario_results": "",
  "git_env_cleared": "none", "run_env_cleared": "none"
}
```

Second, the shape every amplihack-rs record takes: a Rust CLI PR.
`gadugi_required` is `false`, so the tool never looks for a scenario
directory and `gadugi_scenario_dir_state` is `not-consulted` with
`gadugi_scenario_dir` empty. `qa_scenarios` still lists the selected
project's `tests/parity/scenarios/` fixtures, which `cargo test` runs;
`gadugi_scenario_count` counts a consulted gadugi directory only, so it
stays `0`:

```json
{
  "qa_status": "PASS", "qa_reason": "", "qa_repo_type": "rust-cli",
  "qa_dir": ".",
  "qa_command": "cargo test --workspace --locked --no-fail-fast",
  "qa_command_source": "detected", "qa_criterion1_declaration": "none",
  "qa_command_paths": "crates/amplihack-cli/src/commands/orch/autodrive/mod.rs",
  "qa_candidates": "", "qa_ci_skip": "no-match",
  "qa_suite_commands_count": "1",
  "qa_scenarios": "tests/parity/scenarios/pr554-quality-improvements.yaml",
  "qa_exit_code": "0", "qa_summary": "test result: ok. 1312 passed; 0 failed",
  "qa_round": "round-1",
  "head_sha": "4d1a7c0e9b6f2a5d8c3e1b4f7a0d2c5e8b1f4a7d",
  "docs_only": "false", "gadugi_required": "false",
  "gadugi_status": "NOT_REQUIRED",
  "gadugi_scenario_dir": "", "gadugi_scenario_dir_state": "not-consulted",
  "gadugi_scenario_count": "0",
  "gadugi_validate_exit_code": "", "gadugi_run_exit_code": "",
  "gadugi_scenarios_validated": "0", "gadugi_scenarios_run": "0",
  "gadugi_scenarios_passed": "0", "gadugi_scenarios_failed": "0",
  "gadugi_failed_scenarios": "", "gadugi_scenario_results": "",
  "git_env_cleared": "none", "run_env_cleared": "none"
}
```

A documentation-only PR records the second shape, not the first, whatever
the repository type: `docs_only` `true` makes `gadugi_required` `false`
(#2219), so `gadugi_status` is `NOT_REQUIRED` and the state is
`not-consulted`. The single exception is an operator who sets
`AUTODRIVE_QA_SCENARIO_DIR`, which is read before the documentation-only
measurement: `gadugi_required` is then `true` and the state is `operator` or
`operator-missing`, never `detected`. A record carrying `docs_only` `true`
beside `gadugi_status` `FOUND` and state `detected` cannot be produced.

| Field | Meaning |
| --- | --- |
| `qa_status`, `qa_reason` | The measurement's own summary of the declared command and the scenario discovery. See [the status values](#the-status-values). |
| `qa_repo_type` | `rust-cli`, `python`, `node` or `unknown`. A combination outside the detection table is `unknown`, never an error. |
| `qa_dir`, `qa_command`, `qa_command_source`, `qa_exit_code` | Where and what ran, from which source, and its exit status. See [source precedence](#source-precedence). |
| `qa_criterion1_declaration` | The merge-base `.amplihack/qa.toml` declaration (#2458): `none`, `multi-area-command` or `ci-job:<name>`. |
| `qa_command_paths` | The changed-path set the scope was taken from, or `(unknown)`. |
| `qa_candidates`, `qa_ci_skip`, `qa_suite_commands_count` | Unresolved candidate projects, why CI selection was skipped, and how many suite commands ran. |
| `qa_scenarios` | Scenario files, as space-separated repository-relative paths, capped at 2,000 bytes. From `gadugi_scenario_dir` where it was consulted; for a Rust CLI project where it was not, the selected project's `tests/parity/scenarios/*.y{a,}ml`, which `cargo test` runs. Top-level regular files only: a symlinked or non-regular entry is named here and not followed. |
| `qa_summary` | The recorded causes and the last lines of the command's output, sanitised. |
| `qa_round` | `AUTODRIVE_ROUND_LABEL`, or `round`. |
| `head_sha` | The head measured: hexadecimal digits, or empty where the head could not be read. The tool constrains the value to that set before printing it, so a ref name, an option-like string or any other output of a misread head cannot reach a reader through this field. |
| `docs_only` | `true`, `false` or `unknown`, from the [documentation-only measurement](#documentation-only-measurement). |
| `gadugi_required` | What `autodrive_gadugi_required` reports: `true` for a Node project, for any repository where `AUTODRIVE_QA_SCENARIO_DIR` is set, and where the scope helper did not load; `false` for a documentation-only PR, for a Rust CLI or Python project, and under a #2458 declaration. It is a fact about the repository and the operator environment; no consumer conditions criterion 1 on it, and it is not a signal that the measurement is broken — see [telling the three `qa-command-missing` causes apart](#telling-the-three-qa-command-missing-causes-apart). |
| `gadugi_status` | `NOT_REQUIRED`, `NOT_INSTALLED`, `NO_SCENARIOS` or `FOUND`. `NOT_REQUIRED` whenever `gadugi_required` is `false`, which is every Rust CLI and Python repository and every documentation-only PR; the tool then consults no scenario directory and calls no tool. `FOUND` means scenarios were discovered and listed; the tool runs none. |
| `gadugi_scenario_dir`, `gadugi_scenario_dir_state` | The scenario directory and how it was arrived at. The state is written from a closed-set literal, never from the operator's value: `not-consulted` (`gadugi_required` is `false`, so no directory was looked for; `gadugi_scenario_dir` is empty), `operator` (named by `AUTODRIVE_QA_SCENARIO_DIR` and present), `operator-missing` (named and absent), `detected` (the first present of `tests/agentic`, `tests/gadugi/scenarios` and `scenarios`) or `absent` (gadugi required, none of the three present; the default `tests/agentic` is reported). `not-consulted` is the common value: it holds for every Rust CLI and Python repository, every documentation-only PR and every PR under a #2458 declaration. |
| `gadugi_scenario_count` | How many scenario files were found in a consulted scenario directory. `0` when the state is `not-consulted`, even where `qa_scenarios` lists parity fixtures. |
| `gadugi_validate_exit_code`, `gadugi_run_exit_code`, `gadugi_scenarios_validated`, `gadugi_scenarios_run`, `gadugi_scenarios_passed`, `gadugi_scenarios_failed`, `gadugi_failed_scenarios`, `gadugi_scenario_results` | Run-shaped fields kept so existing readers stay valid. The tool runs no scenario, so they are `""` or `0`. The agent's scenario runs are reported in `qa_judgement_notes`. |
| `git_env_cleared`, `run_env_cleared` | See [git repository environment helper](git-repo-env-helper.md). |

A reader tolerates a field it does not know. It does not tolerate a value
outside the sets above: an unknown value, a missing field or an unparseable
record is `qa-measurement-unavailable`, never a pass.

### The status values

`qa_status` and `qa_reason` describe the measurement, and nothing more. No
consumer conditions criterion 1 on either: a `BLOCKED` record is an input to
the judgement, and deleting the record entirely leaves criterion 1
worse-informed, never stuck.

| `qa_status` | `qa_reason` | Measured when |
| --- | --- | --- |
| `PASS` | `""` | Every suite command that ran exited 0, and the scenario discovery is complete (`gadugi_status` `NOT_REQUIRED` or `FOUND`). |
| `FAIL` | `qa-command-failed`, `no-scenarios` | A suite command exited non-zero, or the scenario directory holds no scenario. |
| `BLOCKED` | `qa-config-invalid`, `qa-command-missing`, `qa-command-not-installed`, `gadugi-test-missing` | The suite could not be selected or run, or `gadugi-test` is absent. |
| `QA_UNRESOLVED` | `qa-unresolved` | Manifest detection found several candidate projects and could not scope the change to one. |

`qa_reason` is the first token that applies, in a fixed order. Where no
check failed by name and the measurement is still incomplete, the tool
prints an `ERROR:` line saying the measurement is incomplete and records
`qa-command-missing` with `BLOCKED` rather than `PASS`.

`gadugi-validate-failed`, `gadugi-run-failed` and `gadugi-scenario-unnamed`
are gone: the tool runs no scenario, so it cannot observe a validation or a
run failing, and no path reaches them. The unknown-reason safety net at the
end of the status mapping now records `BLOCKED` with `qa-command-missing`,
the same incomplete measurement as above, rather than inventing a failure it
did not observe. Every token the tool picks is a literal from the list, so
the net is unreachable in practice; it is not removed, because it is what
keeps an unrecognised token from being read as a pass. A reader that meets
one of the three retired tokens is reading a record an older tool wrote:
treat it as any failing measurement — an input to the judgement, never an
outcome.

#### Why `FAIL` kept its name

`qa_status` keeps all four values, and `FAIL` keeps its spelling. A name like
`measured-commands-failed` would carry the design in the word itself, so the
choice is worth stating rather than rediscovering.

The field is a record shape other things already read: the declaration tests
in `tests/integration/issue_2458_qa_declaration_test.rs` assert on these
literals, and `autodrive_round_evidence.sh` hashes the record file without
reading any field in it. Renaming would rewrite those assertions and every
example here, and change no behaviour: the measurement issued no verdict
before the rename and issues none after it.

What makes `FAIL` a fact rather than an outcome is what reads it. Nothing
conditions criterion 1 on `qa_status`. The judgement takes it as one input
among the diff, the issue and the scenarios, and `autodrive_round_word.sh`,
which decides how the loop proceeds, reads none of these fields at all. A
`FAIL` record describes a command that exited non-zero or a scenario
directory that holds no scenario, and both are things the judgement acts on:
it decides whether this PR caused the failure, or writes the missing
scenario.

So the one change to avoid here is re-reading the value as a verdict: a
consumer that ends a round because `qa_status` is `FAIL` restores the
behaviour of issue #2569, whatever the field is called by then. The
behaviour tests
`a_failing_declared_command_is_evidence_and_not_the_verdict` and
`an_absent_gadugi_test_is_reported_and_never_ends_a_light_drive` in
`crates/amplihack-cli/tests/issue_2569_criterion1_judgement.rs` exist to
catch that, and a change that makes either of them fail is reintroducing the
stop, not updating a test.

### Telling the three `qa-command-missing` causes apart

`qa-command-missing` is overloaded. Three unrelated conditions record it,
and only the first is the broken install that the judgement names
`qa-measurement-unavailable`. They are distinguished by the fields beside
it, never by `gadugi_required`:

| Cause | How the record reads |
| --- | --- |
| The scope helper did not load, or the merge base could not be resolved: nothing about the change was measured. | `qa_repo_type` `unknown`, `qa_command_paths` `(unknown)`, `docs_only` `unknown`, `qa_command_source` `""`, `qa_command` `""`, `qa_suite_commands_count` `0`, and `qa_summary` naming the missing `autodrive_qa_scope.sh` or `autodrive_qa_sources.sh`. The tool also prints an `ERROR:` line. |
| No suite command exists for the repository type, or `AUTODRIVE_QA_DIR` names a directory that does not exist. | `qa_command_paths` is a real path set and `docs_only` is `true` or `false`, so the change *was* measured. `qa_summary` names either `no repository test command for repository type <type>` or `AUTODRIVE_QA_DIR <dir> not found`. |
| The incomplete-measurement fallback: no check failed by name, yet no suite command ran or the scenario discovery did not complete. | The change was measured, and `qa_summary` names none of the causes above. `qa_suite_commands_count` is `0`, or `gadugi_status` is neither `NOT_REQUIRED` nor `FOUND`. |

The second and third are facts about this repository, and the judgement
reads them as such: it decides from the diff and the scenarios what testing
the PR needs. Only the first means the measurement itself cannot be trusted.
## Source precedence

Use the first present source. An invalid earlier source is not permission to
select a later, weaker command.

| Order | Source | Rules |
| --- | --- | --- |
| 1 | Operator environment | Either `AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS` being set selects this source, even if empty. |
| 2 | `.amplihack/qa.toml` at the merge base | Any entry present, an invalid `.amplihack` entry, or a PR change to this configuration selects this source. Invalid configuration is recorded; it does not fall through. |
| 3 | Test step from merge-base CI | Exactly one clean, in-scope test step across the workflows. Otherwise record the skip reason and use detection. |
| 4 | Merge-base manifests | Detect the project and scope it to the complete changed-path set. Ambiguity is recorded as `qa-unresolved`. |

The selected command source does not decide what testing the PR needs. That
is the judgement's to make from the diff, the issue and the scenarios, and an
operator's suite override neither settles it nor removes it.

Running a suite is not sandboxed, whichever source chose it. `npm test` runs
`scripts.test`, `pytest` imports `conftest.py` and `cargo` runs `build.rs`,
so a suite always runs the branch's own code. No timeout is applied: a suite
is never cut off mid-run (#439).

Repository configuration, workflows, and candidate manifests come from the
merge-base tree, not working-tree files or PR-authored replacements. A PR
cannot select its own easier suite by adding a manifest or editing the QA
configuration. An unknown base is recorded, and the judgement takes the
change set from the diff itself rather than treating the measurement as
complete.

## Operator environment

These are environment variables, not recipe context keys.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One word-split command with globbing disabled, run in `AUTODRIVE_QA_DIR`; no implicit shell evaluation. |
| `AUTODRIVE_QA_DIR` | `.` | Directory for the single command, relative to repository root or absolute. A missing directory gives `qa_reason` `qa-command-missing`. |
| `AUTODRIVE_QA_COMMANDS` | unset | Additional commands, one per line, run from the repository root after the single command. Ignore blank and whole-line comment entries. This operator-controlled source permits shell commands. |
| `AUTODRIVE_QA_SCENARIO_DIR` | unset; the tool then consults a directory only where `gadugi_required` is `true`, taking the first existing of `tests/agentic`, `tests/gadugi/scenarios` and `scenarios`, otherwise reporting `tests/agentic` | Where the judgement looks for and writes scenarios. Setting it makes `gadugi_required` `true` in any repository type, ahead of the documentation-only measurement, so the state is `operator` or `operator-missing` and never `detected`. It makes no scenario compulsory. A path that does not exist is the recoverable blocker `qa-scenario-dir-missing:<path>`, recorded as `gadugi_scenario_dir_state` `operator-missing`. |

An explicitly empty command override records source `env` and `qa_reason`
`qa-command-missing`; it does not fall back. Sources other than the single
environment command choose their own directory.

`AUTODRIVE_QA_BASE` and `AUTODRIVE_QA_HEAD` are observation inputs. The
script resolves neither for itself and uses whatever values it is given.
Inside a round, `step-02a-qa-inputs` resolves the base alone (see
[resolving the measurement base](#resolving-the-measurement-base)), and an
`AUTODRIVE_QA_BASE` an operator sets there is the first source it consults.

The step sets no head. `AUTODRIVE_QA_HEAD` is left unset, so the measurement
takes the checkout's own `HEAD`, and nothing compares that commit with the
head GitHub reports for the PR, `headRefOid`: the round trusts that the
worktree it was given is the worktree of the PR it selected. `AUTODRIVE_QA_HEAD`
is read by `autodrive_qa_scope.sh`, which accepts the literal `HEAD` or a full
SHA present in the clone and nothing else, so a short SHA or an empty value
makes everything unknown rather than measuring some other commit.
`light-qa-inputs` passes the head explicitly because its worktree holds the
change under construction. Fixing the head to the value GitHub reports is not
part of this change. With `AUTODRIVE_QA_BASE` unset the tool measures a
preflight of the head commit (see
[Declaring how criterion 1 is met](#declaring-how-criterion-1-is-met)). A
preflight record describes the head commit only: `qa_command_paths` is
`(unknown)` and `docs_only` is `unknown`, so the judgement takes the change
set from the diff. A round always exports a base, so a preflight record comes
from running the tool by hand.

## Repository configuration

A root Rust workspace can declare its suite with this complete configuration:

```toml
command = "cargo test --workspace --locked --no-fail-fast"
dir = "."
```

Saved as `.amplihack/qa.toml` and committed before the PR's merge base, this
selects source `repo-file`, type `configured`, directory `.`, and the
command shown. A passing invocation exits zero; a failing one is evidence the
judgement acts on, recorded as `qa-command-failed:<name>`.

The configuration contract is deliberately narrow:

- At most 4096 bytes; a regular Git blob, never a symlink, submodule, or
  directory. `.amplihack` must be a directory in the merge-base tree.
- Lines are blank, whole-line `#` comments, or `key = "value"`. Values have
  no backslashes; no tables, inline comments, duplicate or unknown keys.
- Bytes are tabs, line feeds, or printable ASCII. No carriage returns or NULs.
- `command` is required and non-empty after word splitting. `dir` defaults
  to `.`. `multi_area` and `scenarios_ci_job` are optional; see
  [Declaring how criterion 1 is met](#declaring-how-criterion-1-is-met).
- Commands use only letters, digits, spaces, and `._/-=+:,@%`. Shell
  substitutions, redirections, quoting, pipelines, and wildcards are rejected.
- Directories are relative, use only `A-Za-z0-9._/-`, do not start with
  `/` or `-`, and have no empty or `..` component. The physical directory
  must exist inside the repository.
- Changed paths must be known and non-empty. A PR changing `.amplihack`
  itself or `.amplihack/qa.toml` is not eligible for this source, recorded as
  `changed-in-pr`.

Commands are executed as arguments, never passed through `eval` or implicit
shell source. Invalid configuration reports `qa-config-invalid` with a cause
such as `symlink`, `not-regular`, `too-large`, `syntax`, `unknown-key`,
`duplicate-key`, `declaration-conflict`, `unknown-value`, `missing-command`,
`unsafe-command`, `unsafe-dir`, `dir-not-found`, `changed-in-pr`,
`paths-empty`, or `paths-unknown`.
The contents of invalid configuration never appear in diagnostics.

### Declaring how criterion 1 is met

Where detection requires gadugi scenarios (a Node project, or unresolved
candidates of which any is Node), the merge-base `.amplihack/qa.toml` can
declare how criterion 1 is met instead (#2458). A file gives at most one
declaration; both keys together are `declaration-conflict`.

| Key | Value | `qa_criterion1_declaration` | Criterion 1 is met when |
| --- | --- | --- | --- |
| `multi_area` | `command` only; anything else is `unknown-value` | `multi-area-command` | The declared `command` exits `0`, and it carries the evidence for this PR's change. The measurement searches for and runs no gadugi scenario. |
| `scenarios_ci_job` | A check-run name: 1 to 128 bytes of `A-Za-z0-9`, space and `._/()+:,@%-`, no leading `-` | `ci-job:<name>` | The declared `command` exits `0`, **and** the head has a check-run with exactly that name and every check-run of that name passed. |

A repository with `web/package.json` and `infra/pyproject.toml` and no root
manifest has unresolved candidates, one of them Node. Its scenarios are JSON
and TypeScript files run by its own test suite, not gadugi YAML, so it
declares:

```toml
# .amplihack/qa.toml
command = "npm test"
multi_area = "command"
```

With that file at the merge base, a PR that changes `web/` and `infra/` runs
`npm test` once, runs no `gadugi-test`, and records:

```console
$ jq -r '.qa_status, .gadugi_status, .qa_criterion1_declaration' "$EVIDENCE"
PASS
NOT_REQUIRED
multi-area-command
```

With `command = "npm test"` alone at the merge base (no `multi_area` line),
and an empty scenario directory, the same PR records `qa_status` `FAIL`,
`qa_reason` `no-scenarios`, `gadugi_status` `NO_SCENARIOS` and
`qa_criterion1_declaration` `none`. Those are measured facts: the judgement
writes the missing scenarios into the PR and runs them rather than stopping
on them.

**Read from the merge base.** When `AUTODRIVE_QA_BASE` is set, the
declaration is read from the same merge-base blob as `command`, never from
the PR head or the working tree. A PR that changes `.amplihack/qa.toml` is not
eligible, recorded as `changed-in-pr`, so a PR cannot declare the rule it is
judged by. A new declaration first applies to the PR after the one that adds it.

**A preflight reads the PR head.** Only when `AUTODRIVE_QA_BASE` is unset
does the declaration come from the head commit's `.amplihack/qa.toml`, read
in `decl-only` mode. The `changed-in-pr` cause cannot apply, because there
is no base to compare with. The head declaration has one effect:
`autodrive_gadugi_required` can report `false`, so the preflight skips the
`gadugi-test` install check. `autodrive_qa_report` still reports
`invalid` `paths-unknown` and chooses no command, so a preflight measures the
head commit only. An `AUTODRIVE_QA_BASE` that is set but empty, or names a
base whose merge base cannot be resolved, does not fall back to the head; the
declaration is then `none`. Merge evidence measures against the PR's own
base, resolved as
[resolving the measurement base](#resolving-the-measurement-base) sets out. A
record measured where no base resolved describes the head commit only, and its
declaration does not count for a merge.

**Fails closed.** The declaration is `none`, and gadugi stays required
wherever detection requires it, when `AUTODRIVE_QA_SCENARIO_DIR`,
`AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS` is set (even to empty);
qa.toml is absent or invalid; `AUTODRIVE_QA_BASE` is set but empty, or the
merge base cannot be resolved, or a git read fails; or, for `scenarios_ci_job`
only, the PR changes anything under `.github/` (`qa_summary` names
`ci-job-workflow-changed`). Where gadugi is not required anyway, such as a
Rust or Python project or a
[documentation-only change](#documentation-only-measurement), the evidence also
records `none`.

**The declared job is an expected check.** For `ci-job:<name>`, the merge
evidence step passes `<name>` as the third argument to
[`autodrive_expected_checks.sh`](autodrive-expected-checks.md#declared-scenarios-job),
taking it from the measured QA evidence record, never from an agent's
summary. Only `GREEN` meets criterion 1.

#### How merge-ready agents use the declaration

The agents that judge criterion 1 are told about the declaration in two
places: the criterion 1 row of the
[merge-ready skill](../../amplifier-bundle/skills/merge-ready/SKILL.md) and the
criterion 1 bullet of `amplifier-bundle/recipes/autodrive-merge-round.yaml`.
The judging agent runs no measurement of its own: `step-02a-qa-inputs` runs
`autodrive_qa_evidence.sh` against the base it resolved, and the criterion 1
prompt of `autodrive-merge-evidence.yaml` states that rule, with what a
preflight record does not support. The guidance is agent judgement; no script
checks the agent's reading.

Both texts give the same rules:

| `qa_criterion1_declaration` | Criterion 1 is judged as |
| --- | --- |
| `multi-area-command` | Met when the declared `command` passed and it covers this PR's change. No missing-scenario blocker is raised, and the repository's own suite carries the evidence instead of a gadugi scenario. |
| `ci-job:<name>` | Met when the declared `command` passed **and** criterion 8's CI measurement shows every check-run named `<name>` at the head passed. A missing check-run is `MISSING`, not met. |
| `none` | The existing detection rules apply unchanged. `none` alone is not a gap and not a blocker. |

- **Source.** The agent takes the value only from the measured record at
  `{{autodrive_qa_evidence}}`. It never takes it from the step-02 summary, the
  PR description, or a qa.toml read from the PR head or the worktree.
- **Base binding.** A declaration counts only in a record measured against
  the PR's own base, which
  [resolving the measurement base](#resolving-the-measurement-base)
  establishes. A preflight record, measured where no base resolved, reads its
  declaration from the PR head rather than from a merge base, so it can carry
  `multi-area-command` or `ci-job:<name>` that no base branch ever committed.
  The agent reads a preflight's declaration as `none` whatever it says, and
  reads a declaration found outside the record as `none` too.
- **Exclusions.** The declaration does not apply when the PR changes
  `.amplihack/qa.toml`; when the operator sets `AUTODRIVE_QA_SCENARIO_DIR`,
  `AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS`, even to an empty value;
  when qa.toml is absent or invalid; when the merge base is unknown while
  `AUTODRIVE_QA_BASE` is set, or a git read fails; and, for
  `scenarios_ci_job` only, when the PR changes anything under `.github/`.
  The evidence then records `none`.
- **Not a waiver.** A declaration committed at the merge base is the base
  branch's own rule for how criterion 1 is met, not a waiver of the
  judgement. Where the PR's change is user-facing, the judgement still
  exercises it outside-in and says so; the declaration tells it which suite
  carries that evidence.
- **No self-declaration.** No agent adds, edits or suggests a qa.toml
  declaration in the PR under review, including during repair and
  blocker-clearing.

Applied to #2458: a repository with a root `package.json`, a multi-area PR, a
merge-base qa.toml with `multi_area = "command"`, a passing `command`, and
evidence measured against the PR's base recording `multi-area-command`. An
agent following either text judges criterion 1 met.
## CI selection

Read regular top-level `.yml` and `.yaml` blobs from merge-base
`.github/workflows/`. Both parent directories must be real tree directories.
Skip this advisory source if listing or reading fails, there are more than
50 files, names contain bytes outside `A-Za-z0-9._-`, an entry is not a
regular blob, or any file exceeds 256 KiB.

Recognized starts are `cargo test`, `cargo nextest run`, `npm test`,
`npm run test`, `pytest`, `python -m pytest`, and `python3 -m pytest`.
A clean match is a single line without substitutions, quoting, backslashes,
shell operators, environment assignments, or multiline continuation. A block
scalar containing a recognized test is a non-clean match. Workflow/job
`defaults.run.working-directory` also makes a match non-clean.

There must be exactly one match and it must be clean. Its step-local
`working-directory` has `.` components removed and must satisfy the same
directory rules as repository configuration. It must contain each changed
path belonging to a detected candidate and lie inside that path's deepest
candidate; an unrelated project's test does not cover the change.

A PR changing `.github`, a workflow, or `.amplihack/qa.toml` skips CI
selection. Record `qa_ci_skip`: `paths-empty`, `paths-unknown`,
`changed-in-pr`, `no-workflows`, `list-failed`, `too-many-files`,
`unsafe-name`, `not-regular`, `too-large`, `read-failed`, `not-clean`,
`no-match`, `several-matches`, `unsafe-command`, `unsafe-dir`,
`dir-not-found`, or `out-of-scope`. Skipping CI command selection does not
waive platform CI checks.

## Manifest detection and scope

Candidates are directories with regular merge-base `Cargo.toml`,
`pyproject.toml`, `setup.py`, or `package.json` manifests within the
established search depth (for example, `a/b/Cargo.toml` is included,
`a/b/c/Cargo.toml` is not). Exclude hidden directories, `target`,
`worktrees`, and `node_modules`. Same-directory precedence is Cargo,
Python, Node.

Collapse nested candidates when an ancestor has the same ecosystem;
different ecosystems remain separate. Choose the deepest candidate containing
every changed path that belongs to any candidate. Paths outside all candidates
are set aside. If no paths remain, or paths are unknown/empty, require exactly
one top-level candidate. Multiple unmatched candidates give `qa_status` `QA_UNRESOLVED` with
`qa_reason` `qa-unresolved`, a measured fact and an input to the judgement.

A root project contains all paths. Changing only a nested Node project under
a Cargo root selects Node; changing the root README too selects the root.
Declare a repository-wide command when shared files outside candidates need
wider coverage.

Unrecognized root project files prevent selecting a nested helper as the
main project: `requirements.txt`, `setup.cfg`, `tox.ini`, `Pipfile`,
`go.mod`, `pom.xml`, `build.gradle`, `build.gradle.kts`, `Makefile`,
`makefile`, `GNUmakefile`, `CMakeLists.txt`, `meson.build`, `build.sbt`,
`Gemfile`, `composer.json`, `mix.exs`, and `flake.nix`. With no recognized
root manifest, these yield `qa_repo_type` `unknown` at the root and
`qa_reason` `qa-command-missing` until a suite is declared.

A search error cannot select a readable candidate while hiding another.
Use the root candidate if established; otherwise scope is unresolved. Unsafe
candidate directories are not executed. A missing command or an ambiguous
scope is recorded for the judgement, which decides from the diff and the
scenarios what testing the PR needs.

## Scenario judgement

The repository type no longer decides whether a PR needs a scenario; the
judgement does, from the PR's own change. The table below is what each
detected project contributes to that judgement.

| Detected project | Default suite | What the judgement usually does |
| --- | --- | --- |
| Rust CLI | `cargo test --workspace --locked --no-fail-fast` | Exercises a user-facing change through the repository's own outside-in tests, including parity scenarios under the selected project's `tests/parity/scenarios/`. |
| Python | `pytest` | Exercises a user-facing change through the repository's own outside-in tests. |
| Node | `npm test` | Exercises a user-facing change through the scenario directory's scenarios, writing or extending the ones this PR needs. |
| Unknown | No detected command | Judges from the diff and the scenarios what exercises the change. `qa-command-missing` here says only that no suite is declared, not that the measurement failed. |
| Unresolved candidates | No detected command | The same, naming the candidates from `qa_candidates`. |
| Any, explicit scenario directory | Selected suite still runs | Looks there first. A directory that does not exist is `qa-scenario-dir-missing:<path>` with a constrained fallback or a hand-off. |
| Unknown merge base | Cannot establish trusted selection | `qa-measurement-unavailable`, recognised by `qa_command_paths` `(unknown)` with `docs_only` `unknown`: takes the change set from the diff and judges from it. |

Where a user-facing change needs a scenario, the judgement authors or
extends it through `qa-team`, commits it to the PR branch, and runs only the
scenarios this PR changed or added. With `gadugi-test` that means `validate`
over the staging directory first, then one scenario per process, each file
copied alone into a staging directory outside the repository: concurrent
scenarios in one directory share a single CLI runner, and the first to
finish kills the others (gadugi-agentic-test #207). `--scenario` matches a
substring of the scenario's `name:`, not a path, so `-d` must point at the
staging directory and never at the shared one. The rule and its reason are
stated for the agent in
[criterion-1-qa-judgement.md](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md),
which is the only place that enforces it now that the tool runs nothing.
Each failure is classified `product`, `scenario` or `environment`, fixed,
and run again; a validation failure is a `scenario` cause. A failing
scenario is never weakened, skipped or deleted to reach a pass.

An absent runner, an empty scenario directory, a scenario directory that
does not exist and a failing suite command are
[named recoverable blockers](#named-recoverable-blockers). The judgement
clears them with other evidence or hands off to a named owner. It installs
nothing automatically, and it asserts nothing in place of evidence.

Host limits apply to every run: only this PR's scenarios, a private
`TMPDIR`, a private socket for every `tmux` invocation with `-S`, never the
full suite on a shared host, and in amplihack-rs never `tests/gadugi/*` on
the host. A failure caused by the host and unrelated to the PR's files is
recorded as such with its reason.

## Documentation-only measurement

`docs_only` is a measured fact, not an exemption. It binds the judgement in
one direction only:

| `docs_only` | What it means for the judgement |
| --- | --- |
| `false` | Binding. The judgement may not be `not_user_facing`. |
| `true` | Advisory. The judgement may still find the change user-facing and exercise it. |
| `unknown` | The change set could not be measured. The judgement reads the diff and, in doubt, treats the change as user-facing. A `not_user_facing` judgement remains open, but the round accepts one only where the notes name what the change set was judged from. |
| absent | The record carries no `docs_only` field at all, as the `qa-measurement-unavailable` record does. Read exactly as `unknown`. |

A documentation-only PR meets criterion 1 through a recorded
`not_user_facing` judgement whose reason cites files that appear in the diff,
never through the measurement alone.

`unknown`, and an absent `docs_only` read as `unknown`, are the states in
which criterion 1 could be satisfied with no change set measured, no suite
command run and no scenario executed. The round's acceptance closes that:
see
[how the merge round reads the judgement](#how-the-merge-round-reads-the-judgement).

The measurement requires a known non-empty full diff, with every path
qualifying. A shortened display of changed paths is never sufficient.

Qualifying paths end exactly in `.md`, `.markdown`, or `.rst`, or are root
`LICENSE`, `NOTICE`, or `COPYING`. A `.txt` qualifies under a `docs/`
component or as root `README.txt`, `CHANGELOG.txt`, `CHANGES.txt`,
`LICENSE.txt`, `NOTICE.txt`, `COPYING.txt`, `AUTHORS.txt`,
`CONTRIBUTING.txt`, or `INSTALL.txt`. Qualification is case-sensitive.

Then apply case-insensitive exclusions: paths under any dot-directory or
`amplifier-bundle/`; any `AGENTS.md`, `CLAUDE.md`, or `SKILL.md`;
and `.txt` paths containing `requirements` or `constraints`, or named
`CMakeLists.txt`. Instructions and build inputs are not documentation, and a
`docs/` directory alone does not make code documentation.

An empty or unreadable diff gives `unknown`. One non-documentation path
gives `false` for the whole PR. An explicit scenario-directory override does
not change the measurement; it changes where the judgement looks for
scenarios.

## The record hash

The round binds the record it judged, so a later edit is visible:

| Step | What it does |
| --- | --- |
| `step-00d-qa-evidence-hash` | Runs `autodrive_round_evidence.sh qa-evidence-hash` on `AUTODRIVE_QA_EVIDENCE` after the evidence sub-recipe wrote it and before the assessment agent runs, and reports `qa_evidence_sha`. |
| `step-03`, `step-05` | Re-read the hash, so a record changed after it was judged reports `qa_evidence=modified` and the next round measures and judges again. |

When the path is not a regular file, `qa_evidence_sha` is empty and the tool
prints a `WARNING:` line saying the record was not written, so this round
cannot show the record was unmodified. The hash is a tamper check on a file;
it decides no criterion.

## How the merge round reads the judgement

`autodrive-merge-round.yaml` step-02 reads criterion 1 as the recorded
judgement:

- a `not_user_facing` judgement meets criterion 1 when its reason cites
  files that appear in the PR diff **and** the measured `docs_only` is
  `true`. This is recorded as criterion `qa` Verified with that evidence,
  never NotApplicable;
- a measured `docs_only` of `false` is binding against a `not_user_facing`
  judgement. Criterion 1 is not met, whatever the reason cites, and the next
  round judges the change as user-facing;
- a measured `docs_only` of `unknown` means no change set was measured, so a
  `not_user_facing` judgement meets criterion 1 only where the notes name
  what the change set was judged from, such as the diff that was read
  against the recorded base. Where they do not, criterion 1 is not met and
  the next round measures and judges again. The rule is there because an
  unmeasured change set is the one state in which the cited files have no
  measurement to be checked against;
- the `qa-measurement-unavailable` record carries no `docs_only` field at
  all. An absent field is read as `unknown` and takes the rule above, never
  as a fourth state of its own. A documentation-only PR behind a stale
  bundle or a containment failure therefore still meets criterion 1 on a
  judgement whose notes name what the change set was judged from, and no
  blocker leaves criterion 1 unreachable in every later round;
- a `user_facing` judgement meets criterion 1 when the scenarios covering
  this PR's change were run and passed, as the notes report. A passing
  declared command alone does not;
- `qa_status`, `gadugi_required` and `gadugi_status` are facts in the
  write-up. None of them meets or holds criterion 1 by itself;
- where the notes report a named blocker still open, or scenarios that do
  not cover the change, criterion 1 is not met and the next round acts on
  it;
- where `qa_handoff` is set, the round records the blocker with its owner
  and what they need, and the drive ends on the hand-off path of
  [step failure recovery](step-failure-recovery.md). Only the named owner
  clears it.

Step-04 addresses an open criterion-1 blocker in the same loop: it uses
`Skill(skill="qa-team")` to write or extend this PR's scenarios in
`gadugi_scenario_dir`, commits them, runs only those scenarios with a
private `TMPDIR` and a private `tmux` socket, and never deletes or weakens a
failing scenario to reach a pass. An absent `gadugi-test` is answered with
other evidence or a hand-off, never with an install.

`autodrive_round_word.sh` (ready, not-ready, waiting, hand-off) is the loop's
own control and reads none of these fields.

## Retired stop reasons

These criterion-1 outcomes no longer exist. None of them ends a drive.

| Retired | What happens now |
| --- | --- |
| `qa-gadugi-no-scenarios`, the `NO_SCENARIOS` hold | The judgement writes or extends scenarios in the PR and runs them. `no-scenarios` survives only as a measured `qa_reason`. |
| `gadugi-validate-failed`, `gadugi-run-failed`, `gadugi-scenario-unnamed` | Gone: the tool runs no scenario, so it cannot observe a validation or a run failing, and the unknown-reason safety net records the incomplete measurement `qa-command-missing` instead. A reader that meets one of them is reading an older tool's record. The judgement classifies a failure it sees as `product`, `scenario` or `environment`, fixes it and runs again. |
| `gadugi-test-missing` kept as a blocker in every report | An absent tool is a fact. Criterion 1 is met by other evidence, or a hand-off names the tool and its owner. It is never installed automatically. |
| `gadugi_required` deciding criterion 1 from the repository type | Reported as a fact only. The judgement decides what testing a PR needs. |
| "Missing required QA tooling remains BLOCKED" | Replaced by `qa-tool-not-installed:<tool>`, a recoverable input. |
| `qa-command-failed` as an outcome | Evidence: the judgement decides whether the PR caused it. |
| "explicit operator selection preserved" for an operator scenario directory that does not exist | `qa-scenario-dir-missing:<path>`, with a constrained fallback or a hand-off. |
| `QA_UNRESOLVED` and `BLOCKED` holding the round | Measured facts and inputs to the judgement. |

`autodrive_gadugi_required`, `autodrive_repo_type`, `autodrive_qa_target`,
`autodrive_qa_config`, `autodrive_qa_docs_only` and
`autodrive_scenario_results` remain in `autodrive_trust.sh` as measurements.
No rule was added to a shell script for this change.

## Evidence

The measurement records the selected source, type, command, directory, base,
head, changed scope, scenario directory with its state, scenario list,
`qa_criterion1_declaration`, suite exit statuses and its own summary. The
judgement records whether the change is user-facing with the diff files it
cites, the scenarios covering this PR's change with their results and
commits, each failure with its cause and what was done, any named blocker and
how it was cleared, and any hand-off. A configured suite does not settle what
the judgement must exercise.

Finalization reassesses the judgement at the verified head. A
`not_user_facing` judgement recorded for an earlier documentation-only diff
does not carry over to a head that changes code: the citation no longer
matches the diff, and the round judges again. See [evidence validity](auto-drive-evidence.md) and
[workflow configuration](auto-drive-to-merge.md).

## Related

- [Record a criterion-1 QA judgement by hand](../howto/record-criterion-1-qa-judgement.md)
- [Criterion-1 judgement guidance](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md)
- [Auto-drive evidence and outcomes](auto-drive-evidence.md)
- [Auto drive to merge reference](auto-drive-to-merge.md)
- [Step failure recovery](step-failure-recovery.md)
- [Loop-health evaluator](loop-health-evaluator.md)
