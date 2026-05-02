# Amplihack Install Completeness Verification

`amplihack install` stages the full Amplihack framework from the resolved framework source into the user's install directory. The install is complete only when every required framework component has been copied and verified.

The installer fails loudly on incomplete installs. Missing framework sources, missing staged directories, partial copies, copy errors, and insufficient skill coverage are installation errors, not warnings.

## What gets installed

The source of truth is the resolved framework source for the current install. For the canonical amplihack-rs layout, that source is `amplifier-bundle/`.

The installer stages the framework components described by the install mapping in `crates/amplihack-cli/src/commands/install/types.rs`. For the bundle layout, the current required mapped categories are:

| Source path | Destination purpose | Required behavior |
| --- | --- | --- |
| `amplifier-bundle/skills/` | Skills | Required |
| `amplifier-bundle/agents/` | Agent definitions | Required |
| `amplifier-bundle/context/` | Shared framework context | Required |
| `amplifier-bundle/recipes/` | Workflow recipes | Required |
| `amplifier-bundle/tools/amplihack/` | Amplihack framework tools | Required |
| `amplifier-bundle/tools/xpia/` | XPIA framework tools | Required |
| `amplifier-bundle/behaviors/` | Behavior definitions | Required |
| `amplifier-bundle/modules/` | Shared framework modules | Required |

Additional source categories, such as `commands/` or `hooks/`, are source-conditional required: if the source-derived manifest includes them, they must be copied and verified. They are not best-effort optional assets.

The full `amplifier-bundle/` is also staged under the user's Amplihack home directory so local installs have the same framework assets available after update and install.

## Usage

Update Amplihack, then install the framework assets:

```bash
amplihack update
amplihack install
```

Capture the command output when diagnosing install behavior. A successful install means all required framework directories were copied and verified.

## Verification behavior

After copying files, `amplihack install` verifies the staged framework against a source-derived manifest.

The manifest is generated from the resolved source at install time. This avoids hard-coded skill counts and stale component lists.

Verification checks:

1. Required source component directories exist.
2. Required destination component directories exist at the install destinations resolved by the install mapping.
3. Every source child directory required by the manifest exists at the corresponding destination.
4. Every source skill directory exists at the staged skills destination.
5. The staged skill directory count is at least the source skill directory count. This is an additional guard; extra destination skills cannot mask a missing source skill.
6. The full staged `amplifier-bundle/` exists under the user's Amplihack home directory.
7. Source-conditional categories such as `commands/` and `hooks/` are verified when they exist in the source-derived manifest.

If any check fails, `amplihack install` exits non-zero with an actionable error message.

Copy failures must include both the source path and destination path in the diagnostic so the user can distinguish source packaging problems from destination filesystem problems.

## Failure examples

If a required source directory is missing:

```text
install failed: required framework source directory is missing: amplifier-bundle/skills
```

If a destination directory was not staged:

```text
install failed: required framework destination directory is missing: <resolved install destination>/skills
```

If only part of the skills directory was copied:

```text
install failed: staged skills are incomplete: expected at least <source skill count> skill directories, found <installed skill count>
```

If an individual source component directory is missing from the staged destination:

```text
install failed: staged framework component missing: skills/github
```

If a copy operation fails:

```text
install failed: failed to copy <source path> to <destination path>: <cause>
```

These failures indicate a broken installation source, packaging error, copy failure, or incomplete staged target. Re-running install without fixing the underlying issue does not convert the failure into success.

## Packaging guarantee

Published Amplihack artifacts must include `amplifier-bundle/`, and package metadata must include that directory in the published file set.

This guarantees that installs performed after `amplihack update` have access to the same framework assets as repository-local installs. If the package does not contain `amplifier-bundle/`, install verification fails instead of producing a partial framework.

## Diagnosing a remote install

Capture the installed state:

```bash
amplihack --version
which amplihack
ls -la ~/.amplihack/
find ~/.amplihack -maxdepth 3 -type d | sort
find ~/.amplihack/.claude/skills -maxdepth 1 -type d | sort
find ~/.amplihack/.claude/agents -maxdepth 2 -type d | sort
```

Reproduce a clean install with logs:

```bash
amplihack update 2>&1 | tee /tmp/amp-update.log
rm -rf ~/.amplihack/.claude
amplihack install 2>&1 | tee /tmp/amp-install.log
find ~/.amplihack/.claude -maxdepth 3 -type d | sort > /tmp/installed-dirs.txt
```

Compare the installed directories against the source bundle:

```bash
find amplifier-bundle -maxdepth 4 -type d | sort > /tmp/expected-dirs.txt
```

The installed framework should contain the expected staged equivalents for every required source component. Missing skills, agents, recipes, tools, context, behaviors, modules, or source-conditional categories are install failures.

## Configuration

`amplihack install` does not require extra configuration for completeness verification.

The installer resolves the framework source from the current Amplihack installation and verifies the staged result under the user's Amplihack home directory.

Relevant paths:

| Path | Purpose |
| --- | --- |
| `amplifier-bundle/` | Authoritative framework source for the canonical bundle layout |
| `~/.amplihack/amplifier-bundle/` | Staged full framework bundle |
| `~/.amplihack/.claude/` | Root for mapped framework assets resolved by the installer |

For exact component destinations, use the install mapping in `crates/amplihack-cli/src/commands/install/types.rs` as the source of truth.

## Integration behavior

Install completeness is covered by an integration test that runs `amplihack install` into an isolated temporary home.

The test asserts:

1. Every expected source skill directory is present at the destination.
2. Every expected source agent directory is present at the destination.
3. Every expected source child directory for each manifest category is present at the corresponding destination when child-directory parity applies.
4. Every source-conditional category is present at the destination when it exists in the source manifest.
5. The staged skill count is at least the source skill count, in addition to exact source skill name coverage.
6. The full `amplifier-bundle/` is staged.
7. Package metadata includes `amplifier-bundle/`.
8. Copy failures surface both source and destination paths.

This test prevents regressions where published or local installs silently omit framework components.
