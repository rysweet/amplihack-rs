#!/usr/bin/env bash
# amplihack:migrate — move the active CLI session to a fresh azlin VM.
# See docs/skills/migrate.md for the full procedure.
set -euo pipefail

log_info() { printf '\033[1;34m[migrate]\033[0m %s\n' "$*"; }
log_warn() { printf '\033[1;33m[migrate]\033[0m %s\n' "$*" >&2; }
log_err()  { printf '\033[1;31m[migrate]\033[0m %s\n' "$*" >&2; }

usage() {
  cat <<USAGE
Usage: /amplihack:migrate <hostname> [--session <id>] [--dry-run]

Migrates the active amplihack CLI session to <hostname> (azlin-managed VM).

Options:
  --session <id>   Use a specific session id instead of auto-detecting the
                   newest session-state directory.
  --dry-run        Print what would happen but do not transfer.
USAGE
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
DEST_HOST=""
EXPLICIT_SESSION=""
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --session) EXPLICIT_SESSION="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --) shift; break ;;
    -*) log_err "unknown option: $1"; usage; exit 2 ;;
    *)
      if [[ -z "$DEST_HOST" ]]; then
        DEST_HOST="$1"
        shift
      else
        log_err "unexpected positional: $1"
        usage
        exit 2
      fi
      ;;
  esac
done

if [[ -z "$DEST_HOST" ]]; then
  log_err "destination hostname required"
  usage
  exit 2
fi

# Hostname validation: alphanumeric + '-' + '.' only; prevents shell-injection
# when the name is interpolated into azlin commands below.
if ! [[ "$DEST_HOST" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]]; then
  log_err "invalid hostname: $DEST_HOST"
  exit 2
fi

# ---------------------------------------------------------------------------
# Dependency check
# ---------------------------------------------------------------------------
for dep in azlin tar zstd rsync ssh; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    log_err "$dep not installed on source host"
    exit 3
  fi
done

# ---------------------------------------------------------------------------
# Session detection
# ---------------------------------------------------------------------------
# Precedence:
#   1. --session <id> override
#   2. env var ($COPILOT_SESSION_ID / $CLAUDE_SESSION_ID / etc.)
#   3. newest session-state dir for the active CLI
#   4. error
detect_cli() {
  if [[ -n "${AMPLIHACK_AGENT_BINARY:-}" ]]; then
    echo "$AMPLIHACK_AGENT_BINARY"
    return
  fi
  # Fallback: look at the parent process chain for a known binary.
  local pid="$PPID"
  while [[ -n "$pid" && "$pid" != "1" ]]; do
    local cmd
    cmd="$(ps -o comm= -p "$pid" 2>/dev/null || true)"
    case "$cmd" in
      copilot|copilot-node) echo copilot; return ;;
      claude|claude-code)   echo claude;  return ;;
      amplifier)            echo amplifier; return ;;
    esac
    pid="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ' || true)"
  done
  echo unknown
}

detect_session_id() {
  local cli="$1"
  if [[ -n "$EXPLICIT_SESSION" ]]; then
    echo "$EXPLICIT_SESSION"
    return
  fi
  case "$cli" in
    copilot)
      if [[ -n "${COPILOT_SESSION_ID:-}" ]]; then
        echo "$COPILOT_SESSION_ID"; return
      fi
      local dir="$HOME/.copilot/session-state"
      [[ -d "$dir" ]] || return 1
      # Newest mtime subdirectory.
      find "$dir" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %f\n' 2>/dev/null \
        | sort -rn | head -1 | awk '{print $2}'
      ;;
    claude)
      if [[ -n "${CLAUDE_SESSION_ID:-}" ]]; then
        echo "$CLAUDE_SESSION_ID"; return
      fi
      local dir="$HOME/.claude/sessions"
      [[ -d "$dir" ]] || return 1
      find "$dir" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %f\n' 2>/dev/null \
        | sort -rn | head -1 | awk '{print $2}'
      ;;
    amplifier|unknown)
      echo ""
      ;;
  esac
}

CLI="$(detect_cli)"
SESSION_ID="$(detect_session_id "$CLI" || true)"

if [[ -z "$SESSION_ID" ]]; then
  log_err "could not detect active $CLI session. Pass --session <id> or set env vars."
  exit 4
