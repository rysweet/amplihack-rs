---
title: Self-heal framework assets
description: Executable eligibility, automatic framework repair and installation opt-out behaviour.
doc_type: reference
review_schedule: as-needed
---

# Self-heal framework assets

> [Home](../index.md) > [Features](README.md) > Self-heal framework assets

`amplihack` can repair missing, outdated or incompatible framework assets
before dispatching a command. Automatic installation is permitted only when
the running executable belongs to a complete, non-retired release beneath
the user's installed `~/.amplihack/releases` tree.

A checkout or Cargo build cannot install itself automatically, even when its
version is higher than the installed version. This prevents a development
command or Git hook from switching the host's release beneath running sessions.

## Contents

- [Executable eligibility](#executable-eligibility)
- [Startup and launch behaviour](#startup-and-launch-behaviour)
- [Refused automatic repair](#refused-automatic-repair)
- [Diagnosing a refusal](#diagnosing-a-refusal)
- [Scope and known limits](#scope-and-known-limits)
- [Never an implicit downgrade](#never-an-implicit-downgrade-1526)
- [Skip rules](#skip-rules)
- [Automatic installation opt-out](#bypass-amplihack_skip_auto_install)
- [Explicit install and update](#explicit-install-and-update)
- [Stamp file](#stamp-file)
- [Failure mode](#failure-mode)
- [See also](#see-also)

## Executable eligibility

The origin decision uses the running executable reported by
`std::env::current_exe()`. It canonicalises paths and applies the existing
release-layout confinement, ownership, permission and complete-release checks.
The decision is read-only. It creates no directories, temporary files, markers,
caches or locks.

| Executable origin | Eligible for implicit installation? |
| --- | --- |
| Valid executable within a complete, non-retired installed release | Yes, subject to the opt-out, version and source checks |
| Published `~/.local/bin/amplihack` symlink resolving into such a release | Yes, subject to the same checks |
| Checkout, Cargo target or hook build cache | No |
| Directory whose name merely resembles or shares a prefix with `releases` | No |
| Symlink within the releases tree resolving outside it | No |
| Incomplete or retired release | No |
| Executable whose location cannot be obtained, resolved or validated | No |

Version metadata and release-like directory names do not establish origin.
`AMPLIHACK_HOME` selects a candidate framework source; it cannot grant
installation eligibility or redirect the trusted installed releases tree.
A redirected releases root cannot make a checkout eligible.

This restriction covers every implicit installation route: startup self-heal,
launch bootstrap, binary deployment, command restoration, hook and provider
reconciliation, host-client acquisition, and automatic startup updates. Having
a framework bundle already present does not authorise a secondary repair path.

## Startup and launch behaviour

Before automatic repair, self-heal checks executable eligibility and the
opt-out, then assesses the installed version stamp and bundle compatibility.
Ineligible and opted-out executions use an uncached, read-only compatibility
assessment; they do not write `.bundle-compat-cache.json`.

For an eligible executable without the opt-out:

1. A missing, malformed or older stamp requests a re-stage. An incompatible
   bundle can also require repair when the stamp matches.
2. An implicit install obtains the installation lock and rechecks the state
   before changing anything.
3. Source selection prefers compatible local sources with the binary's
   version. Otherwise, an eligible release build fetches its release tag,
   never `main`. See [Framework source selection](../reference/framework-source-selection.md).
4. The installer validates and stages a new release before publishing links,
   then writes the version stamp after a successful install.

Launch bootstrap applies the same origin restriction before filling gaps,
including missing commands, hooks and provider assets. Profile and rotator
reconciliation cannot bypass it. An already satisfied installation continues
without an origin-refusal diagnostic.

Host-client discovery may resolve an already usable Claude, Copilot, Codex or
Amplifier without installation authority. If a client is missing, or an owned
npm client needs an automatic upgrade, the shared acquisition helper checks
the opt-out and executable origin before creating installation directories,
cleaning npm caches or invoking npm/uv. This includes Codex resolution before
framework self-heal and RustyClawd's fallback to Claude acquisition.

A refused missing-client acquisition fails startup with an executable-origin
diagnostic. A refused upgrade reports the refusal, rechecks the unchanged
client and continues when it remains usable. Read-only resolution itself
does not require origin eligibility.

Automatic startup updates check eligibility before acquiring a release,
writing update state or switching installation links. Starting a checkout
binary does not give a startup update the authority of an explicit `update`.

## Refused automatic repair

When an ineligible executable would install or repair assets, stderr explains
that its executable location prevents automatic installation. Printed paths
are sanitised. Startup self-heal and launch bootstrap share diagnostic
suppression so the same refusal is reported at most once per process.

A non-empty `AMPLIHACK_SKIP_AUTO_INSTALL` takes precedence. It stops the same
installation paths and reports its own diagnostic, so an opted-out execution
prints no origin refusal even when the executable is ineligible. See
[Diagnosing a refusal](#diagnosing-a-refusal).

Refusal occurs before source acquisition or installation-state mutation:

| State | Behaviour on refusal |
| --- | --- |
| Releases and their files | No release added, removed or changed |
| Bundle and executable links | Literal targets unchanged |
| Version stamps | Contents unchanged; no new stamp |
| Compatibility cache | No write or refresh |
| Installation lock | No creation or acquisition for the refused operation |
| Framework source | No acquisition, fetch or staging |
| Managed commands, hooks, profiles and provider configuration | No installation or reconciliation |
| Startup update state | No write or host-release switch |
| Host-client installation directories and npm caches | No creation, cleanup or installer invocation |

The refusal itself does not abort command dispatch. A command that can use
available assets keeps its normal behaviour and exit status. A command
requiring unavailable assets may fail through its normal validation with a
clear error. Refusal never reports repair as successful.

The restriction concerns installation assets. Ordinary command, session and
runtime writes retain their existing rules.

### Startup regression coverage

The `issue_1955_install_cli::issue_2496` process tests exercise the actual CLI:

- `missing_client_startup_refuses_every_provider_before_acquisition` covers
  Claude, Copilot, Codex, Amplifier and RustyClawd with missing clients.
- `upgrade_startup_refuses_acquisition_and_keeps_the_usable_client` supplies
  a terminal and a local registry response to reach automatic npm upgrades
  for Claude, Copilot and Codex.
- `usable_client_startup_from_checkout_does_not_require_acquisition` checks
  that a checkout can still resolve and launch an existing client.
- `eligible_missing_client_startup_reaches_the_isolated_installer` checks
  that a complete installed release can still acquire a missing client.

These tests use disposable homes, configuration, temporary roots and PATHs
with native Rust installer tripwires. Refusal checks compare installation
files, modes and literal link targets and assert no installer executes.
The same module retains canonical-origin, preservation, wrapper, explicit
install and eligible self-heal regressions. Framework-bootstrap checks alone
do not cover the earlier host-client resolution path.

The separate `issue_1954_first_install_restager` and
`issue_1954_install_leaves_live_runs_alone` process suites require Linux
`/proc` and are excluded on macOS. A macOS run cannot establish their
first-update restager and live-run preservation guarantees; report them as
unavailable, even when the portable origin and startup regressions pass.

## Diagnosing a refusal

The refusal line names the executable and the rule, not the particular check
that failed. The specific cause is recorded at `debug` level, so a single run
under `RUST_LOG` reports it:

```sh
RUST_LOG=amplihack_cli=debug amplihack recipe list
```

The cause line names the failing check and the path it was applied to. That
path is sanitised in the same way as the refusal line. Log output is coloured
only when stderr is a terminal, so a redirected or piped capture is plain text
and carries no escape sequences. A broader filter such as `RUST_LOG=debug`
enables unrelated targets whose output carries no such guarantee, so keep the
filter narrow, and read what you captured before attaching it to an issue.

Two cases report nothing here:

- `doctor`, `install`, `update` and the other commands in
  [Skip rules](#skip-rules) do not run startup self-heal. Use a command that
  does; `recipe list` above is one.
- A non-empty `AMPLIHACK_SKIP_AUTO_INSTALL` suppresses implicit installation
  before eligibility is assessed, so neither the refusal line nor the cause is
  emitted. Unset it for the diagnostic run. Such an execution reports the
  [bypass diagnostic](#bypass-diagnostic) instead.

Each cause the `debug` line can report, and what to do about it:

| Reported check | Cause | Remedy |
| --- | --- | --- |
| `executable-outside-installed-release` | The executable is a checkout or Cargo build, for example `target/debug/amplihack`. A symlinked release directory or `bin` also lands here, because the executable then resolves outside the trusted tree | Expected for a checkout. Run `amplihack install` explicitly, or launch the installed `~/.local/bin/amplihack`. Replace a symlinked release directory or `bin` with the real directory |
| `directory-is-a-symlink` | `~/.amplihack` or `~/.amplihack/releases` is a symlink rather than a real directory | Replace the link with the real directory. A redirected releases tree is never trusted |
| `directory-writable-by-group-or-others` | `~/.amplihack`, `~/.amplihack/releases`, the release directory or its `bin` is writable by group or others | `chmod 700 ~/.amplihack ~/.amplihack/releases`, then `chmod 700` the release directory and its `bin` |
| `directory-not-owned-by-this-user`, `executable-not-owned-by-this-user` | One of those directories, or the executable itself, is owned by another user, typically after a `sudo` install | Reinstall as the account that owns `$HOME`, or restore ownership with `chown` |
| `directory-missing-or-unreadable` | `~/.amplihack` or `~/.amplihack/releases` is absent or cannot be read | Run `amplihack install` to create the installation |
| `release-incomplete` | The release has no `.complete` marker, or the marker is a symlink | The release is partial. Run `amplihack install` to stage a complete one |
| `release-retired` | The executable sits under `releases/.retired/` | The release has been superseded. Launch `~/.local/bin/amplihack` instead |
| `executable-not-in-release-bin` | The executable resolves inside a release but outside its `bin` | Reinstall. A link inside `bin` that escapes its release is refused |
| `executable-not-a-regular-file` | The path is a directory, device or socket rather than a regular file | Reinstall |
| `executable-writable-by-group-or-others` | The executable itself is writable by group or others | `chmod 755` the file, or reinstall to restore the installed modes |
| `home-directory-unresolved`, `executable-path-unknown`, `executable-path-unresolved` | `HOME` is unset, or the executable's own path cannot be obtained or resolved | Set `HOME`, or install and launch from `~/.local/bin/amplihack` |
| `unsupported-platform` | The platform has no Unix ownership and mode semantics, so confinement cannot be assessed | Install and update explicitly. See [Scope and known limits](#scope-and-known-limits) |

These names are a published vocabulary, not free text, so they are safe to
quote in an issue.

In every one of these cases `amplihack install` remains available, subject to
its own source validation, so the host can still be repaired deliberately.

## Scope and known limits

This is an integrity control against a same-user process publishing itself to
the host without being asked. It crosses no privilege boundary, performs no
signature or content attestation, and is not an anti-tamper measure. A user who
can write `~/.amplihack` can install whatever they like through the explicit
command, which is the intended way to do it.

Two limits follow from that scope:

- **Hard links.** A checkout binary hard-linked into a release `bin` is
  indistinguishable from the installed file by any path-based check, and is
  accepted. Creating that link already requires write access to the release,
  so it is deliberate action rather than the accident this guard addresses.
- **Time of check.** The eligibility checks and the subsequent write are not
  atomic. The window is same-user only, and the private-directory checks run
  again when the install writes.

On platforms without Unix ownership and permission semantics, the ownership
and mode checks cannot be made. Implicit installation is refused outright
there, rather than proceeding on the path, confinement and completeness
checks alone, and explicit `amplihack install` is the only route.

The verdict is recomputed at each boundary rather than cached, so a tree that
changes mid-run cannot carry an earlier decision forward.

## Never an implicit downgrade (#1526)

Executable eligibility does not override the downgrade guard. For eligible
executables, automatic re-stage and launch bootstrap retain these rules:

| Running version compared with stamp | Framework behaviour | Binary behaviour |
| --- | --- | --- |
| Older | No re-stage; downgrade warning | Unchanged |
| Equal | No re-stage unless bundle repair is needed | Existing binaries kept |
| Newer | Re-stage permitted | Replacement requires a strictly newer version than the recorded binaries version |
| Stamp missing or malformed | Re-stage permitted | Only empty binary slots filled |

Version ordering uses semver precedence. Helper binary replacements must come
from this build's own copies, including the supported hooks-binary override.
A helper found elsewhere on `PATH` has an unknown version and may only fill
an empty slot. These deployment rules apply after origin eligibility; they
cannot make a checkout eligible.

An implicit re-stage never writes `~/.claude/settings.json`. For eligible
launches, bootstrap retains its existing first-install hook repair rules.
Copilot and Codex reconciliation retain their configuration ownership and
client-selection checks. Ineligible and opted-out executions skip these
installation writes, including when the installed framework bundle is usable.

When an implicit re-stage keeps a binary whose version differs from the new
framework, it records the previous binaries version, or `unknown`, in the
[stamp](#stamp-file) and warns. Explicit install remains the way to replace
those binaries deliberately.

## Skip rules

Examples of startup self-heal exclusions (this table is not exhaustive):

| Invocation | Behaviour |
| --- | --- |
| `install`, `uninstall`, `update` | Skip startup self-heal to avoid recursion or undoing explicit intent |
| `completions`, `doctor`, `help` | Skip startup repair for diagnostic and read-only commands |
| `--help`, `-h`, `--version`, `-V` | Short-circuit before dispatch |
| No arguments | Print help without repair |
| Non-empty `AMPLIHACK_SKIP_AUTO_INSTALL` | Suppress implicit installation |

These exclusions do not authorise an implicit writer reached later in a
command. Each installation boundary enforces the origin restriction.

## Bypass: `AMPLIHACK_SKIP_AUTO_INSTALL`

Any non-empty value disables implicit installation. An unset or empty value
leaves the origin and version safeguards active. For example, set the opt-out
in a development or CI shell:

```sh
export AMPLIHACK_SKIP_AUTO_INSTALL=1
```

Subsequent checkout commands inherit the opt-out. It does not make missing
assets available or suppress an explicit `install` or `update`.

### Bypass diagnostic

When the stamp differs from the binary or the installed bundle is
incompatible, self-heal emits one diagnostic on stderr identifying
`AMPLIHACK_SKIP_AUTO_INSTALL`, the stamp, the running version and the bundle
compatibility result. A missing stamp is reported as `<missing>`.
A matching stamp with a compatible bundle produces no bypass diagnostic.

### What the bypass skips

Under the opt-out, self-heal and launch installation paths perform no source
fetch, staging, binary deployment, link publication, command restoration or
hook/provider reconciliation. They create no installation lock, fetch marker
or compatibility cache. Local version and bundle checks remain read-only;
an existing cache is not refreshed.

The npm tool update check has separate controls: `--skip-update-check` and
`AMPLIHACK_NONINTERACTIVE`. See [Manage tool update notifications](../howto/manage-tool-update-checks.md).

### Git hooks and test subprocesses

`scripts/git-hook-cargo.sh` unconditionally exports
`AMPLIHACK_SKIP_AUTO_INSTALL=1` to its children, including the checkout
binary used by `hygiene artifact-guard`. A caller's empty or conflicting
value cannot re-enable automatic installation through the wrapper.

Ordinary Rust subprocess fixtures default to the same opt-out and use
disposable homes and isolated configuration. Source-origin regression
fixtures deliberately remove the variable within those private roots to
exercise the origin safeguard. Tests never read or modify the real
`~/.claude`, `~/.amplihack`, `~/.codex` or shell profile root.

## Explicit install and update

The origin restriction governs implicit actions. Explicit `amplihack install`
and `amplihack update` remain supported from their existing valid release
sources, including when the running executable is outside the installed
releases tree or the opt-out is set.

An explicit update retains its post-update invocation of the new binary's
`install --force-refresh`. Startup update handling cannot use that explicit
path to bypass an origin refusal.

Explicit operations retain source validation, version checks, locking,
complete-release validation and publication safeguards. Neither the opt-out
nor explicit invocation broadens the accepted sources. See the
[Install command reference](../reference/install-command.md) and
[Post-update installation](update-reexec-new-binary.md).

## Stamp file

| Property | Contract |
| --- | --- |
| Path | `~/.amplihack/.installed-version` |
| Contents | Framework version on the first line. An optional second line, `binaries: <version>` or `binaries: unknown`, records binaries kept by an implicit re-stage |
| Writes | Atomic temporary-file rename, no trailing newline, mode `0600` |
| Missing or malformed | Treated as no prior install; repair still requires eligible origin |
| Other read errors | Reported through existing error handling |
| Symlink or non-regular file | Refused without reading or overwriting its target |

The binaries line does not identify the version of each executable separately.
Successful explicit installation and self-heal both write the stamp. Blocked
or opted-out repair leaves it untouched.

The advisory `~/.amplihack/.install.lock` serialises permitted self-heal,
install, update and uninstall operations. A waiting self-heal rechecks the
installation after acquiring the lock. Origin refusal happens before this
lock is acquired or created. Publication uses the existing
[Versioned install layout](../concepts/versioned-install-layout.md).

## Failure mode

For eligible automatic repair, installation errors propagate and produce a
non-zero exit status. A failed repair does not mark the stamp as current,
and releases the installation lock.

### Failed source fetch with a compatible bundle

If a release source cannot be fetched, or this build cannot fetch a release
tag, self-heal may continue only when the installed bundle passes the
compatibility check. It warns that it is keeping the installed bundle and
leaves the stamp unchanged. Explicit install and update do not continue past
a failed fetch. See [Framework source selection](../reference/framework-source-selection.md).

An origin refusal occurs before source acquisition, so it cannot enter this
fetch-failure fallback or write a fetch-failure marker.

### Unresolvable home directory

When the home directory cannot be resolved, startup self-heal skips
installation. It cannot establish an eligible installed release. Commands
requiring home-directory assets report their own errors; diagnostic commands
can still run.

## See also

- [Install command reference](../reference/install-command.md)
- [Framework source selection](../reference/framework-source-selection.md)
- [Environment variables](../reference/environment-variables.md)
- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md)
- [Versioned install layout](../concepts/versioned-install-layout.md)
- [Issue #2496: checkout builds replacing installed releases](https://github.com/rysweet/amplihack-rs/issues/2496)
