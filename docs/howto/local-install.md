---
title: Install framework assets from a local repository
description: Stage a local framework bundle while retaining release executable provenance.
doc_type: howto
review_schedule: as-needed
---

# Install framework assets from a local repository

Use `--local` to select framework assets from a checkout. Installation still
publishes executables from a supported release source. This option does not
install a development CLI or its sibling binaries.

## Prepare the sources

Use a supported release CLI and obtain the framework checkout:

```sh
git clone https://github.com/rysweet/amplihack-rs ~/src/amplihack-rs
```

The command creates `~/src/amplihack-rs`. If you already have that checkout,
use it directly.

The framework path must exist, contain a recognised framework root marker and
pass [bundle compatibility validation](../reference/framework-bundle-compatibility.md).
The installer canonicalises the root. Symlinks within the asset tree are
skipped rather than followed; provide regular files for assets you need staged.

## Stage the local framework

```sh
amplihack install --local ~/src/amplihack-rs
```

Installation reports the selected framework path and version. If the asset
version differs from the CLI's release version, it prints both versions and a
warning, then stages the compatible bundle. A recognised asset version also
selects the executable release version. The release allocation and
`.installed-version` stamp use the selected executable release version.

Install resolves all required release executables before acquiring framework
sources, taking the installation lock or changing profiles. It uses an eligible
installed release of the selected version, or obtains that exact published
platform archive. An unavailable archive is an error when no matching eligible
release is installed. The installer never falls back to publishing a checkout
build, even when you invoke `install` from that build.

After success, check the CLI selected by your shell:

```sh
command -v amplihack
amplihack --version
```

The path should be your user-level `~/.local/bin/amplihack`; the version should
be the selected release version. For an unversioned asset tree, installation
uses the running CLI version to select release executables.

## Prepare an offline host

`--local` avoids fetching the framework bundle. It does not make every install
phase offline. Before disconnecting:

- Provide an eligible installed release matching the selected executable
  version; a compiled checkout is insufficient.
- Provide the compatible local framework tree.
- Provision `recipe-runner-rs` on `PATH`, or set `RECIPE_RUNNER_RS_PATH` to its
  executable. Otherwise the recipe-runner phase needs network access.

Optional tool provisioning retains its own requirements; see the
[install reference](../reference/install-command.md).

## Recover from refusal

If release executables cannot be resolved, the active release, stamp and links
stay unchanged. Restore access to a supported published release and rerun
installation, or use `amplihack update` to select the available newer release.
`--local` cannot bypass executable validation.

This also limits `install --local` recovery guidance after a framework-tag
fetch failure: it replaces the asset source only. It cannot recover missing
release executables or make an unpublished development version installable.

To exercise development CLI changes without publishing them, follow
[Run a checkout build safely](run-checkout-safely.md).

## Related documentation

- [First installation](first-install.md)
- [Install command reference](../reference/install-command.md)
- [Framework source selection](../reference/framework-source-selection.md)
- [Executable eligibility](../features/self-heal-asset-restage.md#executable-eligibility)