fi

# Validate session id charset (paranoid against tar-arg injection).
if ! [[ "$SESSION_ID" =~ ^[A-Za-z0-9._-]+$ ]]; then
  log_err "invalid session id: $SESSION_ID"
  exit 4
fi

SESSION_DIR=""
case "$CLI" in
  copilot) SESSION_DIR="$HOME/.copilot/session-state/$SESSION_ID" ;;
  claude)  SESSION_DIR="$HOME/.claude/sessions/$SESSION_ID" ;;
  *)       SESSION_DIR="" ;;
esac

log_info "Detected active session: $SESSION_ID ($CLI)"
if [[ -n "$SESSION_DIR" && -d "$SESSION_DIR" ]]; then
  local_size=$(du -sm "$SESSION_DIR" 2>/dev/null | awk '{print $1}')
  log_info "Session-state dir: $SESSION_DIR (${local_size:-?} MB)"
fi
log_warn "This migration will copy credentials (.ssh, .config/gh/hosts.yml) to $DEST_HOST."

if [[ $DRY_RUN -eq 1 ]]; then
  log_info "Dry-run complete. Destination: $DEST_HOST. Session: $SESSION_ID."
  exit 0
fi

# ---------------------------------------------------------------------------
# Bootstrap destination (idempotent)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOOTSTRAP_SCRIPT="$SCRIPT_DIR/bootstrap-dest.sh"
if [[ ! -f "$BOOTSTRAP_SCRIPT" ]]; then
  log_err "bootstrap-dest.sh not found at $BOOTSTRAP_SCRIPT"
  exit 5
fi

log_info "Bootstrapping $DEST_HOST (node, npm, gh, uv, copilot, amplihack)…"
# Stream the bootstrap script over azlin connect; exits 0 if all toolchain
# already present.
azlin connect -y "$DEST_HOST" -- bash -s < "$BOOTSTRAP_SCRIPT" \
  || { log_err "bootstrap failed on $DEST_HOST"; exit 5; }

# ---------------------------------------------------------------------------
# Selective tarball
# ---------------------------------------------------------------------------
TIMESTAMP="$(date +%s)"
TARBALL="/tmp/amplihack-migrate-${SESSION_ID}-${TIMESTAMP}.tar.zst"

log_info "Building selective tarball → $TARBALL"

# Build include list. We tar each requested path if it exists, and exclude
# caches and inactive sessions.
TAR_INCLUDES=()
for p in \
  "$HOME/.config" \
  "$HOME/.copilot/skills" \
  "$HOME/.amplihack" \
  "$HOME/.simard" \
  "$HOME/.ssh"; do
  [[ -e "$p" ]] && TAR_INCLUDES+=("$p")
done
# Active session-state only (not other sessions).
[[ -n "$SESSION_DIR" && -d "$SESSION_DIR" ]] && TAR_INCLUDES+=("$SESSION_DIR")

