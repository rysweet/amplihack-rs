# Automode Safety Guide

**Type**: Explanation (Understanding-Oriented)

!!! danger "Critical"
    Automode works in the current directory and can conflict with active sessions.
    Read this guide before using auto mode.

## The Problem

When you launch `amplihack claude --auto` from within an active Claude Code session:

- Automode tries to stage files in the same `.claude/` directory
- Conflicts with existing structure
- Can overwrite uncommitted changes
- Results in: `OSError: Directory not empty`
- **Risk: Data loss**

## Safe Usage Patterns

### Option 1: Git Worktrees (Recommended)

For parallel automode sessions:

```bash
# Commit current work first
git add -A && git commit -m "checkpoint: before automode"

# Create worktrees for each task
git worktree add ./worktrees/automode-task1 -b automode-task1
git worktree add ./worktrees/automode-task2 -b automode-task2

# Launch from worktrees (isolated environments)
cd ./worktrees/automode-task1
amplihack claude --auto --max-turns 10 -- -p "task 1 description"
```

**Benefits**: Complete isolation, no file conflicts, can run in parallel.

### Option 2: Commit First

For single automode sessions:

```bash
# Save current work
git add -A && git commit -m "WIP: before automode"

# Launch automode
amplihack claude --auto --max-turns 10 -- -p "task description"

# If issues occur, rollback
git reset HEAD~1
```

### Option 3: Separate Clone

For experimental usage:

```bash
git clone <repo-url> ~/automode-workspace
cd ~/automode-workspace
amplihack claude --auto --max-turns 10 -- -p "task"
```

## What NOT To Do

| Dangerous Action | Risk |
| ---------------- | ---- |
| Launch from active session with uncommitted work | Lost changes, conflicts |
| Multiple automode in same directory | File staging conflicts, crashes |

## Pre-Flight Checklist

Before launching automode:

- [ ] All important changes are committed
- [ ] OR using a git worktree
- [ ] OR in a separate clone
- [ ] Understand automode will modify `.claude/` directory
- [ ] Have recovery plan if things go wrong

## Recovery

If automode crashes and changes are lost:

```bash
# Check git reflog for lost commits
git reflog

# Check for stashes
git stash list

# Restore to last good state
git reset --hard HEAD
```

## Recommended Parallel Workflow

```bash
# 1. Commit current state
git add -A && git commit -m "checkpoint"

# 2. Create worktrees
for i in {1..3}; do
  git worktree add ./worktrees/automode-$i -b automode-$i
done

# 3. Launch in background
(cd ./worktrees/automode-1 && amplihack claude --auto --max-turns 10 -- -p "task 1") &
(cd ./worktrees/automode-2 && amplihack claude --auto --max-turns 10 -- -p "task 2") &
(cd ./worktrees/automode-3 && amplihack claude --auto --max-turns 10 -- -p "task 3") &
wait

# 4. Review results, cleanup
git worktree remove ./worktrees/automode-{1..3}
```

## Related

- [Auto Mode](../concepts/auto-mode.md) — auto mode overview and usage
- [Worktree Support](../concepts/worktree-support.md) — worktree management in amplihack-rs
- [Troubleshoot Worktree](../howto/troubleshoot-worktree.md) — worktree troubleshooting
