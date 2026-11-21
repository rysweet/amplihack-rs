---
title: QA Blocker Scenario (Issue 2569)
description: The outside-in scenario that drives the shipped amplihack binary through the autodrive-merge-evidence recipe and shows that a QA measurement which cannot be completed is recorded as a named blocker and does not end the round.
last_updated: 2026-11-20
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# QA Blocker Scenario (Issue 2569)

`tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh` is the
outside-in check for [criterion 1 as a recorded judgement](auto-drive-qa.md).
It runs the shipped `amplihack` binary against the shipped
`autodrive-merge-evidence` recipe and observes three things from outside:

- A measurement that cannot be completed is recorded as a named, recoverable
  blocker, and the round is not ended: the recipe's three agent steps all run.
- The change set is measured against the PR's own base, taken from the recipe
  or from a lookup of the PR. Where neither can supply one, the record says so
  and measures no change set.
- A scenario directory the operator named that does not exist is a recorded
  state, whatever bytes its name holds.

The Rust tests in
`crates/amplihack-cli/tests/issue_2569_criterion1_judgement.rs` pin the
recipe step in process. This scenario adds the shipped binary, the pinned
recipe runner and a process boundary between them.

## Contents

- [Files](#files)
- [Where it runs](#where-it-runs)
- [Environment](#environment)
- [Isolation](#isolation)
- [Stand-ins](#stand-ins)
- [Set-up checks](#set-up-checks)
- [Every run](#every-run)
- [Case A: Node repository with no scenarios](#case-a-node-repository-with-no-scenarios)
- [Case B: the base from the recipe, then from the PR, then none](#case-b-the-base-from-the-recipe-then-from-the-pr-then-none)
- [Case C: operator-named scenario directory is missing](#case-c-operator-named-scenario-directory-is-missing)
- [Output and exit status](#output-and-exit-status)
- [What the scenario does not do](#what-the-scenario-does-not-do)
- [Related](#related)

## Files

| Path | Role |
| --- | --- |
| `tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh` | The scenario body. Self-asserting, prints one result line per assertion. |
| `tests/gadugi/scenarios/issue-2569-qa-blocker-not-stop.yaml` | A `gadugi-test` descriptor that wraps the script, matching the descriptors for issues 2785, 2806 and 2857. It expects exit status `0`, `ALL_CASES_PASSED`, and each `PASS` line the script prints, and sets no timeout. It is for running the same cases through `gadugi-test` by hand. |
| `.github/workflows/ci.yml` | One step in the `install-smoke` job, named `Outside-in: QA blockers are named, not stops (issue #2569)`, placed after the issue 2806 step. |

## Where it runs

CI's Install Smoke Test job runs the script directly against the binary that
job installs:

```yaml
- name: "Outside-in: QA blockers are named, not stops (issue #2569)"
  shell: bash
  run: AMPLIHACK_2569_QA_BIN="$(command -v amplihack)" bash tests/gadugi/run-issue-2569-qa-blocker-not-stop-scenario.sh
```

The step holds no expression of the form `${{ ... }}`, names no secret and
leaves the workflow-level `contents: read` permission as it was. CI does not
run `gadugi-test`; the YAML descriptor is not part of the CI path.

Where `/tmp` is shared or limited, run the static checks (`bash -n`,
`shellcheck` and a grep) and read the CI result instead of running the script
there. Running the whole `gadugi-test` suite on a shared host emptied `/tmp`
on 12 November, and this script creates its scratch root under `TMPDIR`. See [run the QA blocker scenario](../howto/run-the-qa-blocker-scenario.md)
for the commands.

## Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `AMPLIHACK_2569_QA_BIN` | unset | Path to an `amplihack` binary to test. Unset, the script builds the binary with `cargo` first, which writes `target/` in the checkout. CI sets it from `command -v amplihack`. |
| `TMPDIR` | `/tmp` | Where the single scratch root is created, with `mktemp -d`. Point it at a private folder on a shared host. |

The pinned recipe runner is resolved once, before any command runs, the way
the sibling scenarios resolve it: `RECIPE_RUNNER_RS_PATH`, then
`recipe-runner-rs` on `PATH`, made absolute. CI does not set
`RECIPE_RUNNER_RS_PATH`, so the `PATH` lookup is the one it uses. The resolved
path is then passed to every command explicitly (see
[isolation](#isolation)), because `HOME` and `PATH` inside a command are not
the caller's. A missing binary, runner or host tool is a failure that names
the missing item. It is never a skip.

## Isolation

Every command under test runs through one function, `scenario_env()`. It
starts `env -i` and passes an allowlist, so nothing else from the caller's
environment reaches the binary, the recipe or the stand-ins.

| Group | Variables | Value |
| --- | --- | --- |
| Search path | `PATH` | One directory, the stand-in directory. It holds the stand-ins, `amplihack` (a link to the binary under test), `recipe-runner-rs` (a link to the resolved runner) and a link for each host tool the recipe steps and the measurement tool call. No `gadugi-test` link is made. |
| Homes | `HOME`, `TMPDIR`, `XDG_CONFIG_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Folders inside the scratch root. Neither `~/.claude` nor `~/.amplihack` of the user running it is read or written. |
| Locale and terminal | `LANG`, `TZ`, `TERM` | `C`, `UTC` and `dumb`. |
| Git | `GIT_CONFIG_NOSYSTEM`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_GLOBAL`, `GIT_TERMINAL_PROMPT`, `GIT_ALLOW_PROTOCOL`, `GIT_ASKPASS`, `SSH_ASKPASS` | `1`, `/dev/null`, `/dev/null`, `0`, `file`, `true`, `true`. |
| Git identity | `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL` | A fixed identity at `qa@example.invalid`. |
| Product paths | `AMPLIHACK_HOME`, `AMPLIHACK_WORK_ROOT`, `AMPLIHACK_CLAUDE_BINARY_PATH`, `AMPLIHACK_LAUNCHER_BINARY`, `RECIPE_RUNNER_RS_PATH`, `RECORD_DIR` | The checkout, a work root inside the scratch root, the stand-in agent, the binary under test, the resolved runner (an absolute path), and the folder where stand-ins record their calls. Each case has its own `TMPDIR`, work root and record folder. |
| Product switches | `AMPLIHACK_WORK_ROOT_EXPLICIT`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_NONINTERACTIVE`, `AMPLIHACK_GOVERNOR`, `AMPLIHACK_ROTATE_AUTOSTART`, `AMPLIHACK_SKIP_REFLECTION`, `AMPLIHACK_SKIP_POWER_STEERING`, `AMPLIHACK_USAGE_LIMIT_WAIT` | `1`, `claude`, `1`, `off`, `0`, `1`, `1` and `0`. Reflection and power steering are off so that the stand-in agent is called only by the recipe's own steps, and the usage-limit wait is `0` so that nothing waits. |
| Stand-in data | `QA_GH_BASE` | `qa-base`, the base the `gh` stand-in answers with. |
| Quiet | `NO_UPDATE_NOTIFIER`, `AMPLIHACK_SKIP_AUTO_INSTALL`, `AMPLIHACK_NO_UPDATE_CHECK`, `AMPLIHACK_NO_FRESHNESS_CHECK`, `RECIPE_RUNNER_NO_UPDATE_CHECK` | `1`, so no install, update or freshness work runs during a case. |

A case that needs one more variable adds it to that command alone. Case C does
this for `AUTODRIVE_QA_SCENARIO_DIR`.

No token, key, socket or configuration variable is on the allowlist. In
particular `GH_TOKEN`, `GITHUB_TOKEN`, the enterprise token variables,
`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `SSH_AUTH_SOCK`, `GH_CONFIG_DIR`,
`BASH_ENV` and `ENV` never reach a command. The measurement tool's own
scrub list covers only the suite process it starts; the allowlist covers the
rest of the recipe.

The same Git variables apply to the commands that create the scratch
repository, so a runner-level `core.hooksPath`, credential helper, URL rewrite
or signing setting cannot reach it.

The scratch root is one `mktemp -d` folder whose children are created under
`umask 077`. An `EXIT` trap removes that root and nothing else. It rejects an
empty value or `/`, and it expands no glob inside the root. The scenario
starts no background process.

## Stand-ins

The stand-ins are written from quoted heredocs into a private `PATH`
directory with mode `0700`. Data reaches them through the environment only,
never through interpolation into their source.

| Stand-in | Behaviour |
| --- | --- |
| `gh` | Answers one read-only call, in the form the shipped lookup uses: `pr view <digits> --repo github.com/<owner>/<name> --json baseRefName --jq .baseRefName`. The `github.com/` prefix on the repository is part of what it accepts. It prints the base named by `QA_GH_BASE`, records its argument vector in `RECORD_DIR`, and fails every other call, so a write call, or a lookup in any other form, fails the case. |
| `claude` | Stands for the recipe's agent steps. It answers `--version`, records one line for each call that carried a prompt and never the prompt itself, and prints one fixed JSON object. It performs no action. |
| `npm` | Records its call and exits `0`. A Node repository's suite command therefore passes, so a blocker in the record cannot come from the suite. |

No `gadugi-test` stand-in exists, and the stand-in directory holds no link to
the real one. Case A measures that condition, so a set-up check confirms it
(see [set-up checks](#set-up-checks)).

A scratch repository's `origin` is set only where a leg needs one, and then to
the string `https://github.com/qa-owner/qa-repo.git`. Nothing contacts it: the
stand-in agent performs no action, the measurement tool runs no command that
reaches a remote, and `GIT_ALLOW_PROTOCOL=file` stops Git using any other
protocol.
The URL exists so that the shipped lookup can establish the GitHub repository
and reach the `gh` stand-in.

## Set-up checks

Three checks run before any case, each printing one result line. A failed
check stops the scenario before a case runs, with a line that names set-up:

| Label | Passes when |
| --- | --- |
| `set-up paths` | The scratch root, the checkout, the resolved tools directory and the resolved `merge-ready` skill directory are each one absolute line holding no double quote, backslash, `$`, backtick, doubled brace or control byte. These are the characters the recipe's measurement step rejects in a path, so no case can fail for that reason later. A failure names the item, as `unusable: tools-dir`, and never prints the path. |
| `set-up trees` | The scratch root neither equals the checkout, lies inside it, nor contains it, both resolved with `pwd -P`. |
| `set-up gadugi-test-absent` | `command -v gadugi-test` finds nothing under `scenario_env`. Where it is found, case A would record `FAIL` with `no-scenarios` and not `BLOCKED`, so the scenario stops at set-up and names the tool. |

The second check keeps two trees apart. `AMPLIHACK_HOME` is the checkout. It
ships `amplifier-bundle/tools` and the `merge-ready` skill, which are the
trusted tools and guidance. The repository under review, passed as
`repo_path`, is a scratch Git repository under the scratch root. The
measurement step treats a tools directory that resolves inside the worktree
under review as no trust root, and the Rust test
`a_tools_directory_inside_the_worktree_under_review_is_not_a_trust_root`
pins that. The scenario never makes the two trees overlap: it re-checks each
scratch repository as it creates it and stops at set-up if one overlaps the
checkout. Every case passes `repo_path` explicitly. A run with no `repo_path`
could otherwise measure the checkout itself.

## Every run

Each of the six runs (case A, case B legs 1 to 4, and case C) must end
normally, and its evidence must stay private. Each run prints these three
results, prefixed with its own label, such as `case-b leg-2`:

| Label | Assertion |
| --- | --- |
| `exit-status` | `recipe run autodrive-merge-evidence` exits `0`. A blocker in the record is not a failure of the round. |
| `agent-calls` | The `claude` stand-in was called three times: once for each of the recipe's agent steps, `step-01-sync-with-base`, `step-02-qa-team-scenarios` and `step-03-wait-for-ci`. `step-02a-qa-inputs` is a bash step and does not call it. |
| `mode` | The folder that holds `qa-evidence.json` is `0700` and the record file is `0600`. |

The count is read from the stand-in's call log, never from prompt text. The
same fixed JSON answers all three steps, so the log can show that the round
reached its last agent step and cannot show which prompt each call carried.
The scenario asserts the first and not the second. A change to the number of
agent steps in the recipe changes the expected count with it.

Each case creates the evidence folder with mode `0700` before the run. The
measurement tool publishes the record through a rename under `umask 077`, and
the `mode` assertion fails if a change to that path widens either mode. See
[the record file](auto-drive-qa.md#the-record-file). A run in which no record
was written, as happens when the measurement step cannot find its tools,
shows an empty file mode and fails here.

## Case A: Node repository with no scenarios

**Setup.** The scratch repository holds a root `package.json`, one commit and
no scenario directory. The stand-in directory has the `npm` stand-in and no
`gadugi-test`, which the set-up check has confirmed. The recipe runs with
`repo_path` set to that repository and `autodrive_qa_evidence` set to a path
inside a `0700` folder.

**Expected.** The measurement is incomplete and the round carries on.

Beyond the [assertions made in every run](#every-run):

| Label | Assertion |
| --- | --- |
| `record` | The record is one readable JSON object. |
| `qa_status` | Not `PASS`. One of `FAIL`, `BLOCKED` or `QA_UNRESOLVED`. |
| `qa_reason` | Not empty, and one of `qa-command-failed`, `no-scenarios`, `qa-config-invalid`, `qa-command-missing`, `qa-command-not-installed`, `gadugi-test-missing` or `qa-unresolved`, the tokens the measurement tool writes. See [the status values](auto-drive-qa.md#the-status-values). |
| `gadugi_status` | `NOT_INSTALLED`. The absent tool is measured as a fact, whichever reason the tool picks first. |

For this fixture the measurement records `BLOCKED` with `qa_reason`
`gadugi-test-missing`: Node requires `gadugi-test`, the `npm` stand-in lets
the suite command pass, and the stand-in directory holds no `gadugi-test`. The
`qa_reason` assertion accepts any member of the closed set, so a change to the
order in which the tool picks a cause does not fail the case. It still fails
if the record reads `PASS`, is empty, or carries a reason outside the set. The
`gadugi_status` assertion pins the measured fact itself, which does not depend
on that order. On a host where `gadugi-test` is found the record would read
`FAIL` with `no-scenarios`, which is why the scenario stops at set-up there.

## Case B: the base from the recipe, then from the PR, then none

Case B has four legs. Each runs the recipe against a fresh scratch repository
and passes `pr_number` as digits only. Each leg makes the
[assertions made in every run](#every-run) and asserts, as `record`, that the
record is one readable JSON object. The tables below list the rest.

Legs 1, 2 and 4 give the repository an `origin` (see
[stand-ins](#stand-ins)) so that the shipped lookup can establish the GitHub
repository. Legs 1 and 2 also give it two commits. The first commit is also
`refs/remotes/origin/qa-base`, set with `git update-ref` and not fetched,
because the measurement reads the merge base from that ref. The second commit
adds `src/index.js`. A change set measured against `qa-base` therefore holds
that one path.

**Leg 1: the recipe supplies the base.** `autodrive_base_branch` is
`qa-base`.

| Observation | Assertion |
| --- | --- |
| `qa_base_source`, `qa_base` | `recipe`, `qa-base` |
| `qa_command_paths` | `src/index.js` |
| `gh` stand-in's call log | Empty. The recipe's value is used before any lookup. |

**Leg 2: the lookup supplies the base.** `autodrive_base_branch` is empty and
`QA_GH_BASE` is `qa-base`, so the `gh` stand-in answers the lookup.

| Observation | Assertion |
| --- | --- |
| `qa_base_source`, `qa_base` | `pr-base`, `qa-base` |
| `qa_command_paths` | `src/index.js` |
| `gh` stand-in's call log | One call, the form the shipped lookup uses: `pr view <pr_number> --repo github.com/qa-owner/qa-repo --json baseRefName --jq .baseRefName`. |

This is the one leg that exercises the `pr-base` source. A lookup in any other
form is not answered by the stand-in, the record then reads `none`, and the leg
fails on the source.

**Leg 3: nothing answers.** `autodrive_base_branch` is empty and the
repository has no `origin`. The shipped lookup stops before it calls `gh`,
because no GitHub repository can be established from `origin`, so the `gh`
stand-in is not reached in this leg. A lookup that reaches `gh` and is not
answered is not covered by any leg.

| Observation | Assertion |
| --- | --- |
| `qa_base_source`, `qa_base` | `none`, empty |
| `qa_command_paths` | `(unknown)` |
| `docs_only` | `unknown`. The changed-path list is not known, and the predicate reads `true` only for a known, non-empty list. |
| `gh` stand-in's call log | Empty |

No change set is guessed, and the literal `main` is never substituted.

**Leg 4: a hostile base.** `autodrive_base_branch` is
`-HOSTILEBASE2569$(touch injected2569)`. The recipe's branch-name check
rejects it.

| Observation | Assertion |
| --- | --- |
| `qa_base_source`, `qa_base` | `none`, empty |
| `record-hostile-text` | The record holds no line containing `HOSTILEBASE2569`. The tool writes `qa_base` only from a base that passed the branch-name check. |
| `gh` stand-in's call log | Empty. A rejected recipe value is not replaced by a lookup. |
| `injected-file`: a file named `injected2569` anywhere under the scratch root | Absent. No substitution reached a shell. |

The scenario reads the record, which the measurement tool writes, for the
value. It does not read the runner's own output, which may echo the context
values it was given, and it cannot read the step's standard error, which the
runner does not forward. The step's `[base]` line saying that the base was
rejected is therefore not asserted. The observable result is the record: source
`none`, an empty base, and no change set.

The resolution order the four legs pin is the one in
[resolving the measurement base](auto-drive-qa.md#resolving-the-measurement-base):
the recipe's observed target, then a lookup of the PR's base, then nothing.

## Case C: operator-named scenario directory is missing

The fixture is a Node repository: a root `package.json` and one commit, as in
case A. The tool consults a scenario directory only where `gadugi-test` is
required. Where it is not, as for a documentation-only change or a
criterion-1 declaration, the state reads `not-consulted` whatever the operator
named, so the case would pass or fail for the wrong reason. A Node repository
with no base requires it, and the scenario checks that this holds.

`AUTODRIVE_QA_SCENARIO_DIR` is added to this command's environment only. It
names a directory that does not exist, and the name holds a double quote, a
backslash, a tab and an ESC byte.

Beyond the [assertions made in every run](#every-run), where `exit-status` `0`
says that a missing directory the operator named is a blocker the judgement
carries on from and not a failure of the round:

| Label | Assertion |
| --- | --- |
| `record-shape` | `lines=1 esc=0 readable`: the record file is one line, holds no ESC byte, and parses as a single JSON object. |
| `gadugi_required` | `true`. This is a precondition. If it fails, the case reports that the fixture does not require `gadugi-test`, and does not go on to judge the state. |
| `gadugi_scenario_dir_state` | `operator-missing`, written from the tool's closed set of literals. |

This is the failure that once ended every round, when a launcher named a
directory that did not exist and no round could clear it. The judgement
either falls back to the repository's own tracked scenarios or records a
hand-off; see
[named recoverable blockers](auto-drive-qa.md#named-recoverable-blockers).

## Output and exit status

| Outcome | Standard output | Exit status |
| --- | --- | --- |
| Every assertion held | One `PASS` line per assertion, then `ALL_CASES_PASSED` | `0` |
| Any assertion failed or a set-up check failed | A `FAIL` line for each, then `SCENARIO_FAILED` | `1` |

A passing line is `PASS: <label> - <observed value>`. The descriptor expects
each one, including the ` - ` after the label, so that one label cannot match
inside a longer one. For example:

```text
PASS: case-b leg-2 qa_base_source - pr-base
```

A failure line names the case, the field, the expected value and the observed
value, for example:

```text
FAIL: case-b leg-3 qa_base_source: expected none, observed recipe
```

An empty value is shown as `(empty)`. A value is shown as printable ASCII, cut
to 120 bytes, so a failing case cannot carry a control byte into a log. The
script never enables `set -x`, never prints its environment and never prints
the stand-in call logs. The record's `qa_guidance` field holds an absolute path
to the installed guidance, so a failure line never carries it.

## What the scenario does not do

- It does not judge a change. The stand-in agent prints fixed JSON, so no
  judgement text is produced or read.
- It does not reach GitHub, a live agent, a rotator or the network.
- It observes the `recipe`, `pr-base` and `none` base sources, the three this
  recipe can produce. It does not cover a lookup that reaches `gh` and is not
  answered. `operator` and `clone-default` come from other callers.
- It reads neither the runner's own output nor a bash step's standard error.
  The runner does not forward the latter, so the scenario asserts on the
  record, the exit status and the stand-ins' call logs.
- It does not sleep, apply a timeout, poll or retry. The stand-ins return at
  once.
- It does not run the repository's own scenarios or the rest of
  `tests/gadugi/`.
- It does not add a recipe variable, a record field, a subcommand or a stop
  condition. It observes the ones that exist.

## Related

- [Run the QA blocker scenario](../howto/run-the-qa-blocker-scenario.md)
- [Auto-drive QA (Criterion 1)](auto-drive-qa.md)
- [Record a criterion-1 QA judgement by hand](../howto/record-criterion-1-qa-judgement.md)
- [Step failure recovery](step-failure-recovery.md)
- [CI pipeline](ci-pipeline.md)
