---
title: Auto-drive QA (Criterion 1)
description: How criterion 1 is judged inside auto-drive - the measured QA record, the declared QA command, the named recoverable blockers, and the recorded judgement the merge round reads.
last_updated: 2026-11-21
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive QA (Criterion 1)

Criterion 1 of auto-drive's merge readiness is a recorded QA judgement. The
judging agent reads the PR diff and linked issue, exercises the PR's own
user-visible change as a user would, and records the results. A change with
no user-visible effect earns a judgement supported by changed files. The PR
is the scope of the judgement. Cross-repository user journeys are outside
that scope.

The drive takes its QA tools and
[criterion-1 guidance](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md)
from a trusted installed bundle outside the worktree under review. QA uses
the actual linked worktree. See [shared evidence and finalization](auto-drive-evidence.md).

## Contents

- [How a round judges criterion 1](#how-a-round-judges-criterion-1)
  - [The measurement base](#the-measurement-base)
  - [Light drives](#light-drives)
  - [Measuring by hand](#measuring-by-hand)
- [What the judgement records](#what-the-judgement-records)
- [Named recoverable blockers](#named-recoverable-blockers)
- [The measured record](#the-measured-record)
  - [The status values](#the-status-values)
- [Source precedence](#source-precedence)
- [Operator environment](#operator-environment)
- [Repository configuration](#repository-configuration)
  - [Declaring how criterion 1 is met](#declaring-how-criterion-1-is-met)
- [CI selection](#ci-selection)
- [Manifest detection and scope](#manifest-detection-and-scope)
- [Scenario judgement](#scenario-judgement)
- [Documentation-only measurement](#documentation-only-measurement)
- [The record hash](#the-record-hash)
- [How the merge round reads the judgement](#how-the-merge-round-reads-the-judgement)
- [Evidence](#evidence)

## How a round judges criterion 1

| Step | Responsibility |
| --- | --- |
| `step-02a-qa-inputs` | Runs `autodrive_qa_evidence.sh` from the round's tools directory (or `$AMPLIHACK_HOME/amplifier-bundle/tools`) in the PR worktree. The tool measures the change set, the scenario inventory, whether `gadugi-test` is present, and the suite command outcomes. It runs no scenario. |
| `step-02-qa-team-scenarios` | Reads the linked issue, the diff, the guidance and the measurement. Records a judgement backed by the diff and, for a user-facing change, by outside-in results. Diagnoses failures, repairs the product or the scenario, and runs the affected scenario again. |

The measurement step passes the tool's output on unchanged, and the agent
reads whatever facts it carries. Where the tool is missing or prints
nothing, the step prints a record naming `qa-measurement-unavailable` and
exits 0, and the agent judges from the diff. The agent reads the guidance
from the installed bundle, never from the worktree under review.

### The measurement base

The base decides the merge base used for the changed paths, `docs_only` and
the repository's committed QA configuration. The measurement takes it from
`AUTODRIVE_QA_BASE` and records it as `qa_base`. On the merge path the round
passes down the base its own preflight observed for the PR
(`autodrive_base_branch`); before #2569 the step-02 agent set
`AUTODRIVE_QA_BASE` to the same value. Before a PR exists, `light-qa-inputs`
passes the branch `origin/HEAD` names, which is the branch the light run
starts from and opens the PR against. Where no base is passed, or the scope
helper cannot resolve it, the changed paths read `(unknown)` and `docs_only`
reads `unknown`, and the agent takes the change set from the diff.

### Light drives

A light run with an existing PR uses the merge-round QA path. For a new
change, `light-qa-inputs` measures and `light-qa` records a pre-publication
judgement as `light_checks`. The change is not committed yet, so the agent
reviews `git diff <starting commit>` and `git status --porcelain
--untracked-files=all` rather than the measured paths. The merge round judges
again at the published PR head.

### Measuring by hand

To see the facts a round would judge from, run the tool in the PR's
checkout:

```bash
REPO_PATH="$PWD" AUTODRIVE_QA_BASE=main \
  bash "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_qa_evidence.sh" \
  | jq '{qa_base, head_sha, docs_only, qa_command_paths, gadugi_status, qa_status, qa_reason}'
```

Then follow the
[criterion-1 guidance](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md)
to judge, run and record.

## What the judgement records

| Field | Meaning |
| --- | --- |
| `qa_judgement` | `user_facing` or `not_user_facing`. |
| `qa_judgement_notes` | The reason with the files it cites, the scenarios written, extended or run with their results, each failure with its cause and what was done, and any named blocker. |
| `qa_handoff` | `null` when no person needs to act. A hand-off carries `step_id`, `failure`, `choice` (`handoff`), `reason`, `owner` and `needs`. The owner is a person or a team. See [step failure recovery](step-failure-recovery.md). |

The notes reach a PR comment. Raw command output, environment values,
credentials, private paths and email addresses stay out of them.

## Named recoverable blockers

| Evidence | How the agent responds |
| --- | --- |
| No usable measurement | Names `qa-measurement-unavailable` and judges from the diff. |
| An operator-named scenario directory that does not exist | Names `qa-scenario-dir-missing:<path>`, falls back to the repository's own scenario directory and records both paths, or hands off to the operator. |
| No scenario covers the change | Writes or extends a scenario for the change and runs it. |
| An absent runner or suite executable | Names `qa-tool-not-installed:<tool>`, uses other repository evidence, or hands off to the tool's owner. Installs nothing. |
| A failing suite command or scenario | Classifies the cause as product, scenario or environment, repairs what the PR can repair, and runs it again. |

If a person must act, the agent records a hand-off. Otherwise an unresolved
gap stays in the readiness and repair loop.

## The measured record

The record is one JSON line of string fields, printed on standard output
and, when `AUTODRIVE_QA_EVIDENCE` is set, written to that file with mode
`0600`. The judging agent reads the file and does not change it; see
[the record hash](#the-record-hash).

| Field | Meaning |
| --- | --- |
| `qa_status`, `qa_reason` | The measurement's own summary. See [the status values](#the-status-values). |
| `qa_repo_type` | `rust-cli`, `python`, `node`, `unknown`, or `configured` for a command selected from configuration. |
| `qa_dir`, `qa_command`, `qa_command_source`, `qa_exit_code` | Where and what ran, from which source, and its exit status. See [source precedence](#source-precedence). |
| `qa_criterion1_declaration` | The merge-base `.amplihack/qa.toml` declaration (#2458): `none`, `multi-area-command` or `ci-job:<name>`. |
| `qa_command_paths` | The changed paths, or `(unknown)`. |
| `qa_candidates`, `qa_ci_skip`, `qa_suite_commands_count` | Unresolved candidate projects, why CI selection was skipped, and how many suite commands ran. |
| `qa_scenarios` | Scenario files, as repository-relative paths. From `gadugi_scenario_dir` where it was consulted; for a Rust CLI project, the selected project's `tests/parity/scenarios/*.y{a,}ml`. |
| `qa_summary` | The causes in the order they were found, followed by the last lines of the suite output, sanitised. |
| `qa_round` | `AUTODRIVE_ROUND_LABEL`, or `round`. |
| `qa_base` | The base the change set was measured against, as the caller supplied it; empty where none was. |
| `head_sha` | The commit measured, or empty where there is none. |
| `docs_only` | `true`, `false` or `unknown`, from the [documentation-only measurement](#documentation-only-measurement). |
| `gadugi_required` | What `autodrive_gadugi_required` reports: `true` for a Node project and wherever `AUTODRIVE_QA_SCENARIO_DIR` is set; `false` for a documentation-only PR, a Rust CLI or Python project, and under a #2458 declaration. It decides only whether the tool looks for a scenario directory. |
| `gadugi_status` | `NOT_REQUIRED` (no directory consulted), `NOT_INSTALLED`, `NO_SCENARIOS` or `FOUND` (scenarios listed; none run). |
| `gadugi_scenario_dir`, `gadugi_scenario_dir_state` | The scenario directory, and how it was found: `not-consulted`, `operator` (named by `AUTODRIVE_QA_SCENARIO_DIR`), `operator-missing` (named and absent), `detected` (the first present of `tests/agentic`, `tests/gadugi/scenarios` and `scenarios`) or `absent` (none present; `tests/agentic` is reported). |
| `gadugi_scenario_count` | How many scenario files a consulted directory held. |
| `gadugi_validate_exit_code`, `gadugi_run_exit_code`, `gadugi_scenarios_validated`, `gadugi_scenarios_run`, `gadugi_scenarios_passed`, `gadugi_scenarios_failed`, `gadugi_failed_scenarios`, `gadugi_scenario_results` | Kept so older readers still parse the record. The tool runs no scenario, so each is always `""` or `0`. The agent's own runs are in `qa_judgement_notes`. |
| `git_env_cleared`, `run_env_cleared` | See [git repository environment helper](git-repo-env-helper.md). |

### The status values

`qa_status` and `qa_reason` describe the measurement. They are inputs to
the judgement, not its answer.

| `qa_status` | `qa_reason` | Measured when |
| --- | --- | --- |
| `PASS` | `""` | Every suite command that ran exited 0, and the scenario discovery is complete (`gadugi_status` `NOT_REQUIRED` or `FOUND`). |
| `FAIL` | `qa-command-failed`, `no-scenarios` | A suite command exited non-zero, or the scenario directory holds no scenario. |
| `BLOCKED` | `qa-config-invalid`, `qa-command-missing`, `qa-command-not-installed`, `gadugi-test-missing` | The suite could not be selected or run, or `gadugi-test` is absent. |
| `QA_UNRESOLVED` | `qa-unresolved` | Manifest detection found several candidate projects and could not scope the change to one. |

`qa_reason` is the first token that applies. Where nothing failed by name
and the measurement is still incomplete, the tool records
`qa-command-missing` with `BLOCKED` rather than `PASS`. The tool no longer
writes `gadugi-validate-failed`, `gadugi-run-failed` or
`gadugi-scenario-unnamed`, because it runs no scenario.

`qa-command-missing` means the scope helper did not load or the merge base
could not be resolved only when `qa_command_paths` is `(unknown)`,
`docs_only` is `unknown` and `qa_repo_type` is `unknown`. Otherwise the
change was measured and the token says that the repository declares no suite
command, or that `AUTODRIVE_QA_DIR` does not exist.

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

Treat suites and scenarios as branch-controlled code, including suites
selected from a declaration at the merge base. Use the test execution
boundary for the reviewed worktree and keep publishing credentials,
agent profiles, and credential sockets outside it.

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

| Variable | Purpose |
| --- | --- |
| `REPO_PATH` | The checkout to measure. Defaults to the working directory. |
| `AUTODRIVE_QA_BASE` | A branch name for direct measurement. Unset selects head-only preflight. An explicitly empty value means the base is unresolved and leaves the declaration empty. The recipes supply the actual or planned PR target. |
| `AUTODRIVE_QA_HEAD` | For direct scope-helper use, `HEAD` or a full commit SHA present in the clone. The measurement tool sets it to the head it measures. |
| `AUTODRIVE_QA_EVIDENCE` | Optional output path inside private state. See [the measured record](#the-measured-record). |
| `AUTODRIVE_ROUND_LABEL` | Label recorded in `qa_round`. Defaults to `round`. |

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

For projects where scenario detection applies, the merge-base
`.amplihack/qa.toml` can identify where their scenario coverage runs. This
includes Node projects and unresolved candidates that include Node. Set at
most one declaration key. Setting both records `declaration-conflict`.

| Key | Value | `qa_criterion1_declaration` | Criterion 1 is met when |
| --- | --- | --- | --- |
| `multi_area` | `command` only; anything else is `unknown-value` | `multi-area-command` | The declared `command` exits `0`, and it carries the evidence for this PR's change. The measurement searches for and runs no gadugi scenario. |
| `scenarios_ci_job` | A check-run name: 1 to 128 bytes of `A-Za-z0-9`, space and `._/()+:,@%-`, no leading `-` | `ci-job:<name>` | The declared `command` exits `0`, **and** the head has a check-run with exactly that name and every check-run of that name passed. |

For a repository whose root `npm test` covers affected projects across
areas, use this complete declaration:

```toml
command = "npm test"
multi_area = "command"
```

A successful command produces `qa_exit_code` `0` and
`qa_criterion1_declaration` `multi-area-command` when the declaration
applies. The judging agent verifies coverage of the PR's change in that
suite.

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
base, as [the measurement base](#the-measurement-base) sets out. A
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
summary. The named check must pass at the assessed head for the declaration to count.

Readiness and expected checks use a declaration only from this round's
measurement, and only if it describes this PR's change. A PR description,
agent summary, or `qa.toml` read from the PR head cannot supply it. The QA
judgement still says how the declared suite or CI job exercises the PR's own
change, and an agent clearing a blocker leaves declaration policy to the base
branch.

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

The agent selects coverage from the PR's own change and the repository's
outside-in test entry points.

| Detected project | Default suite | What the judgement usually does |
| --- | --- | --- |
| Rust CLI | `cargo test --workspace --locked --no-fail-fast` | Exercises a user-facing change through the repository's own outside-in tests, including parity scenarios under the selected project's `tests/parity/scenarios/`. |
| Python | `pytest` | Exercises a user-facing change through the repository's own outside-in tests. |
| Node | `npm test` | Exercises a user-facing change through the scenario directory's scenarios, writing or extending the ones this PR needs. |
| Unknown | No detected command | Judges from the diff and the scenarios what exercises the change. `qa-command-missing` here says only that no suite is declared, not that the measurement failed. |
| Unresolved candidates | No detected command | The same, naming the candidates from `qa_candidates`. |
| Any, explicit scenario directory | Selected suite still runs | Looks there first. A directory that does not exist is `qa-scenario-dir-missing:<path>`, with a fallback or a hand-off. |
| Unknown merge base | Cannot establish trusted selection | `qa-measurement-unavailable`, recognised by `qa_command_paths` `(unknown)` with `docs_only` `unknown`: takes the change set from the diff and judges from it. |

Reuse, write or extend scenarios that exercise the changed behaviour, and
keep them in the PR. On a shared host, run only scenarios the PR adds or
changes, one at a time, each staged alone outside the repository and run by
its `name:`, under a private `TMPDIR` and with a private socket for every
`tmux` call. The full suite belongs to CI, including all amplihack-rs
`tests/gadugi/` scenarios.

## Documentation-only measurement

`docs_only` classifies paths for measurement. All values are advisory for
the QA judgement. `true` supports a documentation-only reading. `false`
means the path rules found at least one other kind of file. `unknown` or
an absent value calls for inspecting the actual diff to establish scope.

The agent cites changed files and explains whether they affect what users,
operators, or callers observe. A behavior-neutral configuration change
can earn a `not_user_facing` judgement with `docs_only=false`. A file that
changes user-visible behavior needs outside-in coverage, regardless of its
classification.

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

The read-only readiness agent reviews the QA judgement against the PR's
actual diff and current head. It records criterion 1 as verified when:

- A `not_user_facing` judgement cites files in the diff and explains why
  the complete change leaves behavior unchanged. When `docs_only` is
  unknown or absent, the notes also name the source of the change set.
- A `user_facing` judgement shows passing outside-in coverage of the PR's
  own change, including any repair and passing rerun.

Any applicable declared command and named CI check must pass at the
assessed head. Missing coverage, unresolved failures, or a changed head can leave
readiness incomplete. The existing loop sends actionable blockers to
repair and reassesses the resulting commit. Ordinary CI, evidence
integrity, and finalization checks continue to apply.

If the agent records a complete QA hand-off through
[step failure recovery](step-failure-recovery.md), the assessment reports the
owner and requested action in the closing outcome. The round records
`HANDOFF` and retains the owner and requested action in the write-up used
for operator reporting. Finalization remains unselected.

## Evidence

The measurement records the selected source, type, command, directory, base,
head, changed scope, scenario directory with its state, scenario list,
`qa_criterion1_declaration`, suite exit statuses and its own summary. The
judgement records whether the change is user-facing with the diff files it
cites, the scenarios covering this PR's change with their results and
commits, each failure with its cause and what was done, any named blocker and
how it was cleared, and any hand-off. A configured suite does not settle what
the judgement must exercise.

Readiness and finalization use evidence for the verified head. When the
head moves, the drive measures and judges the new diff and reassesses its
checks. See [evidence validity](auto-drive-evidence.md) and
[workflow configuration](auto-drive-to-merge.md).

## Related

- [Criterion-1 judgement guidance](../../amplifier-bundle/skills/merge-ready/criterion-1-qa-judgement.md)
- [Auto-drive evidence and outcomes](auto-drive-evidence.md)
- [Auto drive to merge reference](auto-drive-to-merge.md)
- [Step failure recovery](step-failure-recovery.md)
- [Loop-health evaluator](loop-health-evaluator.md)
