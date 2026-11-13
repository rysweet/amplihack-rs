---
title: Merge-Ready Files Resolver
description: inputs, output, messages and test coverage of autodrive_merge_ready_files.sh, the lookup that step-00 of the auto-drive merge round runs.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Merge-Ready Files Resolver

[`autodrive_merge_ready_files.sh`](../../amplifier-bundle/tools/autodrive_merge_ready_files.sh)
finds the merge-ready skill's `SKILL.md` and `pr-description-template.md` for
the auto-drive merge round. Step `step-00-merge-ready-files` of
[`autodrive-merge-round.yaml`](../../amplifier-bundle/recipes/autodrive-merge-round.yaml)
runs it. The merge-ready skill sets `disable-model-invocation: true`, so the
round reads these files and does not invoke the skill (#1517).

The script is read-only and is safe under `set -u`, including under bash 3.2
on macOS.

## Contents

- [Inputs](#inputs)
- [Output](#output)
- [Messages on stderr](#messages-on-stderr)
- [Where the criteria come from](#where-the-criteria-come-from)
- [Test coverage](#test-coverage)
- [Related documentation](#related-documentation)

## Inputs

| Variable | Required | Use |
| --- | --- | --- |
| `AMPLIHACK_HOME` | Yes, as an absolute path | The only place the script searches. It looks for `$AMPLIHACK_HOME/amplifier-bundle/skills/merge-ready`. |
| `REPO_PATH` | No (defaults to `.`) | The repository under review. The script uses its git toplevel (`TOP`) only to decide whether to print the branch warning. It never searches it for criteria. |

`HOME`, the git toplevel and PR-worktree copies are never searched. The skill
and its template always come from the same directory.

## Output

On success the script exits 0 and prints one line of JSON on stdout:

```json
{"skill_dir":"/home/ci/.amplihack/amplifier-bundle/skills/merge-ready","skill_md":"/home/ci/.amplihack/amplifier-bundle/skills/merge-ready/SKILL.md","template":"/home/ci/.amplihack/amplifier-bundle/skills/merge-ready/pr-description-template.md","skill_md_sha":"3b18e512dba79e4c8300dd08aeb37f8e728b8dad"}
```

`skill_md_sha` is `git hash-object --no-filters` of `SKILL.md`, which is 40
hexadecimal characters. On failure the script exits 1, writes the error to
stderr and leaves stdout empty, because step-00 parses stdout as JSON.

## Messages on stderr

| Message | Exit | Meaning |
| --- | --- | --- |
| `ERROR: trusted AMPLIHACK_HOME is required for merge-ready criteria` | 1 | `AMPLIHACK_HOME` is unset, empty or relative. |
| `INFO: merge-ready criteria from <dir> (<sha>)` | 0 | The directory and `SKILL.md` hash that were used. |
| `WARNING: merge-ready criteria come from the branch under review and differ from <base>` | 0 | See [where the criteria come from](#where-the-criteria-come-from). |
| `WARNING: skipping merge-ready candidate with an unusable path (...)` | none | The candidate path holds a quote, backslash, control byte or `{{`/`}}`, so the script skips it. |
| `ERROR: merge-ready-template-not-found: <dir>/pr-description-template.md (...)` | 1 | `SKILL.md` exists but its template does not. The template is never taken from another directory. |
| `ERROR: merge-ready-skill-files-not-found: searched <dirs>` | 1 | No candidate held a `SKILL.md`. `<dirs>` lists every directory searched. |

The not-found message prints `searched ${SEARCHED}` with no fallback text.
`SEARCHED` cannot be empty at that point, because `AMPLIHACK_HOME` must be an
absolute path and always adds one candidate. An earlier version printed a
fallback that named `REPO_PATH`, `HOME` and the git toplevel, which the
resolver no longer searches (#2561).

Two messages strip control bytes with `LC_ALL=C tr -d '\000-\037\177'`: the
skipped-candidate warning and the not-found error. Both can print a candidate
path that the script has not yet checked. The `INFO` line, the
template-not-found error and the branch warning print the chosen directory
without that filter. This is safe because the script has already skipped any
candidate whose path holds a control byte, quote, backslash or template brace,
so a crafted `AMPLIHACK_HOME` cannot add log lines or GitHub workflow commands
such as `::error::` through any message.

## Where the criteria come from

In a repository that ships `amplifier-bundle/`, such as amplihack-rs itself,
`AMPLIHACK_HOME` may point into the branch under review. When the chosen
directory is under `TOP`, the script compares `SKILL.md` in the working tree
with the base branch. The base is `origin/HEAD` if it is set, otherwise
`origin/main`. If they differ, it prints the warning above and still exits 0,
because the merge gate measures criteria 1 and 3 again.

When the directory is outside `TOP`, no comparison is made and no warning is
printed.

## Test coverage

| Test | Where it runs | What it checks |
| --- | --- | --- |
| `merge_ready_files_resolver_runs_under_set_u_in_a_git_repository` in [`auto_drive_to_merge_test.rs`](../../tests/integration/auto_drive_to_merge_test.rs) | `cargo test` | The resolver exits 0 under `set -u`, prints valid JSON and a 40-character hash. `TOP` is the repository root: with `refs/remotes/origin/main` set to `HEAD` and an uncommitted change to `SKILL.md`, the install inside the repository prints the branch warning and the install outside it does not. |
| `merge_ready_files_resolver_is_read_only_and_set_u_safe` in the same file | `cargo test` | The script writes nothing and keeps the fixed `merge-ready-skill-files-not-found: searched` prefix. |
| [`run-autodrive-merge-ready-files-scenario.sh`](../../tests/gadugi/run-autodrive-merge-ready-files-scenario.sh) | CI, `check` job | The shipped resolver and the shipped step-00 bash, run as black boxes. See below. |

[`test-auto-drive-to-merge.sh`](../../amplifier-bundle/recipes/tests/test-auto-drive-to-merge.sh)
checks only that the resolver is present in the tools directory. It no longer
runs resolver cases (#2340), and it no longer sets an unused `RESOLVER`
variable (#2561). The two tests above cover that path.

### The CI scenario step

The `check` job in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)
runs the scenario directly after the merge-validations scenario step:

```yaml
# Issues #2536, #2561: CI runs the scenario's driver directly, not gadugi-test.
- name: Gadugi — merge-ready files resolver scenario (issues #2536, #2561)
  shell: bash
  run: |
    command -v jq >/dev/null 2>&1 || { sudo apt-get update -qq && sudo apt-get install -y -qq jq; }
    bash tests/gadugi/run-autodrive-merge-ready-files-scenario.sh
```

The scenario needs only `bash`, `git` and `jq`, so it runs in `check` rather
than `install-smoke`. It builds its installs in a `mktemp -d` directory, which
it removes on exit, and runs each case under `env -i` with a temporary
`HOME`. The step's exit status decides the result. A passing run prints seven
`PASS:` lines, no `FAIL:` line, and ends with `ALL_CASES_PASSED`:

```text
PASS: merge_ready_files_resolved — ...
PASS: merge_ready_template_never_mixed — ...
PASS: merge_ready_missing_install_named — ...
PASS: merge_round_reads_skill_files — ...
PASS: merge_round_step00_resolves — ...
PASS: merge_round_step00_leaves_tool_recovery_to_agent — ...
PASS: merge_round_step00_rust_needs_no_gadugi — ...
merge-ready read from its files; gadugi-test checked before the build where criterion 1 needs it
ALL_CASES_PASSED
```

A failing case prints a `FAIL:` line, and the run ends with `SCENARIO_FAILED`
and exit 1, which fails the job. If a case fails, fix the shipped resolver or
recipe, not the scenario.

Do not run `tests/gadugi/*` on a shared factory host. Read the result of the
CI step instead.

## Related documentation

- [Auto Drive To Merge Reference](auto-drive-to-merge.md#how-the-merge-round-applies-merge-ready) - how the merge round applies the criteria
- [Merge-ready authority](../../amplifier-bundle/skills/merge-ready/SKILL.md) - the criteria the resolver locates
- [Framework source selection](framework-source-selection.md) - how `AMPLIHACK_HOME` is chosen
