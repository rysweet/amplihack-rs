#!/usr/bin/env bash
set -euo pipefail

COPILOT_BIN="${COPILOT_BIN:-copilot}"
EXPECTED_VERSION=1.0.83-2
SENTINEL="copilot-gateway-secret-must-not-reach-tools"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v "$COPILOT_BIN" >/dev/null 2>&1 && [[ ! -x "$COPILOT_BIN" ]]; then
    if [[ "${COPILOT_CONTRACT_OPTIONAL:-0}" == "1" && "${CI:-}" != "true" ]]; then
        echo "SKIP: Copilot CLI is absent and COPILOT_CONTRACT_OPTIONAL=1"
        exit 0
    fi
    echo "FAIL: Copilot CLI is required; set COPILOT_BIN or install @github/copilot@$EXPECTED_VERSION" >&2
    exit 1
fi

grep -Fq "VERIFIED_COPILOT_CLI_VERSIONS: &[&str] = &[\"$EXPECTED_VERSION\"]" \
    "$repo_root/crates/amplihack-utils/src/litellm_proxy.rs" || {
    echo "FAIL: real-CLI contract version and Rust attestation set differ" >&2
    exit 1
}
actual_version="$("$COPILOT_BIN" --version | sed -nE 's/.*([0-9]+\.[0-9]+\.[0-9]+-[0-9]+).*/\1/p' | head -1)"
if [[ "$actual_version" != "$EXPECTED_VERSION" ]]; then
    echo "FAIL: expected GitHub Copilot CLI $EXPECTED_VERSION, got ${actual_version:-unknown}" >&2
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
printf '%s' "${COPILOT_PROVIDER_API_KEY-unset}" >"$CAPTURE_FILE"
EOF
chmod +x "$tmp/capture-env.sh"

cat >"$tmp/mcp-server.cjs" <<'EOF'
const fs = require("fs");
const readline = require("readline");

fs.writeFileSync(
  process.env.MCP_CAPTURE_FILE,
  process.env.COPILOT_PROVIDER_API_KEY ?? "unset",
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
      serverInfo: { name: "copilot-scrub-contract", version: "1.0.0" },
    };
  } else if (request.method === "tools/list") {
    result = { tools: [] };
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`);
});
EOF

cat >"$tmp/gateway.cjs" <<'EOF'
const fs = require("fs");
const http = require("http");

let requests = 0;
const server = http.createServer((request, response) => {
  if (request.method !== "POST") {
    response.writeHead(404).end();
    return;
  }
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => { body += chunk; });
  request.on("end", () => {
    requests += 1;
    const payload = JSON.parse(body);
    const shellTool = (payload.tools ?? []).find((tool) =>
      ["shell", "bash"].includes(tool.function?.name?.toLowerCase())
    );
    if (requests === 1 && !shellTool) {
      fs.writeFileSync(process.env.GATEWAY_ERROR_FILE, "no shell tool in request");
      response.writeHead(500).end();
      return;
    }

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    const send = (data) => response.write(`data: ${JSON.stringify(data)}\n\n`);
    const base = {
      id: `chatcmpl_scrub_${requests}`,
      object: "chat.completion.chunk",
      created: 1,
      model: "gateway-model",
    };
    if (requests === 1) {
      send({
        ...base,
        choices: [{
          index: 0,
          delta: {
            role: "assistant",
            tool_calls: [{
              index: 0,
              id: "call_scrub",
              type: "function",
              function: {
                name: shellTool.function.name,
                arguments: JSON.stringify({
                  command: `${process.env.CAPTURE_SCRIPT}`,
                  description: "Inspect the scrubbed shell environment",
                }),
              },
            }],
          },
          finish_reason: null,
        }],
      });
      send({
        ...base,
        choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }],
      });
    } else {
      send({
        ...base,
        choices: [{
          index: 0,
          delta: { role: "assistant", content: "contract complete" },
          finish_reason: null,
        }],
      });
      send({
        ...base,
        choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
      });
    }
    response.write("data: [DONE]\n\n");
    response.end();
  });
});
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(process.env.PORT_FILE, String(server.address().port));
});
EOF

mkdir "$tmp/home"
PORT_FILE="$tmp/port" GATEWAY_ERROR_FILE="$tmp/gateway-error" \
    CAPTURE_SCRIPT="$tmp/capture-env.sh" \
    node "$tmp/gateway.cjs" >"$tmp/gateway.log" 2>&1 &
server_pid=$!
for _ in {1..100}; do
    [[ -s "$tmp/port" ]] && break
    sleep 0.05
done
[[ -s "$tmp/port" ]] || { echo "FAIL: mock gateway did not start" >&2; exit 1; }

mcp_config="$(printf '{"mcpServers":{"scrub-contract":{"type":"stdio","command":"node","args":["%s"]}}}' "$tmp/mcp-server.cjs")"
if ! (
    cd "$tmp"
    HOME="$tmp/home" \
        CAPTURE_FILE="$tmp/shell-capture" \
        MCP_CAPTURE_FILE="$tmp/mcp-capture" \
        COPILOT_PROVIDER_BASE_URL="http://127.0.0.1:$(<"$tmp/port")/v1" \
        COPILOT_PROVIDER_API_KEY="$SENTINEL" \
        COPILOT_PROVIDER_TYPE="openai" \
        COPILOT_PROVIDER_WIRE_API="completions" \
        COPILOT_MODEL="gateway-model" \
        "$COPILOT_BIN" \
            --allow-all-tools \
            --no-custom-instructions \
            --disable-builtin-mcps \
            --additional-mcp-config "$mcp_config" \
            --secret-env-vars=COPILOT_PROVIDER_API_KEY \
            --no-color \
            --no-remote \
            --no-remote-export \
            --no-auto-update \
            -p "Use the shell tool requested by the model exactly once, then stop."
) >"$tmp/copilot.log" 2>&1; then
    echo "FAIL: GitHub Copilot CLI contract session failed" >&2
    cat "$tmp/copilot.log" >&2
    [[ ! -f "$tmp/gateway-error" ]] || cat "$tmp/gateway-error" >&2
    exit 1
fi

[[ -f "$tmp/shell-capture" ]] || {
    echo "FAIL: GitHub Copilot CLI did not exercise the real shell tool" >&2
    cat "$tmp/copilot.log" >&2
    [[ ! -f "$tmp/gateway-error" ]] || cat "$tmp/gateway-error" >&2
    exit 1
}
[[ "$(<"$tmp/shell-capture")" == "unset" ]] || {
    echo "FAIL: shell subprocess received COPILOT_PROVIDER_API_KEY" >&2
    exit 1
}
if grep -Fq "$SENTINEL" "$tmp/copilot.log"; then
    echo "FAIL: Copilot output leaked COPILOT_PROVIDER_API_KEY" >&2
    exit 1
fi

[[ -f "$tmp/mcp-capture" ]] || {
    echo "FAIL: GitHub Copilot CLI did not start the configured stdio MCP server" >&2
    cat "$tmp/copilot.log" >&2
    exit 1
}
[[ "$(<"$tmp/mcp-capture")" == "unset" ]] || {
    echo "FAIL: stdio MCP subprocess received COPILOT_PROVIDER_API_KEY" >&2
    exit 1
}

echo "PASS: GitHub Copilot CLI $EXPECTED_VERSION scrubs shell and stdio MCP environments and redacts output"
