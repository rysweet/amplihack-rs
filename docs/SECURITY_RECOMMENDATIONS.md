# Security Recommendations for Amplihack

## Critical Security Issues

### 1. API Key Exposure (HIGH PRIORITY)

**Issue**: API keys hard-coded or committed in source code or ordinary
configuration files

**Solution**:

```bash
# Use environment variables for provider keys; never commit them
export ANTHROPIC_API_KEY="your_key_here"  # pragma: allowlist secret
export OPENAI_API_KEY="your_key_here"  # pragma: allowlist secret
```

For external LiteLLM routing, obtain a restricted virtual key from a secret
manager and expose it only as `AMPLIHACK_LITELLM_API_KEY` in the launch
environment. Do not put the key in command arguments or project configuration.
Restrict it at the gateway by tenant, route, model alias, budget, and rate.
Client-side model selection is not an authorization boundary.

Launch setup subprocesses, including Docker probes and builds, do not receive
any `AMPLIHACK_LITELLM_*` variable. Amplihack validates the configuration once
and projects translated credentials only onto the final supported agent
command. The narrow Docker transport exception is the trusted final
`docker run` client: it receives only the restricted virtual key so it can
inject that key into the final container. The endpoint and model remain
command arguments, not gateway environment variables.

Amplihack currently permits Claude Code `2.1.247` and sets
`CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` for routed Claude Code processes. It probes
the exact executable before checkout, auto-mode staging, launch setup, session
tracking, or Docker operations and fails closed when the executable is missing,
the probe fails, output is malformed or unknown, or the release is outside the
runtime-attested set.

Routed Copilot CLI similarly requires the exact runtime-attested release
`1.0.83-2`. Amplihack supplies
`--secret-env-vars=COPILOT_PROVIDER_API_KEY`, which keeps the restricted
gateway key in Copilot while removing it from shell and stdio MCP subprocess
environments and redacting it from tool output. Missing, failed, malformed,
ambiguous, or unverified version probes fail closed. Routed launches also
disable Copilot auto-update so the attested executable cannot drift during the
session.

RustyClawd does not provide a verified equivalent, so its complete descendant
process tree remains credential-trusted.

### 2. Tool Calling Configuration

**Current Secure Settings**:

- `ENFORCE_ONE_TOOL_CALL_PER_RESPONSE=true`
- `AMPLIHACK_TOOL_RETRY_ATTEMPTS=3`
- Tool validation enabled

**Recommended Adjustments for Functionality**:

```bash
# Allow multiple tool calls for complex workflows
export ENFORCE_ONE_TOOL_CALL_PER_RESPONSE=false

# Increase retry attempts for reliability
export AMPLIHACK_TOOL_RETRY_ATTEMPTS=5

# Enable tool fallback for robustness
export ENABLE_TOOL_FALLBACK=true
```

### 3. Supply Chain Security

**LiteLLM dependency removal**: The `litellm` dependency was removed due to a
PyPI supply-chain attack (see commit `ead2a7cb0`). Amplihack does not install,
embed, import, start, or manage LiteLLM. Optional external-gateway routing
connects supported agent CLIs to a separately operated service and preserves
that dependency boundary. See
[why the LiteLLM gateway stays external](concepts/external-litellm-boundary.md).

### 4. Enhanced File Logging Security

**Current Security** (Already Excellent):

- Localhost-only binding
- Credential sanitization
- Connection limits
- Proper file permissions

**Additional Recommendations**:

- Add audit logging for tool executions
- Implement rate limiting per IP
- Add request signature validation

## Implementation Priority

1. **IMMEDIATE**: Fix API key exposure
2. **HIGH**: Adjust tool calling limits for functionality
3. **MEDIUM**: Review dependencies for supply chain risks
4. **LOW**: Enhanced audit logging

## Security Compliance Status

- **COMPLIANT**: Log streaming security
- **COMPLIANT**: Tool calling error handling
- **COMPLIANT**: Localhost binding
- **NEEDS FIX**: API key management
- **NEEDS TUNING**: Tool execution limits
