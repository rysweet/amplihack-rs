---
title: "Verify Generated Hook Line Endings"
description: "Check that generated amplihack hook scripts are LF-only and Bash-parseable after Copilot hook staging."
last_updated: 2026-06-19
review_schedule: as-needed
owner: amplihack
doc_type: howto
---

# Verify Generated Hook Line Endings

Use this guide when changing Copilot hook setup, legacy launcher staging, or
shared hook I/O utilities. It verifies that generated Bash-executable hook
scripts contain LF-only line endings and can be parsed by Bash.

## Prerequisites

- You are in a target repository where amplihack can stage Copilot hooks.
- `amplihack` is on `PATH`.
- `bash` is available.

## 1. Stage Copilot hooks

From the target repository, run install or launch Copilot. `amplihack install`
attempts best-effort Copilot hook staging; `amplihack copilot` stages hooks
before launch.

```bash
amplihack install >/dev/null
```

If you want to validate the launch path specifically, run:

```bash
amplihack copilot
```

Exit Copilot after the session starts, then continue with the checks below.

After staging succeeds, the target repository contains generated hook scripts under:

```text
.github/hooks/
```

## 2. Assert generated hooks contain no CR bytes

Check the raw files for carriage-return bytes:

```bash
if [ ! -d .github/hooks ]; then
  echo "expected .github/hooks after staging" >&2
  exit 1
fi

checked=0
for hook in .github/hooks/*; do
  [ -f "$hook" ] || continue
  checked=$((checked + 1))

  if grep -q "$(printf '\r')" "$hook"; then
    echo "generated hook contains CR bytes: $hook" >&2
    exit 1
  fi
done

if [ "$checked" -eq 0 ]; then
  echo "expected at least one generated hook in .github/hooks" >&2
  exit 1
fi
```

No output with exit code 0 means staging produced at least one generated hook
file and every generated hook file is LF-only.

## 3. Parse generated hooks with Bash

Run Bash syntax validation without executing hook behavior:

```bash
if [ ! -d .github/hooks ]; then
  echo "expected .github/hooks after staging" >&2
  exit 1
fi

checked=0
for hook in .github/hooks/*; do
  [ -f "$hook" ] || continue
  checked=$((checked + 1))
  bash -n "$hook"
done

if [ "$checked" -eq 0 ]; then
  echo "expected at least one generated hook in .github/hooks" >&2
  exit 1
fi
```

`bash -n` catches CR-related shell syntax problems such as:

```text
$'\r': command not found
```

It does not exercise the kernel or `/usr/bin/env` interpreter lookup, so it
does not catch a bad shebang such as `/usr/bin/env: 'bash\r': No such file or
directory`. The raw-byte check above is the required guard for that failure;
executing the generated hook in an integration test can also catch it.

## 4. Simulate CRLF input in tests

Regression tests should construct generated script content with CRLF input and
assert the written bytes are LF-only:

```rust
use amplihack_types::hook_io::normalize_executable_script_line_endings;

let generated = "#!/usr/bin/env bash\r\necho staged\r\n";
let normalized = normalize_executable_script_line_endings(generated);

assert_eq!(normalized, "#!/usr/bin/env bash\necho staged\n");
assert!(!normalized.as_bytes().contains(&b'\r'));
```

The test should write the normalized script through the same staging boundary
used by the CLI or launcher code, then read the file back as bytes:

```rust
let bytes = std::fs::read(&hook_path)?;
assert!(!bytes.contains(&b'\r'));
```

## 5. Simulate lone CR input in tests

Lone carriage returns are normalized because generated content may be assembled
from mixed sources:

```rust
use amplihack_types::hook_io::normalize_executable_script_line_endings;

let generated = "#!/usr/bin/env bash\recho staged\r";
let normalized = normalize_executable_script_line_endings(generated);

assert_eq!(normalized, "#!/usr/bin/env bash\necho staged\n");
assert!(!normalized.contains('\r'));
```

## 6. Validate both staging paths

Run focused tests for both write paths when changing hook staging:

```bash
cargo test -p amplihack-types hook_io
cargo test -p amplihack-cli copilot_setup
cargo test -p amplihack-launcher copilot_staging
```

The CLI hook setup tests cover current Copilot staging. The launcher tests cover
legacy or parallel staging paths that can still write generated hook scripts.

## Troubleshooting

| Symptom | Cause | Fix |
| ------- | ----- | --- |
| `$'\r': command not found` | A generated hook contains CR bytes after staging | Normalize generated script content immediately before writing the hook file |
| `/usr/bin/env: 'bash\r': No such file or directory` | The shebang line was written with CRLF | Normalize before `fs::write`; do not rely on Git checkout settings |
| Tests pass on Linux but fail on Windows | Tests did not simulate CRLF or lone CR input | Add synthetic CRLF and lone-CR fixtures with raw-byte assertions |
| Non-hook files changed | Normalization was applied too broadly | Limit normalization to generated executable hook or shell script write boundaries |

## Related

- [Generated Hook Line Endings Reference](../reference/generated-hook-line-endings.md)
- [Hook Specifications Reference](../reference/hook-specifications.md)
- [Copilot Parity Control Plane Reference](../reference/copilot-parity-control-plane.md)
