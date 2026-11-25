# Context Management

Context management preserves useful session state with native agent workflows and the bundled `scripts/context-management.sh` helper.

Use it to create compact handoffs, recover after compaction, and decide what context to retain. Prefer `/transcripts`, `/reflect`, session checkpoints, direct repository inspection, and helper snapshots.

```bash
scripts/context-management.sh status
scripts/context-management.sh snapshot "before handoff"
scripts/context-management.sh show latest
```

## Handoff Template

```markdown
## Goal
## User requirements
## Decisions made
## Files changed or inspected
## Validation
## Remaining work
## Blockers
```

Fixtures under `tests/fixtures/` are sample payloads for future native tests.
