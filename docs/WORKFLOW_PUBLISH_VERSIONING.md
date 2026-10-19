# Publish Workflow: Classification-Driven Commit Prefix & Version Bump

The `workflow-publish` recipe (`amplifier-bundle/recipes/workflow-publish.yaml`)
derives both the implementation commit's conventional-commit prefix **and** the
`[workspace.package].version` bump from the *real change type* of the branch.
Bugfix branches produce `fix:` commits and leave the workspace version
untouched; on feature and breaking branches step 14 bumps the version, and
step 15 commits that bump only when `allow_version_bump` is set (see
[Step-15 handling of the version bump](#step-15-handling-of-the-version-bump)).

This closes the class of bug reported in
[#944](https://github.com/rysweet/amplihack-rs/issues/944) (previously observed
on #920 and #943), where every implementation commit was hardcoded to `feat:`
and every publish run bumped `[workspace.package].version` — even on pure
bugfix branches — requiring manual reverts.

## Behavior

The publish workflow classifies the branch diff and maps it to a commit prefix
and a version-bump action:

| Change classification | Commit prefix | `[workspace.package].version` |
| --------------------- | ------------- | ----------------------------- |
| `PATCH` (fix/docs/refactor) | `fix:`  | **unchanged** (`new_version == current_version`) |
| `MINOR` (backward-compatible feature/API) | `feat:`  | minor bumped (`X.Y.Z → X.(Y+1).0`) |
| `MAJOR` (breaking change) | `feat!:` | major bumped (`X.Y.Z → (X+1).0.0`) |

`step-14-bump-version` collapses `docs` and `refactor` changes into the `PATCH`
bucket (see its prompt), so all three map to a single `fix:` prefix. This is a
deliberate lossy simplification — the workflow does **not** emit distinct
Conventional Commits `docs:`/`refactor:` prefixes. What matters for #944 is the
version-bump gate, and every `PATCH`-class change correctly produces `fix:` with
no version bump.

The classification comes from `step-14-bump-version`, an architect agent that
reviews `git diff main...HEAD` and emits a `version_bump` JSON object (see
[Classification output](#classification-output)). Both the version-bump action
and the downstream commit prefix read from this single source of truth, so they
can never disagree.

### Examples

**Bugfix branch** — a branch that only fixes a defect:

```
Classification : PATCH
Commit title   : fix: update 2 files in crates
Cargo.toml     : version = "0.9.4"   (unchanged)
Cargo.lock     : re-synced to 0.9.4  (no diff)
package.json   : re-synced to 0.9.4  (no diff)
```

The publish PR contains **no** `[workspace.package].version` diff.

**Feature branch** — a backward-compatible feature:

```
Classification : MINOR
Commit title   : feat: update 4 files in bins, crates, docs
Cargo.toml     : version = "0.9.4" → "0.10.0"   (committed only when allow_version_bump=true)
```

**Breaking change branch**:

```
Classification : MAJOR
Commit title   : feat!: update 3 files in crates, docs
Cargo.toml     : version = "0.9.4" → "1.0.0"    (committed only when allow_version_bump=true)
```

## How the prefix is resolved

The Step 15 commit block resolves the prefix deterministically, in priority
order:

1. **Classification signal (primary).** The step reads the environment variable
   `VERSION_BUMP_CHANGE_CLASSIFICATION` (uppercased form, dot → single `_`),
   falling back to the `RECIPE_VAR_version_bump__change_classification` alias
   (dot → double `__`), produced by `step-14-bump-version`. A whitelist `case`
   maps it:
   - `PATCH → fix:`
   - `MINOR → feat:`
   - `MAJOR → feat!:`
2. **Task-description heuristic (fallback, rarely reached).** The primary signal
   above is derived from the actual branch diff, so it is populated on virtually
   every real publish run. Only if the classification variable is empty or unset
   (e.g. the bump step was skipped) does the block fall back to scanning
   `$TASK_DESCRIPTION` for bugfix intent (`bug`, `bugfix`, `fix`); a match yields
   `fix:`. Because this fallback is text-based, a feature titled "add fix-it
   command" could be mislabeled `fix:` — but only when the diff-derived
   classification is unavailable, which is the exception, not the rule.
3. **Default.** Otherwise the prefix defaults to `feat:`, preserving the
   historical behavior for feature/release runs.

The prefix is the only part of the commit message the task description can
influence. Since issue
[#1575](https://github.com/rysweet/amplihack-rs/issues/1575) the rest of the
subject is built from the staged index by `workflow_commit_message.sh`:

```bash
COMMIT_TITLE=$(bash "$MSG" subject "$COMMIT_PREFIX") || COMMIT_TITLE=""
[ -n "$COMMIT_TITLE" ] || COMMIT_TITLE=$(printf '%supdate files' "$COMMIT_PREFIX")
```

A bugfix run touching seven files produces
`fix: update 7 files in amplifier-bundle, docs, tests`. No task-description
text reaches the commit. See
[Step-15 commit message](features/workflow-publish-task-attribution.md#step-15-commit-message).

The classification string is only ever consumed through a whitelist `case`
statement — it is never `eval`'d or interpolated into a command — preserving
the injection-safety guarantees from #469/#311.

## Classification output

`step-14-bump-version` emits the following JSON (`output: version_bump`,
`parse_json: true`), unchanged in schema so downstream consumers stay stable:

```json
{
  "current_version": "0.9.4",
  "new_version": "0.9.4",
  "change_classification": "PATCH",
  "rationale": "Only fixes a defect; no API surface change.",
  "changes_summary": "Corrected off-by-one in retry backoff calculation."
}
```

For a `PATCH` classification the agent performs **no edit** to the
`version = "X.Y.Z"` line, and sets `new_version == current_version`. For
`MINOR`/`MAJOR` it bumps the version as before.

## Step-15 handling of the version bump

Step 14's version edit reaches the commit only when both of these hold:

- the `allow_version_bump` context variable is `true`, `1` or `yes` (the
  default is `"false"`);
- step 14 asked for a bump: `new_version` and `current_version` are both
  non-empty and differ. Missing step-14 output counts as no bump.

Otherwise, when every changed line in the staged `Cargo.toml`, `Cargo.lock`
and `package.json` files is the project's own version line (`[package]` or
`[workspace.package]` in `Cargo.toml`, a `[[package]]` entry without `source`
in `Cargo.lock`, the top-level `"version"` in `package.json`),
`step-15-commit-push` saves
those changes to
`$(git rev-parse --git-path amplihack/excluded-version-churn.patch)`, restores
the files, and prints a `WARNING` naming the patch. If any of those files also
has other changes, including a dependency `version` change, all of them stay
staged with a `WARNING`. The commit prefix
still follows the classification. See
[Step-15 staging](features/workflow-publish-task-attribution.md#version-churn).

## Lockfile & package.json consistency

`step-14b-sync-lockfile` (`cargo update --workspace --offline`) and
`step-14c-sync-package-json` (a scoped `[workspace.package]` read that syncs the
root `package.json` `version` via `jq`, falling back to `python3 json`) run on
every publish, including no-bump `PATCH` runs. Both skip gracefully when their
target file is absent (non-Rust / non-JS workspaces). On a bugfix branch they
simply re-sync to the unchanged version, so `Cargo.lock` and `package.json`
remain valid and produce no spurious diff.

## Scope & limitations

- Only `workflow-publish.yaml` implements this behavior. The
  `consensus-publish.yaml` recipe is out of scope and unaffected.
- The `PATCH` bucket intentionally folds `docs` and `refactor` changes into a
  single `fix:` prefix rather than emitting distinct Conventional Commits
  `docs:`/`refactor:` prefixes. This is a documented simplification, not a
  defect: the goal is correct version-bump gating, not full commit-type fidelity.
- Version bumping remains diff-gated inside the architect agent; the commit
  prefix is a deterministic, whitelist-mapped string. A prompt-injection attempt
  that steers the classification can at most select a fixed prefix string — it
  cannot alter published artifacts or bypass the diff-based bump gate.
- This change is not retroactive; it does not revert version bumps already
  merged on #920 or #943.

## Related

- Issue: [#944](https://github.com/rysweet/amplihack-rs/issues/944)
- Recipe: `amplifier-bundle/recipes/workflow-publish.yaml`
  (`step-14-bump-version`, `step-14b-sync-lockfile`,
  `step-14c-sync-package-json`, Step 15 commit block)
