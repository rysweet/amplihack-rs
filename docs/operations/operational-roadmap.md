# Operational Roadmap

This roadmap is intentionally small. It tracks only operational work needed to
make amplihack releases safe across local development, fleet hosts, Azure
DevOps, and workflow observability.

> **Status:** This is a curated implementation backlog, not a release status
> report. Keep completed work in PRs, issues, and release notes; keep this file
> focused on the remaining operational contracts.

## Themes

| Theme | Goal | Done when |
| --- | --- | --- |
| Fleet rollout verification | Every release has repeatable install/update/default-workflow evidence on representative hosts. | Canary evidence exists for reachable `azlin` hosts, failures have issues, and obsolete intermediate branches are closed or deleted. |
| Azure DevOps E2E coverage | Azure DevOps repositories exercise tracking and PR-routing paths without relying on GitHub assumptions. | At least one accessible Azure DevOps checkout runs default-workflow, or a documented access limitation plus local fixture coverage exists. |
| Release/version contract monitoring | Installed binaries, update behavior, and docs agree on the expected release version. | Install/update canaries verify `amplihack --version`; release docs and package metadata match; drift files an issue. |
| Workflow observability | Publish/finalize outcomes are understandable without reading recipe internals. | Terminal states such as `no_diff`, `existing_open_pr`, and `already_merged` appear in recipe output and can be collected by CI or fleet scripts. |

## Backlog

| Item | Theme | Priority | Acceptance criteria |
| --- | --- | --- | --- |
| Maintain v0.10.6+ canary ledger | Fleet rollout verification | High | `docs/operations/v0.10.6-canary-evidence.md` or its successor records real sanitized commands/results for each reachable host. |
| Add fleet canary wrapper | Fleet rollout verification | Medium | One command runs install, update, version check, and default-workflow canary with JSON output and no secret logging. |
| Establish Azure DevOps validation checkout | Azure DevOps E2E coverage | High | A stable checkout or fixture is available for default-workflow validation; absence is explicitly recorded when blocked. |
| Expand Azure DevOps workflow fixtures | Azure DevOps E2E coverage | Medium | Local tests cover Azure DevOps remote detection, work item reuse, publish skip/routing, and finalize no-op behavior. |
| Monitor release contract drift | Release/version contract monitoring | High | CI or release automation checks version strings, install target behavior, update behavior, and docs references before release. |
| Archive publish/finalize JSON summaries | Workflow observability | Medium | Fleet or CI jobs collect `pr_publish_result` and `workflow_result.terminal_state` without requiring recipe log scraping. |
| Surface cleanup dry-run metrics | Workflow observability | Medium | `amplihack hygiene cleanup --format json` output can be archived by fleet jobs without exposing private paths beyond configured redaction. |

## Triage rules

- Keep this roadmap capped to the four themes above.
- Move implementation details into issues or PR descriptions.
- Remove completed items once their docs and validation hooks are stable.
- Do not add aspirational platform work unless it protects release rollout,
  Azure DevOps E2E, version contracts, or observability.
