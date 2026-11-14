---
title: Auto-drive QA (Criterion 1)
description: How criterion 1 is judged inside auto-drive - the measured inputs file, the declared QA command, the qa-record helper, the evidence record and the PASS, FAIL and HANDOFF outcomes.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive QA (Criterion 1)

Inside auto-drive, criterion 1 of [merge-ready](../../amplifier-bundle/skills/merge-ready/SKILL.md)
is a judgement made by an agent step, not a table of hard stops. A script
measures facts about the PR and writes them to `qa-inputs.json`. The agent
reads those facts with the PR diff, the linked issue and the existing
scenarios, decides whether the PR changes user-facing behaviour, tests it
accordingly, and records the outcome through
`amplihack orch helper autodrive qa-record`. The merge round reads that record.

The written guidance the agent follows is
[criterion-1-qa-judgement.md](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md).
For the evidence and finalization contracts shared by all criteria, see
[current behaviour and authority](auto-drive-to-merge.md#current-behavior-and-authority).

## Contents

- [How a round judges criterion 1](#how-a-round-judges-criterion-1)
- [Outcomes](#outcomes)
- [The inputs file](#the-inputs-file)
- [The declared QA command](#the-declared-qa-command)
- [Operator environment](#operator-environment)
- [Repository configuration](#repository-configuration)
- [CI selection](#ci-selection)
- [Manifest detection and scope](#manifest-detection-and-scope)
- [Documentation-only measurement](#documentation-only-measurement)
- [The qa-record helper](#the-qa-record-helper)
- [The judgement record](#the-judgement-record)
- [The evidence record](#the-evidence-record)
- [How the merge round reads the record](#how-the-merge-round-reads-the-record)
- [Removed stop reasons](#removed-stop-reasons)

## How a round judges criterion 1

`autodrive-merge-evidence.yaml` runs two steps for criterion 1, in this
order, before round step-00d hashes the evidence:

| Step | Kind | What it does |
| --- | --- | --- |
| `step-02a-qa-inputs` | bash | Runs the trusted `autodrive_qa_evidence.sh`. It selects and runs the declared QA command, measures the changed files, the documentation-only flag, the repository type and the existing scenarios, and writes `qa-inputs.json`. It issues no verdict. |
| `step-02-qa-team-scenarios` | agent | Reads the guidance, the diff, the linked issue, `qa-inputs.json` and the existing scenarios. Judges, tests, fixes and re-runs until the result is `PASS` or `HANDOFF`, then calls `qa-record` as its last action. Its output `qa_evidence` is the text `qa-record` printed. |

`step-02a-qa-inputs` sets `AUTODRIVE_QA_BASE` to the PR's observed base
branch and `AUTODRIVE_QA_HEAD` to the verified head SHA, replacing any
inherited values (see [base and head](#base-and-head)).

The agent loop inside step-02:

1. Decide `user_facing` or `not_user_facing`, citing the changed files that
   support the decision.
2. For `not_user_facing` (documentation, configuration with no behaviour
   change), record the reason and the cited files, and pass.
3. For `user_facing`, find the affected user journeys and their outside-in
   scenarios. Write or extend missing scenarios in the PR's scenario
   directory and commit them to the PR branch.
4. Run the scenarios, in CI or isolation where the repository requires it.
5. On a failure, classify its cause as `product`, `scenario` or
   `environment`, fix it, commit the fix and run again. Each attempt is one
   entry in `qa_iterations`.
6. Stop on `PASS`, or on `HANDOFF` when only a person can clear the cause.
   Record `FAIL` only when the same failure recurs with no new fix to try.

No missing scenario, repository type, missing tool or failing declared
command ends a drive by itself. Only a recorded `HANDOFF` hands the drive to
a person.

### Light drives

`auto-drive-light.yaml` checks the change before the PR exists, so it has its
own pair of steps, in this order after `light-implement`. Both run under
the same condition as before, `not pr_number and not existing_branch`.

| Step | Kind | What it does |
| --- | --- | --- |
| `light-qa-inputs` | bash | Runs `autodrive_qa_evidence.sh` with `AUTODRIVE_QA_EVIDENCE` fixed at `$QUICK_CHANGE_STATE_DIR/qa/qa-evidence.json`, `AUTODRIVE_QA_BASE` set to the repository's default branch (the branch `light-worktree` started from) and `AUTODRIVE_QA_HEAD` set to the worktree's `HEAD`. Writes `qa-inputs.json` beside that path. |
| `light-qa` | agent | Follows the same guidance, calls `qa-record` with those two paths and returns its output as `light_checks`. |

The light record is a pre-PR check. It does not meet criterion 1: once the
PR is open, `light-merge-loop` runs `autodrive-merge-loop.yaml`, and each of
its rounds runs `step-02a-qa-inputs` and `step-02-qa-team-scenarios` against
the PR head and records again. The `light-prereqs` step no longer looks for
`gadugi-test`; it prints `{}`.

## Outcomes

| `qa_status` | Meaning | Effect on the round |
| --- | --- | --- |
| `PASS` | The judgement is recorded and its testing passed. | Criterion 1 is met when the record is consistent (see [below](#how-the-merge-round-reads-the-record)). |
| `FAIL` | Testing failed and the agent stopped without a new fix. An interim state. | The round is not merge-ready. Step-04 fixes, commits and records again. The drive continues. |
| `HANDOFF` | Only a person can clear the cause, for example a service the scenarios need is unreachable. | The round is not merge-ready. The blocker is owned by the named person and the drive ends on the same path `BLOCKED` follows. |

`BLOCKED` and `QA_UNRESOLVED` are no longer criterion-1 statuses. An
unresolved or invalid declared command is an input to the judgement.

## The inputs file

`step-02a-qa-inputs` writes `qa-inputs.json` in the same directory as
`qa-evidence.json`: the path is `${AUTODRIVE_QA_EVIDENCE%/*}/qa-inputs.json`.
`AUTODRIVE_QA_EVIDENCE` must be an absolute path containing a `/`; otherwise
the script exits 1. The file is written under `umask 077` to a `mktemp`
file and moved into place with `mv -f`. The script also exits 1, writing
nothing, when it cannot enter `REPO_PATH`: measuring any other directory
would record that directory's files and head as the PR's. Otherwise it exits
1 only when it cannot write the file; every other condition is recorded
inside it.

Example for a PR that changes two Markdown files in a Node repository:

```json
{
  "schema": 1,
  "qa_round": "round-1",
  "head_sha": "9f2c1e4b7a0d3f6e8c5b2a1d4e7f0a3c6b9d2e5f",
  "base_sha": "4e0e6f8d4c1a2b3c4d5e6f708192a3b4c5d6e7f8",
  "repo_type": "node",
  "docs_only": "true",
  "changed_files": ["docs/install.md", "README.md"],
  "scenario_dir": "tests/agentic",
  "existing_scenarios": [],
  "declared_qa": {
    "source": "detected",
    "dir": ".",
    "command": "npm test",
    "status": "ran",
    "exit_status": 0,
    "cause": "",
    "candidates": "",
    "ci_skip": "no-match",
    "log_tail": "Tests: 42 passed, 42 total"
  },
  "git_env_cleared": "none",
  "run_env_cleared": "none"
}
```

| Field | Meaning |
| --- | --- |
| `schema` | Always `1`. |
| `qa_round` | `AUTODRIVE_ROUND_LABEL`, or `round`. |
| `head_sha`, `base_sha` | The PR head tested and the merge base the command source was read from. |
| `repo_type` | `rust-cli`, `python`, `node` or `unknown`. A combination outside the detection table is `unknown`, never an error. |
| `docs_only` | `true`, `false` or `unknown`, from the [documentation-only measurement](#documentation-only-measurement). A fact for the agent, not an exemption. |
| `changed_files` | The full changed-path list from `git diff --name-only -z`. Names with newlines or quotes are JSON-encoded. |
| `scenario_dir` | `AUTODRIVE_QA_SCENARIO_DIR`, else the first existing of `tests/agentic`, `tests/gadugi/scenarios` and `scenarios`, else `tests/agentic`. |
| `existing_scenarios` | Scenario files tracked in `scenario_dir`, from `git ls-files -z`. |
| `declared_qa` | The declared command and its result. See [the declared QA command](#the-declared-qa-command). |
| `git_env_cleared`, `run_env_cleared` | See [git repository environment helper](git-repo-env-helper.md). |

`log_tail` stays in the inputs file. It is never copied into
`qa-evidence.json`, because test output can contain secrets.

`qa-record` requires every field above except the fields inside
`declared_qa`: each of those may be absent and then defaults to empty (`""`,
or null for `exit_status`), so `"declared_qa": {}` is accepted and is
recorded with an empty status. An empty status means no declared command was
run. `qa-record` ignores fields it does not know, so a newer inputs script
(#2458) does not break an older helper. The strings it copies into the evidence (`qa_round` when `--round` is absent,
`repo_type`, `docs_only`, `git_env_cleared`, `run_env_cleared` and the
`declared_qa` strings except `log_tail`) must meet the judgement's
[type limits](#type-limits): the inputs file sits in a directory the agent
can write, so it is checked like standard input. A field that breaks them
exits 1, names the field (for example `inputs.declared_qa.command`) and not
its value, and writes nothing.

Before the PR exists (`light-qa-inputs`), the change is uncommitted, and the
script diffs committed history only. `changed_files`, `base_sha` and
`docs_only` therefore do not describe the change there; `light-qa` takes the
change set from the working tree instead.

When `git diff` from a known merge base fails, the script prints a
`WARNING:` line, writes `changed_files` as `[]` and clears `base_sha`, the
recorded sign that the change set is unknown. When `git ls-files` fails,
`existing_scenarios` is `[]` and a `WARNING:` line says the listing failed.

## The declared QA command

The declared command is evidence for the judgement, not a verdict. A
failing command does not end the drive; the agent decides whether the
failure is caused by the PR, fixes it, or hands it off.

The command comes from the first present source. An invalid earlier source
is not permission to select a later, weaker command.

| Order | Source | Rules |
| --- | --- | --- |
| 1 | Operator environment | Either `AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS` being set selects this source, even if empty. |
| 2 | `.amplihack/qa.toml` at the merge base | An entry of any kind, an invalid `.amplihack` entry, or a PR change to this configuration selects this source. Invalid configuration is recorded; it does not fall through. |
| 3 | Test step from merge-base CI | Exactly one clean, in-scope test step across the workflows. Otherwise record the skip reason and use detection. |
| 4 | Merge-base manifests | Detect the project and scope it to the complete changed-path set. Ambiguity is recorded as `unresolved`. |

Repository configuration, workflows and candidate manifests come from the
merge-base tree, never from the PR head. A PR cannot select its own easier
suite by adding a manifest or editing the QA configuration.

`declared_qa` fields:

| Field | Values |
| --- | --- |
| `source` | `env`, `repo-file`, `ci`, `detected` or `none`. |
| `dir`, `command` | Where and what ran, or empty. |
| `status` | `ran`, `missing`, `not-installed`, `unresolved` or `config-invalid`. |
| `exit_status` | The command's exit status, or `null` when nothing ran. |
| `cause` | For `config-invalid`, the cause listed under [repository configuration](#repository-configuration); for `missing` and `not-installed`, a short description. |
| `candidates` | For `unresolved`, the candidate project directories. |
| `ci_skip` | Why CI selection was skipped, or empty. |
| `log_tail` | The last lines of the command's output. |

The block that builds `declared_qa` in `autodrive_qa_evidence.sh` carries the
comment `# #2458: add kind / scenarios fields from qa.toml here`. It is the
one place where further `qa.toml` keys from #2458 enter the judgement. It
must read only the merge-base copy.

## Operator environment

These are environment variables, not recipe context keys.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One word-split command with globbing disabled, run in `AUTODRIVE_QA_DIR`; no implicit shell evaluation. |
| `AUTODRIVE_QA_DIR` | `.` | Directory for the single command, relative to repository root or absolute. A missing directory gives `status` `missing`. |
| `AUTODRIVE_QA_COMMANDS` | unset | Additional commands, one per line, run from the repository root after the single command. Blank and whole-line comment entries are ignored. This operator-controlled source permits shell commands. |
| `AUTODRIVE_QA_SCENARIO_DIR` | first existing of `tests/agentic`, `tests/gadugi/scenarios`, `scenarios`; otherwise `tests/agentic` | Where the agent looks for and writes scenarios. It no longer makes scenarios compulsory. |
| `AUTODRIVE_QA_EVIDENCE` | set by the recipe | Absolute path of `qa-evidence.json`. `qa-inputs.json` is written beside it. |

An explicitly empty command override records source `env` and `status`
`missing`; it does not fall back.

### Base and head

`autodrive_qa_evidence.sh` reads two further variables. The recipe steps set
both and replace inherited values, so an operator cannot redirect a drive's
evidence to another revision. Run by hand, the script uses what you set.

| Variable | Meaning when the script runs |
| --- | --- |
| `AUTODRIVE_QA_BASE` | The base branch name, without `origin/`. The script resolves `refs/remotes/origin/<base>`, so fetch it first, and takes the merge base of that and the head. A name that fails `git check-ref-format --branch`, or a missing remote-tracking ref, makes the changed paths unknown. Unset: no base, so the merge base is the head, `changed_files` is empty and `docs_only` is `unknown`. |
| `AUTODRIVE_QA_HEAD` | The head to measure: a full commit SHA present in the clone. Default `HEAD`. Anything else makes the changed paths unknown. |

Unknown changed paths are recorded, not errors: `docs_only` is `unknown` and
the command source falls to the rules for unknown paths below.

## Repository configuration

A root Rust workspace can declare its suite with this complete configuration:

```toml
command = "cargo test --workspace --locked --no-fail-fast"
dir = "."
```

Saved as `.amplihack/qa.toml` and committed before the PR's merge base, this
selects source `repo-file`, directory `.`, and the command shown.

The configuration contract is deliberately narrow:

- At most 4096 bytes; a regular Git blob, never a symlink, submodule, or
  directory. `.amplihack` must be a directory in the merge-base tree.
- Lines are blank, whole-line `#` comments, or `key = "value"`. Values have
  no backslashes; no tables, inline comments, duplicate or unknown keys.
- Bytes are tabs, line feeds, or printable ASCII. No carriage returns or NULs.
- `command` is required and non-empty after word splitting. `dir` defaults
  to `.`.
- Commands use only letters, digits, spaces, and `._/-=+:,@%`. Shell
  substitutions, redirections, quoting, pipelines, and wildcards are rejected.
- Directories are relative, use only `A-Za-z0-9._/-`, do not start with
  `/` or `-`, and have no empty or `..` component. The physical directory
  must exist inside the repository.
- Changed paths must be known and non-empty. A PR changing `.amplihack`
  itself or `.amplihack/qa.toml` is refused for this source with cause
  `changed-in-pr`.

Commands are executed as arguments, never passed through `eval` or implicit
shell source. Invalid configuration gives `status` `config-invalid` with a
cause such as `symlink`, `not-regular`, `too-large`, `syntax`,
`unknown-key`, `duplicate-key`, `missing-command`, `unsafe-command`,
`unsafe-dir`, `dir-not-found`, `changed-in-pr`, `paths-empty`, or
`paths-unknown`. The rejected file contents are never shown.

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
selection. `ci_skip` is one of `paths-empty`, `paths-unknown`,
`changed-in-pr`, `no-workflows`, `list-failed`, `too-many-files`,
`unsafe-name`, `not-regular`, `too-large`, `read-failed`, `not-clean`,
`no-match`, `several-matches`, `unsafe-command`, `unsafe-dir`,
`dir-not-found`, or `out-of-scope`. Skipping CI command selection does not
waive platform CI checks.

## Manifest detection and scope

Candidates are directories with regular merge-base `Cargo.toml`,
`pyproject.toml`, `setup.py`, or `package.json` manifests within the
established search depth (for example, `a/b/Cargo.toml` is included,
`a/b/c/Cargo.toml` is not). Hidden directories, `target`, `worktrees`, and
`node_modules` are excluded. Same-directory precedence is Cargo, Python,
Node.

Nested candidates collapse when an ancestor has the same ecosystem;
different ecosystems remain separate. The deepest candidate containing every
changed path that belongs to any candidate is chosen. Paths outside all
candidates are set aside. If no paths remain, or paths are unknown or empty,
exactly one top-level candidate is required. Several unmatched candidates
give `status` `unresolved`.

A root project contains all paths. Changing only a nested Node project under
a Cargo root selects Node; changing the root README too selects the root.
Declare a repository-wide command when shared files outside candidates need
wider coverage.

Unrecognized root project files prevent selecting a nested helper as the
main project: `requirements.txt`, `setup.cfg`, `tox.ini`, `Pipfile`,
`go.mod`, `pom.xml`, `build.gradle`, `build.gradle.kts`, `Makefile`,
`makefile`, `GNUmakefile`, `CMakeLists.txt`, `meson.build`, `build.sbt`,
`Gemfile`, `composer.json`, `mix.exs`, and `flake.nix`. With no recognized
root manifest, these give `repo_type` `unknown` and `status` `missing`.

A search error cannot select a readable candidate while hiding another.
Unsafe candidate directories are not executed. A missing command or an
ambiguous scope is recorded for the agent, which judges from the diff and
the scenarios what testing the PR needs.

## Documentation-only measurement

`docs_only` is a measured fact the agent may cite. It is not an exemption:
a documentation-only PR passes only through a recorded `not_user_facing`
judgement that cites its files.

The measurement requires a known non-empty full diff, with every path
qualifying. Qualifying paths end exactly in `.md`, `.markdown`, or `.rst`,
or are root `LICENSE`, `NOTICE`, or `COPYING`. A `.txt` qualifies under a
`docs/` component or as root `README.txt`, `CHANGELOG.txt`, `CHANGES.txt`,
`LICENSE.txt`, `NOTICE.txt`, `COPYING.txt`, `AUTHORS.txt`,
`CONTRIBUTING.txt`, or `INSTALL.txt`. Qualification is case-sensitive.

Case-insensitive exclusions then apply: paths under any dot-directory or
`amplifier-bundle/`; any `AGENTS.md`, `CLAUDE.md`, or `SKILL.md`; and
`.txt` paths containing `requirements` or `constraints`, or named
`CMakeLists.txt`. Instructions and build inputs are not documentation. An
empty or unreadable diff gives `unknown`. One non-documentation path gives
`false` for the whole PR.

## The qa-record helper

`qa-record` is the only way a judgement reaches `qa-evidence.json`. It
checks the record's shape and types and writes it. It judges nothing and
does not compare cited files with the diff.

```text
amplihack orch helper autodrive qa-record \
  --inputs <qa-inputs.json> --repo <repository> --out <qa-evidence.json> [--round <label>]
```

| Option | Meaning |
| --- | --- |
| `--inputs` | The inputs file from `step-02a-qa-inputs`. Opened with `O_NOFOLLOW`; must be a regular file of at most 1 MiB. |
| `--repo` | The PR worktree. `head_sha` is read here with `git rev-parse --verify HEAD^{commit}`, with inherited `GIT_*` variables removed and Git configuration isolated. |
| `--out` | The evidence file to write. Its parent directory must be private: owned by the user, with no group or other access (mode `0700` or stricter), the same check the auto-drive evidence store applies. |
| `--round` | Overrides `qa_round` from the inputs file. |

The judgement is read from standard input, at most 1 MiB.

| Exit | Meaning |
| --- | --- |
| `0` | The record was written. Standard output holds exactly the bytes written. |
| `2` | The judgement was refused. Nothing was written. |
| `1` | Environment error: the inputs file, the repository or the output directory could not be used. Nothing was written. This includes an inputs file whose copied fields break the [type limits](#type-limits). |

A refusal prints one line on standard error, at most 200 bytes, naming the
rule and the field path. It never repeats the refused value:

```text
qa-record: refused: handoff-incomplete at qa_handoff.owner
```

Environment errors name the operation and the operating-system error, never
file contents.

The file is written to a temporary file in the same directory, created with
`O_CREAT|O_EXCL|O_NOFOLLOW` and mode `0600` whatever the umask, then synced,
renamed over `--out`, and the directory synced. A symlink at `--out` is
replaced by a regular file; its target is never written.

## The judgement record

Every field is required. Absent values are written as `[]` or `null`.
Unknown fields are refused, so `head_sha` and `qa_declared_command` cannot
be supplied on standard input.

```json
{
  "qa_status": "PASS",
  "qa_judgement": "not_user_facing",
  "qa_judgement_reason": "The PR changes only installation prose; no code, configuration or build input changes.",
  "qa_cited_files": ["docs/install.md", "README.md"],
  "qa_scenarios": [],
  "qa_iterations": [],
  "qa_handoff": null
}
```

| Field | Type |
| --- | --- |
| `qa_status` | `PASS`, `FAIL` or `HANDOFF`. |
| `qa_judgement` | `user_facing` or `not_user_facing`. |
| `qa_judgement_reason` | Non-empty string, at most 16 KiB. |
| `qa_cited_files` | List of repository-relative paths. |
| `qa_scenarios` | List of `{path, written_in_pr, commit, result}`. `written_in_pr` is a boolean; `commit` is a commit ID or `null`; `result` is `pass` or `fail`. |
| `qa_iterations` | List of `{attempt, failing_scenario, cause, fix_commit, result}`. `attempt` is a positive integer; `failing_scenario` a path, or `null` when the attempt was a run of the declared QA command recorded in `declared_qa`; `cause` is `product`, `scenario` or `environment`; `fix_commit` a commit ID or `null`; `result` is `pass` or `fail`. |
| `qa_handoff` | `null`, or `{step_id, failure, choice, reason, owner, needs}` with `choice` equal to `handoff`. These names match the arguments of `orch helper step-recovery record`. |

### Shape rules

| Rule id | Refused when |
| --- | --- |
| `schema` | A field is missing, unknown or of the wrong type, or breaks a type limit below. |
| `reason-empty` | `qa_judgement_reason` is empty. |
| `not-user-facing-needs-cited-files` | `not_user_facing` with `PASS` cites no file. |
| `user-facing-needs-passing-scenario` | `user_facing` with `PASS` lists no scenario whose `result` is `pass`. A passing declared QA command does not satisfy this rule. |
| `handoff-incomplete` | `HANDOFF` with `qa_handoff` `null`, or with an empty `step_id`, `failure`, `reason`, `owner` or `needs`. |
| `handoff-without-status` | `qa_handoff` is set while `qa_status` is not `HANDOFF`. |
| `iteration-order` | `attempt` values are not 1, 2, 3 and so on in order. |
| `input-too-large` | Standard input exceeds 1 MiB. |

### Type limits

Enforced under rule `schema`:

- Strings are a single line, contain no control characters or ANSI escape
  sequences, and are at most 4 KiB (16 KiB for `qa_judgement_reason`).
- Lists hold at most 1,000 entries.
- Paths are relative and have no `..` component. The helper never opens them.
- Commit IDs are 7 to 64 lowercase hexadecimal characters. `HEAD` and branch
  names are refused.

These limits keep the person-facing blocker text and later recovery records
free of forged lines and escape sequences.

## The evidence record

`qa-record` writes the judgement fields, then adds:

| Field | Source |
| --- | --- |
| `head_sha` | `git rev-parse --verify HEAD^{commit}` in `--repo`; 40 or 64 lowercase hexadecimal characters. |
| `qa_reason` | Derived: empty for `PASS`, `qa-scenario-failed` for `FAIL`, `qa-handoff` for `HANDOFF`. |
| `qa_declared_command` | `{source, dir, command, exit_status}` from `declared_qa` in the inputs file. |
| `declared_qa` | The inputs file's `declared_qa` block without `log_tail`. |
| `qa_round`, `qa_repo_type`, `qa_dir`, `qa_command`, `qa_command_source`, `qa_exit_code`, `docs_only`, `git_env_cleared`, `run_env_cleared` | Copied from the inputs file for readers of the earlier flat format. |

The record contains no `gadugi_*` fields and no `log_tail`.

## How the merge round reads the record

`autodrive_round_evidence.sh` reads `qa_status` as before; `PASS` is the
only value that can meet criterion 1. In `autodrive-merge-round.yaml`
step-02, criterion 1 is met only when `qa_status` is `PASS` and the record is
consistent:

- for `not_user_facing`, every cited file appears in the PR diff;
- for `user_facing`, the scenarios list their results;
- a record without `qa_judgement`, or with any `gadugi_*` field, was not
  written by `qa-record` and does not meet criterion 1.

A documentation-only judgement is recorded as criterion `qa` Verified with
this evidence, never NotApplicable.

In step-04:

| `qa_status` | Step-04 action |
| --- | --- |
| `FAIL` | Fix the cause, commit, and run step-02 again, which records again. |
| `HANDOFF` | Record the blocker `qa-handoff: <reason> (owner: <owner>, needs: <needs>)`, owned by a person. The drive ends with that reason and `held_by` `criterion 1: qa_status=HANDOFF`. |

Only the named owner clears a `HANDOFF`. Directly beneath the `HANDOFF`
branch in step-04 is the comment marking the #2539 connection point, where
the hand-off will call `orch helper step-recovery record --step-id …
--reason … --owner … --needs …` once #2570 merges.

If the agent changes files after recording, the step-00d hash no longer
matches, the round sees `qa_evidence=modified`, and the next round records
again.

## Removed stop reasons

These `qa_reason` values and their stops no longer exist. None of them ends
a drive.

| Removed | What happens now |
| --- | --- |
| `no-scenarios` | The agent writes or extends scenarios in the PR. |
| `gadugi-validate-failed`, `gadugi-run-failed`, `gadugi-scenario-unnamed` | The agent classifies the failure, fixes it and runs again. |
| `gadugi-test-missing` | The agent runs scenarios in CI or isolation, or hands off when no runner is reachable. |
| `qa-command-failed` | The failure is evidence; the agent decides whether the PR caused it. |
| `qa-command-missing`, `qa-command-not-installed`, `qa-unresolved`, `qa-config-invalid` | Recorded in `declared_qa.status`; inputs to the judgement. |
| `gadugi_required` and the repository-type scenario table | Removed. The judgement decides what testing a PR needs. |

The helpers `autodrive_gadugi_required` and `_autodrive_qa_needs_gadugi`,
and the `greq` output of `autodrive_qa_report`, are deleted.
`autodrive_repo_type`, `autodrive_qa_target`, `autodrive_qa_config` and
`autodrive_qa_docs_only` remain as measurements.

## Related

- [Record a criterion-1 QA judgement by hand](../howto/record-criterion-1-qa-judgement.md)
- [Criterion-1 judgement guidance](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md)
- [Auto-drive evidence and outcomes](auto-drive-evidence.md)
- [Auto drive to merge reference](auto-drive-to-merge.md)
- [Loop-health evaluator](loop-health-evaluator.md)
