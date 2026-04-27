<!-- Ported from upstream amplihack. Rust-specific adaptations applied where applicable. -->

# How to Troubleshoot Power Steering in Worktrees

Practical guide for fixing common Power Steering issues in git worktree environments.

!!! warning "Use `amplihack doctor` for Diagnostics"
    The preferred diagnostic tool for amplihack-rs is `amplihack doctor`, which
    reports worktree status, state directory health, and .disabled file locations.
    The bash scripts below provide manual equivalents for environments where the
    binary is not available.

## Quick Diagnosis

Run the built-in diagnostic command:

```bash
amplihack doctor
```

If the `amplihack` binary is not available, use this manual diagnostic:

```bash
echo "=== Power Steering Worktree Diagnostic ==="

# Check if in git repo
if ! git rev-parse --git-dir >/dev/null 2>&1; then
    echo "✗ Not a git repository"; exit 1
fi
echo "✓ Git repository detected"

# Check worktree status
COMMON_DIR="$(git rev-parse --git-common-dir)"
GIT_DIR="$(git rev-parse --git-dir)"
COMMON_ABS="$(cd "$COMMON_DIR" && pwd)"
GIT_ABS="$(cd "$GIT_DIR" && pwd)"

if [ "$COMMON_ABS" != "$GIT_ABS" ]; then
    echo "Worktree: Yes"
else
    echo "Worktree: No"
fi
echo "Common dir: $COMMON_DIR"
echo "Git dir: $GIT_DIR"

# Check state directory
STATE_DIR="$COMMON_DIR/.claude/runtime/power-steering"
echo ""
echo "State directory: $STATE_DIR"
echo "Exists: $([ -d "$STATE_DIR" ] && echo 'true' || echo 'false')"

COUNTER_FILE="$STATE_DIR/workflow_classification_block_counter.json"
if [ -f "$COUNTER_FILE" ]; then
    echo "Counter file: $COUNTER_FILE"
    echo "Counter data: $(cat "$COUNTER_FILE")"
fi

# Check .disabled file
echo ""
echo ".disabled file locations:"
for LOC in ".disabled" \
           "$COMMON_DIR/.claude/runtime/power-steering/.disabled" \
           "$(git rev-parse --show-toplevel)/.disabled"; do
    if [ -f "$LOC" ]; then
        echo "  ✓ $LOC → Power Steering disabled via this file"
    else
        echo "  ✗ $LOC"
    fi
done

echo ""
echo "=== Diagnostic Complete ==="
```

## Common Issues

### Issue: Counter Keeps Resetting

**Symptoms**:

- Block counter shows "9 more blocks" every time
- Counter doesn't increment
- State not persisting between invocations

**Diagnosis**:

```bash
# Check if state directory exists
STATE_DIR="$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"
echo "State directory: $STATE_DIR"
echo "Exists: $([ -d "$STATE_DIR" ] && echo 'true' || echo 'false')"
echo "Writable: $([ -d "$STATE_DIR" ] && [ -w "$STATE_DIR" ] && echo 'true' || echo 'false')"
```

**Solution 1: Create State Directory**

```bash
# Create state directory
mkdir -p "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"

# Verify creation
ls -la "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"
```

**Solution 2: Fix Permissions**

```bash
# Fix directory permissions
chmod 755 "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"

# Fix file permissions
chmod 644 "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/"*.json
```

**Solution 3: Check File Lock**

```bash
# Check for stale locks
ls -la "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/"*.lock

# Remove stale locks (if no amplihack processes running)
rm "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/"*.lock
```

### Issue: .disabled File Not Working

**Symptoms**:

- Created `.disabled` file but Power Steering still blocks
- Different behavior in worktree vs main repo
- Inconsistent disable behavior

**Diagnosis**:

```bash
# Check which .disabled file locations exist
COMMON_DIR="$(git rev-parse --git-common-dir)"
ROOT_DIR="$(git rev-parse --show-toplevel)"
FOUND=0
for LOC in ".disabled" \
           "$COMMON_DIR/.claude/runtime/power-steering/.disabled" \
           "$ROOT_DIR/.disabled"; do
    if [ -f "$LOC" ]; then
        echo "Found .disabled at: $LOC"
        FOUND=1
    fi
done
if [ "$FOUND" -eq 0 ]; then
    echo "No .disabled file found"
    echo "Checked: ./.disabled, $COMMON_DIR/.claude/runtime/power-steering/.disabled, $ROOT_DIR/.disabled"
fi
```

