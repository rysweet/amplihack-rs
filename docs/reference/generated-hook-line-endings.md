---
title: "Generated Hook Line Endings Reference"
description: "Reference for LF-only line ending normalization on generated amplihack hook and shell wrapper scripts."
last_updated: 2026-06-19
review_schedule: as-needed
owner: amplihack
doc_type: reference
---

# Generated Hook Line Endings Reference

amplihack writes generated hook and shell wrapper scripts with LF-only line
endings so Bash can execute them on Linux, macOS, WSL, and Windows-native
checkouts. This applies even when amplihack itself runs from a Windows-native
working tree or reads generated script templates that contain CRLF (`\r\n`) or
lone carriage returns (`\r`).

## Contract

Every generated Bash-executable hook script written by amplihack is normalized
at the final write or staging boundary:

| Input line ending | Written line ending |
| ----------------- | ------------------- |
| LF (`\n`)         | LF (`\n`)           |
| CRLF (`\r\n`)     | LF (`\n`)           |
| CR (`\r`)         | LF (`\n`)           |
| Mixed endings     | LF (`\n`)           |

The written script bytes must not contain carriage-return bytes (`0x0d`).

This prevents:

- bad interpreter failures such as `/usr/bin/env: 'bash\r': No such file or directory`
- shell parse errors such as `$'\r': command not found`
- Copilot hook failures caused by `.github/hooks/*` files generated from a CRLF checkout

## Scope

Line ending normalization applies only to generated executable scripts that
Bash or a hook host may run.

| Destination | Normalized? | Reason |
| ----------- | ----------- | ------ |
| target repo `.github/hooks/session-start` | yes | Copilot executes it as a hook script |
| target repo `.github/hooks/stop` | yes | Copilot executes it as a hook script |
| target repo `.github/hooks/pre-tool-use` | yes | Copilot executes it as a hook script |
| target repo `.github/hooks/post-tool-use` | yes | Copilot executes it as a hook script |
| target repo `.github/hooks/user-prompt-submit` | yes | Copilot executes it as a hook script |
| target repo `.github/hooks/pre-compact` | yes | Copilot executes it as a hook script |
| generated `_error_handler` script content | yes | Bash-facing executable support script |
| user-level generated Copilot hook wrappers | yes | Bash-facing hook scripts |
| legacy launcher-staged `.github/hooks/*` wrappers | yes | Bash-facing hook scripts |
| JSON manifests | no | data files are not executable shell scripts |
| copied docs, agent prompts, recipes, and unrelated repo files | no | not shell hook write boundaries |
| downstream repositories outside generated hook destinations | no | amplihack does not rewrite user source files |

Normalization is intentionally narrow. amplihack does not change target
repository Git configuration, global checkout settings, `.gitattributes`, or
unrelated files that happen to use CRLF.

## Write boundary behavior

Hook staging normalizes script content immediately before `fs::write` or the
equivalent file-write operation. Permission handling is unchanged: the script is
still made executable after writing through the same chmod path used by the
staging code.

The boundary rule is:

1. Build or collect the generated script content.
2. Normalize CRLF and lone CR to LF.
3. Write the normalized bytes.
4. Preserve existing executable permission behavior.

Do not normalize earlier template files as a substitute for this boundary. The
write boundary is the last safe point before bytes become executable hook
scripts.

## Rust API

Shared hook I/O utilities expose a single normalization helper:

```rust
use amplihack_types::hook_io::normalize_executable_script_line_endings;

let script = "#!/usr/bin/env bash\r\necho ready\r\n";
let normalized = normalize_executable_script_line_endings(script);

assert_eq!(normalized, "#!/usr/bin/env bash\necho ready\n");
assert!(!normalized.as_bytes().contains(&b'\r'));
```

### `normalize_executable_script_line_endings`

```rust
pub fn normalize_executable_script_line_endings(content: &str) -> String
```

