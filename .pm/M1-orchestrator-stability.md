# M1: Orchestrator Stability

Stabilize the smart-orchestrator and default-workflow recipes so that
routine development tasks complete reliably without duplicate issues or
silent failures.

## Definition of Done

- [ ] smart-orchestrator `file-routing-bug` step uses idempotency guards
      (same two-guard pattern as default-workflow step-03)
- [ ] Hollow-success detection thresholds validated against 10+ real
      session transcripts
- [ ] Adaptive recovery tested with forced routing gaps (all conditions
      false)
- [ ] No duplicate issues created when the same routing failure repeats
      within 24 hours
- [ ] Reflection rounds correctly distinguish PARTIAL from HOLLOW
- [ ] Recursion guard logs are actionable (include tree ID and depth)
- [ ] `detect-execution-gap` diagnostic banner includes recipe version
- [ ] All changes covered by recipe-level integration tests

## Deliverables

| Deliverable | File | Type |
|---|---|---|
| Dedup guards in smart-orchestrator | `amplifier-bundle/recipes/smart-orchestrator.yaml` | Recipe fix |
| Hollow-success test cases | `tests/recipes/` | Tests |
| Issue dedup reference doc | `docs/reference/issue-dedup.md` | Documentation |
| Recovery concept doc | `docs/concepts/smart-orchestrator-recovery.md` | Documentation |

## Dependencies

None. This milestone is self-contained.
