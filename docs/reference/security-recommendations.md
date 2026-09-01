# Security Recommendations

**Type**: Reference (Information-Oriented)

Operational security checklist and recommendations for amplihack deployments.

## Critical Issues

### 1. API Key Exposure (HIGH)

Never hard-code or commit API keys in source code or ordinary configuration
files. Use environment variables for provider keys:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # Claude API
export OPENAI_API_KEY="sk-..."          # OpenAI API (if using Copilot)
```

!!! danger "Never Commit Keys"
    If a key appears in a config file or source code, rotate it immediately.

For external LiteLLM routing, obtain a restricted virtual key from a secret
manager and expose it only as `AMPLIHACK_LITELLM_API_KEY` in the launch
environment. Do not put the key in command arguments or project configuration.
Restrict it in LiteLLM by tenant, route, model alias, budget, and rate; the
client-selected model is not an authorization control.

Launch setup subprocesses do not receive any `AMPLIHACK_LITELLM_*` variable.
Amplihack validates the configuration once and projects translated credentials
only onto the final supported agent command.

Amplihack currently permits Claude Code `2.1.247` and sets
`CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`. That exact release is pinned because the
scrub control is not an upstream documented compatibility contract. A new
Claude Code release remains blocked until the Bash, hook, and stdio MCP
subprocess isolation test passes for it and the verified-version set is
updated. The selected executable is validated before checkout, auto-mode
staging, memory configuration, session tracking, Docker operations, or child
creation. Missing executables, failed probes, malformed or unknown output, and
all unverified versions fail closed. On Linux, Claude's scrub enforcement also
requires `bubblewrap` and `socat`; Claude refuses to start if either dependency
is unavailable.
RustyClawd has no verified subprocess-scrubbing capability, so treat its
complete descendant process tree as credential-trusted.

### 2. Tool Calling Configuration

Default secure settings:

| Setting                              | Default | Purpose                     |
| ------------------------------------ | ------- | --------------------------- |
| `ENFORCE_ONE_TOOL_CALL_PER_RESPONSE` | `true`  | Limit concurrent tool calls |
| `AMPLIHACK_TOOL_RETRY_ATTEMPTS`      | `3`     | Retry limit                 |

For complex workflows requiring multiple parallel tool calls:

```bash
export ENFORCE_ONE_TOOL_CALL_PER_RESPONSE=false
export AMPLIHACK_TOOL_RETRY_ATTEMPTS=5
export ENABLE_TOOL_FALLBACK=true
```

### 3. Supply Chain Security

The `litellm` dependency was removed from upstream amplihack due to a PyPI
supply-chain attack. Amplihack does not install, embed, import, start, or
manage LiteLLM. Optional
[external-gateway routing](../concepts/external-litellm-boundary.md) connects
supported agent CLIs to a separately operated service without restoring the
dependency.

That prohibition covers LiteLLM as an **in-process dependency**. An
operator-managed LiteLLM deployment is a different trust boundary and may be
used by the optional gateway feature; amplihack never installs, starts, or
manages it. See
[why the gateway stays external](../concepts/external-litellm-boundary.md) and
the [supply-chain section](../SECURITY_RECOMMENDATIONS.md) for the distinction.

Run supply chain checks:

```bash
cargo audit          # Check for known vulnerabilities
cargo deny check     # License and advisory checks
```

### 4. File Logging Security

The logging subsystem enforces:

- Localhost-only binding (no remote access)
- Credential sanitization in log output
- Connection limits
- Proper file permissions (`0600` for logs containing session data)

## Implementation Priority

| Priority      | Action                                         |
| ------------- | ---------------------------------------------- |
| **Immediate** | Ensure no API keys in source or config files   |
| **High**      | Review tool calling limits for your workflow    |
| **Medium**    | Run `cargo audit` in CI                        |
| **Low**       | Add audit logging for tool executions          |

## Compliance Status

| Area                        | Status    |
| --------------------------- | --------- |
| Log streaming security      | Compliant |
| Tool calling error handling | Compliant |
| Localhost binding           | Compliant |
| API key management          | Review    |
| Tool execution limits       | Tunable   |

## Related

- [Security Context Preservation](../concepts/security-context-preservation.md) — ReDoS and input validation protections
- [Security Audit: Copilot CLI Flags](../reference/security-audit-copilot-cli-flags.md) — flag isolation review
- [Environment Variables](../reference/environment-variables.md) — all configurable env vars
