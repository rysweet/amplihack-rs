<!-- Ported from upstream amplihack. Rust-specific adaptations applied where applicable. -->

# Migration Guide: Power Steering Worktree Support

Guide for users upgrading from pre-worktree Power Steering to the worktree-aware version.

## What's New

Power Steering now includes full git worktree support with:

1. **Shared state persistence** - Counter works across all worktrees
2. **Multi-path .disabled detection** - Check current dir, shared dir, and project root
3. **Hard maximum enforcement** - Automatic approval after 10 blocks
4. **Clear error messages** - Actionable instructions for fixes

## Do You Need to Migrate?

Check if you are affected:

```bash
# Are you using git worktrees?
git worktree list

# If this shows multiple entries, you'll benefit from the upgrade
```

If you see multiple worktrees, the upgrade fixes:

- Counter resetting in worktrees (now persists)
- .disabled file not working (now checks multiple locations)
- Potential infinite loops (now has hard maximum)

## Migration Steps

### Step 1: Backup Current State

```bash
# Backup existing counter (if in main repo)
if [ -f .git/.claude/runtime/power-steering/workflow_classification_block_counter.json ]; then
    cp .git/.claude/runtime/power-steering/workflow_classification_block_counter.json \
       /tmp/power-steering-backup.json
    echo "Backed up counter state"
fi
```

### Step 2: Upgrade amplihack

```bash
# Pull latest changes
git pull origin main

# Or reinstall
cargo install amplihack
```

### Step 3: Verify Installation

```bash
# Verify amplihack-rs is installed and working
amplihack --version

# Run the built-in diagnostic
amplihack doctor
```

### Step 4: Migrate State (Automatic)

The new version automatically handles state migration:

**Before** (pre-worktree):

```
.git/.claude/runtime/power-steering/
└── workflow_classification_block_counter.json
```

**After** (worktree-aware):

```
$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/
└── workflow_classification_block_counter.json
```

In standard repos, these are the same location. No action required.

### Step 5: Migrate .disabled Files

If you had .disabled files in worktrees, move them to shared location:

```bash
# Check for .disabled files in worktrees
git worktree list | awk '{print $1}' | while read worktree; do
    if [ -f "$worktree/.disabled" ]; then
        echo "Found .disabled in: $worktree"
        # Move to shared location
        touch "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"
        echo "Migrated to shared location"
        rm "$worktree/.disabled"
    fi
done
```

### Step 6: Test Worktree Functionality

```bash
# Create test worktree
git worktree add /tmp/test-worktree-migration main

# Test counter persistence
cd /tmp/test-worktree-migration

# Verify the common git dir resolves correctly
git rev-parse --git-common-dir

# Cleanup
cd -
git worktree remove /tmp/test-worktree-migration
```

## Configuration Changes

### No Configuration Required

The upgrade is **backward compatible**. No configuration changes needed.

### Optional: Centralize .disabled File

For worktree users, centralize .disabled file:

```bash
# Old: .disabled in each worktree (still works)
# New: .disabled in shared location (recommended)

# Create shared .disabled
touch "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"

# Remove individual worktree .disabled files
git worktree list | awk '{print $1}' | while read worktree; do
    rm -f "$worktree/.disabled"
done
```

## Breaking Changes

### None

This release has **no breaking changes**. All existing functionality works identically.

### Behavior Changes

1. **Counter persists across worktrees** (previously reset)
2. **.disabled file checked in 3 locations** (previously only CWD and root)
3. **Hard maximum enforced** (new feature, prevents infinite loops)

## Rollback Plan

If you encounter issues, rollback to previous version:

```bash
# Restore from backup
if [ -f /tmp/power-steering-backup.json ]; then
    cp /tmp/power-steering-backup.json \
       .git/.claude/runtime/power-steering/workflow_classification_block_counter.json
fi

# Revert amplihack to a previous version
cargo install amplihack --version <previous-version>
```

## Verification Checklist

After migration, verify these work:

- [ ] Counter persists in main repo
- [ ] Counter persists in worktrees
- [ ] Counter shared across all worktrees
- [ ] .disabled file works in current directory
- [ ] .disabled file works in shared location
- [ ] .disabled file works in project root
- [ ] Hard maximum triggers after 10 blocks
- [ ] Error messages are clear

## Common Migration Issues

### Issue: Counter Not Migrating

**Symptom**: Counter reset to 0 after upgrade

**Solution**: Manually migrate counter

```bash
# Copy old counter to new location
OLD_COUNTER=".git/.claude/runtime/power-steering/workflow_classification_block_counter.json"
NEW_COUNTER="$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"

if [ -f "$OLD_COUNTER" ] && [ "$OLD_COUNTER" != "$NEW_COUNTER" ]; then
    mkdir -p "$(dirname "$NEW_COUNTER")"
    cp "$OLD_COUNTER" "$NEW_COUNTER"
    echo "Counter migrated"
fi
```

### Issue: .disabled File Not Working

**Symptom**: Created .disabled but Power Steering still blocks

**Solution**: Check file location. The .disabled file should be in one of:

1. `./.disabled` (current directory)
2. `$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled` (shared location)
3. `$(git rev-parse --show-toplevel)/.disabled` (project root)

## Getting Help

If you encounter migration issues:

1. **Check logs**: `~/.claude/runtime/logs/power-steering-*.log`
2. **File issue**: [GitHub Issues](https://github.com/rysweet/amplihack-rs/issues)

## Related Documentation

- [Worktree Support](../concepts/worktree-support.md) - Feature overview
- [Troubleshoot Worktree](troubleshoot-worktree.md) - Fix common issues
