---
title: Data Stores
---
# Data stores (inventory)

| Store | Backing | Used by |
|---|---|---|
| Cognitive memory graph | embedded graph DB (`crates/amplihack-memory`) | memory store/retrieval |
| Code graph | `crates/amplihack-blarify` | code-structure analysis |
| Session state | on-disk JSON under session-state dirs (`crates/amplihack-state`) | CLI / hooks / multitask |
| Staged assets | `~/.copilot/{agents,skills,context}` | launcher / install (`copilot_setup`) |
