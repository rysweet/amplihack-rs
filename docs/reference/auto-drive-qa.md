---
title: Auto-drive QA Selection
description: QA command precedence, repository detection, scenario obligations, and evidence for the workflow.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive QA Selection

This reference specifies the QA behavior the implementation must preserve.
For its evidence and finalization contracts, see
[current behavior and authority](auto-drive-to-merge.md#current-behavior-and-authority).
It does not establish execution coverage for the
[target host/runtime matrix](auto-drive-to-merge.md#target-compatibility-matrix).

QA runs against the selected PR head in its linked worktree. The agent
establishes prerequisites, executes required tests, inspects results, and
records failures and recovery. A missing required tool is a blocker until
recovered; selecting a different client cannot waive required tests.

## Contents

- [Source precedence](#source-precedence)
- [Operator environment](#operator-environment)
- [Repository configuration](#repository-configuration)
  - [Declaring how criterion 1 is met](#declaring-how-criterion-1-is-met)
- [CI selection](#ci-selection)
- [Manifest detection and scope](#manifest-detection-and-scope)
- [Scenario requirements](#scenario-requirements)
- [Documentation-only changes](#documentation-only-changes)
- [Evidence](#evidence)

## Source precedence

Use the first present source. An invalid earlier source is not permission to
select a later, weaker command.

| Order | Source | Rules |
| --- | --- | --- |
| 1 | Operator environment | Either `AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS` being set selects this source, even if empty. |
| 2 | `.amplihack/qa.toml` at the merge base | An entry of any kind, an invalid `.amplihack` entry, or a PR change to this configuration selects this source. Invalid configuration blocks; it does not fall through. |
| 3 | Test step from merge-base CI | Exactly one clean, in-scope test step across the workflows. Otherwise record the skip reason and use detection. |
| 4 | Merge-base manifests | Detect the project and scope it to the complete changed-path set. Ambiguity remains unresolved. |

The selected command source does not change repository-type scenario
obligations. An operator's suite override cannot turn off gadugi requirements.

Repository configuration, workflows, and candidate manifests come from the
merge-base tree, not working-tree files or PR-authored replacements. A PR
cannot select its own easier suite by adding a manifest or editing the QA
configuration. An unknown base cannot justify optional scenarios or readiness.

## Operator environment

These are environment variables, not recipe context keys.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AUTODRIVE_QA_COMMAND` | unset | One word-split command with globbing disabled, run in `AUTODRIVE_QA_DIR`; no implicit shell evaluation. |
| `AUTODRIVE_QA_DIR` | `.` | Directory for the single command, relative to repository root or absolute. Missing directory blocks. |
| `AUTODRIVE_QA_COMMANDS` | unset | Additional commands, one per line, run from the repository root after the single command. Ignore blank and whole-line comment entries. This operator-controlled source permits shell commands. |
| `AUTODRIVE_QA_SCENARIO_DIR` | first existing of `tests/agentic`, `tests/gadugi/scenarios`, `scenarios`; otherwise `tests/agentic` | Setting it requires gadugi scenarios in any repository type, including a documentation-only change. A missing directory cannot satisfy that requirement. |

An explicitly empty command override records source `env` and blocks with
`qa-command-missing`; it does not fall back. Sources other than the single
environment command choose their own directory.

`AUTODRIVE_QA_BASE` and `AUTODRIVE_QA_HEAD` are internal observation inputs,
replaced with the selected PR's actual base and verified head. Inherited
values cannot redirect evidence to another revision.

## Repository configuration

A root Rust workspace can declare its suite with this complete configuration:

```toml
command = "cargo test --workspace --locked --no-fail-fast"
dir = "."
```

Saved as `.amplihack/qa.toml` and committed before the PR's merge base, this
selects source `repo-file`, type `configured`, directory `.`, and the
command shown. A passing invocation exits zero; failing tests remain blockers.

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
  itself or `.amplihack/qa.toml` blocks this source.

Commands are executed as arguments, never passed through `eval` or implicit
shell source. Invalid configuration reports `qa-config-invalid` with a cause
such as `symlink`, `not-regular`, `too-large`, `syntax`, `unknown-key`,
`duplicate-key`, `declaration-conflict`, `unknown-value`, `missing-command`,
`unsafe-command`, `unsafe-dir`, `dir-not-found`, `changed-in-pr`,
`paths-empty`, or `paths-unknown`.
Do not expose the rejected file contents in diagnostics.

### Declaring how criterion 1 is met

Where detection requires gadugi scenarios (a Node project, or unresolved
candidates of which any is Node), the merge-base `.amplihack/qa.toml` can
declare how criterion 1 is met instead (#2458). A file gives at most one
declaration; both keys together are `declaration-conflict`.

| Key | Value | `qa_criterion1_declaration` | Criterion 1 is met when |
| --- | --- | --- | --- |
| `multi_area` | `command` only; anything else is `unknown-value` | `multi-area-command` | The declared `command` exits `0`. No gadugi scenario is searched for or run. |
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
the same PR records `qa_status` `FAIL`, `qa_reason` `no-scenarios`,
`gadugi_status` `NO_SCENARIOS` and `qa_criterion1_declaration` `none`.

**Read from the merge base only.** The declaration is read from the same
merge-base blob as `command`, never from the PR head or the working tree. A
PR that changes `.amplihack/qa.toml` is refused with `changed-in-pr`, so a PR
cannot declare the rule it is judged by. A new declaration first applies to
the PR after the one that adds it.

**Fails closed.** The declaration is `none`, and gadugi stays required
wherever detection requires it, when `AUTODRIVE_QA_SCENARIO_DIR`,
`AUTODRIVE_QA_COMMAND` or `AUTODRIVE_QA_COMMANDS` is set (even to empty);
qa.toml is absent or invalid; the merge base is unknown or a git read fails;
or, for `scenarios_ci_job` only, the PR changes anything under `.github/`
(`qa_summary` names `ci-job-workflow-changed`). Where gadugi is not required
anyway, such as a Rust or Python project or a
[documentation-only change](#documentation-only-changes), the evidence also
records `none`.

**The declared job is an expected check.** For `ci-job:<name>`, the merge
evidence step passes `<name>` as the third argument to
[`autodrive_expected_checks.sh`](autodrive-expected-checks.md#declared-scenarios-job),
taking it from the measured QA evidence record, never from an agent's
summary. Only `GREEN` meets criterion 1.

The declaration is an input to the merge round's judgement, not a waiver an
agent can write: the merge-round agent uses `qa_criterion1_declaration` from
the evidence and never adds or edits a qa.toml declaration.

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
one top-level candidate. Multiple unmatched candidates give `QA_UNRESOLVED`.

A root project contains all paths. Changing only a nested Node project under
a Cargo root selects Node; changing the root README too selects the root.
Declare a repository-wide command when shared files outside candidates need
wider coverage.

Unrecognized root project files prevent selecting a nested helper as the
main project: `requirements.txt`, `setup.cfg`, `tox.ini`, `Pipfile`,
`go.mod`, `pom.xml`, `build.gradle`, `build.gradle.kts`, `Makefile`,
`makefile`, `GNUmakefile`, `CMakeLists.txt`, `meson.build`, `build.sbt`,
`Gemfile`, `composer.json`, `mix.exs`, and `flake.nix`. With no recognized
root manifest, these yield unknown type at the root and require a declared
suite.

A search error cannot select a readable candidate while hiding another.
Use the root candidate if established; otherwise scope is unresolved. Unsafe
candidate directories are not executed. Missing command or ambiguous scope
blocks until an authorized configuration supplies the required suite.

## Scenario requirements

| Detected project | Default suite | Gadugi scenarios |
| --- | --- | --- |
| Rust CLI | `cargo test --workspace --locked --no-fail-fast` | Not required under qa-team's Rust substitution. Record parity scenarios from the selected project's `tests/parity/scenarios/`. |
| Python | `pytest` | Not required under qa-team's Python substitution. |
| Node | `npm test` | Required unless the documentation-only exception applies or the merge base [declares how criterion 1 is met](#declaring-how-criterion-1-is-met). |
| Unknown | No detected command | A declared suite is required. |
| Unresolved candidates | No detected command | Required if any candidate is Node, unless the merge base [declares how criterion 1 is met](#declaring-how-criterion-1-is-met); suite selection remains blocked without a declared or override command. |
| Any, explicit scenario directory | Selected suite still runs | Required, including documentation-only changes. |
| Unknown merge base | Cannot establish trusted selection | Keep gadugi required; unknown scope cannot grant an exemption. |

Where required, author/update scenarios through `qa-team`, validate with
`gadugi-test validate`, and run each with `gadugi-test run`, one scenario
per process. Every required scenario and suite command must pass. No scenarios,
missing required tooling, validation failure, or execution failure remains
incomplete or blocked. The agent may recover tooling/access when authorized;
it cannot replace the requirement with an assertion.

## Documentation-only changes

The exception requires a known non-empty full diff, with every path qualifying.
A shortened display of changed paths is never sufficient.

Qualifying paths end exactly in `.md`, `.markdown`, or `.rst`, or are root
`LICENSE`, `NOTICE`, or `COPYING`. A `.txt` qualifies under a `docs/`
component or as root `README.txt`, `CHANGELOG.txt`, `CHANGES.txt`,
`LICENSE.txt`, `NOTICE.txt`, `COPYING.txt`, `AUTHORS.txt`,
`CONTRIBUTING.txt`, or `INSTALL.txt`. Qualification is case-sensitive.

Then apply case-insensitive exclusions: paths under any dot-directory or
`amplifier-bundle/`; any `AGENTS.md`, `CLAUDE.md`, or `SKILL.md`;
and `.txt` paths containing `requirements` or `constraints`, or named
`CMakeLists.txt`. Instructions and build inputs are not exempt documentation.
A `docs/` directory alone does not make code documentation.

An empty or unreadable diff is not documentation-only. One non-documentation
path removes the exception for the entire PR. The exception waives scenario
authorship/validation/execution, including existing scenarios, but suite
commands still run and must pass. An explicit scenario-directory override
keeps scenarios required.

## Evidence

Record selected source, type, command, directory, base, head, changed scope,
scenario requirement and reason, `qa_criterion1_declaration`, scenario list,
validation/execution results, suite exit statuses, concrete log references,
blockers, and recovery attempts.
A configured suite does not change independently detected scenario obligations.

Finalization reassesses applicability at the verified head. Evidence marked
not required for an earlier documentation-only diff cannot satisfy a new code
head. See [evidence validity](auto-drive-evidence.md) and
[workflow configuration](auto-drive-to-merge.md).
