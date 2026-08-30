# Security Recommendations for Amplihack

## Critical Security Issues

### 1. API Key Exposure (HIGH PRIORITY)

**Issue**: Hard-coded API keys in configuration files

**Solution**:

```bash
# Use environment variables only — never hard-code keys in files
export ANTHROPIC_API_KEY="your_key_here"  # pragma: allowlist secret
export OPENAI_API_KEY="your_key_here"  # pragma: allowlist secret
```

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

**litellm dependency**: The Python `litellm` package remains removed due to a
PyPI supply chain attack (see commit `ead2a7cb0`). That prohibition is
unchanged: LiteLLM is not a Python or Rust dependency of amplihack, is not
imported into any amplihack process, and is not installed by any amplihack
command.

Distinguish that from **running LiteLLM as an external service**, which is
permitted and is what the optional gateway feature uses. The distinction is the
trust boundary, not the name:

| In-process dependency (forbidden) | External service (permitted) |
|---|---|
| Executes inside an amplihack process | Runs in its own container, under the operator's control |
| A compromised release runs our code | A compromised release is confined to that container |
| Pulled implicitly by a package resolver | Version-pinned container, started explicitly |
| Shares our address space and secrets | Holds provider keys that amplihack never sees |

The reference deployment in `observability/litellm/` is a container profile you
start yourself; nothing auto-starts. Amplihack's own role is limited to setting
vendor environment variables on the child process, so it is never in the agent's
HTTP data path.

The reference stack binds published ports to `127.0.0.1`, requires Grafana and
LiteLLM authentication, disables gateway message logging, and persists spend
records in PostgreSQL.

Amplihack holds exactly one credential, the gateway virtual key, supplied from
the environment. Provider credentials live in the gateway container only. See
[external LiteLLM gateway architecture](concepts/external-litellm-gateway.md).

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
