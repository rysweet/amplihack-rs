# Security Recommendations

**Type**: Reference (Information-Oriented)

Operational security checklist and recommendations for amplihack deployments.

## Critical Issues

### 1. API Key Exposure (HIGH)

Never hard-code API keys in configuration files. Use environment variables:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # Claude API
export OPENAI_API_KEY="sk-..."          # OpenAI API (if using Copilot)
```

!!! danger "Never Commit Keys"
    If a key appears in a config file or source code, rotate it immediately.

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
supply chain attack. amplihack-rs avoids this class of risk by using direct
API integrations via Rust crates with `cargo audit` verification.

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
