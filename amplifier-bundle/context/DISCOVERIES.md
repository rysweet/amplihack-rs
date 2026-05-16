# DISCOVERIES.md

This file documents non-obvious problems, solutions, and patterns discovered during development. It serves as a living knowledge base.

**Archive**: Entries older than 3 months are moved to [DISCOVERIES_ARCHIVE.md](./DISCOVERIES_ARCHIVE.md).

## Table of Contents

### Recent (January 2026)

- [Power-Steering Post-Compaction Transcript Bug](#power-steering-post-compaction-transcript-bug-2026-01-17)
- [Kuzu Memory System: Database Exists But Empty Due to Async/Await Bug](#kuzu-memory-system-async-bug-2026-01-17)

Older entries archived in [DISCOVERIES_ARCHIVE.md](./DISCOVERIES_ARCHIVE.md).

---

## Power-Steering Post-Compaction Transcript Bug (2026-01-17)

### Problem

Power-steering hook blocks session termination with "work incomplete" when session has been compacted, even though all work is actually complete.

**Symptoms** (Issue #1962):

- Session with 767+ messages gets compacted
- Power-steering analyzes only ~50 messages from compacted summary
- Reports "session was truncated at message 50 out of 767 total messages"
- 18/19 checks fail even though PR exists, CI passes, local tests pass

### Root Cause

When Claude Code compacts a session (due to context window limits), the `transcript_path` provided to hooks contains only the compacted summary, not the full conversation history. The pre-compact hook DOES save the full transcript before compaction, but the power-steering checker didn't know to look for it.

**Flow before fix**:

1. Session reaches context limit → Claude Code compacts → Calls pre-compact hook
2. Pre-compact hook saves full transcript to `session_dir/CONVERSATION_TRANSCRIPT.md`
3. User attempts to stop session → stop hook called
4. Power-steering receives `transcript_path` pointing to compacted transcript (~50 messages)
5. SDK analysis sees only "Phase 1: Scope Definition" → Reports work incomplete

### Solution

Two-part fix following the issue's Option A and Option B:

**Part 1: Pre-Compaction Transcript Retrieval**

New method `_get_pre_compaction_transcript(session_id)` checks for compaction by looking for `compaction_events.json` in the session's log directory. If found, returns path to the preserved full transcript.

```rust
// In check() method:
let pre_compaction_path = self.get_pre_compaction_transcript(session_id);
let transcript = if let Some(path) = pre_compaction_path {
    // Session was compacted - load the full transcript
    self.load_pre_compaction_transcript(&path)
} else {
    // Normal case - use provided transcript
    self.load_transcript(transcript_path)
};
```

**Part 2: State-Based Verification Fallback**

New method `_verify_actual_state(session_id)` checks real git/GitHub state when compaction detected:

- CI status via `gh pr view --json statusCheckRollup`
- PR mergeability via `gh pr view --json mergeable`
- Branch currency via `git rev-list --count HEAD..origin/main`

When state verification passes (PR mergeable + CI passing), it can override transcript-based blockers that might be unreliable due to compaction.

### Key Files Changed

- `crates/amplihack-hooks/src/power_steering/`:
  - Added `_get_pre_compaction_transcript()` method
  - Added `_load_pre_compaction_transcript()` method (handles both markdown and JSONL)
  - Added `_verify_actual_state()` method
  - Modified `check()` to use pre-compaction transcript when available
  - Added state-based override logic for post-compaction scenarios

### Testing

- 5 new unit tests in `TestPreCompactionTranscript` class
- Tests cover: no compaction case, compaction detection, markdown parsing, JSONL parsing, state verification
- All 30 tests pass

### Lessons Learned

1. **Hooks should be aware of session compaction**: The transcript provided after compaction may not represent the full session
2. **State is more reliable than transcript analysis**: When transcript might be incomplete, check actual system state (CI, PR, git)
3. **Pre-compact hook saves valuable context**: The full transcript IS preserved - other hooks just need to know where to find it

---

