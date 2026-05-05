# Migration Contract Tests

## Overview

`crates/amplihack-remote/tests/migration_contract_test.rs` contains a suite of
permanent contract tests that guard the de-Pythonification migration (Epic #511,
Issue #536).  These tests do not exercise runtime behaviour — they assert
structural properties of the repository that must remain true forever after the
migration is complete.

Each test encodes a "never regress" constraint.  CI failure means a constraint
has been violated and the offending commit must be reverted or corrected before
merging.

---

## Contract tests

### `github_hooks_scope_creep_is_absent`

**Constraint**: The `.github/hooks/` directory must not exist in the repository.

```
Issue #536 quality gate: "no .github/hooks scope creep"
```

During the migration work, `.github/hooks/` files were accidentally staged
alongside unrelated changes on feature branches.  Issue #536 identified this
as a recurring risk and mandated a permanent enforcement mechanism.

This test resolves to the repository root at compile time (via
`CARGO_MANIFEST_DIR`) and asserts that the path `.github/hooks` does not exist
on the filesystem:

```rust
#[test]
fn github_hooks_scope_creep_is_absent() {
    let hooks_dir = repo_root().join(".github/hooks");
    assert!(
        !hooks_dir.exists(),
        "issue #536 forbids .github/hooks scope creep; remove {} before committing",
        hooks_dir.display()
    );
}
```

**Why `.github/hooks` is forbidden**

Git hooks belong in `.git/hooks/` (local, never committed) or in a
project-managed directory like `amplifier-bundle/modules/hook-*/` (versioned,
tested Python/Rust packages).  A `.github/hooks/` directory sits in neither
place: it is not automatically installed by Git and is not managed by the
amplihack module system.  Committing files there creates confusion about which
hook system is authoritative and may interfere with CI runners that scan
`.github/` for workflow definitions.

**Remediation**

If this test fails in CI, remove the directory before re-pushing:

```bash
git rm -r .github/hooks/
git commit -m "fix: remove .github/hooks scope creep (issue #536)"
git push
```

**Note**: `.github/workflows/` is explicitly permitted and unaffected by this
constraint.

---

### `python_remote_tree_is_deleted_after_native_port`

**Constraint**: No `.py` files may exist under
`amplifier-bundle/tools/amplihack/remote/` after the Rust port.

The 25 Python source files that previously implemented `amplihack remote` have
been replaced by the `amplihack-remote` Rust crate.  This test ensures the
Python tree is never re-introduced:

```rust
#[test]
fn python_remote_tree_is_deleted_after_native_port() {
    let remote_dir = repo_root().join("amplifier-bundle/tools/amplihack/remote");
    let mut python_files = Vec::new();
    collect_python_files(&remote_dir, &mut python_files);

    assert!(
        python_files.is_empty(),
        "issue #536 requires deleting every Python file under {}; still found: {:#?}",
        remote_dir.display(),
        python_files
    );
}
```

---

### `remote_rust_modules_stay_under_500_lines`

**Constraint**: Every `.rs` source file under `crates/amplihack-remote/src/`
must stay at or below 500 lines.

Issue #536 requires module size ≤ 500 lines as a readability and testability
gate.  This test walks the source tree and fails if any file exceeds the limit,
reporting the offending paths and their line counts.

---

### `detached_sessions_default_to_32gb_node_heap_contract`

**Constraint**: `SessionManager::DEFAULT_MEMORY_MB` must equal `32768`.

Remote sessions launched by `amplihack remote start` inherit the
`NODE_OPTIONS=--max-old-space-size=32768` preference.  This test pins the
default so a refactor cannot silently lower the heap budget:

```rust
assert_eq!(
    SessionManager::DEFAULT_MEMORY_MB,
    32_768,
    "remote start must persist memory_mb=32768 to match NODE_OPTIONS=--max-old-space-size=32768"
);
```

---

### `vm_size_tiers_match_documented_capacity_and_azure_skus`

**Constraint**: `VMSize` capacity values and Azure SKU strings must match the
values documented in the architecture spec.

This test locks the mapping between the four VM tiers (`S`, `M`, `L`, `XL`),
their session-capacity multipliers, and the underlying Azure VM sizes.
Changing a SKU string requires updating both the code and this test.

---

## Running the contract tests

```bash
# Run only the migration contract suite
cargo test -p amplihack-remote --test migration_contract_test --locked

# Run with verbose output to see which constraints are being checked
cargo test -p amplihack-remote --test migration_contract_test --locked -- --nocapture
```

All five tests run in under one second because they only inspect the filesystem
and compile-time constants — no network calls, no VM allocation.

---

## Adding a new contract test

1. Add a `#[test]` function to `migration_contract_test.rs`.
2. Reference the GitHub issue that mandates the constraint in the `assert!`
   message so future contributors can trace the reasoning.
3. Keep the test free of I/O side-effects: read files, check paths, inspect
   constants — never write, never spawn processes.
4. Run `cargo test -p amplihack-remote --test migration_contract_test --locked`
   locally before opening a PR.
