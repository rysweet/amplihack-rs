# amplihack doctor — Command Reference

## Synopsis

```
amplihack doctor
```

## Description

Checks the local system for the conditions required to run amplihack correctly and prints a pass/fail summary. Each check is independent: all checks always run regardless of earlier failures. The command exits with code `0` if every check passed, or `1` if any check failed.

`doctor` is the first thing to run when amplihack behaves unexpectedly. Its output identifies exactly which prerequisite is missing so you can fix it directly.

## Options

`doctor` takes no options. The subcommand name alone is sufficient:

```sh
amplihack doctor
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All 7 checks passed |
| `1` | One or more checks failed |

## Checks

`doctor` runs 7 checks in order. Each check prints one line of output.

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

### Check 4 — Python bridge working

Runs `python3 -c "import amplihack"` and checks for a zero exit code. Verifies that Python 3 is present and that the amplihack Python package is importable in the active environment.

```
✓ Python bridge working
✗ Python bridge working — ModuleNotFoundError: No module named 'amplihack'
```

### Check 5 — tmux installed

Runs `tmux -V` and checks for a zero exit code. tmux is required for amplihack's session management features.

```
✓ tmux 3.4
✗ tmux installed — command not found: tmux
```

### Check 6 — amplihack version

Reports the amplihack version from the compiled binary. This check always passes on a valid install; it is informational.

```
✓ amplihack v0.9.1
```

### Check 7 — recipe-runner-rs version

Runs `recipe-runner-rs --version` and reports the version string. This check reuses the same process spawn as Check 3 but reports the actual version string rather than just pass/fail. If Check 3 failed, Check 7 also fails.

```
✓ recipe-runner-rs v0.4.2
✗ recipe-runner-rs version — command not found: recipe-runner-rs
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

## Security Properties

- **No secrets printed.** `settings.json` content is never echoed. Only existence, JSON validity, and the presence of the string `"amplihack"` inside the `hooks` section are reported.
- **Error messages are truncated.** stderr from subprocess checks is truncated to the first line, maximum 200 characters, before being included in output. This prevents adversarial error output from flooding logs.
- **ANSI stripped on external output.** Version strings from `tmux -V` and `recipe-runner-rs --version` have ANSI escape codes stripped before display.
- **Compile-time constants only for self-version.** amplihack's own version (Check 6) uses the `CARGO_PKG_VERSION` compile-time constant; the binary does not spawn itself as a subprocess.
- **All subprocess arguments are compile-time literals.** No user input reaches any `Command::new()` call.

## Example: All Checks Pass

```
$ amplihack doctor
✓ hooks installed
✓ settings.json valid JSON
✓ recipe-runner-rs v0.4.2
✓ Python bridge working
✓ tmux 3.4
✓ amplihack v0.9.1
✓ recipe-runner-rs v0.4.2

All checks passed.
```

## Example: Two Failures

```
$ amplihack doctor
✓ hooks installed
✓ settings.json valid JSON
✗ recipe-runner available — command not found: recipe-runner-rs
✗ Python bridge working — ModuleNotFoundError: No module named 'amplihack'
✓ tmux 3.4
✓ amplihack v0.9.1
✗ recipe-runner-rs version — command not found: recipe-runner-rs

3 check(s) failed.
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