| Property | Behavior |
| -------- | -------- |
| CRLF input | Converts every `\r\n` pair to `\n` |
| Lone CR input | Converts every remaining `\r` to `\n` |
| LF-only input | Leaves content unchanged except for returning an owned `String` |
| Mixed input | Converts all carriage returns to LF-only output |
| Final newline | Preserves the caller's final-newline choice; it does not add or remove one |
| Encoding | Operates on Rust `str` content before bytes are written |
| Idempotence | Running the helper more than once produces the same output |

Use this helper only for generated executable script content. Do not use it as a
general repository file normalizer.

## Configuration

No user configuration is required.

The LF-only contract is automatic whenever these paths write generated hook
scripts:

- `amplihack install` best-effort Copilot staging
- `amplihack copilot`
- Copilot hook staging before launch
- CLI hook setup that writes `.github/hooks/*`
- legacy or parallel launcher staging paths that can write generated hook scripts

Do not ask users to fix this by changing:

- downstream repositories such as application repos using amplihack
- global Git `core.autocrlf`
- repository-wide `.gitattributes`
- editor line-ending settings

Those settings may be useful for a project, but they are not required for
amplihack-generated hook scripts to run.

## Examples

### Generated Copilot hooks

After amplihack stages hooks for a repository, the generated files are safe for
Bash:

```bash
# From the target repository, stage the generated Copilot hooks.
amplihack install >/dev/null

if [ ! -d .github/hooks ]; then
  echo "expected .github/hooks after staging" >&2
  exit 1
fi

checked=0
for hook in .github/hooks/*; do
  [ -f "$hook" ] || continue
  checked=$((checked + 1))
  if grep -q "$(printf '\r')" "$hook"; then
    echo "CR byte found in $hook" >&2
    exit 1
  fi
  bash -n "$hook"
done

if [ "$checked" -eq 0 ]; then
  echo "expected at least one generated hook in .github/hooks" >&2
  exit 1
fi
```

### Windows-native CRLF checkout

A Windows-native checkout may provide CRLF script template content to the
staging code. The generated hook remains LF-only:

```bash
# From a Windows-native checkout of the target repository, attempt hook staging.
amplihack install >/dev/null

if [ ! -f .github/hooks/pre-tool-use ]; then
  echo "expected .github/hooks/pre-tool-use after staging" >&2
  exit 1
fi

if grep -q "$(printf '\r')" .github/hooks/pre-tool-use; then
  echo "pre-tool-use contains CR bytes" >&2
  exit 1
fi

bash -n .github/hooks/pre-tool-use
```

### Lone carriage-return input

Legacy or synthetic inputs containing lone CR bytes are normalized too:

```rust
use amplihack_types::hook_io::normalize_executable_script_line_endings;

let script = "#!/usr/bin/env bash\recho lone-cr\r";
let normalized = normalize_executable_script_line_endings(script);

assert_eq!(normalized, "#!/usr/bin/env bash\necho lone-cr\n");
assert!(!normalized.contains('\r'));
```

## Regression requirements

Regression coverage for hook staging must prove the raw written bytes contain
no carriage returns.

Required coverage:

- direct unit coverage for LF-only, CRLF, lone CR, and mixed line-ending input
- CLI Copilot hook setup coverage for generated `.github/hooks/*`
- user-level wrapper and `_error_handler` coverage when those scripts are written
- legacy or parallel launcher staging coverage for generated hook scripts
- raw-byte assertions such as `!bytes.contains(&b'\r')`
- `bash -n` validation for generated hook scripts where practical

The tests should simulate Windows-native CRLF input or checkout behavior inside
the test fixture. They must not depend on the host platform or global Git
settings.

## Related

- [Verify Generated Hook Line Endings](../howto/verify-generated-hook-line-endings.md)
- [Hook Specifications Reference](hook-specifications.md)
- [Copilot Parity Control Plane Reference](copilot-parity-control-plane.md)
