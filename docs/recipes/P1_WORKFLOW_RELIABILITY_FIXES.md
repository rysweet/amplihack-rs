# P1 Workflow Reliability Fixes

Four reliability improvements to the recipe execution pipeline, addressing
issues #624, #596, #614, and #581.

---

## 1. Verdict Synonym Normalization (Issue #624)

### Problem

The `step-08c-enforce-verdict` bash gate in `workflow-tdd.yaml` only accepted
three exact verdict strings: `WORK_VERIFIED`, `HOLLOW_SUCCESS`, and
`INSUFFICIENT_EVIDENCE`. The work-verifier agent is an LLM that naturally
produces synonyms like `VERIFIED` or `SUCCESS`. These hit the `*) exit 1`
default case, hard-failing the recipe after a PR had already been opened.

### Solution

A synonym-mapping `case` block runs before the verdict gate:

```bash
case "$VERDICT" in
  VERIFIED|SUCCESS|APPROVED|PASS|PASSED) VERDICT="WORK_VERIFIED" ;;
  FAILED|NO_WORK|EMPTY|NO_ARTIFACTS)     VERDICT="HOLLOW_SUCCESS" ;;
  INCONCLUSIVE|UNKNOWN|UNCLEAR|PARTIAL)  VERDICT="INSUFFICIENT_EVIDENCE" ;;
esac
```

The `*)` default case now fail-safes to `INSUFFICIENT_EVIDENCE` (exit 0 with
a loud warning) instead of exit 1. Unknown verdict strings from an LLM never
hard-fail a recipe that has real artifacts.

### Verdict Reference

| Canonical Verdict | Accepted Synonyms | Exit Code | Behavior |
|---|---|---|---|
| `WORK_VERIFIED` | `VERIFIED`, `SUCCESS`, `APPROVED`, `PASS`, `PASSED` | 0 | Recipe continues |
| `HOLLOW_SUCCESS` | `FAILED`, `NO_WORK`, `EMPTY`, `NO_ARTIFACTS` | 1 | Recipe fails (no real work) |
| `INSUFFICIENT_EVIDENCE` | `INCONCLUSIVE`, `UNKNOWN`, `UNCLEAR`, `PARTIAL` | 0 | Warn loudly, continue |
| *(unknown)* | any other string | 0 | Fail-safe to INSUFFICIENT_EVIDENCE |

### Configuration

No configuration required. Synonym mapping is always active.

---

## 2. Agentic Work Verifier — Evidence Priority (Issue #596)

### Problem

The hollow-success guard used brittle `git diff`/`git status` commands to count
uncommitted working-tree changes after step 08. A clean worktree after a
successful commit/push is **correct behavior** — it means work was committed to
the branch. The guard interpreted a clean tree as "no work done," which is wrong.

### Solution

The `step-08c-work-verifier` agent prompt (introduced in issue #615) now
explicitly orders evidence sources by reliability:

| Priority | Evidence Source | Command | What It Proves |
|---|---|---|---|
| PRIMARY | Branch commits | `git log --oneline origin/main..HEAD` | Code was committed |
| PRIMARY | Merged-during-implement | `git log --since='6 hours ago' origin/main` + `gh pr list --search 'is:merged'` | PR was merged within recipe window |
| PRIMARY | Sibling branches | `git branch -a --contains HEAD` | Work landed on another branch |
| PRIMARY | Linked issue | `gh issue view` | Issue closed by merged PR |
| PRIMARY | Recent PRs | `gh pr list --state all` | PR exists for this task |
| SECONDARY | Working tree | `git status --porcelain` + `git diff --stat HEAD` | Uncommitted edits (bonus evidence) |
| — | Intent match | manual review | Artifacts relate to task description |

The key behavioral change: **a clean working tree is documented as correct
behavior after commit/push** and does not count against a `WORK_VERIFIED`
verdict. The agent only reports `HOLLOW_SUCCESS` when all primary evidence
sources show no work.

### Configuration

No configuration required. The evidence priority is embedded in the verifier
prompt.

---

## 3. Bundle Asset Aliases (Issue #614)

### Problem

`amplihack resolve-bundle-asset` only recognized `multitask-orchestrator` as a
valid named asset. The `smart-orchestrator.yaml` recipe references `helper-path`
(line 58) and `hooks-dir` (line 74) during preflight. These assets were
removed in PR #285 but are still needed by the orchestrator.

### Solution

Two named assets re-registered in the `NAMED_ASSETS` table:

| Name | Resolves to | Fallback |
|---|---|---|
| `hooks-dir` | `amplifier-bundle/tools/amplihack/hooks/` | — |
| `helper-path` | `amplifier-bundle/tools/orch_helper.py` | `amplifier-bundle/tools/amplihack/orch_helper.py` |

`helper-path` tries two candidate paths in order and returns the first that
exists on disk.

### Usage

```sh
# Resolve the hooks directory
amplihack resolve-bundle-asset hooks-dir
# /home/user/.amplihack/amplifier-bundle/tools/amplihack/hooks

# Resolve the orchestrator helper
amplihack resolve-bundle-asset helper-path
# /home/user/.amplihack/amplifier-bundle/tools/orch_helper.py
```

### Configuration

No configuration required. Named assets are compiled into the binary.

See also: [resolve-bundle-asset Command Reference](../reference/resolve-bundle-asset-command.md)

---

## 4. Lock Command — Rust CLI Only (Issue #581)

### Problem

The fleet-copilot skill's `SKILL.md` instructed agents to run
`python .claude/tools/amplihack/lock_tool.py lock`. This Python file does not
exist — amplihack is a Rust project. Agents following the skill instructions
would fail immediately.

### Solution

All Python tool references in skill files replaced with the Rust CLI equivalent:

| Before | After |
|---|---|
| `python .claude/tools/amplihack/lock_tool.py lock` | `amplihack lock` |

The `amplihack lock` subcommand creates lock files at
`~/.amplihack/.claude/runtime/locks/` and is the only supported lock mechanism.

### Usage

```sh
# Enable lock mode (called by fleet-copilot skill)
amplihack lock

# Disable lock mode
amplihack unlock

# Check lock status
ls ~/.amplihack/.claude/runtime/locks/
```

### Configuration

No configuration required. Lock file location is fixed at
`~/.amplihack/.claude/runtime/locks/`.

---

## Testing

All four fixes have regression coverage:

| Fix | Test Location | Key Tests |
|---|---|---|
| Verdict synonyms | `tests/integration/issue_615_work_verifier_test.rs` | `enforce_verdict_returns_one_for_unknown_verdict` (updated: exits 0) |
| Evidence priority | `amplifier-bundle/recipes/workflow-tdd.yaml` (prompt text) | Manual: verify agent checks `git log` before `git status` |
| Bundle assets | `crates/amplihack-cli/src/resolve_bundle_asset/mod.rs` | `hooks_dir_registered`, `helper_path_registered` (23 tests total) |
| Lock command | `amplifier-bundle/skills/fleet-copilot/SKILL.md` | Grep: no `python.*lock_tool` references remain |

```sh
# Run all relevant tests
cargo test -p amplihack-cli --lib resolve_bundle_asset
cargo test enforce_verdict
cargo test default_workflow_decomposition
```

---

## Related

- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Includes entries for verdict synonym and clean-worktree scenarios
- [resolve-bundle-asset Command](../reference/resolve-bundle-asset-command.md) — Full command reference with updated named assets
- [Recipe Resilience](../RECIPE_RESILIENCE.md) — Branch sanitization, worktree bases, and publish safety
