---
title: Why the LiteLLM Gateway Stays External
description: Explains the control-plane-only integration with an operator-managed LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: explanation
---

# Why the LiteLLM Gateway Stays External

Amplihack integrates with LiteLLM as a launch-time control plane. It validates
one route, proves that the selected agent can use it without fallback, and
constructs a constrained child process. LiteLLM remains a separate,
operator-managed service.

Amplihack never installs, embeds, provisions, starts, stops, upgrades, or
administers LiteLLM.

## The boundary

```text
operator config
      |
      v
amplihack: validate config, destination, readiness, executable, argv, env
      |
      v
agent CLI ------------------------------------> external LiteLLM gateway
                                                       |
                                                       v
                                                model providers
```

Prompts and responses travel directly between the agent CLI and LiteLLM.
Amplihack does not open a proxy port, forward request bodies, buffer streams,
or inspect usage responses.

## Why amplihack does not embed LiteLLM

An embedded gateway would make amplihack responsible for two unrelated
lifecycle domains:

- launching agent processes; and
- operating a network service with provider credentials, databases, policy,
  accounting, and telemetry.

Keeping those domains separate gives each one a clear owner. Amplihack can
fail before process creation when a route is unsafe. The LiteLLM operator can
upgrade models, rotate provider credentials, enforce budgets, and scale the
gateway without changing amplihack.

This boundary also removes ambiguous partial ownership. A local spend counter
cannot enforce an organization-wide budget, and a launcher-side usage ledger
cannot observe every request made through a shared gateway. Those controls
belong where all gateway traffic is visible.

## Why routing fails closed

Gateway routing is commonly used for credential isolation, audit coverage, and
policy enforcement. Falling back to a direct provider after a gateway failure
would bypass all three while appearing to succeed.

For that reason, an enabled route has only two outcomes:

1. validation and readiness succeed, then the constrained child starts; or
2. the launch fails before the child starts.

There are no retries to another endpoint, direct-provider fallbacks, or
best-effort interpretations of unknown configuration.

## Why readiness is narrow

Amplihack sends one unauthenticated request to `/health/readiness`. It does not
send a model request because readiness must not consume tokens or expose a
prompt. It does not authenticate because a readiness redirect or compromised
endpoint must not receive the gateway credential.

Strict media-type, size, duplicate-key, and JSON checks make readiness a small
protocol rather than an arbitrary response parser. DNS and endpoint policy
reduce server-side request forgery and rebinding risk before the connection is
made.

Readiness proves that one validated gateway destination answered correctly at
launch time. For that request, DNS is resolved exactly once and the connection
is pinned to one address from the validated answer set while the original
hostname remains the HTTP host and TLS SNI name. This prevents a second
resolver lookup from changing the destination between validation and connect.

That pin applies only to amplihack's readiness request. It does not replace
continuous gateway monitoring or control DNS resolution performed later by
the separately implemented agent CLI.

## Why credentials are applied last

The parent process environment contains many possible alternate routes:
provider keys, proxy variables, cloud credentials, runtime injection settings,
and target-specific defaults. Merely adding a LiteLLM URL does not prove the
child uses it.

Amplihack therefore treats routing as a typed environment patch:

1. remove gateway configuration inputs;
2. remove direct and opposite-adapter credentials;
3. remove network and runtime bypasses;
4. rebuild the child environment from a small ambient allowlist and required
   amplihack runtime and session variables;
5. add the selected adapter variables, including the LiteLLM virtual key; and
6. revalidate the executable before spawn.

Applying this patch last prevents later launcher behavior from silently
reintroducing an alternate path. The gateway key must reach the selected
adapter so it can authenticate to LiteLLM; upstream provider credentials do
not reach the child.

## Operational ownership

| Concern | Owner |
|---|---|
| Route selection and launch-time validation | amplihack |
| Child executable and argument safety | amplihack |
| Provider credentials and model mappings | LiteLLM operator |
| Gateway availability and TLS | LiteLLM operator |
| Databases, migrations, accounting, and pricing | LiteLLM operator |
| Budgets, quotas, and rate limits | LiteLLM operator |
| Gateway logs, metrics, traces, and dashboards | LiteLLM operator |

The integration ships no LiteLLM deployment assets and starts no gateway
process. It works with an existing service that satisfies the documented
contract.

## Compatibility

When routing is disabled, the existing launcher behavior remains in force.
The public `--no-litellm` override makes that choice explicit.

Compatibility does not include the removed embedded proxy, management client,
deployment files, observability stack, local pricing, or usage accounting.
Configuration for those components is rejected rather than ignored, so an
obsolete control cannot look active when it is not.

See the [external LiteLLM gateway reference](../reference/external-litellm-gateway.md)
for the complete contract.
