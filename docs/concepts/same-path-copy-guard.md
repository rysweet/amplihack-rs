# Same-Path Copy Guard

Every `copy_dir_recursive` function in amplihack-rs includes a same-path
guard that prevents self-copy when source and destination resolve to the
same filesystem path.

## Problem

When `AMPLIHACK_HOME` points at the source checkout itself—or when a
symlink causes two seemingly-different paths to converge—a recursive
directory copy would either:

1. Enter infinite recursion (copying into itself)
2. Corrupt files by overwriting them mid-read
3. Panic with an OS-level "same file" error

This scenario occurs during local development (`AMPLIHACK_HOME=.`),
symlinked installs, and CI pipelines that reuse the checkout directory
as the install target.

## Solution

Before copying, each function canonicalizes both paths and compares them:

```rust
if let (Ok(canon_src), Ok(canon_dst)) = (source.canonicalize(), dest.canonicalize()) {
    if canon_src == canon_dst {
        tracing::warn!(
            src = %source.display(),
            dst = %dest.display(),
            "skipping copy: source and destination are the same path"
        );
        return Ok(());
    }
}
```

### Design Decisions

| Decision | Rationale |
| --- | --- |
| `canonicalize()` over raw path comparison | Resolves symlinks, `..`, and relative segments so aliased paths are detected |
| `if let` with fallthrough | `canonicalize()` fails on broken symlinks or permission errors; falling through to the normal copy is the safe default |
| `return Ok(())` (not `Err`) | Same-path is not an error—it means the content is already in place |
| `tracing::warn!` | Visible in logs for debugging without halting execution |

### TOCTOU Consideration

There is a time-of-check-to-time-of-use (TOCTOU) window between the
`canonicalize()` check and the actual `fs::copy` / `fs::create_dir_all`
calls. This is accepted because:

- Both paths are controlled by the same user/process
- The guard protects against *configuration* mistakes, not adversarial races
- Eliminating the window would require holding file locks across the entire
  copy, which is disproportionate to the risk

## Affected Functions

| Crate | File | Function |
| --- | --- | --- |
| `amplihack-cli` | `src/auto_stager.rs` | `copy_dir_recursive` |
| `amplihack-launcher` | `src/auto_stager.rs` | `copy_dir_recursive` |
| `amplihack-context` | `src/migration.rs` | `copy_dir_recursive` |
| `amplihack-cli` | `src/commands/mode/migration.rs` | `copy_dir_recursive` |

Each function has an inline `#[test]` that creates a `tempdir`, and
verifies the function returns `Ok(())` without copying when source and
destination are identical:

```rust
#[test]
fn copy_dir_recursive_same_path_returns_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("file.txt"), "hello").unwrap();
    let result = copy_dir_recursive(dir, dir);
    assert!(result.is_ok());
}
```

## Python Equivalent

The Python installer (`amplihack/install.py`) has the same guard using
`os.path.samefile()`, which compares device and inode numbers:

```python
if os.path.exists(target_dir) and os.path.samefile(source_dir, target_dir):
    print(f"  ⚠️  Skipping {dir_path}: source and target are the same path")
    continue
```

## Related

- [Issue #4296](https://github.com/rysweet/amplihack/issues/4296)
- [Python fix PR #4297](https://github.com/rysweet/amplihack/pull/4297)
- [Rust fix PR #201](https://github.com/rysweet/amplihack-rs/pull/201)
