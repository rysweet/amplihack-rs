#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

bash -n observability/litellm/bootstrap-key.sh
grep -Fq 'LITELLM_SALT_KEY: ${LITELLM_SALT_KEY:?set permanent LITELLM_SALT_KEY in .env}' \
  observability/litellm/docker-compose.yml
grep -Fq 'LITELLM_SALT_KEY=' observability/litellm/.env.example

if grep -R -E 'LiteLlmClient|AMPLIHACK_LITELLM_(QUEUE|RATE|INPUT|OUTPUT|COST)' \
  crates docs observability --exclude='issue_1413_external_litellm_gateway.sh'; then
  echo "obsolete embedded LiteLLM references remain" >&2
  exit 1
fi

image_count="$(grep -cE '^[[:space:]]+image:' observability/litellm/docker-compose.yml)"
pinned_count="$(
  grep -cE '^[[:space:]]+image: [^ ]+:[^ ]+@sha256:[0-9a-f]{64}$' \
    observability/litellm/docker-compose.yml
)"
test "$image_count" -eq 5
test "$pinned_count" -eq "$image_count"
if grep -Eq 'image: .*:latest' observability/litellm/docker-compose.yml; then
  echo "floating latest image tag is forbidden" >&2
  exit 1
fi
if grep -E '^[[:space:]]+- "[^"]+:[0-9]+:[0-9]+"' \
  observability/litellm/docker-compose.yml |
  grep -Ev '^[[:space:]]+- "127\.0\.0\.1:'; then
  echo "published service port is not bound to loopback" >&2
  exit 1
fi
grep -Fq 'AMPLIHACK_KEY_MAX_BUDGET: ${AMPLIHACK_KEY_MAX_BUDGET:-}' \
  observability/litellm/docker-compose.yml
grep -Fq 'AMPLIHACK_KEY_BUDGET_DURATION: ${AMPLIHACK_KEY_BUDGET_DURATION:-}' \
  observability/litellm/docker-compose.yml
grep -Fq 'AMPLIHACK_KEY_REQUESTS_PER_MINUTE: ${AMPLIHACK_KEY_REQUESTS_PER_MINUTE:-}' \
  observability/litellm/docker-compose.yml
grep -Fq 'payload = {' observability/litellm/bootstrap-key.sh
if grep -Fq 'payload = json.dumps({' observability/litellm/bootstrap-key.sh; then
  echo "virtual-key request payload must remain an object until serialization" >&2
  exit 1
fi
grep -Fq 'data=json.dumps(payload).encode()' observability/litellm/bootstrap-key.sh
test "$(grep -cE '^[[:space:]]+- otel$' observability/litellm/config.yaml)" -eq 2
grep -Fq 'turn_off_message_logging: true' observability/litellm/config.yaml
grep -Fq 'litellm_proxy_total_requests_metric_total[5m]' \
  observability/litellm/grafana/dashboards/litellm.json
grep -Fq 'litellm_proxy_failed_requests_metric_total[5m]' \
  observability/litellm/grafana/dashboards/litellm.json
for collector in \
  observability/litellm/otel-collector.yaml \
  observability/litellm/otel-collector.external.yaml
do
  COLLECTOR="$collector" python3 - <<'PY'
import os
from pathlib import Path

text = Path(os.environ["COLLECTOR"]).read_text()
for signal in ("traces", "metrics"):
    marker = f"    {signal}:\n"
    start = text.index(marker)
    lines = text[start:].splitlines()
    block_lines = [lines[0]]
    for line in lines[1:]:
        if line.startswith("    ") and not line.startswith("      "):
            break
        block_lines.append(line)
    block = "\n".join(block_lines)
    assert "processors: [attributes/redact, batch]" in block, (
        f"{os.environ['COLLECTOR']} {signal} pipeline must redact before export"
    )
PY
done

if [[ -n "${CI:-}" ]]; then
  echo "PASS: offline external LiteLLM deployment contract checks passed; CI does not launch LiteLLM or real clients"
  exit 0
fi

if ! docker info >/dev/null 2>&1; then
  if [[ "${LITELLM_SMOKE_OPTIONAL:-0}" == "1" ]]; then
    echo "Docker unavailable; static deployment checks passed and LITELLM_SMOKE_OPTIONAL=1 permits a local skip."
    exit 0
  fi
  echo "Docker is unavailable; set LITELLM_SMOKE_OPTIONAL=1 only for an explicit local skip." >&2
  exit 1
fi

litellm_image="$(
  sed -n '/^[[:space:]]*litellm:/,/^[[:space:]]*[a-z].*:/ {
    s/^[[:space:]]*image: //p
  }' observability/litellm/docker-compose.yml
)"
container="amplihack-litellm-ci-$$"
export LITELLM_MASTER_KEY=sk-amplihack-ci
docker run --rm --detach \
  --name "$container" \
  --publish 127.0.0.1::4000 \
  --env LITELLM_MASTER_KEY \
  --volume "$repo_root/observability/litellm/config.ci.yaml:/app/config.ci.yaml:ro" \
  "$litellm_image" \
  --config /app/config.ci.yaml --port 4000 >/dev/null
trap 'docker rm --force "$container" >/dev/null 2>&1 || true' EXIT
port="$(docker port "$container" 4000/tcp | sed -n 's/.*://p')"
for _ in $(seq 1 60); do
  if curl --fail --silent "http://127.0.0.1:$port/health/readiness" >/dev/null; then
    break
  fi
  sleep 1
done
response="$(
  curl --fail --silent \
    -H "Authorization: Bearer $LITELLM_MASTER_KEY" \
    -H 'Content-Type: application/json' \
    --data '{"model":"amplihack-ci","messages":[{"role":"user","content":"test"}]}' \
    "http://127.0.0.1:$port/v1/chat/completions"
)"
grep -Fq 'gateway ok' <<<"$response"
if curl --fail --silent \
  -H "Authorization: Bearer $LITELLM_MASTER_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"model":"amplihack-ci-failure","messages":[{"role":"user","content":"test"}]}' \
  "http://127.0.0.1:$port/v1/chat/completions" >/dev/null; then
  echo "unreachable upstream unexpectedly produced a successful gateway response" >&2
  exit 1
fi
metrics=""
for _ in $(seq 1 30); do
  metrics="$(
    curl --fail --silent \
      -H "Authorization: Bearer ${LITELLM_MASTER_KEY}" \
      "http://127.0.0.1:$port/metrics/"
  )"
  if grep -q '^# HELP litellm_proxy_total_requests_metric_total ' <<<"$metrics" &&
    grep -q '^# HELP litellm_proxy_failed_requests_metric_total ' <<<"$metrics"; then
    break
  fi
  sleep 1
done
grep -q '^# HELP litellm_proxy_total_requests_metric_total ' <<<"$metrics"
grep -q '^# HELP litellm_proxy_failed_requests_metric_total ' <<<"$metrics"
