---
title: Codex installation validation
description: Startup validation, executable selection, and repair policy for Codex.
type: reference
updated: 2026-11-10
---

# Codex installation validation

This reference describes the Rust startup validation contract.

`amplihack codex` checks the selected installation before starting a session.
Recipe Codex agents and the legacy Rust launcher use the same validation.
A successful `codex --version` is insufficient: the installation must also
provide the native executable and helpers required by that version.

For repair steps, see [Repair a Codex installation](../howto/repair-codex-installation.md).
Installing Codex itself is separate from [installing the amplihack Codex plugin](../howto/install-codex-plugin.md).

## Accepted installations

Validation resolves standalone executables, filesystem symlinks, and supported
official npm wrappers. For npm installations, it checks the native executable
selected by the wrapper's platform package. An unrelated executable beside a
wrapper does not establish usability. Launch preserves the validated wrapper
when its environment setup is needed.

### Version and layout compatibility

The supported scope is complete standalone release layouts, symlinks to those
layouts, and official npm wrappers with the native layout for their version.
The following pinned upstream sources establish distinct npm layouts; they do
not establish a continuous supported version range or prove runtime usability.

| Upstream version | Native layout relative to the vendor root | Primary-source evidence |
| --- | --- | --- |
| 0.98.0 | `<target>/codex/codex` (`codex.exe` on Windows), bundled under the wrapper package | [Official 0.98.0 wrapper](https://github.com/openai/codex/blob/rust-v0.98.0/codex-cli/bin/codex.js) |
| 0.166.0 | `<target>/bin/codex` (`codex.exe` on Windows), resolved from the platform package, with a bundled vendor fallback | [Official 0.166.0 wrapper](https://github.com/openai/codex/blob/rust-v0.166.0/codex-cli/bin/codex.js) |

These wrappers map x64 and arm64 targets for Linux, macOS, and Windows; their
platform mappings do not by themselves promise amplihack support on every target.
Validation must follow the selected wrapper's layout and preserve its environment
setup rather than guess a native path from a neighboring executable.

The validator accepts stable standalone versions through 0.98.0 without a
code-mode host, and stable 0.147.0 through 0.166.0 with the required host.
The 0.98.0 workspace predates the host crate. Upstream
[commit 97576b179487](https://github.com/openai/codex/commit/97576b179487)
removed the in-process fallback. The
[0.146.0 provider](https://github.com/openai/codex/blob/rust-v0.146.0/codex-rs/code-mode/src/remote_session.rs)
still has that fallback; the
[0.147.0 provider](https://github.com/openai/codex/blob/rust-v0.147.0/codex-rs/code-mode/src/remote_session.rs)
uses the standalone host. The
[0.147.0 installation context](https://github.com/openai/codex/blob/rust-v0.147.0/codex-rs/install-context/src/lib.rs)
and
[0.166.0 installation context](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/install-context/src/lib.rs)
prefer a packaged `codex-resources/codex-code-mode-host` when package metadata
establishes that layout, then the helper beside the native executable.

Official npm wrappers are supported at 0.98.0 (bundled vendor layout) and
0.147.0 through 0.166.0 (platform package, with bundled vendor fallback).
The [0.147.0 wrapper](https://github.com/openai/codex/blob/rust-v0.147.0/codex-cli/bin/codex.js)
already uses the latter layout. The
[0.166.0 npm package builder](https://github.com/openai/codex/blob/rust-v0.166.0/codex-cli/scripts/build_npm_package.py)
publishes platform packages as npm aliases: their actual package name remains
`@openai/codex`, with a platform-suffixed version (for example
`0.166.0-linux-x64`). Validation checks the alias dependency and platform
package metadata and retains the wrapper for launch.

Versions 0.99 through 0.146, prereleases, and versions newer than 0.166.0 are
inconclusive: their exact capability contract is not established here.
Other legacy npm wrapper versions and unrecognized wrapper extensions are also
inconclusive. This deliberately conservative boundary can reject an otherwise
working installation; select a verified version rather than repeatedly
reinstalling an unverified one. No version-only or agent-session probe is used.

### Conditional capability probing

If filesystem inspection cannot establish usability, a local capability probe
may be used only after its exact invocation has been verified for the supported
version. It must exercise the required capability without credentials, network,
a model response, an agent session, or configuration writes. No suitable
invocation is established by this reference; a trivial `exec` is not assumed to
meet those requirements. Until a safe probe is verified, inspection that cannot
establish usability returns an inconclusive diagnostic and stops startup.

## Executable selection and configuration

Discovery retains the existing candidate order:

1. `AMPLIHACK_CODEX_BINARY_PATH`, then `CODEX_BINARY_PATH`.
2. Executables on `PATH`, in order.
3. amplihack's npm prefix, `~/.npm-global/bin`.
4. Fallback directories, including `~/.cargo/bin` and `~/.local/bin`.

Explicit user overrides retain their authority. A broken external override is
reported rather than silently replaced with another installation. Managed npm
installation cannot repair an override pointing outside the managed prefix.

After validation, interactive and subprocess launches retain the accepted
executable path. They do not resolve bare `codex` again through ambient `PATH`.
Version caching does not replace fresh installation validation after repair.

## Recovery policy

| Validation outcome | Startup behavior |
| --- | --- |
| Usable installation | Launch the accepted target. |
| Conclusively repairable package failure, such as a verified required helper missing from a managed installation | Report the path and concrete failure; apply existing installation policy. |
| Conclusive failure that package replacement cannot repair | Stop with the concrete failure and guidance for its cause. |
| Broken external user override | Stop with instructions to repair, change, or unset the override. |
| Inconclusive validation or resource failure | Stop with the diagnostic; do not assume reinstalling will help. |

CLI recovery installs `@openai/codex` through the existing managed npm prefix,
including optional platform dependencies containing its executable and helpers.
`AMPLIHACK_SKIP_AUTO_INSTALL` refuses automatic Codex recovery. Noninteractive
startup retains the existing automatic installation behavior without prompting.
The legacy launcher honors its explicit `auto_install` argument.
External overrides are rejected with guidance to repair, change, or unset them.
No new consent prompt or installation switch is introduced.

There is at most one installation attempt per startup recovery. After that
attempt, discovery runs uncached and validates the replacement. Installer
success alone does not authorize launch. An unusable replacement returns a
nonzero result identifying its executable, failure, and repair guidance. If the
installer fails, the error retains both the original validation failure and the
installer failure.

## Diagnostics and recipe failures

For a missing helper, the diagnostic identifies the resolved Codex executable
and the missing `codex-code-mode-host` path. Symlink and wrapper failures retain
the discovered and resolved paths where available. Paths and child output are
sanitized for display.

An unrecoverable recipe-agent startup failure immediately fails that agent
step, preserving the diagnostic before agent work begins. The agent does not
start a session whose tools all fail because a required helper is missing.

## Rust integration contract

`amplihack-utils::codex_installation` owns shared, read-only installation
validation. It accepts a candidate and version and returns the validated launch
path, the resolved native path when known, or a structured repairable, non-repairable, resource-failure, or
inconclusive result. Installation consent and configuration remain with callers.

`amplihack-utils::launch_target` validates candidates before acceptance and
retains the accepted target. CLI bootstrap owns policy and replacement
validation; the command builder accepts the resolved executable without
installing anything.

The legacy `amplihack_launcher::codex::check_codex` checks usability rather than
version output alone. `ensure_and_build` applies its existing `auto_install`
argument to missing and unusable installations, validates replacements, and
builds from the accepted target. Prompt delivery and stdin ownership retain
their existing contracts.
