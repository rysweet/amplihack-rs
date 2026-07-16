# Bug Fix #929 — Honest PR titles and honored no-merge directives

> **Issue:** [#929](https://github.com/rysweet/amplihack-rs/issues/929)

---

## Summary

The `default-workflow` publish step used to open pull requests titled
`Update Cargo.lock with N changed files` even when the branch actually
contained an entire feature diff. Those PRs then auto-merged under the user's
identity even when the task explicitly said **do not merge**.

Two independent defects combined to produce that behavior:

1. **Mislabeled PR titles.** The publish helper derived the PR title's *scope*
   from the first file returned by `git diff --name-only`. Because `Cargo.lock`
   sorts early and is refreshed by the deliberate `--locked` sync (issue #915),
   it frequently became the "first changed file" and dictated the whole title —
   even when dozens of substantive source files were part of the diff.
2. **Ignored no-merge intent.** The workflow's merge gate (`should_merge`) had
   no way to observe an explicit "do not merge" instruction in the task, so it
   defaulted to `true` and auto-merged.

The fix makes PR titles reflect the *substantive* change and makes the workflow
honor an explicit no-merge directive so it never auto-merges when told not to.

## Behavior after the fix

### 1. PR titles ignore lockfiles and generated files

When choosing the scope word for a PR title, the publish helper now filters
out lockfiles and generated artifacts before selecting the representative file.
The filtered set includes (case-insensitive basename match):

| Pattern | Examples |
| --- | --- |
| `Cargo.lock` | `Cargo.lock` |
| `*.lock` | `flake.lock`, `poetry.lock`, `Gemfile.lock` |
| `package-lock.json` | `package-lock.json` |
| `pnpm-lock.yaml`, `yarn.lock` | JS lockfiles |
| `go.sum` | Go checksum database |

Scope selection then follows the existing rules against the **first
substantive (non-lockfile) file**:

| First substantive path | Scope word |
| --- | --- |
| `amplifier-bundle/recipes/*` | `workflow recipes` |
| `crates/amplihack-cli/*` | `amplihack CLI` |
| `crates/<name>/*` | `<name>` |
| `tests/*` | `regression coverage` |
| `docs/*` | `documentation` |
| anything else | first path segment |

Only when the diff is **genuinely lockfile-only** (every changed file is a
lockfile/generated artifact) does the title fall back to a lockfile scope. In
that case the scope word is derived from the lockfile itself (the basename of
the first changed file), **not** the previous generic `"workflow changes"`
empty-scope fallback:

```text
Update Cargo.lock (#915)
```

For a lockfile-only diff spanning several lockfiles, the count still reflects
all of them (e.g. `Update Cargo.lock with 2 changed files`).

The `CHANGED_COUNT` used in the title and the full file listing in the PR body
are **unchanged** — they still enumerate *every* changed file, including
lockfiles. Only the scope word is affected.

**Before:**

```text
Update Cargo.lock with 34 changed files (#929)
```

**After** (feature branch touching CLI code plus a refreshed `Cargo.lock`):

```text
Update amplihack CLI with 34 changed files (#929)
```

### 2. Explicit no-merge directives are honored

The workflow now detects a no-merge directive from the task description and
forces `should_merge="false"` at the terminal-state chokepoint. When
`should_merge` is `false`, **no** downstream step — lockfile-sync, the finalize
agent, publish, or the quality loop — may merge the PR. The PR is created (or
left) **open**.

`workflow-terminal-state.yaml` emits `should_merge` from **multiple** terminal
arms (the meaningful-work, follow-up, and merged-state emitters all currently
default it to `"true"`). The `apply_no_merge_policy` step must therefore
override the value **after** every emitter that could set it to `"true"` — a
single guard at one arm is insufficient. The override is applied uniformly so
that whichever terminal arm fires, a detected directive still wins.

#### Recognized phrasings

Detection is case-insensitive and whitespace-tolerant. Any of the following in
the task description trigger no-merge mode:

| Intent | Example phrasing |
| --- | --- |
| Direct | `do not merge`, `don't merge`, `dont merge` |
| Hyphen/flag form | `no-merge`, `no merge` |
| Admin form | `no admin merge`, `no admin-merge` |
| Leave-open form | `leave it open`, `leave the PR open`, `leave ... open` |

> **Design note.** The `leave ... open` wildcard form is intentionally
> conservative in span: it should match only a short `leave`→`open` window
> (e.g. same sentence/clause) so that unrelated prose like *"leave the config
> open to extension"* does not accidentally suppress merges. Because detection
> is fail-closed, a false positive only ever suppresses an auto-merge (never
> enables one), but a too-greedy match still hurts usability.

An optional explicit context flag is also honored for forward compatibility:

```yaml
context:
  no_merge: "true"
```

When either the keyword detector or the `no_merge` flag fires, the emitted
terminal-state JSON carries `should_merge: "false"`.

#### Default behavior is unchanged

Tasks **without** a no-merge directive behave exactly as before:
`should_merge` remains `"true"` and the workflow may auto-merge according to
the existing gates. The no-merge logic is purely additive.

## Configuration

### `task_description` propagation

`task_description` is defined once in the top-level `default-workflow.yaml`
context. recipe-runner **merges parent context into every phase sub-recipe**
(see the header comment in `workflow-publish.yaml`: *"Parent context is merged
by recipe-runner"*), so the value already flows down to
`workflow-terminal-state` — the callers (`workflow-publish.yaml`,
`workflow-finalize.yaml`) do **not** need an explicit
`context: task_description: ...` forwarding block.

The only wiring change is that `workflow-terminal-state` now **declares
`task_description` as an input with an empty default**, so the merged value is
resolvable at the merge chokepoint and self-documents the dependency:

```yaml
# amplifier-bundle/recipes/workflow-terminal-state.yaml
inputs:
  # ... existing inputs ...
  - name: task_description
    description: "Original task text. Scanned for explicit no-merge directives
                  (e.g. 'do not merge', 'no-merge', 'leave open') that force
                  should_merge=false."

context:
  # ... existing defaults ...
  task_description: ""
```

The empty default makes detection fail **closed** for merge safety: if the
parent context ever omits `task_description`, the probe sees an empty string,
finds no directive, and simply leaves the existing `should_merge` gate intact —
a missing directive never *enables* auto-merge, it only ever suppresses it.

### Merge-policy inputs summary

| Input / signal | Values | Effect |
| --- | --- | --- |
| `should_merge` (context) | `"true"` (default) / `"false"` | Master merge gate consumed by all downstream steps |
| `no_merge` (context, optional) | `"true"` / unset | Forces `should_merge="false"` when set |
| `task_description` keywords | free text | Forces `should_merge="false"` when a no-merge phrase is present |

## Examples

### Feature branch with a refreshed lockfile

Task:

```text
Add a --json flag to `amplihack status`.
```

Branch changes: `crates/amplihack-cli/src/commands/status.rs`,
`crates/amplihack-cli/src/output.rs`, `Cargo.lock`.

Resulting PR title:

```text
Update amplihack CLI with 3 changed files (#0)
```

`Cargo.lock` is still listed in the PR body's **Changed files** section; it
just no longer dictates the title.

### Task that forbids merging

Task:

```text
Fix issue #929 ... Do NOT merge the PR; leave it open.
```

The terminal-state probe detects `do not merge` and `leave it open`, sets
`should_merge="false"`, and the workflow publishes an **open** PR. No step
auto-merges.

### Genuinely lockfile-only diff

Task:

```text
Run `cargo update -p serde` and open the resulting lockfile PR.
```

Branch changes: `Cargo.lock` only. Resulting PR title:

```text
Update Cargo.lock (#0)
```

This is the one case where a lockfile scope is correct, and it is preserved.

## Security considerations

The directive scanner treats the task description as **untrusted data**:

- **No command injection.** The task text and changed-file paths are always
  double-quoted and matched with `[[ ... =~ ... ]]` / here-strings. They are
  never passed to `eval` or interpolated into a command.
- **ReDoS resistance.** Keyword matching uses static, non-catastrophic
  alternations authored in the recipe. User text is always the *subject* of a
  match, never the *pattern*.
- **Filename injection.** Changed-file paths are iterated with
  `while IFS= read -r` and their basenames are sanitized to `[A-Za-z0-9._/-]`
  before pattern classification.
- **Fail-closed merge policy.** On any parse ambiguity or error the policy
  biases toward `should_merge="false"`. Errors can never enable auto-merge.
- **Least privilege.** The directive may only *remove* merge capability, never
  grant it. Merge remains gated solely by `should_merge`.
- **No leakage.** Only a derived boolean/scope is emitted. The full task
  description is never written into the PR title, and env is not printed inside
  `set -x` regions.

## Regression coverage

`amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` adds two
cases, and confirms the default path is unchanged:

1. **Leading-`Cargo.lock` title selection.** A diff whose alphabetically first
   file is `Cargo.lock` but which also contains substantive source files
   produces a title scoped to the substantive file, not `Cargo.lock`.
2. **No-merge suppression.** A task description containing a no-merge phrase
   yields `should_merge="false"` from `workflow-terminal-state`.
3. **Default unchanged.** A task description with no directive yields
   `should_merge="true"`.

Focused checks:

```sh
bash amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh
```

Broader checks:

```sh
cargo build
cargo check --workspace
```

## Files changed

| File | Change |
| --- | --- |
| `amplifier-bundle/tools/workflow_publish_pr.sh` | Filter lockfiles/generated files from PR-title scope selection; lockfile-only fallback |
| `amplifier-bundle/recipes/workflow-terminal-state.yaml` | Declare `task_description` input (default `""`); detect no-merge directive; force `should_merge="false"` after every arm that sets it `"true"` |
| `amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh` | Regression cases for title selection and no-merge suppression |

> The caller recipes (`workflow-publish.yaml`, `workflow-finalize.yaml`) are
> **not** edited: `task_description` reaches `workflow-terminal-state`
> automatically via recipe-runner's parent-context merge.

## Out of scope

- PR **body** redesign (only the title's scope word changed).
- Merge policy for tasks *without* a no-merge directive (unchanged).
- External azork PRs (#83 / #87).
- Recipe-runner refactor.

## Related

- The deterministic `--locked` `Cargo.lock` sync (issue #915) is *why* a
  refreshed `Cargo.lock` reliably appears in feature diffs; this fix stops that
  sync from dictating PR titles.
- [Default workflow overview](../workflows.md)
