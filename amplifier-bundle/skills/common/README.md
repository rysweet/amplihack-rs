# Common Skills Infrastructure

Shared native helpers used by bundled skills.

## OOXML helpers

Office files (`.docx`, `.pptx`, `.xlsx`) are ZIP archives containing XML files.
Use the shared shell helpers for raw OOXML workflows:

```bash
bash common/ooxml/scripts/unpack.sh document.docx unpacked/
# edit XML files
bash common/ooxml/scripts/pack.sh unpacked/ modified.docx
```

## Verification

The legacy Python verification helper is not shipped in `amplihack-rs`. Use the
repository guard for helper-script integrity:

```bash
scripts/check-skills-no-missing-helpers.sh
```

For skill-specific runtime dependencies, inspect the skill's `DEPENDENCIES.md`
and verify required commands directly with `command -v <tool>`.
