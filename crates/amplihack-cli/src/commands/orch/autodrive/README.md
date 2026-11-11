# Portable auto-drive records

This module implements local workflow safeguards for issue #2329. It never
contacts a hosting service, decides semantic readiness, or performs a merge.
Agents observe and assess facts under trusted merge-ready policy.

- `authorization`: strictly capture invocation booleans before child execution.
  The full composer defaults to merge authorization; direct consumers and
  light mode default to verification. Dry-run suppresses follow-ups as well.
  Resume captures fresh authority; no candidate or inherited marker grants it.
- `trusted`: pin and privately copy launch recipes, skills, templates, helpers
  and evaluator; freeze authorization in child prompts and literal recipe
  contexts. Clean exact retired environment entries without sourcing them.
- `records` / `validation`: closed schema-1 identity, criterion, attempt,
  recovery, finalization and completion records. Exactly 13 criteria, current
  head/base/run binding, concrete observations and no contradictions establish
  structural readiness. Semantic correctness remains the agent's obligation.
- `state`: create private directories and files, reject unsafe historical
  modes, links, traversal and special files; bound reads; validate and hash
  opened bytes; publish new snapshots atomically without replacement.
- `outcome`: keep readiness, requested action, observed state and stop reason
  separate. Both accepted round and evaluator completion are necessary. Merge
  success additionally requires authorized expected-head protection and
  platform confirmation; queue admission or exit zero is insufficient.
- `review`: verify the established post-review allowlist against local Git.
- `completion`: seal accepted round bytes only after the existing loop engine
  observes both completion signals; reject receipts from an older invocation.
- `followups`: validate private MINOR_ONLY issue mappings for reporting.
- `helper`: optional local CLI operations under `amplihack orch helper
  autodrive`. Required record validation in the recipe uses these local
  operations; platform access and recovery have no prescribed interface.

The full serialized agent contract is in the bundled skill's
`evidence-contract.md`. `tests.rs` exercises authorization, stale/mismatched
records, concrete evidence, failure/recovery, private state and outcomes. The
CLI integration suite exercises runner boundaries without executing a real
agent or merge. Neither suite establishes live recovery or macOS execution.

Private state and prompts are not an OS security boundary against a process
holding the same user's credentials. Repository protections remain required.
