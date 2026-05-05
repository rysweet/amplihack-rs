# M3: Bundled Python Winddown

Eliminate all executable Python from `amplifier-bundle/tools/`, making
the amplihack installation fully independent of a Python runtime.

## Definition of Done

- [x] `session_tree.py` ported to Rust (module in `amplihack-cli` or
      standalone binary)
- [x] Session tree state format (`/tmp/amplihack-session-trees/*.json`)
      remains compatible
- [x] Memory discovery adapter migrated to Rust SQLite backend
- [x] Memory graph-DB adapter migrated or removed
- [x] All `amplifier-bundle/tools/*.py` files deleted or converted to
      non-executable assets
- [x] `amplihack doctor` probe AC9 (validate-no-python) updated to scan
      new module paths
- [x] `amplihack install` no longer checks for or requires Python
- [x] CI pipeline drops Python from build matrix
- [x] Integration tests confirm full operation without Python on PATH
- [x] Migration guide updated with winddown completion notice

## Deliverables

| Deliverable | Location | Type |
|---|---|---|
| Rust session tree | `crates/amplihack-cli/src/commands/session_tree/` | Code |
| Memory adapter completion | `crates/amplihack-memory/` | Code |
| Python file removal | `amplifier-bundle/tools/*.py` | Deletion |
| AC9 probe update | `crates/amplihack-cli/src/commands/doctor/` | Code |
| Retirement direction doc | `docs/concepts/amplihack-retirement-direction.md` | Documentation |

## Dependencies

- Completed after the Rust recipe runner and native hook binaries replaced the
  remaining Python execution paths.