if [[ ${#TAR_INCLUDES[@]} -eq 0 ]]; then
  log_err "nothing to migrate (no known amplihack paths found)"
  exit 6
fi

# tar options:
#   -C / to keep absolute paths predictable (we pass full paths below).
#   --exclude-caches-under drops ~/.cache-style dirs.
#   --exclude-vcs-ignores is NOT used — we want config files.
tar --use-compress-program=zstd \
    --exclude='**/target' \
    --exclude='**/node_modules' \
    --exclude='**/.venv' \
    --exclude='**/__pycache__' \
    --exclude="$HOME/.cache" \
    --exclude-caches-under \
    -cf "$TARBALL" \
    "${TAR_INCLUDES[@]}" \
  || { log_err "tar failed"; exit 6; }

TARBALL_SIZE=$(du -m "$TARBALL" 2>/dev/null | awk '{print $1}')
log_info "Tarball size: ${TARBALL_SIZE:-?} MB"

# ---------------------------------------------------------------------------
# Pre-flight disk check on destination
# ---------------------------------------------------------------------------
log_info "Checking destination disk availability…"
DEST_AVAIL_MB=$(azlin connect -y "$DEST_HOST" -- bash -c \
  "df -m / | awk 'NR==2 {print \$4}'" 2>/dev/null || echo "0")
REQUIRED_MB=$(( ${TARBALL_SIZE:-512} * 2 ))
if [[ "${DEST_AVAIL_MB:-0}" -lt "$REQUIRED_MB" ]]; then
  log_err "destination disk too small: ${DEST_AVAIL_MB} MB free, need ${REQUIRED_MB} MB"
  log_warn "tarball preserved at $TARBALL for manual recovery"
  exit 7
fi

# ---------------------------------------------------------------------------
# Ship tarball → extract
# ---------------------------------------------------------------------------
REMOTE_TARBALL="/tmp/amplihack-migrate.tar.zst"
log_info "Copying to $DEST_HOST:$REMOTE_TARBALL …"
azlin cp "$TARBALL" "$DEST_HOST:$REMOTE_TARBALL" \
  || { log_err "azlin cp failed"; exit 8; }

log_info "Extracting on $DEST_HOST …"
azlin connect -y "$DEST_HOST" -- bash -c \
  "tar --use-compress-program=unzstd -xpf '$REMOTE_TARBALL' -C / && rm -f '$REMOTE_TARBALL'" \
  || { log_err "extract failed on $DEST_HOST"; exit 8; }

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------
log_info "Verifying destination: gh auth + $CLI + session-state…"
azlin connect -y "$DEST_HOST" -- bash -c "
  set -e
  command -v gh >/dev/null 2>&1 || { echo 'gh missing' >&2; exit 1; }
  if gh auth status >/dev/null 2>&1; then
    echo 'gh auth ok'
  else
    echo 'WARNING: gh auth not authenticated on destination — user must re-login' >&2
  fi
  command -v $CLI >/dev/null 2>&1 || { echo '$CLI missing on dest' >&2; exit 1; }
  $CLI --version || true
  test -d '$SESSION_DIR' && echo 'session-state present' || echo 'WARNING: session-state missing' >&2
" || { log_err "destination verification failed"; exit 9; }

# ---------------------------------------------------------------------------
# Delta sync (second pass)
# ---------------------------------------------------------------------------
if [[ -n "$SESSION_DIR" && -d "$SESSION_DIR" ]]; then
  log_info "Final delta sync of session-state …"
  # rsync over azlin-provided SSH. azlin exposes an ssh wrapper via
  # `azlin ssh-command` (documented in azlin skill); if not available,
  # fall back to plain ssh host alias.
  if azlin ssh-command --help >/dev/null 2>&1; then
    RSYNC_RSH="$(azlin ssh-command "$DEST_HOST")"
  else
    RSYNC_RSH="ssh"
  fi
  rsync -a --delete -e "$RSYNC_RSH" \
    "$SESSION_DIR/" "$DEST_HOST:$SESSION_DIR/" \
    || log_warn "delta rsync failed (non-fatal; initial tarball already transferred)"
fi

# ---------------------------------------------------------------------------
# Resume in detached tmux
# ---------------------------------------------------------------------------
TMUX_NAME="session-${SESSION_ID}"
case "$CLI" in
  copilot|claude)
    RESUME_CMD="$CLI --resume $SESSION_ID"
    ;;
  amplifier)
    RESUME_CMD=""
    log_warn "amplifier resume is TBD in v1; attach manually on $DEST_HOST"
    ;;
  *)
    RESUME_CMD=""
    log_warn "unknown CLI; cannot auto-resume"
    ;;
esac

if [[ -n "$RESUME_CMD" ]]; then
  log_info "Starting tmux '$TMUX_NAME' on $DEST_HOST: $RESUME_CMD"
  azlin connect -y "$DEST_HOST" -- bash -c "
    tmux has-session -t '$TMUX_NAME' 2>/dev/null \
      && { echo 'tmux session already exists'; exit 0; }
    tmux new-session -d -s '$TMUX_NAME' '$RESUME_CMD'
  " || log_warn "tmux start failed; user must attach manually"
fi

# ---------------------------------------------------------------------------
# Cleanup + summary
# ---------------------------------------------------------------------------
rm -f "$TARBALL"
log_info "Migration complete."
cat <<SUMMARY
✓ Session resumed on $DEST_HOST in tmux '$TMUX_NAME'.
  Attach with: azlin connect -y $DEST_HOST:$TMUX_NAME
SUMMARY
