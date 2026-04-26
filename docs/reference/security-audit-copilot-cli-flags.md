# Security Audit: Copilot CLI Flags Fix

**Type**: Reference (Information-Oriented)

Security review of the Copilot CLI flag isolation changes that ensure correct
permission flags are selected based on the active agent binary.

## Scope

- `smart-orchestrator.yaml` — classify-and-decompose step conditionally selects
  CLI flags based on `$AGENT_BIN`
- `claude_process.py` — `_build_command()` selects permission flags based on
  delegate type
- `auto_mode.py` — `_run_sdk_subprocess()` routes flags per agent binary

**Verdict**: APPROVED

## Security Checklist

| Area                         | Status | Notes                                                |
| ---------------------------- | ------ | ---------------------------------------------------- |
| Input validation             | PASS   | Env vars validated; unknown binaries raise error     |
| Output encoding              | PASS   | No user content reflected unsanitized                |
| Authentication/authorization | N/A    | No auth changes                                      |
| Sensitive data handling      | PASS   | Session tokens stripped before subprocess invocation  |
| No hardcoded secrets         | PASS   | Clean                                                |
| Error messages               | PASS   | Emit binary name and exit code only; no stack traces |

## Findings

### FINDING 1 (MEDIUM): Copilot branch soft constraint

Claude branch uses `--disallowed-tools` (runtime-enforced tool blocklist).
Copilot branch uses `--allow-all-tools` with a prompt-injected constraint.
Prompt constraints are *soft* — the model can still invoke tools.

**Risk**: LOW. The classify step is a single-turn invocation. Even if the model
ignores the constraint, the subprocess exits after one turn.

**Mitigation**: Copilot CLI does not support `--disallowed-tools`. This is the
best available approach.

### FINDING 2 (LOW): `$AGENT_BIN` glob matching breadth

Any binary path containing `copilot` or `codex` matches. An attacker who
controls `AMPLIHACK_AGENT_BINARY` already has shell access, making this
non-exploitable.

### FINDING 3 (LOW): Session limit overrides via environment

`_get_positive_int_env()` allows raising `max_total_api_calls` and
`max_session_duration` via env vars. Validation correctly rejects non-positive
and non-integer values with warnings, falling back to safe defaults.

### FINDING 4 (POSITIVE): Unknown agent binary fails loudly

Unknown agent binaries raise `RuntimeError` immediately rather than silently
degrading with wrong flags. This follows the fail-secure principle.

### FINDING 5 (POSITIVE): Branch name validation

Branch names are validated against `[^a-zA-Z0-9/_.-]` before use in CLI
commands, preventing argument injection via exotic branch names.

## Test Coverage

- 21 tests validate YAML case-statement flag isolation
- 8 tests validate Python subprocess flag routing, timeouts, and
  unknown-binary rejection
- All 29 tests passing

## Conclusion

No blocking security issues. The implementation follows defense-in-depth:
runtime-enforced restrictions where available (Claude), prompt-level constraints
as fallback (Copilot), and fail-loud behavior for unknown binaries.

## Related

- [Security Recommendations](../reference/security-recommendations.md) — operational security checklist
- [Security Context Preservation](../concepts/security-context-preservation.md) — input validation protections
