# Bug Fix #675 — `amplihack update` Does Not Refresh the amplifier-bundle

> **Issue:** [#675](https://github.com/rysweet/amplihack-rs/issues/675)

---

## Summary

After `amplihack update` downloads a new binary, the post-update install
re-stages the **old** `amplifier-bundle/` from `~/.amplihack/` instead of
downloading fresh framework assets from the upstream archive. This causes
new binary + stale recipes, producing errors like "orch_helper.py not found"
for users who upgraded from Python-era versions.

## Root Cause

In `crates/amplihack-cli/src/commands/install/clone.rs`,
`find_bundled_framework_root()` checks for an existing
`~/.amplihack/amplifier-bundle/` directory **before** falling back to the
network download. When `amplihack update` downloads a new binary (e.g. v0.9.61)
and triggers post-update install, `run_install()` finds the OLD bundle at
`~/.amplihack/` and re-stages it — the local copy wins the resolution race
against the network download.

**Resolution order before fix:**

```
1. Compile-time workspace root
2. AMPLIHACK_HOME
3. Walk-up from executable
4. ~/.amplihack/amplifier-bundle/    ← OLD bundle found here, stops looking
5. Network download (never reached)
```

## Fix

Added a `force_refresh: bool` parameter to `run_install()`. When
`force_refresh` is `true`, the function skips `find_bundled_framework_root()`
entirely and goes directly to `download_and_extract_framework_repo()` to fetch
a fresh bundle from `REPO_ARCHIVE_URL` (the `main.tar.gz` archive).

**Call sites:**

| Caller | `force_refresh` | Behavior |
|--------|-----------------|----------|
| `amplihack install` (standalone) | `false` | Unchanged — prefers local bundle |
| `amplihack update` (post-update) | `true` | **Fix** — always downloads fresh |
| Self-heal (startup check) | `false` | Unchanged — prefers local for speed |
| `ensure_framework_installed()` | `false` | Unchanged — bootstrap prefers local |

**No user-facing behavior change** for standalone `amplihack install`. The
`--local` flag continues to take priority over `force_refresh` (returns early
before the bundled-root check).

## Files Changed

| File | Change |
|------|--------|
| `crates/amplihack-cli/src/commands/install/mod.rs` | Added `force_refresh: bool` param to `run_install()`; guard `find_bundled_framework_root()` behind `!force_refresh` |
| `crates/amplihack-cli/src/commands/mod.rs` | Pass `force_refresh: false` for standalone `amplihack install` |
| `crates/amplihack-cli/src/update/check.rs` | Pass `force_refresh: true` in post-update closure (**the fix**) |
| `crates/amplihack-cli/src/self_heal.rs` | Pass `force_refresh: false` for startup self-heal |
| `crates/amplihack-cli/src/commands/install/tests/install_flow.rs` | Pass `force_refresh: false` at test call sites |

## Verification

After the fix, `amplihack update` produces:

```
✓ Updated amplihack to v0.9.61
📦 Forcing fresh framework download from upstream...
✓ Staged framework assets (47 files, 12 directories)
amplihack installed successfully.
```

The fresh download ensures all recipes, agents, and tools match the new binary
version.

## Workaround (pre-fix binaries)

Users on pre-#675 binaries who encounter stale bundle errors after update can
run:

```sh
# Force a manual re-install (will still prefer local bundle, but
# if the local bundle was updated by the binary swap, this works)
amplihack install

# Or, remove the stale bundle first to force network download:
rm -rf ~/.amplihack/amplifier-bundle/
amplihack install
```

## Related

- Issue [#666](https://github.com/rysweet/amplihack-rs/issues/666) — Stale
  Python references in documentation (related symptom, different root cause)
- [Self-Heal Asset Re-Stage](../features/self-heal-asset-restage.md) — The
  startup version-stamp check (uses `force_refresh: false`)
- [Install Command Reference](../reference/install-command.md) — Full
  `run_install()` API documentation including `force_refresh`
- [Manage Tool Update Checks](../howto/manage-tool-update-checks.md) — What
  happens during `amplihack update`
