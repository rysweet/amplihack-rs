---
title: Install/update PATH conflict reference
description: Reference for Rust binary precedence, stale Python/uvx wrapper neutralization, PATH persistence, and install/update repair APIs.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Install/update PATH conflict reference

`amplihack install` and `amplihack update` make the Rust `amplihack` binary in
`~/.local/bin` the selected user-level command. They do this by deploying the
Rust binaries, persistently prepending `$HOME/.local/bin` for future shells,
neutralizing only positively identified stale Python/uvx wrappers that shadow
the Rust binary, and failing visibly when an unknown executable would still
shadow the Rust binary.

The resolver never executes discovered candidates while classifying them.

## Scope

The repair covers:

| Name | Purpose |
| --- | --- |
| `amplihack` | Main Rust CLI and update target. |
| `amplihack-hooks` | Rust hook dispatcher registered in settings files. |

Stale-wrapper neutralization applies only to a file whose basename is exactly
`amplihack` and that appears before the preferred Rust binary in ordered
`PATH` resolution. Later stale wrappers are left in place because they do not
control the command. The installer does not rewrite arbitrary `PATH` entries or
delete unrelated binaries.

## Install/update order

Both install and the post-update repair path use the same order:

1. Deploy Rust binaries to `~/.local/bin`.
2. Persist the managed PATH profile block so future shells resolve
   `$HOME/.local/bin` first.
3. Analyze ordered `PATH` candidates.
4. Quarantine stale Python/uvx `amplihack` wrappers only when they shadow the
   Rust binary, are clearly identified, and are safe to move.
5. Refresh `~/.amplihack/amplifier-bundle` from the current Rust distribution.
6. Verify the selected `amplihack` is the Rust binary.

If any required repair cannot be completed and an earlier candidate would still
shadow the Rust binary, install/update exits non-zero with the conflicting path
and manual repair guidance.

## Candidate classifications

| Classification | Meaning | Automatic action |
| --- | --- | --- |
| `CurrentRustBinary` | The running Rust binary or a binary with the current Rust identity. | Accepted. |
| `PreferredRustBinary` | The Rust binary deployed to `~/.local/bin/amplihack`. | Accepted and made first for future shells. |
| `StalePythonWrapper` | A user-controlled wrapper whose content positively identifies old Python amplihack launch behavior. | Quarantined when safe and shadowing Rust. |
| `StaleUvxWrapper` | A user-controlled wrapper whose content positively identifies uvx/uv wrapper launch behavior for amplihack. | Quarantined when safe and shadowing Rust. |
| `UnknownExecutable` | An executable named `amplihack` that is not positively identified as Rust or stale wrapper. | Not modified; reported as a conflict if it shadows Rust. |
| `Inaccessible` | Metadata, ownership, canonicalization, or content could not be inspected. | Not modified; reported with the underlying error. |

## Safe wrapper neutralization

A stale wrapper is eligible for quarantine only when all conditions hold:

1. The basename is exactly `amplihack`.
2. The candidate appears before the preferred Rust `~/.local/bin/amplihack` in
   the current ordered `PATH`, or is otherwise the command that would be
   selected before repair.
3. The path is under a user-controlled or amplihack-managed location, such as
   `$HOME/.local/bin`, `$HOME/bin`, `$HOME/.cargo/bin`, `$HOME/.local/share`,
   or `$AMPLIHACK_HOME`.
4. The file is not under a denied system prefix such as `/usr/bin`,
   `/usr/local/bin`, `/bin`, `/usr/sbin`, `/sbin`, `/opt`, or a package-manager
   directory outside `$HOME`.
5. The file is owned by the current user or otherwise safely movable by the
   current user.
6. File content contains positive stale-wrapper evidence, such as a Python
   shebang plus old amplihack wrapper markers, uvx/uv launch markers, or known
   package-wrapper boilerplate for the Python amplihack distribution.
7. Symlink candidates have both a safe symlink path and a safe resolved target.
   Ambiguous symlinks, external symlink paths, or escaping targets are skipped
   and reported instead of followed destructively.

The neutralizer moves eligible wrappers to:

```text
~/.amplihack/quarantine/stale-wrappers/<timestamp>/
```

Each quarantine directory includes `manifest.json` with:

| Field | Description |
| --- | --- |
| `original_path` | Absolute original path. |
| `quarantine_path` | Sanitized path below the quarantine directory. |
| `kind` | `stale-python-wrapper` or `stale-uvx-wrapper`. |
| `size` | File size in bytes. |
| `modified_unix_secs` | Source file modification time, when available. |
| `action` | Currently `quarantined`. |

The manifest records metadata only. It does not copy full wrapper contents into
logs.

## Unknown and system-managed conflicts

Unknown executables are never deleted or quarantined automatically. If an
unknown executable remains before `~/.local/bin/amplihack` after the managed
PATH repair, install/update fails with guidance.

System-managed paths are never mutated automatically, even if they are writable:

```text
/usr/bin
/usr/local/bin
/bin
/usr/sbin
/sbin
/opt
```

Example diagnostic:

```text
Cannot select Rust amplihack because an unknown executable shadows it:
  /usr/local/bin/amplihack

amplihack did not modify this file. Move ~/.local/bin before /usr/local/bin,
update the system package, or remove the stale system copy through your normal
administrative process, then run amplihack install again.
```

## PATH persistence

Install writes an idempotent managed block to the detected user shell profile:
`~/.bashrc` for bash, `~/.zshrc` for zsh, `~/.kshrc` for ksh, and
`~/.config/fish/config.fish` for fish. If the shell cannot be detected or is not
supported, install leaves profiles unchanged and prints manual PATH guidance
instead. If the detected profile cannot be written, install/update fails with
the profile write error instead of silently continuing.

