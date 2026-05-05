# Skill Verification

`amplihack-rs` does not ship the legacy Python skill verification helper.

Use the repository guard instead:

```bash
scripts/check-skills-no-missing-helpers.sh
```

For runtime dependencies, inspect the skill's `DEPENDENCIES.md` and verify the
required system commands directly with `command -v <tool>`.
