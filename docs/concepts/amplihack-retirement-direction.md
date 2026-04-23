# amplihack Python Retirement Direction

The strategic direction for migrating bundled Python components to Rust,
eliminating the Python runtime dependency.

## Contents

- [Migration status](#migration-status)
- [What moves where](#what-moves-where)
- [Compatibility guarantees](#compatibility-guarantees)
- [Milestones](#milestones)

---

## Migration status

| Component | Python location | Rust status | Notes |
|---|---|---|---|
| CLI entry point | `amplifier-bundle/amplihack` | ✅ Complete | `amplihack-cli` crate |
| Install/uninstall | `amplifier-bundle/tools/installer.py` | ✅ Complete | `commands/install/` |
| Hook registration | `amplifier-bundle/tools/hooks.py` | ✅ Complete | `commands/install/hooks.rs` |
| Binary resolution | `amplifier-bundle/tools/binary_finder.py` | ✅ Complete | `binary_finder.rs` |
| Recipe runner | `amplifier-bundle/tools/recipe_runner.py` | ⚠️ Partial | `recipe-runner-rs` (separate repo) |
| Session tree | `amplifier-bundle/tools/session_tree.py` | ❌ Not started | Still Python-only |
| Memory/discoveries | `amplifier-bundle/tools/memory/` | ⚠️ Partial | SQLite backend in Rust, graph DB pending |
| Code graph (SCIP) | `amplifier-bundle/tools/blarify/` | ✅ Complete | LadybugDB via `amplihack-memory` crate |
| Fleet dashboard | N/A | ✅ Native Rust | No Python predecessor |
| Agent framework | N/A | ✅ Native Rust | `amplihack-agent-*` crates |

**Legend**: ✅ = Rust implementation is production-ready. ⚠️ = Partial
migration, both implementations coexist. ❌ = Not started.

## What moves where

### Stays in `amplifier-bundle/` (non-code assets)

These are configuration and content files, not executable code:

- YAML recipes (`recipes/*.yaml`)
- Agent definitions (`agents/amplihack/*.md`)
- Skill definitions (`skills/`)
- Context files (`context/*.md`)
- Templates (`templates/`)

### Moves to Rust crates

- **Recipe execution** → `recipe-runner-rs` (external binary)
- **Session tree management** → proposed: new module in `amplihack-cli`
- **Memory adapters** → `amplihack-memory` crate (in progress)

### Deleted when migration completes

- `amplifier-bundle/tools/recipe_runner.py` — replaced by `recipe-runner-rs`
- `amplifier-bundle/tools/session_tree.py` — replaced by Rust module
- `amplifier-bundle/tools/binary_finder.py` — already replaced
- Python `__pycache__` directories and `.pyc` files

## Compatibility guarantees

1. **Recipe YAML format is stable.** Recipes written for the Python runner
   must work with the Rust runner without modification.

2. **Context variable names are stable.** Steps reference variables like
   `{{task_description}}` — these names cannot change.

3. **CLI interface is stable.** `amplihack recipe run <name> --context k=v`
   works identically regardless of which runner executes it.

4. **Environment variables are stable.** All `AMPLIHACK_*` env vars
   documented in the [Environment Variables](../reference/environment-variables.md)
   reference retain their semantics.

## Milestones

### M1: Orchestrator Stability

Stabilize the smart-orchestrator and default-workflow recipes. No Python
migration work — purely recipe-level fixes.

- Issue dedup guards in smart-orchestrator
- Adaptive recovery hardening
- Hollow-success detection tuning

### M2: Recipe Runner Consolidation

Achieve full feature parity in `recipe-runner-rs` and remove the Python
fallback.

- Port remaining step types
- Validate all recipes against Rust runner
- Remove `recipe_runner.py`

### M3: Bundled Python Winddown

Eliminate all executable Python from `amplifier-bundle/tools/`.

- Port `session_tree.py` to Rust
- Complete memory adapter migration
- Remove Python runtime requirement from install
- Update `validate-no-python` probe (AC9) to cover new modules
- Blocked on: M2 completion

See the [.pm/ milestone files](../../.pm/) for detailed acceptance criteria.

## Related

- [Recipe Runner Architecture](./recipe-runner-architecture.md) — Why the runner is external
- [Migrate from Python](../howto/migrate-from-python.md) — User-facing migration guide
- [Bootstrap Parity](./bootstrap-parity.md) — Why Rust replicates the Python install flow
