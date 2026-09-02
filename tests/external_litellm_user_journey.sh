#!/usr/bin/env bash
set -euo pipefail

AMPLIHACK_BIN="${AMPLIHACK_BIN:-target/debug/amplihack}"
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
EXPECTED_CLAUDE_VERSION=2.1.247
GATEWAY_KEY="gateway-user-journey-secret"

[[ -x "$AMPLIHACK_BIN" ]] || {
    echo "FAIL: build amplihack or set AMPLIHACK_BIN to an executable" >&2
    exit 1
}
[[ -x "$CLAUDE_BIN" ]] || {
    echo "FAIL: set CLAUDE_BIN to Claude Code $EXPECTED_CLAUDE_VERSION" >&2
    exit 1
}
actual_version="$("$CLAUDE_BIN" --version | sed -nE 's/.*([0-9]+\.[0-9]+\.[0-9]+).*/\1/p' | head -1)"
[[ "$actual_version" == "$EXPECTED_CLAUDE_VERSION" ]] || {
    echo "FAIL: expected Claude Code $EXPECTED_CLAUDE_VERSION, got ${actual_version:-unknown}" >&2
    exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
amplihack_bin="$(cd "$(dirname "$AMPLIHACK_BIN")" && pwd)/$(basename "$AMPLIHACK_BIN")"
claude_dir="$(cd "$(dirname "$CLAUDE_BIN")" && pwd)"
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

cat >"$tmp/gateway.cjs" <<'EOF'
const fs = require("fs");
const http = require("http");

const server = http.createServer((request, response) => {
  if (request.method !== "POST") {
    response.writeHead(404).end();
    return;
  }
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => { body += chunk; });
  request.on("end", () => {
    fs.writeFileSync(process.env.REQUEST_PATH_FILE, request.url);
    fs.writeFileSync(
      process.env.AUTH_FILE,
      request.headers.authorization ?? request.headers["x-api-key"] ?? "unset",
    );
    fs.writeFileSync(process.env.BODY_FILE, body);
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    const events = [
      ["message_start", {
        type: "message_start",
        message: {
          id: "msg_user_journey",
          type: "message",
          role: "assistant",
          model: "gateway-model",
          content: [],
          stop_reason: null,
          stop_sequence: null,
          usage: { input_tokens: 1, output_tokens: 1 },
        },
      }],
      ["content_block_start", {
        type: "content_block_start",
        index: 0,
        content_block: { type: "text", text: "" },
      }],
      ["content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: "gateway user journey complete" },
      }],
      ["content_block_stop", { type: "content_block_stop", index: 0 }],
      ["message_delta", {
        type: "message_delta",
        delta: { stop_reason: "end_turn", stop_sequence: null },
        usage: { output_tokens: 1 },
      }],
      ["message_stop", { type: "message_stop" }],
    ];
    for (const [event, data] of events) {
      response.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
    }
    response.end();
  });
});
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(process.env.PORT_FILE, String(server.address().port));
});
EOF

mkdir "$tmp/home"
PORT_FILE="$tmp/port" \
    REQUEST_PATH_FILE="$tmp/request-path" \
    AUTH_FILE="$tmp/auth" \
    BODY_FILE="$tmp/body" \
    node "$tmp/gateway.cjs" >"$tmp/gateway.log" 2>&1 &
server_pid=$!
for _ in {1..100}; do
    [[ -s "$tmp/port" ]] && break
    sleep 0.05
done
[[ -s "$tmp/port" ]] || {
    echo "FAIL: mock gateway did not start" >&2
    exit 1
}

if ! (
    cd "$tmp"
    HOME="$tmp/home" \
        PATH="$claude_dir:$PATH" \
        AMPLIHACK_HOME="$repo_root" \
        AMPLIHACK_NONINTERACTIVE=1 \
        AMPLIHACK_SKIP_MMDC=1 \
        AMPLIHACK_LITELLM_ENDPOINT="http://127.0.0.1:$(<"$tmp/port")" \
        AMPLIHACK_LITELLM_API_KEY="$GATEWAY_KEY" \
        AMPLIHACK_LITELLM_MODEL="gateway-model" \
        "$amplihack_bin" claude --subprocess-safe --no-reflection -- \
            -p "Reply with the gateway response." \
            --no-session-persistence \
            --output-format stream-json \
            --verbose
) >"$tmp/session.log" 2>&1; then
    echo "FAIL: amplihack-to-Claude gateway session failed" >&2
    cat "$tmp/session.log" >&2
    exit 1
fi

grep -Fq "gateway user journey complete" "$tmp/session.log"
[[ "$(<"$tmp/request-path")" == "/v1/messages?beta=true" ]] || {
    echo "FAIL: Claude used unexpected gateway path: $(<"$tmp/request-path")" >&2
    exit 1
}
grep -Fq "$GATEWAY_KEY" "$tmp/auth"
node -e '
const body = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
if (body.model !== "gateway-model") process.exit(1);
' "$tmp/body"
if grep -Fq "$GATEWAY_KEY" "$tmp/session.log"; then
    echo "FAIL: gateway key leaked into user-visible session output" >&2
    exit 1
fi

echo "PASS: amplihack routed a real Claude Code session through the configured gateway"
