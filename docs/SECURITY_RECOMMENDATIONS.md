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

External LiteLLM routing also supports the dedicated
`AMPLIHACK_LITELLM_API_KEY_FILE` credential-file variable. The referenced file
must be private to the effective user and satisfy the gateway's
[protected file contract](reference/external-litellm-gateway.md#protected-file-contract).
Do not put the key in `litellm-config.toml` or any project configuration file.

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
