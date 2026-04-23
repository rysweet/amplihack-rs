# M2: Recipe Runner Consolidation

Achieve full feature parity in `recipe-runner-rs` and remove the Python
recipe runner fallback.

## Definition of Done

- [ ] All step types (`bash`, `agent`, `recipe`) supported by Rust runner
- [ ] All 16 YAML recipes in `amplifier-bundle/recipes/` pass validation
      with Rust runner
- [ ] All 16 recipes execute successfully with Rust runner (no Python
      fallback triggered)
- [ ] `recipe_runner.py` deleted from `amplifier-bundle/tools/`
- [ ] Freshness check in `freshness.rs` covers all supported platforms
- [ ] No recipe references Python runner as a fallback

## Gap Issues

1. **Condition evaluation** — Rust runner's Jinja-like condition parser may
   not cover all Python expressions used in existing recipes.
2. **Timeout handling** — Python runner and Rust runner may differ in how
   they signal timeouts to agent subprocesses.
3. **Context variable escaping** — Multi-line values with special characters
   may be handled differently between runners.

## Deliverables

| Deliverable | Location | Type |
|---|---|---|
| Step type parity | `recipe-runner-rs` repo | Code |
| Recipe validation suite | `tests/recipes/` | Tests |
| Python runner removal | `amplifier-bundle/tools/recipe_runner.py` | Deletion |
| Architecture doc | `docs/concepts/recipe-runner-architecture.md` | Documentation |

## Dependencies

No hard dependency on M1, but M1's recipe fixes may surface runner gaps.
