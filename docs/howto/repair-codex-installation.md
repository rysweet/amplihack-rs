---
title: Repair a Codex installation
description: Recover from Codex startup rejection caused by an incomplete installation.
type: howto
updated: 2026-11-10
---

# Repair a Codex installation

This guide describes recovery from the Rust startup validation gate.

Use this guide when `amplihack codex` rejects an installation before starting
a session, such as a standalone binary missing `codex-code-mode-host`.

## Identify the rejected installation

Read the startup diagnostic for the executable path and concrete failure.
`codex --version` can succeed even when required helpers are absent. See the
[validation reference](../reference/codex-installation-validation.md) for
supported layouts and recovery policy.

If validation is inconclusive or reports a resource failure, address that
reported cause before retrying. Such failures do not establish that the package
needs reinstalling. If the failure cannot be repaired by replacing the package,
follow the diagnostic to address its cause instead.

## Repair the selected package

For a conclusively repairable package failure in a discovered installation
without an external user override, interactive CLI startup automatically installs
the managed npm package unless `AMPLIHACK_SKIP_AUTO_INSTALL` is set.
Noninteractive CLI startup uses the same policy without prompting. The legacy
launcher follows its explicit `auto_install` argument. amplihack validates the replacement before
launching; a second rejection means the repair did not produce a usable target.

For an explicitly selected standalone installation, reinstall the complete
release for the selected version and platform, including its required helpers.
Keep the release layout intact instead of copying only the `codex` executable.
For an npm installation, reinstall the official `@openai/codex` package into the
prefix that owns the rejected path, including its selected native package.

If `AMPLIHACK_CODEX_BINARY_PATH` or `CODEX_BINARY_PATH` selects the broken
installation, repair that installation, change the override to a complete one,
or unset the override to resume normal discovery. Check both variables: removing
the higher-priority override can expose the other one. Installing into
amplihack's managed prefix does not replace an external override.

## Retry startup

Run `amplihack codex` again. A usable installation reaches session startup.
Otherwise, startup returns an error naming the remaining failure rather than
opening a session with broken tools.

For a recipe, retry the failed agent step through the recipe's normal retry
procedure after repairing the installation. The original startup diagnostic
remains the explanation for the failed attempt.
