# Dynamic Debugger MCP Testing

`amplihack-rs` ships shell lifecycle helpers for this skill, not the legacy Python test harness. Validate the bundle with:

```bash
bash amplifier-bundle/skills/dynamic-debugger/tests/test_e2e.sh
```

For an end-to-end debugger session, start the DAP MCP server with `scripts/start_dap_mcp.sh`, connect a DAP client, then stop it with `scripts/cleanup_debug.sh`.
