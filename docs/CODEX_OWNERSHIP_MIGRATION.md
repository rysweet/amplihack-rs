# Codex Ownership Ledger Migration

> Implemented by PR #1540 (commit `4a5067e6`) in
> `crates/amplihack-cli/src/commands/install/codex_plugin/`. This page
> describes that change and lands together with it.

Earlier Amplihack releases wrote `~/.amplihack/codex/ownership.json` with
`schema_version: 1` and an *unframed* SHA-256 digest (64 hex characters) of the
Codex plugin package at `~/.amplihack/codex/market/plugin`. Current releases
compute a *framed* digest (`v2:<hex>`). Upgrading used to leave these users
stuck on:

```
legacy or changed Codex ownership proof; preserve package and reconcile manually
```

The Codex plugin preparation step (`planning::prepare`, run when the Codex
launcher is prepared and by `amplihack install`) now migrates a verified legacy
ledger automatically. Only the Codex package is affected; the change touches
only the `codex_plugin` module and adds a test file, so Claude and Copilot code
paths are unchanged.

## What you will see

On the first Codex preparation after upgrading, one line is printed to stderr:

```
  ↻ Migrated verified legacy Codex ownership proof to the current format
```

`ownership.json` is rewritten with the framed digest. Later runs take the
normal path, so migration happens once.

## Safety model

The ledger is only a claim. Ownership is re-derived from the files on disk.
Migration proceeds only when all of these hold:

1. The ledger parses, `schema_version` is `1`, `codex_home` matches the current
   Codex home, and the digest is framed or exactly 64 hex characters.
2. Native plugin identity verification (`verify_identity`) has passed.
3. The package directory is a real directory, not a symlink.
4. The unframed digest recomputed from the package equals the ledger digest.
5. `plugin.json`, `hooks/hooks.json` and `bin/hook` are regular files (not
   symlinks); the manifest name is `amplihack`; `hooks.json` is the expected
   empty hooks document; `bin/hook` is byte-identical to Amplihack's own
   generated wrapper (never content from the ledger).
6. The ledger's hook declarations equal the declarations Amplihack generates
   for that package path (fixed events and subcommands only).

The unframed digest is accepted only to migrate. Afterwards the ledger holds a
framed digest, so a legacy digest cannot authorize anything else.

The new ledger is written with the existing `atomic_json` routine: temp file in
the same directory, `fsync`, check the ledger is unchanged since it was read,
rename, then sync the directory. Existing file permissions are preserved.
`path_scope` checks reject symlinked or foreign directory components.

### Residual risk

The checks use pathnames, not file descriptors, so a small TOCTOU window remains
between verification and the write. `atomic_json` re-reads the ledger just
before the rename and fails with "Codex JSON changed concurrently; retry" if it
changed, but a same-user process that swaps package files in that window is not
excluded. This matches the module's existing documented guarantees
(`path_scope` does not promise syscall-level race exclusion).

## Refusal behavior (fail closed)

Migration is refused, nothing is modified, and the
`legacy or changed Codex ownership proof` error is returned when:

| Condition | Result |
|---|---|
| Package content modified since install | refused |
| Ledger digest altered | refused |
| Foreign content whose digest matches the ledger | refused |
| Hooks pointing at other commands or paths | refused |
| `schema_version` other than 1, or `codex_home` mismatch | refused |

Amplihack never overwrites a package it cannot prove it generated. Inspect
`~/.amplihack/codex/`, back up anything you want to keep, then remove that
directory and run `amplihack install` to reinstall cleanly.

## Testing

Tests are in `codex_plugin/legacy_migration_tests.rs` and use a temporary
`HOME`; they never touch the real `~/.amplihack`:

- `verified_legacy_digest_ownership_migrates_to_framed_proof`
- `tampered_legacy_package_is_refused_without_changes`
- `tampered_legacy_digest_is_refused_without_changes`
- `matching_legacy_digest_over_foreign_content_is_not_trusted`
- `legacy_hooks_pointing_elsewhere_are_refused`

```
CARGO_TARGET_DIR=/tmp/amplihack-target cargo test -p amplihack-cli --lib legacy_migration
```

Use any writable directory for `CARGO_TARGET_DIR` if the default `target/` is
read-only or shared.

## Maintainer note

If the framed digest format changes again, add a new migration step rather than
widening the set of accepted legacy digests.