**Solution 1: Create in Correct Location**

```bash
# For current worktree only
touch .disabled

# For all worktrees (recommended)
touch "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"

# For backward compatibility
cd "$(git rev-parse --show-toplevel)"
touch .disabled
```

**Solution 2: Verify File Visibility**

```bash
# Check file exists and is readable
ls -la .disabled
cat .disabled  # File can be empty

# Check shared location
ls -la "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"
```

**Solution 3: Clear Cached State**

```bash
# Sometimes state cache prevents detection
rm "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"

# Restart amplihack session
```

### Issue: Hard Maximum Triggered

**Symptoms**:

- Message: "Classification blocked 10 times (hard maximum reached)"
- Automatic approval despite not wanting it
- Repeated classification failures

**This is a Bug**: File an issue at <https://github.com/rysweet/amplihack-rs/issues>

**Immediate Workaround**:

```bash
# Reset counter
rm "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"

# Disable Power Steering temporarily
touch "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"
```

**Collect Diagnostic Info**:

```bash
# Save session logs
cp ~/.claude/runtime/logs/*.log /tmp/power-steering-issue/

# Save counter state
cp "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json" \
   /tmp/power-steering-issue/

# Include in issue report
```

### Issue: Different Behavior in Worktree vs Main Repo

**Symptoms**:

- Counter works in main repo but not worktree
- .disabled file works in one location but not another
- State not shared between worktrees

**Diagnosis**:

```bash
# Compare common git directory
cd /path/to/main-repo
echo "Main repo common dir: $(git rev-parse --git-common-dir)"

cd /path/to/worktree
echo "Worktree common dir: $(git rev-parse --git-common-dir)"

# Should be the same!
```

**Solution: Verify Worktree Setup**

```bash
# Check worktree is properly configured
git worktree list

# Verify worktree .git points to correct location
cat /path/to/worktree/.git
# Should show: gitdir: /path/to/main/.git/worktrees/<name>

# Re-create worktree if corrupted
cd /path/to/main-repo
git worktree remove /path/to/worktree
git worktree add /path/to/worktree branch-name
```

### Issue: Infinite Loop Despite Hard Maximum

**Symptoms**:

- Power Steering blocks more than 10 times
- Hard maximum not triggering
- Counter goes above 10

**This Should Never Happen**: File an issue immediately at <https://github.com/rysweet/amplihack-rs/issues>

**Emergency Workaround**:

```bash
# Kill amplihack process
pkill -f amplihack

# Force disable
touch "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/.disabled"

# Reset counter
rm "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"
```

## Manual Counter Reset

Reset block counter to start fresh:

```bash
# Simple reset (removes counter)
rm "$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"

# Reset with confirmation
COUNTER="$(git rev-parse --git-common-dir)/.claude/runtime/power-steering/workflow_classification_block_counter.json"
if [ -f "$COUNTER" ]; then
    echo "Current data: $(cat "$COUNTER")"
    read -p "Reset to 0? (y/n): " REPLY
    if [ "$REPLY" = "y" ]; then
        rm "$COUNTER"
        echo "Counter reset"
    fi
else
    echo "No counter file found"
fi
```

## Checking Shared State Location

Verify where Power Steering stores shared state:

```bash
# Show state directory
STATE_DIR="$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"
echo "State directory: $STATE_DIR"
echo "Full path: $(cd "$STATE_DIR" 2>/dev/null && pwd || echo '(does not exist)')"

if [ -d "$STATE_DIR" ]; then
    echo ""
    echo "Contents:"
    ls -1 "$STATE_DIR"
fi
```

## Verifying Worktree Detection

Test if the current directory is a git worktree:

