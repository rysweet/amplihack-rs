# Operations

Operational documentation covers release canaries, workflow terminal-state
behavior, conservative local cleanup, downstream validation, and the curated
roadmap for rollout work, autonomous mode, and PR recovery readiness.

## Runbooks and references

| Document | Use it for |
| --- | --- |
| [v0.10.6 Canary Evidence](v0.10.6-canary-evidence.md) | Evidence contract and current access limitations for install, update, default-workflow, and Azure DevOps checkout validation without leaking secrets. |
| [Hygiene Cleanup](hygiene-cleanup.md) | Safe disk cleanup for stale worktrees, detached Cargo targets, and old session artifacts. |
| [Workflow Publish and Finalize Resilience](workflow-resilience.md) | Idempotent publish/finalize behavior, observed brittle failure modes, and agentic finalizer terminal-state expectations. |
| [Prompt Delivery Downstream Validation](prompt-delivery-downstream-validation.md) | Simard/RabbitHole-style delegated workflow validation against prompt delivery behavior. |
| [Operational Roadmap](operational-roadmap.md) | Curated backlog for fleet rollout, Azure DevOps E2E, release contract monitoring, and workflow observability. |
| [Auto Mode](../AUTO_MODE.md) | Autonomous multi-turn execution. |
| [Auto Mode Safety](../AUTOMODE_SAFETY.md) | Safety guardrails and approval boundaries for autonomous execution. |
| [PR Recovery Readiness](../PR_RECOVERY_READINESS.md) | Existing-PR recovery readiness contract. |

## Operating principles

- Prefer dry-run and evidence first.
- Treat already-terminal workflow states as success only when they are
  positively identified.
- Reject ambiguous cleanup candidates instead of guessing.
- Record command shapes and outcomes, not credentials, token values, full
  environments, or private prompt content.
