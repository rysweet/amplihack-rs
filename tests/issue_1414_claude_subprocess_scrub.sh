#!/usr/bin/env bash
set -euo pipefail

CLAUDE_BIN="${CLAUDE_BIN:-claude}"
EXPECTED_VERSION=2.1.247
NODE_BIN="$(command -v node)"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
grep -Fq "VERIFIED_CLAUDE_CODE_VERSIONS: &[&str] = &[\"$EXPECTED_VERSION\"]" \
    "$repo_root/crates/amplihack-utils/src/litellm_proxy.rs" || {
    echo "FAIL: real-CLI contract version and Rust attestation set differ" >&2
    exit 1
}
actual_version="$("$CLAUDE_BIN" --version | sed -nE 's/.*([0-9]+\.[0-9]+\.[0-9]+).*/\1/p' | head -1)"
if [[ "$actual_version" != "$EXPECTED_VERSION" ]]; then
    echo "FAIL: expected Claude Code $EXPECTED_VERSION, got ${actual_version:-unknown}" >&2
    exit 1
fi

tmp="$(mktemp -d)"
server_pid=
cleanup() {
    if [[ -n "$server_pid" ]]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -rf "$tmp"
}
trap cleanup EXIT

cat >"$tmp/capture-env.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s' "${ANTHROPIC_AUTH_TOKEN-unset}" >"$CAPTURE_DIR/$1"
EOF
chmod +x "$tmp/capture-env.sh"

cat >"$tmp/mcp-server.cjs" <<'EOF'
const fs = require("fs");
const readline = require("readline");

fs.writeFileSync(
  `${process.env.CAPTURE_DIR}/mcp`,
  process.env.ANTHROPIC_AUTH_TOKEN ?? "unset",
);
const lines = readline.createInterface({ input: process.stdin });
lines.on("line", (line) => {
  const request = JSON.parse(line);
  if (!Object.hasOwn(request, "id")) return;
  let result = {};
  if (request.method === "initialize") {
    result = {
      protocolVersion: request.params.protocolVersion,
      capabilities: {},
      serverInfo: { name: "scrub-contract", version: "1.0.0" },
    };
  } else if (request.method === "tools/list") {
    result = { tools: [] };
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`);
});
EOF

cat >"$tmp/gateway.js" <<'EOF'
const http = require("http");

let requests = 0;
const server = http.createServer((request, response) => {
  if (request.method !== "POST") {
    response.writeHead(404).end();
    return;
  }
  request.resume();
  request.on("end", () => {
    requests += 1;
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    const events = [[
      "message_start",
      {
        type: "message_start",
        message: {
          id: `msg_scrub_${requests}`,
          type: "message",
          role: "assistant",
          model: "gateway-model",
          content: [],
          stop_reason: null,
          stop_sequence: null,
          usage: { input_tokens: 1, output_tokens: 1 },
        },
      },
    ]];
    if (requests === 1) {
      events.push(["content_block_start", {
        type: "content_block_start",
        index: 0,
        content_block: { type: "tool_use", id: "tool_scrub", name: "Bash", input: {} },
      }]);
      events.push(["content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: {
          type: "input_json_delta",
          partial_json: JSON.stringify({
            command: `${process.env.CAPTURE_SCRIPT} bash`,
            description: "Record scrubbed Bash environment",
          }),
        },
      }]);
      events.push(["content_block_stop", { type: "content_block_stop", index: 0 }]);
      events.push(["message_delta", {
        type: "message_delta",
        delta: { stop_reason: "tool_use", stop_sequence: null },
        usage: { output_tokens: 1 },
      }]);
    } else {
      events.push(["content_block_start", {
        type: "content_block_start",
        index: 0,
        content_block: { type: "text", text: "" },
      }]);
      events.push(["content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: "done" },
      }]);
      events.push(["content_block_stop", { type: "content_block_stop", index: 0 }]);
      events.push(["message_delta", {
        type: "message_delta",
        delta: { stop_reason: "end_turn", stop_sequence: null },
        usage: { output_tokens: 1 },
      }]);
    }
    events.push(["message_stop", { type: "message_stop" }]);
    for (const [event, data] of events) {
      response.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
    }
    response.end();
  });
});
server.listen(0, "127.0.0.1", () => {
  require("fs").writeFileSync(process.env.PORT_FILE, String(server.address().port));
});
EOF

cat >"$tmp/settings.json" <<EOF
{
  "permissions": { "allow": ["Bash"] },
  "hooks": {
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "$tmp/capture-env.sh hook"
      }]
    }]
  }
}
EOF
cat >"$tmp/mcp.json" <<EOF
{
  "mcpServers": {
    "scrub-contract": {
      "type": "stdio",
      "command": "$NODE_BIN",
      "args": ["$tmp/mcp-server.cjs"]
    }
  }
}
EOF
mkdir "$tmp/home"

PORT_FILE="$tmp/port" CAPTURE_SCRIPT="$tmp/capture-env.sh" \
    node "$tmp/gateway.js" >"$tmp/gateway.log" 2>&1 &
server_pid=$!
for _ in {1..100}; do
    [[ -s "$tmp/port" ]] && break
    sleep 0.05
done
[[ -s "$tmp/port" ]] || { echo "FAIL: mock gateway did not start" >&2; exit 1; }

if ! (
    cd "$tmp"
    CAPTURE_DIR="$tmp" \
        HOME="$tmp/home" \
        ANTHROPIC_BASE_URL="http://127.0.0.1:$(<"$tmp/port")" \
        ANTHROPIC_AUTH_TOKEN="gateway-secret" \
        ANTHROPIC_MODEL="gateway-model" \
        CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1 \
        "$CLAUDE_BIN" -p \
            --allowedTools "Bash" \
            --no-session-persistence \
            --output-format stream-json \
            --verbose \
            --debug-file "$tmp/debug.log" \
            --settings "$tmp/settings.json" \
            --mcp-config "$tmp/mcp.json" \
            --strict-mcp-config \
            "Use the Bash tool exactly as requested, then stop."
) >"$tmp/claude.log" 2>&1; then
    echo "FAIL: Claude Code contract session failed" >&2
    cat "$tmp/claude.log" >&2
    cat "$tmp/debug.log" >&2
    exit 1
fi

for surface in bash hook mcp; do
    [[ -f "$tmp/$surface" ]] || {
        echo "FAIL: Claude Code did not exercise the $surface subprocess surface" >&2
        cat "$tmp/claude.log" >&2
        cat "$tmp/debug.log" >&2
        exit 1
    }
    value="$(<"$tmp/$surface")"
    [[ "$value" == "unset" ]] || {
        echo "FAIL: $surface subprocess received ANTHROPIC_AUTH_TOKEN" >&2
        exit 1
    }
done

echo "PASS: Claude Code $EXPECTED_VERSION scrubs Bash, hook, and stdio MCP environments"
