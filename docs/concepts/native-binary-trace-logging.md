# Native Binary Trace Logging

**Complete documentation for the native binary with optional trace logging**

!!! note "Rust Port"
    amplihack-rs is already a native binary built in Rust. The trace logging
    system described here is integrated directly into the Rust binary using
    the `tracing` crate, providing zero-cost abstractions when logging is
    disabled.

## Overview

amplihack uses Anthropic's native Claude binary with optional JSONL trace
logging. This provides excellent performance, zero external dependencies, and
enhanced security.

## Key Improvements

- **No NPM dependency**: Uses native Claude binary directly
- **Optional by default**: Trace logging disabled by default, zero overhead when off
- **High performance**: <0.1ms overhead when disabled, <10ms when enabled
- **Automatic security**: TokenSanitizer removes API keys and secrets automatically
- **Session-scoped logs**: JSONL files organized by session in `.claude/runtime/amplihack-traces/`
- **Direct API integration**: Automatic request/response capture via callbacks

### Performance

| Metric | Native Binary |
|---|---|
| Overhead (disabled) | <0.1ms |
| Overhead (enabled) | <10ms |
| NPM dependency | None |
| Default state | Disabled |
| Security | Automatic |

## Quick Start

### Enable Trace Logging

Trace logging is **disabled by default**. Enable it when needed:

```bash
# Enable for single session
AMPLIHACK_TRACE_LOGGING=true amplihack

# Enable permanently
export AMPLIHACK_TRACE_LOGGING=true
echo 'export AMPLIHACK_TRACE_LOGGING=true' >> ~/.bashrc

# Launch amplihack
amplihack
```

### View Traces

```bash
# List trace files
ls -lh .claude/runtime/amplihack-traces/

# View latest trace
tail -f .claude/runtime/amplihack-traces/trace_*.jsonl | jq .

# Analyze token usage
cat .claude/runtime/amplihack-traces/*.jsonl | \
  jq -s '[.[] | select(.response.usage != null) | .response.usage] |
         {calls: length, total_tokens: map(.total_tokens) | add}'
```

## Documentation Map

### For Users

1. **Feature Overview**
   - What is trace logging and why use it
   - Common scenarios and use cases
   - Quick start guide
   - Security considerations

2. **How-To Guide**
   - Enable/disable trace logging
   - Analyze traces with jq
   - Monitor token usage
   - Archive traces for compliance
   - Export to CSV

### For Developers

3. **Developer Reference**
   - Architecture overview
   - Module reference (TraceLogger, TokenSanitizer, etc.)
   - Trace entry schema
   - Performance characteristics
   - Extension points

### For Troubleshooting

4. **Troubleshooting Guide**
   - No trace files created
   - Malformed JSONL entries
   - Sensitive data in traces
   - High disk usage
   - Performance degradation
   - Permission errors

## Common Use Cases

### Debug Unexpected Responses

```bash
# Enable trace logging
export AMPLIHACK_TRACE_LOGGING=true

# Run session that shows unexpected behavior
amplihack
# ... reproduce issue ...
exit

# Analyze the conversation
cat .claude/runtime/amplihack-traces/trace_*.jsonl | \
  jq 'select(.event=="request" or .event=="response")'
```

### Monitor Token Usage

```bash
cat .claude/runtime/amplihack-traces/*.jsonl | \
  jq -s '[.[] | select(.response.usage != null) | .response.usage] |
         {total_calls: length,
          total_prompt: map(.prompt_tokens) | add,
          total_completion: map(.completion_tokens) | add,
          total_tokens: map(.total_tokens) | add}'
```

### Compliance Audit Trail

```bash
# Enable permanent trace logging
export AMPLIHACK_TRACE_LOGGING=true

# Archive daily
mkdir -p ~/amplihack-audit/$(date +%Y%m%d)
cp .claude/runtime/amplihack-traces/*.jsonl ~/amplihack-audit/$(date +%Y%m%d)/

# Generate compliance report
cat ~/amplihack-audit/*/*.jsonl | \
  jq -r '[.timestamp, .session_id, .event] | @csv' > audit-report.csv
```

## Architecture

### Component Diagram

```
amplihack Session
       │
       ▼
API Callback Manager
       │
       ▼
TraceLogger
  (checks AMPLIHACK_TRACE_LOGGING)
       │
       ▼
TokenSanitizer
  (removes API keys, secrets)
       │
       ▼
JSONL Writer
  (.claude/runtime/amplihack-traces/)
```

### File Structure

```
.claude/runtime/amplihack-traces/
├── trace_20260122_143022_a3f9d8.jsonl
├── trace_20260122_151345_b4e2f1.jsonl
└── trace_20260123_090015_c5f3a2.jsonl

File naming: trace_YYYYMMDD_HHMMSS_SESSION.jsonl
```

### Trace Entry Format

Each trace file contains JSONL (newline-delimited JSON):

```jsonl
{"timestamp":"2026-01-22T14:30:22.451Z","session_id":"a3f9d8","event":"request","request":{...}}
{"timestamp":"2026-01-22T14:30:23.102Z","session_id":"a3f9d8","event":"response","response":{...}}
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AMPLIHACK_TRACE_LOGGING` | `false` | Enable/disable trace logging |
| `AMPLIHACK_TRACE_DIR` | `.claude/runtime/amplihack-traces/` | Trace output directory |
| `AMPLIHACK_TRACE_MAX_SIZE` | `100MB` | Maximum trace file size |
| `AMPLIHACK_TRACE_SANITIZE` | `true` | Auto-sanitize sensitive tokens |

## Security

### Automatic Token Sanitization

The TokenSanitizer automatically removes sensitive data from traces:

| Token Type | Pattern | Replacement |
|---|---|---|
| GitHub tokens | `gho_*`, `ghp_*` | `[REDACTED-GITHUB-TOKEN]` |
| OpenAI keys | `sk-*` | `[REDACTED-OPENAI-KEY]` |
| Anthropic keys | `sk-ant-*` | `[REDACTED-ANTHROPIC-KEY]` |
| Bearer tokens | `Bearer <token>` | `[REDACTED-BEARER-TOKEN]` |
| JWT tokens | `eyJ*.eyJ*.*` | `[REDACTED-JWT-TOKEN]` |

See [Security Recommendations](../reference/security-recommendations.md) for
full security guidance.

## Related Documentation

- [JSON Logging](../reference/json-logging.md) — structured auto-mode event logging
- [Benchmarking](../reference/benchmarking.md) — performance measurement with eval-recipes
- [Security Recommendations](../reference/security-recommendations.md) — security best practices