```bash
# Test worktree detection
COMMON_ABS="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
GIT_ABS="$(cd "$(git rev-parse --git-dir)" && pwd)"
if [ "$COMMON_ABS" != "$GIT_ABS" ]; then
    echo "Is worktree: true"
else
    echo "Is worktree: false"
fi
echo "Common git dir: $COMMON_ABS"
```

## Debugging Classification Blocks

Enable detailed logging to see why classification is blocked:

```bash
# Set debug environment variable
export AMPLIHACK_DEBUG=1

# Run amplihack
amplihack

# Check logs
tail -f ~/.claude/runtime/logs/power-steering-*.log
```

## Test Power Steering After Fixes

Verify Power Steering works correctly:

```bash
# Test 1: Counter persistence
STATE_DIR="$(git rev-parse --git-common-dir)/.claude/runtime/power-steering"
mkdir -p "$STATE_DIR"
echo '{"count": 5}' > "$STATE_DIR/workflow_classification_block_counter.json"
READBACK="$(cat "$STATE_DIR/workflow_classification_block_counter.json")"
echo "$READBACK" | grep -q '"count": 5' && echo "✓ Counter persistence works" || echo "✗ Counter not persisted"

# Test 2: .disabled detection
touch "$STATE_DIR/.disabled"
[ -f "$STATE_DIR/.disabled" ] && echo "✓ .disabled detection works" || echo "✗ .disabled not detected"
rm "$STATE_DIR/.disabled"

# Test 3: Worktree detection
COMMON_ABS="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
GIT_ABS="$(cd "$(git rev-parse --git-dir)" && pwd)"
if [ "$COMMON_ABS" != "$GIT_ABS" ]; then
    echo "✓ Worktree detection: true"
else
    echo "✓ Worktree detection: false (standard repo)"
fi
```

## Reporting Issues

When filing issues, include this diagnostic output:

```bash
# Preferred: use amplihack doctor
amplihack doctor > /tmp/power-steering-diagnostic.txt 2>&1

# Fallback: manual bash diagnostic
{
echo "=== Power Steering Diagnostic Report ==="
echo ""
echo "Git Configuration:"
echo "  Common dir: $(git rev-parse --git-common-dir)"
echo "  Git dir: $(git rev-parse --git-dir)"
echo "  Root: $(git rev-parse --show-toplevel)"

COMMON_ABS="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
GIT_ABS="$(cd "$(git rev-parse --git-dir)" && pwd)"
echo ""
echo "Worktree Status:"
[ "$COMMON_ABS" != "$GIT_ABS" ] && echo "  Is worktree: true" || echo "  Is worktree: false"
echo "  Common dir: $COMMON_ABS"

STATE_DIR="$COMMON_ABS/.claude/runtime/power-steering"
echo ""
echo "State Directory:"
echo "  Path: $STATE_DIR"
echo "  Exists: $([ -d "$STATE_DIR" ] && echo 'true' || echo 'false')"
echo "  Writable: $([ -d "$STATE_DIR" ] && [ -w "$STATE_DIR" ] && echo 'true' || echo 'false')"

COUNTER="$STATE_DIR/workflow_classification_block_counter.json"
echo ""
echo "Counter File:"
echo "  Path: $COUNTER"
echo "  Exists: $([ -f "$COUNTER" ] && echo 'true' || echo 'false')"
[ -f "$COUNTER" ] && echo "  Contents: $(cat "$COUNTER")"

echo ""
echo ".disabled File:"
for LOC in ".disabled" "$STATE_DIR/.disabled" "$(git rev-parse --show-toplevel)/.disabled"; do
    [ -f "$LOC" ] && echo "  Found: $LOC"
done

echo ""
echo "=== End Diagnostic Report ==="
} > /tmp/power-steering-diagnostic.txt

cat /tmp/power-steering-diagnostic.txt
```

Include this report when filing issues at: <https://github.com/rysweet/amplihack-rs/issues>

## Related Documentation

- [Worktree Support](../concepts/worktree-support.md) - Conceptual overview of worktree support
- [Power Steering Compaction](../concepts/power-steering-compaction.md) - Power Steering compaction concepts
- [Power Steering Compaction API](../reference/power-steering-compaction-api.md) - API reference
- [Troubleshoot Worktree](troubleshoot-worktree.md) - General worktree troubleshooting
