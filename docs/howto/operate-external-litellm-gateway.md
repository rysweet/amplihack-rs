---
title: Operate an External LiteLLM Gateway
description: Configure controls, telemetry, key rotation, Docker routing, troubleshooting, and shutdown.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: howto
---

# Operate an External LiteLLM Gateway

Use these procedures after completing the
[LiteLLM gateway quickstart](../tutorials/litellm-gateway-quickstart.md).

## Configure budgets and rate limits

The reference deployment creates keys without a budget or rate limit unless
controls are explicitly configured. Set controls before creating or rotating a
key:

```bash
export AMPLIHACK_KEY_MAX_BUDGET=25
export AMPLIHACK_KEY_BUDGET_DURATION=30d
export AMPLIHACK_KEY_REQUESTS_PER_MINUTE=60
observability/litellm/bootstrap-key.sh
```

The controls belong to the LiteLLM virtual key and apply across launches and
machines that use it. Unset all three variables to create a key without those
controls. A budget amount and duration must be configured together.

Do not report missing limits or missing provider cost data as zero. In the
LiteLLM UI, an absent limit means disabled and missing cost means unknown.

## Rotate an agent key

Run the same bootstrap command used for initial creation:

```bash
observability/litellm/bootstrap-key.sh
```

Rotation stores the replacement key before revoking the previous key. Reload
the shell configuration:

```bash
export AMPLIHACK_LITELLM_API_KEY="$(
  cat observability/litellm/.amplihack-api-key
)"
```

If old-key revocation cannot be confirmed, the command exits unsuccessfully and
keeps `observability/litellm/.amplihack-api-key.previous`. Preserve both files
and rerun `bootstrap-key.sh`; it retries revocation before creating another key.

## Export privacy-safe telemetry

The default stack keeps telemetry local. To export infrastructure telemetry,
set the external collector endpoint and authorization value in
`observability/litellm/.env`, then apply both Compose files:

```dotenv
AMPLIHACK_EXTERNAL_OTLP_ENDPOINT=https://otel.example.net
AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION=******
```

```bash
docker compose \
  -f observability/litellm/docker-compose.yml \
  -f observability/litellm/docker-compose.external-otel.yml \
  up -d
```

The collector removes prompt, completion, message, and authorization
attributes. The supplied LiteLLM configuration also disables prompt and
completion logging. Do not change those filters without reviewing the data
classification and retention policy of the destination.

## Use a remote gateway

Remote gateways require HTTPS:

```bash
export AMPLIHACK_LITELLM_ENDPOINT=https://gateway.example.net
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL=amplihack-default
amplihack copilot
```

The endpoint must not contain user information, a query string, or a fragment.
Terminate TLS with a certificate trusted by the child CLI.

## Launch an agent in Docker

Use an HTTPS endpoint reachable from the container:

```bash
export AMPLIHACK_LITELLM_ENDPOINT=https://gateway.example.net
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL=amplihack-default
amplihack copilot --docker
```

The Docker launcher forwards the API key by environment-variable name; the
value does not appear in `docker run` arguments. The image must carry the
LiteLLM-routing capability label and a compatible amplihack version. Rebuild an
unlabeled or stale image rather than bypassing the check.

Host-loopback endpoints are rejected for Docker launches. Do not work around
that restriction with host networking. Use a container-reachable HTTPS
gateway, or run the agent on the host for the local reference stack.

## Update reference images

Every reference image is pinned by version and registry digest. To update one:

1. Choose a released version.
2. Resolve its registry digest.
3. Update both the tag and digest in
   `observability/litellm/docker-compose.yml`.
4. Start the stack and confirm the non-billable readiness endpoint.
5. Create a restricted key and verify one request through each launcher.

Never replace a digest with a floating tag such as `latest`.

## Troubleshoot routing

| Symptom | Resolution |
| --- | --- |
| Routing is unexpectedly disabled | Set all three required variables in the same shell. |
| Configuration is rejected | Remove surrounding whitespace and control characters. Use HTTPS, or literal `127.0.0.0/8` or `::1` loopback for host development. |
| Endpoint is rejected despite using `localhost` | Use a literal loopback address. `http://localhost` is intentionally rejected. |
| Model option is rejected | Remove the option or make every explicit primary and fallback model exactly match `AMPLIHACK_LITELLM_MODEL`. |
| A session option is rejected | Start a new gateway-routed session; existing sessions may retain a different provider path. |
| Copilot remote option is rejected | Remove remote, export, connect, or share options. Those paths are unavailable in gateway mode. |
| Claude settings option is rejected | Remove `--settings` or `--setting-sources`. Gateway mode disables mutable settings sources. |
| Codex or Amplifier is rejected | Disable the gateway variables or use Claude Code, Copilot CLI, or rustyclawd. |
| Docker rejects a loopback endpoint | Use a container-reachable HTTPS gateway or launch on the host. |
| Docker rejects the image | Rebuild with the current amplihack image definition. |
| Gateway cannot be reached | Check the child CLI error, gateway readiness, DNS, TLS trust, and firewall. Amplihack does not retry or fall back. |
| Usage appears but cost is missing | Configure provider/model cost in LiteLLM. Treat the value as unknown until LiteLLM records it. |
| Agent usage is absent from Grafana | Use the LiteLLM UI. Grafana covers infrastructure telemetry, not authoritative request accounting. |

## Shut down and remove data

Stop services while retaining PostgreSQL and dashboard state:

```bash
docker compose -f observability/litellm/docker-compose.yml down
```

Delete persisted state only as an intentional destructive operation:

```bash
docker compose -f observability/litellm/docker-compose.yml down --volumes
```

Remove local key files after revoking their keys in LiteLLM. Unset the three
`AMPLIHACK_LITELLM_*` variables to restore ordinary launcher behavior.

See the [external LiteLLM gateway reference](../reference/external-litellm-gateway.md)
for the complete configuration and routing contract.
