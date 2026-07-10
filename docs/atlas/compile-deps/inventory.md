# compile-deps inventory

| crate | kinds | internal deps |
|---|---|---|
| `amplihack` | bin,test | `amplihack-cli`, `amplihack-hooks`, `amplihack-state`, `amplihack-types` |
| `amplihack-agent-core` | lib,test | `amplihack-memory` |
| `amplihack-agent-eval` | lib,test | — |
| `amplihack-agent-generator` | lib,test | — |
| `amplihack-asset-resolver-bin` | bin,test | `amplihack-cli` |
| `amplihack-blarify` | lib | — |
| `amplihack-builders` | lib,test | — |
| `amplihack-cli` | custom-build,example,lib,test | `amplihack-builders`, `amplihack-hive`, `amplihack-launcher`, `amplihack-memory`, `amplihack-reflection`, `amplihack-remote`, `amplihack-state`, `amplihack-types`, `amplihack-utils`, `amplihack-workflows` |
| `amplihack-context` | lib | — |
| `amplihack-delegation` | lib,test | — |
| `amplihack-domain-agents` | lib,test | `amplihack-agent-core`, `amplihack-memory`, `amplihack-workflows` |
| `amplihack-fleet` | lib | — |
| `amplihack-hive` | lib,test | `amplihack-agent-core`, `amplihack-memory` |
| `amplihack-hooks` | lib,test | `amplihack-cli`, `amplihack-security`, `amplihack-state`, `amplihack-types`, `amplihack-utils`, `amplihack-workflows` |
| `amplihack-hooks-bin` | bin,test | `amplihack-hooks`, `amplihack-state`, `amplihack-types` |
| `amplihack-launcher` | lib,test | `amplihack-types`, `amplihack-utils` |
| `amplihack-memory` | lib,test | — |
| `amplihack-multilspy` | lib | — |
| `amplihack-orchestration` | lib,test | `amplihack-utils` |
| `amplihack-recovery` | lib | — |
| `amplihack-reflection` | lib,test | — |
| `amplihack-remote` | lib,test | — |
| `amplihack-safety` | lib | — |
| `amplihack-security` | lib | — |
| `amplihack-session` | example,lib,test | — |
| `amplihack-state` | lib | `amplihack-types` |
| `amplihack-types` | lib | — |
| `amplihack-utils` | lib,test | — |
| `amplihack-workflows` | lib,test | `amplihack-utils` |