The block prepends `$HOME/.local/bin` so the user-local Rust binary wins in
new shells:

```bash
# >>> amplihack managed PATH >>>
# Added by amplihack install
export PATH="$HOME/.local/bin:$PATH"
# <<< amplihack managed PATH <<<
```

Fish receives fish-compatible syntax:

```fish
# >>> amplihack managed PATH >>>
# Added by amplihack install
fish_add_path --prepend $HOME/.local/bin
# <<< amplihack managed PATH <<<
```

The block is bounded by markers so subsequent installs update it in place. It
does not remove unrelated `PATH` entries. A later duplicate of
`$HOME/.local/bin` is harmless because the first occurrence wins.

## Configuration

No new user configuration is required.

| Input | Role |
| --- | --- |
| `HOME` | Used to resolve the preferred user bin directory, expected as `~/.local/bin`. |
| `PATH` | Ordered command candidates for conflict analysis. |
| `AMPLIHACK_HOME` | Install root for bundle staging and stale-wrapper quarantine; defaults to `~/.amplihack`. |
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | Optional test/CI hint for locating `amplihack-hooks`; does not bypass PATH conflict reporting. |
| Shell profile file | Installer updates the managed block in `~/.bashrc`, `~/.zshrc`, `~/.kshrc`, or `~/.config/fish/config.fish` when the detected profile is writable. |

## Contributor API

The shared resolver lives in `crates/amplihack-cli/src/path_conflicts.rs`.
The stale wrapper neutralizer lives in
`crates/amplihack-cli/src/commands/install/stale_wrappers.rs`.

### `PathCandidateKind`

Classifies an executable candidate.

| Variant | Meaning |
| --- | --- |
| `CurrentRustBinary` | Candidate is the running/current Rust binary. |
| `PreferredRustBinary` | Candidate is the Rust binary in the preferred user bin directory. |
| `StalePythonWrapper` | Candidate is a positively identified stale Python wrapper. |
| `StaleUvxWrapper` | Candidate is a positively identified stale uvx wrapper. |
| `UnknownExecutable` | Candidate is executable but not safely classifiable. |
| `Inaccessible` | Candidate could not be inspected. |

### `PathAnalysisInput`

Inputs supplied by install/update callers and tests.

| Field | Type | Description |
| --- | --- | --- |
| `path_env` | `String` | Raw `PATH` value to scan in order. |
| `home_dir` | `PathBuf` | Home directory used to derive `~/.local/bin`. |
| `amplihack_home` | `PathBuf` | Install root used for quarantine and bundle paths. |
| `current_exe` | `PathBuf` | Running executable path. |
| `binary_names` | `Vec<String>` | Normally `amplihack` and `amplihack-hooks`. |
| `filesystem` | trait-backed adapter | Metadata, canonicalization, ownership, writability, and content inspection. |

### `BinaryResolution`

Ordered candidate list for one binary.

| Field | Description |
| --- | --- |
| `name` | Binary name. |
| `candidates` | Existing executable candidates in shell resolution order. |
| `first` | First executable candidate. |
| `preferred_user_bin` | Expected user-local target. |
| `shadowed_user_bin` | User-local candidate when another path appears earlier. |

### `PathConflictReport`

Side-effect-free analysis consumed by install and update.

| Field | Description |
| --- | --- |
| `resolutions` | Per-binary command resolution details. |
| `warnings` | User-facing advisory messages. |
| `stale_wrappers` | Eligible shadowing stale wrappers, already classified but not moved. |
| `unknown_conflicts` | Unknown candidates that would shadow the Rust binary. |
| `inaccessible_conflicts` | Candidates that could not be inspected. |
| `rust_first_after_repair` | Whether the Rust binary will resolve first after PATH repair and shadowing wrapper quarantine. |

### `StaleWrapperNeutralizer`

Moves eligible shadowing stale wrappers into quarantine and returns a manifest
summary.

Callers must pass only candidates already classified by the resolver. The
neutralizer re-checks safe location, basename, metadata, and symlink boundaries
before moving a file.

### `InstallTargetDecision`

Decision used by update replacement logic.

| Variant | Meaning |
| --- | --- |
| `PreferredUserBin { target, warnings }` | Install or replace `~/.local/bin/amplihack`. |
| `CurrentExeDir { target }` | Reuse the current executable directory only when it is the safe user-level bin directory. |
| `ManualRepairRequired { attempted_target, guidance }` | Do not replace anything; return actionable guidance. |

Callers must propagate `ManualRepairRequired` as a user-visible error and must
not fall back to privileged writes or broad `PATH` cleanup.

### `PathPrecedenceManager`

Owns profile-file updates. It writes or updates the managed block in the
detected shell profile and reports which file changed. It must preserve all
unrelated profile content.

## Regression coverage

Automated tests cover:

1. stale Python wrapper quarantine
2. stale uvx wrapper quarantine
3. unknown executable conflict reporting without mutation
4. external symlink-to-safe-target conflict reporting without mutation
5. fish-compatible PATH profile syntax
6. PATH profile write failures surfacing as install/update errors
7. `$HOME/.local/bin` managed-block prepend idempotence
8. install selecting the Rust user-level binary after stale wrapper repair
9. update invoking the new binary's install repair path
10. no mutation of system-managed paths

Focused validation:

```bash
cargo test -p amplihack-cli stale_wrapper
cargo test -p amplihack-cli path_conflicts
cargo test -p amplihack-cli path_precedence_tests
cargo test -p amplihack-cli build_install_command
```

## See also

- [Repair stale amplihack wrappers and PATH conflicts](../howto/repair-install-update-path-conflicts.md)
- [Framework bundle compatibility reference](framework-bundle-compatibility.md)
- [amplihack install reference](install-command.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
