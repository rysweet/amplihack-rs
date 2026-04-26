# Recipe Discovery Troubleshooting

## Overview

Recipe Runner includes enhanced discovery diagnostics to help troubleshoot
recipe loading issues, especially in subprocess isolation environments like
`/tmp` clones.

## Features

### Global-First Search Priority

Recipes are discovered in this priority order:

1. **Installed Package Path** — `site-packages/amplihack/amplifier-bundle/recipes/` (for installed amplihack)
2. **Repository Root** — repo-root `amplifier-bundle/recipes/` (for editable installs)
3. **Global Installation** — `~/.amplihack/.claude/recipes/` (user-installed recipes)
4. **CWD Bundle** — `amplifier-bundle/recipes/` (CWD-relative, legacy compatibility)
5. **CWD Source** — `src/amplihack/amplifier-bundle/recipes/` (CWD-relative, development)
6. **Project-local** — `.claude/recipes/` (project-specific recipes, can override)

!!! note "Rust Port"
    In amplihack-rs, the Rust binary embeds bundled recipes at compile time.
    The global installation path (`~/.amplihack/.claude/recipes/`) and
    project-local path (`.claude/recipes/`) still apply for user overrides.

**Why this matters**: When amplihack is installed and you run from any
directory, the installed path (1) is the only reliable location for bundled
recipes. CWD-relative paths (4, 5) only work when running from the amplihack
repo directory.

### Debug Logging

Enable debug logging to see exactly which paths are searched:

```bash
AMPLIHACK_VERBOSE=1 amplihack recipe list
```

**Output shows**:

- Each directory searched
- Whether directories exist
- Which recipes are found in each location
- Total recipe count

### Installation Verification

Check if global recipes are properly installed:

```bash
amplihack recipe list --long
```

If the list is empty, verify that `~/.amplihack/.claude/recipes/` contains
recipe YAML files.

## Common Issues

### Issue: "No recipes discovered" when running from different directory

**Symptom**: `amplihack recipe list` returns empty when running from a project
other than amplihack's own repo.

**Cause**: Discovery used only CWD-relative paths in earlier versions.

**Solution**: Upgrade to amplihack >= 0.9.0 which includes absolute package
paths in discovery.

### Issue: "No recipes discovered" in /tmp clone

**Symptom**: `amplihack recipe list` returns empty in subprocess isolation.

**Cause**: Global recipes not installed at `~/.amplihack/.claude/recipes/`.

**Solution**: Verify global installation exists:

```bash
ls ~/.amplihack/.claude/recipes/*.yaml 2>/dev/null || echo "No global recipes found"
```

If missing, re-run the amplihack installer:

```bash
amplihack install
```

### Issue: Wrong recipe version loaded

**Symptom**: Unexpected recipe behavior — a local override shadows the bundled
version.

**Cause**: Local recipe in `.claude/recipes/` overriding global recipe with
same name.

**Solution**: Enable verbose logging to see which path won:

```bash
AMPLIHACK_VERBOSE=1 amplihack recipe show <recipe-name>
```

### Issue: Recipe not found after pip install

**Symptom**: `amplihack recipe run default-workflow` fails with "recipe not
found" after `pip install amplihack`.

**Cause**: The package path is not in the search order (pre-0.9.0).

**Solution**: Upgrade or manually copy recipes:

```bash
# Copy bundled recipes to global location
cp -r $(python -c "import amplihack; print(amplihack.__path__[0])")/amplifier-bundle/recipes/* \
  ~/.amplihack/.claude/recipes/
```

## Version History

### Version 0.9.0 (March 2026)

- Recipe discovery now includes installed package path
- Absolute paths resolved via compile-time embedding work for all installs
- Recipes discoverable from any working directory
- Added package-relative and repo-root search paths

### Version 0.5.32

- Recipe discovery now works in `/tmp` clones
- Global recipes prioritized for subprocess isolation
- Debug logging added for troubleshooting

## Related Documentation

- [Recipe Quick Reference](../reference/recipe-quick-reference.md) — CLI cheat sheet
- [Recipe CLI Examples](recipe-cli-examples.md) — real-world usage
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — how recipes execute
- [Recipe Resilience](../concepts/recipe-resilience.md) — error handling in recipes
