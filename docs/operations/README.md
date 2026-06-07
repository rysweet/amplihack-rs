# Operations

Operational documentation covers release canaries, workflow terminal-state
behavior, conservative local cleanup, downstream validation, and the curated
roadmap for rollout work.

## Runbooks and references

| Document | Use it for |
| --- | --- |
| [v0.10.6 Canary Evidence](v0.10.6-canary-evidence.md) | Evidence contract and current access limitations for install, update, default-workflow, and Azure DevOps checkout validation without leaking secrets. |
| [Hygiene Cleanup](hygiene-cleanup.md) | Safe disk cleanup for stale worktrees, detached Cargo targets, and old session artifacts. |
| [Workflow Publish and Finalize Resilience](workflow-resilience.md) | Idempotent publish/finalize behavior for no-diff, already-merged, closed-after-merge, and existing-PR states. |
| [Prompt Delivery Downstream Validation](prompt-delivery-downstream-validation.md) | Simard/RabbitHole-style delegated workflow validation against prompt delivery behavior. |
| [Operational Roadmap](operational-roadmap.md) | Curated backlog for fleet rollout, Azure DevOps E2E, release contract monitoring, and workflow observability. |

## Operating principles

- Prefer dry-run and evidence first.
- Treat already-terminal workflow states as success only when they are
  positively identified.
- Reject ambiguous cleanup candidates instead of guessing.
- Record command shapes and outcomes, not credentials, token values, full
  environments, or private prompt content.
