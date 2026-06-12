# Artifact Guard

**Status:** Implemented.

Artifact Guard prevents generated, runtime, cache, and build artifacts from
leaking into the parent repository worktree during agent and plugin workflows.
It is a blocking safety gate for broad staging, pre-commit, and publication
paths. It reports violations with remediation guidance and never deletes,
moves, unstages, or rewrites files.

## Contents

- [Planned behavior](#planned-behavior)
- [Command-line interface](#command-line-interface)
- [Default prohibited rules](#default-prohibited-rules)
- [Allowlist configuration](#allowlist-configuration)
- [Workflow and pre-commit coverage](#workflow-and-pre-commit-coverage)
- [Output isolation](#output-isolation)
- [Intended Rust API](#intended-rust-api)
- [Fixing violations](#fixing-violations)

## Behavior

Artifact Guard:

1. Scan repo-relative paths only.
2. Check staged paths before commit.
3. Check tracked, untracked, and selected ignored-present artifact paths before
   broad staging and workflow publication.
4. Fail closed on invalid configuration, invalid paths, Git failures, and unsafe
   allowlist entries.
5. Print actionable remediation before exiting.
6. Leave the repository unchanged.

`.gitignore` reduces Git noise, but it is not an authorization mechanism. The
guard may still report ignored-present dependency trees, runtime directories, or
cache directories when those paths indicate parent-worktree pollution.

`target/` is intentionally special-cased. Normal Rust commands create an ignored
`target/` directory in the repository, and the guard must not make ordinary
`cargo test`, `cargo clippy`, or pre-commit usage hostile. By default:

| `target/` source | Planned result |
| --- | --- |
| Staged | Violation |
| Tracked | Violation |
| Untracked because it is not ignored | Violation |
| Ignored-present only | Not a violation |

Workflow steps that need strict build-output isolation should set
`CARGO_TARGET_DIR` to an isolated location instead of relying on the default
repo-local `target/` directory.

## Command-line interface

```bash
amplihack hygiene artifact-guard \
  --repo <path> \
  --mode <mode> \
  [--allowlist <path>]
```

| Option | Description | Default |
| --- | --- | --- |
| `--repo <path>` | Repository worktree to scan. The path must resolve inside a Git repository. | current directory |
| `--mode <mode>` | Scan mode: `pre-commit`, `pre-publish`, `all`, `staged`, or `worktree`. | `pre-commit` |
| `--allowlist <path>` | Optional allowlist file. Relative paths resolve from the repository root. | `.amplihack-artifact-allowlist` when present |

| Mode | Sources checked | Typical use |
| --- | --- | --- |
| `pre-commit` | Staged, tracked, untracked, and selected ignored-present artifact candidates | Local pre-commit hook |
| `pre-publish` | Staged, tracked, untracked, and selected ignored-present artifact candidates | Workflow broad-staging and PR/finalize gates |
| `all` | Staged, tracked, untracked, and selected ignored-present artifact candidates | Manual full safety scan |
| `staged` | Staged paths only | Diagnose why a commit is blocked |
| `worktree` | Tracked, untracked, and selected ignored-present artifact candidates | Check local leftovers before cleanup or staging |

Use `all` for safety gates. Use `staged` only for focused debugging because it
does not detect ignored or untracked leftovers.

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | No prohibited artifacts were found |
| `1` | Prohibited artifacts were found |
| `2` | The guard could not complete because configuration, paths, mode, allowlist, or Git state was invalid |

Violation output:

```text
Artifact Guard blocked 3 prohibited artifact paths.

source           path                  rule
staged           dist/plugin.js        plugin-bundle
ignored-present  node_modules/         dependency-tree
untracked        .claude/runtime/      claude-runtime

Remediation:
  - Move generated, plugin, cache, and runtime output into an isolated directory.
  - Remove local artifact leftovers from the parent worktree.
  - If the artifact is intentional source material, add a narrow reviewed entry
    to .amplihack-artifact-allowlist.
```

Configuration errors exit with code `2`:

```text
Artifact Guard configuration error:
  .amplihack-artifact-allowlist:4 rejects absolute paths: /tmp/plugin.js

Fix the allowlist entry or run without the allowlist.
```

## Default prohibited rules

Artifact Guard rejects these paths by default when they appear in the parent
repository worktree:

| Rule | Examples | Ignored-present behavior |
| --- | --- | --- |
| Dependency trees | `node_modules/`, `packages/*/node_modules/` | Blocked |
| Plugin bundles | `dist/plugin.js`, `*/dist/plugin.js` | Blocked |
| Claude runtime | `.claude/runtime/` | Blocked |
| Nested worktrees | `worktrees/` | Blocked |
| Cache directories | `.cache/`, `.npm/`, `.pnpm-store/`, `.yarn/cache/`, `.turbo/`, `.parcel-cache/`, `.pytest_cache/` | Blocked |
| Build output | `dist/`, `build/`, `coverage/`, `.next/`, `out/`, `logs/`, `outputs/`, `index.scip` | Blocked |
| Rust build output | `target/` | Blocked only when staged, tracked, or untracked |
| Generated indexes and logs | `index.scip`, generated runtime logs, generated output directories | Blocked |

Rules match normalized repo-relative paths using `/` separators. The guard does
not need to read artifact file contents; path-level scanning is the intended
contract.

## Allowlist configuration

The default allowlist path is `.amplihack-artifact-allowlist`. This file is
repo-controlled configuration and should be reviewed like a security-sensitive
change because it can permit generated artifacts that the guard would otherwise
block.

Format:

```text
# Blank lines and comments are ignored.
# Entries are repo-relative and use / separators.

tests/fixtures/plugin-output/dist/plugin.js
docs/fixtures/generated-manifests/*.json
examples/minimal-node-project/node_modules/.package-lock.json
```

Matching semantics:

| Rule | Planned behavior |
| --- | --- |
| Path root | Entries are relative to the repository root |
| Separators | `/` only; Windows-style `\` separators are rejected |
| Case | Case-sensitive matching |
| Exact paths | `tests/fixtures/dist/plugin.js` matches only that path |
| `*`, `?`, `[]` | Supported within a path segment |
| `**` | Supported across path segments |
| Directory entries | Must use an explicit suffix such as `tests/fixtures/output/**`; a bare directory does not imply recursive matching |
| Duplicates | Duplicate entries are allowed but normalized to one effective rule |
| Comments | Lines beginning with `#` after optional whitespace are ignored |

Valid entries are narrow and intentional:

```text
tests/fixtures/plugin-output/dist/plugin.js
tests/fixtures/plugin-output/dist/**
examples/generated-output/build/expected-manifest.json
```

Invalid entries fail closed with exit code `2`:

```text
/absolute/path
../outside-repo
node_modules/
node_modules/**
**/node_modules/**
dist/
dist/**
build/**
*
**/*
```

Directory allowlists are accepted only for narrow fixture or example paths. They
must not exempt a default prohibited directory directly at the repository root or
across the whole repository.

## Workflow and pre-commit coverage

Artifact Guard will run before every broad staging operation in the bundled
recipes, not just publish and finalize. The initial guarded recipe set is every
recipe that currently invokes `git add -A`:

| Recipe | Guard placement |
| --- | --- |
| `workflow-finalize.yaml` | Before final broad staging |
| `workflow-publish.yaml` | Before publication staging and before any broad staging in remediation paths |
| `workflow-refactor-review.yaml` | Before broad staging |
| `workflow-tdd.yaml` | Before broad staging |
| `workflow-pr-review.yaml` | Before broad staging |
| `consensus-publish.yaml` | Before broad staging |
| `consensus-pr-feedback.yaml` | Before broad staging |

Future recipe changes must preserve the rule: any new `git add -A` or equivalent
broad-staging step needs an Artifact Guard gate immediately before it.

The planned pre-commit hook is a full repository scan:

```yaml
- id: artifact-guard
  name: amplihack artifact guard
entry: amplihack hygiene artifact-guard --repo . --mode pre-commit
  language: system
  pass_filenames: false
always_run: true
```

`pass_filenames: false` is required. Git normally passes only staged filenames to
pre-commit hooks, which would miss ignored and untracked artifact leftovers.
Artifact Guard must inspect repository state itself.

For local source checkouts where `amplihack` is not on `PATH`, use the workspace
binary:

```bash
cargo run --bin amplihack -- hygiene artifact-guard --repo . --mode all
```

The checked-in pre-commit Cargo hooks set `CARGO_TARGET_DIR` under
`${TMPDIR:-/tmp}` so running the guard, clippy, or tests from hooks does not
recreate Cargo build output inside the parent worktree.

## Output isolation

The preferred fix for a violation is output isolation, not allowlisting.
Allowlisting is only for intentional checked-in fixtures or reviewed generated
artifacts.

These locations are intentionally not prohibited by default:

```text
<repo>/.amplihack/runtime/
<repo>/.amplihack/cache/
<repo>/.amplihack/generated/
<git-common-dir>/.claude/runtime/
/tmp/amplihack-<purpose>-<id>/
```

Use `CARGO_TARGET_DIR` for workflow-owned Rust builds that need to avoid the
repo-local `target/` directory:

```bash
CARGO_TARGET_DIR=.amplihack/cache/cargo-target cargo test --workspace
```

Avoid writing generated output directly to:

```text
<repo>/node_modules/
<repo>/dist/plugin.js
<repo>/.claude/runtime/
<repo>/worktrees/
<repo>/build/
```

## Intended Rust API

The guard core should live in `amplihack_utils::artifact_guard` so CLI commands,
recipes, and tests share one implementation. This is the intended public shape;
implementation may rename fields only if this document is updated in the same
change.

```rust
pub struct ArtifactGuardConfig {
    pub repo_path: PathBuf,
    pub mode: ArtifactGuardMode,
    pub allowlist_path: Option<PathBuf>,
}

pub enum ArtifactGuardMode {
    All,
    Staged,
    Worktree,
}

pub enum ArtifactSource {
    Staged,
    Tracked,
    Untracked,
    IgnoredPresent,
}

pub struct ArtifactViolation {
    pub path: String,
    pub source: ArtifactSource,
    pub rule_id: String,
    pub message: String,
}

pub struct ArtifactGuardReport {
    pub repo_root: PathBuf,
    pub mode: ArtifactGuardMode,
    pub violations: Vec<ArtifactViolation>,
}

pub fn run_artifact_guard(
    config: ArtifactGuardConfig,
) -> Result<ArtifactGuardReport, ArtifactGuardError>;
```

`run_artifact_guard` resolves the repository root, validates the allowlist,
collects candidate paths from Git, applies prohibited rules, applies allowlist
exceptions, and returns a structured report. It must not mutate the repository.

Errors are fail-closed:

| Error class | Examples |
| --- | --- |
| Repository errors | `--repo` is not a Git worktree, Git command fails |
| Path errors | Path escapes repo root, path cannot be normalized |
| Mode errors | Unknown CLI mode |
| Allowlist errors | Unreadable file, absolute path, parent traversal, broad exemption |

## Fixing violations

For a blocked commit:

```bash
amplihack hygiene artifact-guard --repo . --mode staged
git restore --staged dist/plugin.js
```

Then move the build output to an isolated location or remove the local artifact
if it is not needed.

For ignored leftovers before publication:

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

If the guard reports `node_modules/`, `.claude/runtime/`, or another
ignored-present artifact, relocate or remove the local output. Do not treat
`.gitignore` as approval to keep parent-worktree pollution.

For an intentional fixture, add the narrowest allowlist entry that preserves the
test:

```text
tests/fixtures/plugin-output/dist/plugin.js
```

Commit the allowlist change with the fixture. Reviewers should confirm the
artifact is necessary, deterministic, and safe to keep in the repository.

## Review expectations

Review these changes carefully:

1. New or changed `.amplihack-artifact-allowlist` entries.
2. Changes to prohibited rules.
3. Recipe edits around `git add -A`, publication, finalization, or PR creation.
4. Pre-commit changes that remove `pass_filenames: false`.
5. Build, plugin, or runtime changes that redirect outputs into the parent
   worktree.

Reviewers should verify that generated and runtime outputs are isolated, guard
failures are visible, and allowlist entries are not used as substitutes for
proper output placement.

## Related documentation

- [Recipe CLI Reference](reference/recipe-cli-reference.md)
- [Pre-Commit Diagnostics](claude/agents/amplihack/specialized/pre-commit-diagnostic.md)
- [Developing amplihack](DEVELOPING_AMPLIHACK.md)
