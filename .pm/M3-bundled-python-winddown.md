# M3: Bundled Python Winddown

Eliminate all executable Python from `amplifier-bundle/tools/`, making
the amplihack installation fully independent of a Python runtime.

## Definition of Done

- [ ] `session_tree.py` ported to Rust (module in `amplihack-cli` or
      standalone binary)
- [ ] Session tree state format (`/tmp/amplihack-session-trees/*.json`)
      remains compatible
- [ ] Memory discovery adapter migrated to Rust SQLite backend
- [ ] Memory graph-DB adapter migrated or removed
- [ ] All `amplifier-bundle/tools/*.py` files deleted or converted to
      non-executable assets
- [ ] `amplihack doctor` probe AC9 (validate-no-python) updated to scan
      new module paths
- [ ] `amplihack install` no longer checks for or requires Python
- [ ] CI pipeline drops Python from build matrix
- [ ] Integration tests confirm full operation without Python on PATH
- [ ] Migration guide updated with winddown completion notice

## Deliverables

| Deliverable | Location | Type |
|---|---|---|
| Rust session tree | `crates/amplihack-cli/src/commands/session_tree/` | Code |
| Memory adapter completion | `crates/amplihack-memory/` | Code |
| Python file removal | `amplifier-bundle/tools/*.py` | Deletion |
| AC9 probe update | `crates/amplihack-cli/src/commands/doctor/` | Code |
| Retirement direction doc | `docs/concepts/amplihack-retirement-direction.md` | Documentation |

## Dependencies

- **Blocked on M2** — Recipe runner must be fully Rust before removing
  Python runner.
- Session tree port can proceed in parallel with M2.
