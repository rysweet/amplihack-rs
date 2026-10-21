# amplihack doctor — Command Reference

## Synopsis

```
amplihack doctor
```

## Description

Checks the local system for the conditions required to run amplihack correctly and prints a pass/fail summary. It then prints the effective [user configuration](./user-config.md) with the origin of each value. Each check is independent: all checks always run regardless of earlier failures. The command exits with code `0` if every check passed, or `1` if any check failed.

`doctor` is the first thing to run when amplihack behaves unexpectedly. Its output identifies exactly which prerequisite is missing so you can fix it directly.

## Options

`doctor` takes no options. The subcommand name alone is sufficient:

```sh
amplihack doctor
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All 5 checks passed |
| `1` | One or more checks failed |

## Checks

`doctor` runs 5 checks in order. Each check prints one line of output.

### Check 1 — hooks installed

Reads `~/.claude/settings.json` and verifies that at least one hook command string contains `"amplihack"`. A passing result means the amplihack hook dispatcher is registered in Claude Code's settings.

```
✓ hooks installed
✗ hooks installed — no amplihack hook found in ~/.claude/settings.json
```

### Check 2 — settings.json valid JSON

Reads `~/.claude/settings.json` and parses it as JSON. Fails if the file is absent, unreadable, or not valid JSON. This check is independent of Check 1 — both always run.

```
✓ settings.json valid JSON
✗ settings.json valid JSON — expected value at line 14 column 5
```

### Check 3 — recipe-runner available

Runs `recipe-runner-rs --version` and checks that the command succeeds. Verifies the binary is on `PATH` and executable.

```
✓ recipe-runner-rs v0.4.2
✗ recipe-runner available — command not found: recipe-runner-rs
```

### Check 4 — tmux installed

Runs `tmux -V` and checks for a zero exit code. tmux is required for amplihack's session management features.

```
✓ tmux 3.4
✗ tmux installed — command not found: tmux
```

### Check 5 — amplihack version

Reports the amplihack version from the compiled binary. This check always passes on a valid install; it is informational.

```
✓ amplihack v0.9.1
```

## Output Format

Each check line starts with `✓` (U+2713 CHECK MARK) for pass or `✗` (U+2717 BALLOT X) for fail, followed by the check name and, on failure, a dash and a short error message.

At the end of all checks, a summary line is printed:

```
All checks passed.
```

or:

```
2 check(s) failed.
```

ANSI colour codes are always emitted: green for `✓` lines, red for `✗` lines, and bold for the summary. If you need plain text output (for log ingestion, for example), strip ANSI codes with a tool such as `sed 's/\x1b\[[0-9;]*m//g'` or `ansistrip`.

## Effective config

After the summary line, `doctor` prints an `Effective config` section. Its rows are the same as `amplihack config list --show-origin`: every config key in schema order, as `key = value  # origin`. Any problem found in a config file follows in a `Config problems:` block.

```
Effective config:
  agent.binary = "claude"  # user (/home/alice/.amplihack/config.toml)
  paths.home = (unset)  # default
  node.heap_consent = true  # user (/home/alice/.amplihack/config.toml)
  node.max_old_space_mb = (unset)  # default
  recipe.verbose = false  # default
  recipe.step_timeout = (unset)  # default
  recipe.detach = "none"  # default
  recipe.tmux_session_pattern = "amp-{recipe}-{repo}-{ts}"  # default
  recipe.log_dir = (unset)  # default
  orchestrator.max_depth = 2  # project (/home/alice/src/myapp/.amplihack/config.toml)

Config problems:
  /home/alice/src/myapp/.amplihack/config.toml: paths.home: can only be set in the user file; ignored
```

The section is informational. It is not one of the checks, and a config problem never changes the exit code. Keys, values and paths taken from config files are escaped and truncated to 200 characters before they are printed. See [Validation and diagnostics](./user-config.md#validation-and-diagnostics).

## Security Properties

- **No secrets printed.** `settings.json` content is never echoed. Only existence, JSON validity, and the presence of the string `"amplihack"` inside the `hooks` section are reported.
- **Error messages are truncated.** stderr from subprocess checks is truncated to the first line, maximum 200 characters, before being included in output. This prevents adversarial error output from flooding logs.
- **ANSI stripped on external output.** Version strings from `tmux -V` and `recipe-runner-rs --version` have ANSI escape codes stripped before display.
- **Compile-time constants only for self-version.** amplihack's own version (Check 5) uses the `CARGO_PKG_VERSION` compile-time constant; the binary does not spawn itself as a subprocess.
- **All subprocess arguments are compile-time literals.** No user input reaches any `Command::new()` call.

## Example: All Checks Pass

```
$ amplihack doctor
✓ hooks installed
✓ settings.json valid JSON
✓ recipe-runner-rs v0.4.2
✓ tmux 3.4
✓ amplihack v0.9.1

All checks passed.

Effective config:
  agent.binary = (unset)  # default
  paths.home = (unset)  # default
  node.heap_consent = true  # user (/home/alice/.amplihack/config.toml)
  node.max_old_space_mb = (unset)  # default
  recipe.verbose = false  # default
  recipe.step_timeout = (unset)  # default
  recipe.detach = "none"  # default
  recipe.tmux_session_pattern = "amp-{recipe}-{repo}-{ts}"  # default
  recipe.log_dir = (unset)  # default
  orchestrator.max_depth = 3  # default
```

## Example: Two Failures

```
$ amplihack doctor
✓ hooks installed
✓ settings.json valid JSON
✗ recipe-runner available — command not found: recipe-runner-rs
✓ tmux 3.4
✓ amplihack v0.9.1

1 check(s) failed.
$ echo $?
1
```

## Use in CI

`doctor` exits non-zero on any failure, making it safe to use as a readiness gate:

```yaml
- name: Check amplihack prerequisites
  run: amplihack doctor
```

If the job must continue even when checks fail (for example, to collect diagnostic output), use:

```sh
amplihack doctor || true
```

## Related

- [How to Diagnose Problems with amplihack doctor](../howto/diagnose-with-doctor.md) — Actionable fix guide for each failing check
- [Hook Specifications](./hook-specifications.md) — What hooks amplihack registers in settings.json
- [amplihack install](./install-command.md) — Installs the prerequisites that doctor verifies
- [User Configuration](./user-config.md) — The config files and keys shown under `Effective config`
