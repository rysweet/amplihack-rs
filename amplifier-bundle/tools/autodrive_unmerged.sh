#!/usr/bin/env bash
# autodrive_unmerged.sh — sourced by autodrive_state.sh, which defines
# autodrive_pr_state; split out to keep autodrive_state.sh under the brick budget.

# autodrive_require_unmerged <pr> <phase> -> 0 when the PR is not merged.
#
# Issue #1551: only autodrive_merge_gate.sh merges, in phase 3. A PR found
# MERGED after phase 1 or 2 was merged around the gh merge guard (an absolute
# gh path, curl with a token), so the run stops with exit 1 rather than report
# progress. A state that cannot be read is read once more, then also stops it:
# "we do not know" is never "not merged".
autodrive_require_unmerged() {
  local pr="${1:-}" phase="${2:-}" state
  case "$pr" in
    ''|*[!0-9]*) echo "ERROR: #1551: no valid PR number to check after ${phase}; refusing to continue unverified." >&2; return 1 ;;
  esac
  state="$(autodrive_pr_state "$pr")"
  [ "$state" = "UNKNOWN" ] && state="$(autodrive_pr_state "$pr")"
  case "$state" in
    MERGED)
      echo "ERROR: #1551 merged outside autodrive_merge_gate.sh: PR #${pr} is already MERGED after ${phase}, before the crusty and merge-ready phases could clear it. Nothing in auto-drive merged it; the run stops here. Review what merged it and the merged change." >&2
      return 1 ;;
    UNKNOWN)
      echo "ERROR: #1551: PR #${pr} state is unreadable after ${phase} (read twice); a merge outside autodrive_merge_gate.sh cannot be ruled out, so the run stops here. Resume it once gh can read the PR." >&2
      return 1 ;;
  esac
  return 0
}
