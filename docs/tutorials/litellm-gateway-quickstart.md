---
title: LiteLLM Gateway Quickstart
description: Start the reference gateway and route Claude Code, Copilot CLI, and rustyclawd through it.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: tutorial
---

# LiteLLM Gateway Quickstart

This tutorial starts the local reference stack, creates a model-restricted
virtual key, and routes each supported amplihack launcher through one model
alias.

## Prerequisites

- A source checkout of `amplihack-rs`
- Docker with Compose v2
- `curl` and `openssl`
- A provider API key for the upstream model
- `amplihack` installed from the checkout

The local gateway is for host-launched agents. Docker-launched agents need an
HTTPS gateway reachable from inside the container.

## 1. Configure the stack

From the repository root:

```bash
cp observability/litellm/.env.example observability/litellm/.env
chmod 600 observability/litellm/.env
```

Set independently generated values for `LITELLM_MASTER_KEY`,
`POSTGRES_PASSWORD`, and `GF_SECURITY_ADMIN_PASSWORD`. Prefix the LiteLLM
master key with `sk-`:

```bash
printf 'sk-%s\n' "$(openssl rand -hex 32)"
openssl rand -hex 32
openssl rand -hex 32
```

Select an upstream model and set its provider credential in `.env`. For an
Anthropic deployment:

```dotenv
LITELLM_UPSTREAM_MODEL=anthropic/claude-sonnet-4-5-20250929
ANTHROPIC_API_KEY=your-provider-key
```

The populated `.env` is ignored by Git. Never commit it or pass the LiteLLM
master key to an agent.

## 2. Start the gateway

```bash
docker compose -f observability/litellm/docker-compose.yml up -d
until curl --fail --silent \
  http://127.0.0.1:4000/health/readiness >/dev/null; do
  sleep 2
done
```

The readiness endpoint does not invoke a model and cannot create billable
provider traffic.

## 3. Create an agent key

```bash
observability/litellm/bootstrap-key.sh
```

The script creates a virtual key restricted to the `amplihack-default` model
alias and writes it to `observability/litellm/.amplihack-api-key` with
owner-only permissions. Budget and rate-limit controls are absent by default.

## 4. Enable routing

```bash
export AMPLIHACK_LITELLM_ENDPOINT=http://127.0.0.1:4000
export AMPLIHACK_LITELLM_API_KEY="$(
  cat observability/litellm/.amplihack-api-key
)"
export AMPLIHACK_LITELLM_MODEL=amplihack-default
```

All three variables form one configuration unit. A missing, empty, or invalid
value stops the launch before a child process starts.

## 5. Launch the supported agents

Launch Claude Code:

```bash
amplihack launch
```

`amplihack claude` is an equivalent alias and uses the same gateway routing.

Launch GitHub Copilot CLI:

```bash
amplihack copilot
```

Launch rustyclawd:

```bash
amplihack rustyclawd
```

Each child uses the `amplihack-default` alias. Existing terminal streaming,
Ctrl-C handling, and exit-code behavior remain unchanged. A gateway or provider
failure is reported by the child CLI and does not fall back to a direct
provider.

## 6. Confirm accounting

Open the LiteLLM UI at `http://127.0.0.1:4000/ui` and sign in with the
administrative credential from `.env`. Run a short request from one launcher,
then confirm that the virtual key, model alias, token usage, and spend record
appear in LiteLLM.

Grafana at `http://127.0.0.1:3000` shows infrastructure telemetry. It is not the
source of truth for agent usage or cost.

## 7. Stop the stack

```bash
unset AMPLIHACK_LITELLM_ENDPOINT
unset AMPLIHACK_LITELLM_API_KEY
unset AMPLIHACK_LITELLM_MODEL
docker compose -f observability/litellm/docker-compose.yml down
```

PostgreSQL and dashboard volumes remain available for the next start. Add
`--volumes` only when you intentionally want to delete persisted gateway data.

For production controls, key rotation, Docker use, and troubleshooting, see
[Operate an external LiteLLM gateway](../howto/operate-external-litellm-gateway.md).
