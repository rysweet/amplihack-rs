# Recipe Discovery Troubleshooting

## Overview

Recipe Runner includes enhanced discovery diagnostics to help troubleshoot recipe loading issues, especially in subprocess isolation environments like /tmp clones.

## Features

### Global-First Search Priority

Recipes are discovered in this priority order:

1. **Binary-relative Path** - Recipes bundled alongside the `amplihack` binary
2. **Repository Root** - repo-root `amplifier-bundle/recipes/`
3. **Global Installation** - `~/.amplihack/.claude/recipes/` (user-installed recipes)
4. **CWD Bundle** - `amplifier-bundle/recipes/` (CWD-relative, legacy compatibility)
5. **Project-local** - `.claude/recipes/` (project-specific recipes, can override)

**Why this matters**: When amplihack is installed via `cargo install` and you run from any directory, the binary-relative path (1) is the only reliable location for bundled recipes. CWD-relative paths (4) only work when running from the amplihack repo directory.

### Debug Logging

Enable verbose output to see exactly which paths are searched:

```bash
AMPLIHACK_LOG=debug amplihack recipe list
```

**Output shows**:

- Each directory searched
- Whether directories exist
- Which recipes are found in each location
- Total recipe count

### Installation Verification

Check if global recipes are properly installed:

```bash
amplihack recipe list --verbose
```

This displays all discovered recipes and the paths they were loaded from. If no recipes are found, verify that `~/.amplihack/.claude/recipes/` exists and contains recipe YAML files.

## Common Issues

### Issue: "No recipes discovered" when running from different directory

**Symptom**: `amplihack recipe list` returns empty when running from a project other than amplihack's repo
**Cause**: Discovery used only CWD-relative paths before version 0.9.0
**Solution**: Upgrade to amplihack >= 0.9.0 which includes binary-relative paths in discovery

### Issue: "No recipes discovered" in /tmp clone

**Symptom**: `amplihack recipe list` returns empty
**Cause**: Global recipes not installed at `~/.amplihack/.claude/recipes/`
**Solution**: Verify global installation exists

### Issue: Wrong recipe version loaded

**Symptom**: Unexpected recipe behavior
**Cause**: Local recipe overriding global recipe
**Solution**: Enable debug logging to see which path won

## Version History

### Version 0.9.0 (March 2026)

- Issue #2812, PR #2813: Recipe discovery now includes binary-relative path
- Recipes discoverable from any working directory after `cargo install`

### Version 0.5.32

- Issue #2381: Recipe discovery now works in /tmp clones
- Global recipes prioritized for subprocess isolation
- Debug logging added for troubleshooting

## See Also

- [Recipe Runner Documentation](./README.md)
- Testing Results
