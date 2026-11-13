---
title: Binary resolution reference
description: Existing hooks executable lookup and its separation from release publication.
doc_type: reference
review_schedule: as-needed
---

# Binary Resolution Reference

`find_hooks_binary()` locates an existing `amplihack-hooks` executable for
callers that need hook resolution. Installation stages binaries and registers
hooks using separately validated release executables; hook lookup does not
select publication sources. See
[Release binary staging](install-command.md#release-binary-staging-internal).

## Resolution Order

`find_hooks_binary()` tries each location in order and returns the first one that resolves to an existing executable file:

```
Step 1: AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH env var
Step 2: Sibling of the running amplihack executable
Step 3: PATH lookup (which amplihack-hooks)
Step 4: ~/.local/bin/amplihack-hooks
Step 5: ~/.cargo/bin/amplihack-hooks
```

If none of the five steps succeeds, the caller receives an actionable error
listing what was tried.

## Step Details

### Step 1: Environment Variable Override

If `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` is set, its value is used if it points to an existing executable. If the variable is set but the path does not exist, lookup reports an error.

This override affects lookup only. It cannot replace a required executable
from the selected release during installation.

### Step 2: Sibling of Current Executable

Looks for `amplihack-hooks` in the same directory as the running `amplihack` binary. This covers the common case of running both binaries from a `cargo build --release` output directory:

```
target/release/
├── amplihack        ← running this
└── amplihack-hooks  ← found here at Step 2
```

### Step 3: PATH Lookup

`find_binary("amplihack-hooks")` walks `$PATH` entries in order and returns the
first match. System-wide executables can be found here even when no user-local
executable exists.

This step also covers the tarball-to-`/usr/local/bin` install pattern where both `amplihack` and `amplihack-hooks` are placed into the same system directory. In that case Step 2 (sibling-of-exe) resolves the binary when running from that directory, but Step 3 provides a reliable fallback when the shell's `PATH` is used instead.

### Step 4: ~/.local/bin

The user-local binary directory. Release installation publishes executable
links here, so this step can resolve an already installed hooks executable.

### Step 5: ~/.cargo/bin

Where Cargo installs binaries via `cargo install`. Covers the case where a user ran `cargo install --path bins/amplihack-hooks` manually.

## Error Output

If lookup fails, the error lists the attempted locations. For example:

```
❌ Could not find amplihack-hooks binary.

Tried:
  1. AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH — not set
  2. /home/alice/src/amplihack-rs-update/target/release/amplihack-hooks — not found
  3. PATH lookup — not found
  4. /home/alice/.local/bin/amplihack-hooks — not found
  5. /home/alice/.cargo/bin/amplihack-hooks — not found

Fix: build the binary first:
  cargo build --release --bin amplihack-hooks
```

This lookup error is separate from an installation failure to obtain release executables. Building `amplihack-hooks`
can satisfy a development lookup; it does not make that binary an acceptable
release publication source.

## See Also

- [amplihack install reference](./install-command.md) — full install command and environment variables
- [First-time install how-to](../howto/first-install.md) — step-by-step install guide
- [Run a checkout build safely](../howto/run-checkout-safely.md) — development invocation and subprocess isolation
