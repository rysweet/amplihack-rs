#!/usr/bin/env bash
# amplihack:migrate — move the active CLI session to a fresh azlin VM.
# See docs/skills/migrate.md for the full procedure.
set -euo pipefail

log_info() { printf '\033[1;34m[migrate]\033[0m %s\n' "$*"; }
log_warn() { printf '\033[1;33m[migrate]\033[0m %s\n' "$*" >&2; }
log_err()  { printf '\033[1;31m[migrate]\033[0m %s\n' "$*" >&2; }

# ===========================================================================
# Project reconstruction library (issue #909)
#
# Pure, sourceable helpers used by the reconstruction phase. They carry no
# side effects beyond reading/rewriting the passed file, so they can be unit
# tested off-host (see scripts/tests/test_workspace_rewrite.bats) by sourcing
# this script with AMPLIHACK_MIGRATE_LIB=1.
#
# The resumed session on the destination VM must land in a valid git checkout,
# not $HOME. These helpers parse the session's workspace.yaml, validate the
# untrusted fields (reject-don't-sanitize), remap the recorded cross-user
# cwd/git_root under the destination $HOME, and rewrite workspace.yaml.
# ===========================================================================

# migrate_yaml_field <file> <key>
# Extract a flat top-level scalar from a workspace.yaml using grep/sed only
# (no yq dependency). Strips surrounding quotes and trailing whitespace.
# Absence of the key is NOT an error — it prints an empty string and returns 0.
migrate_yaml_field() {
  local file="$1" key="$2" line val
  [[ -f "$file" ]] || { printf '\n'; return 0; }
  # Exact top-level key match: start-of-line, key, colon. This avoids matching
  # substrings such as git_root when asked for root, or root_cause.
  line="$(grep -E "^${key}:" "$file" 2>/dev/null | head -1 || true)"
  [[ -n "$line" ]] || { printf '\n'; return 0; }
  val="${line#*:}"                        # strip up to first colon
  val="${val#"${val%%[![:space:]]*}"}"    # ltrim
  val="${val%"${val##*[![:space:]]}"}"    # rtrim
  if [[ ${#val} -ge 2 && "$val" == \"*\" ]]; then
    val="${val#\"}"; val="${val%\"}"
  elif [[ ${#val} -ge 2 && "$val" == \'*\' ]]; then
    val="${val#\'}"; val="${val%\'}"
  fi
  printf '%s\n' "$val"
}

# migrate_validate_repository <value>
# Accept only owner/name; reject empties, newlines, extra slashes, leading '-'
# (git/gh option-injection), and shell metacharacters.
migrate_validate_repository() {
  local v="$1"
  [[ -n "$v" ]] || return 1
  [[ "$v" == *$'\n'* ]] && return 1
  [[ "$v" =~ ^[A-Za-z0-9_.][A-Za-z0-9._-]*/[A-Za-z0-9_.][A-Za-z0-9._-]*$ ]] || return 1
  return 0
}

# migrate_validate_branch <value>
# Reject empties, newlines, leading '-' (git-option injection), and '..'
# (path traversal). Otherwise allow the git ref charset incl. slashes.
migrate_validate_branch() {
  local v="$1"
  [[ -n "$v" ]] || return 1
  [[ "$v" == *$'\n'* ]] && return 1
  [[ "$v" == -* ]] && return 1
  [[ "$v" == *".."* ]] && return 1
  [[ "$v" =~ ^[A-Za-z0-9._/-]+$ ]] || return 1
  return 0
}

# migrate_validate_host_type <value>
# v1 can only clone github remotes.
migrate_validate_host_type() {
  [[ "$1" == "github" ]] || return 1
  return 0
}

# migrate_remap_git_root <repository>
# Destination main-clone path, re-derived entirely from the validated repo
# name (never the untrusted source path): $HOME/src/<repo>.
migrate_remap_git_root() {
  local repository="$1" repo_name
  repo_name="${repository##*/}"
  printf '%s\n' "$HOME/src/$repo_name"
}

# migrate_remap_cwd <source_cwd> <repository> <branch>
# Plain session -> $HOME/src/<repo>. Worktree session (source cwd contains a
# /worktrees/ segment) -> $HOME/src/<repo>/worktrees/<branch>. The tail is
# re-derived from the validated branch, so double-nested source worktrees are
# normalized to a single level.
migrate_remap_cwd() {
  local source_cwd="$1" repository="$2" branch="$3" repo_name root
  repo_name="${repository##*/}"
  root="$HOME/src/$repo_name"
  if [[ "$source_cwd" == *"/worktrees/"* ]]; then
    printf '%s\n' "$root/worktrees/$branch"
  else
    printf '%s\n' "$root"
  fi
}

# migrate_assert_under_home <path>
# Succeeds only when <path> is strictly under $HOME with no '..' traversal.
migrate_assert_under_home() {
  local path="$1"
  [[ -n "$path" ]] || return 1
  case "$path" in
    ".."|"../"*|*"/../"*|*"/..") return 1 ;;
  esac
  [[ "$path" == "$HOME/"* ]] || return 1
  return 0
}

# migrate_should_reconstruct <repository> <git_root> <host_type>
# Gate for the skip-vs-reconstruct decision. Returns 0 (reconstruct) only when
# the session claims a github repository with a recorded git_root. A cleanly
# absent repository/git_root or a non-github host_type is a skip (return 1),
# NOT a hard failure.
migrate_should_reconstruct() {
  local repository="$1" git_root="$2" host_type="$3"
  [[ -n "$repository" ]] || return 1
  [[ -n "$git_root" ]] || return 1
  [[ "$host_type" == "github" ]] || return 1
  return 0
}

# migrate_rewrite_workspace_yaml <file> <new_cwd> <new_git_root>
# Atomically rewrite the cwd and git_root scalars, preserving every other
# line and the original file mode. Replacement values are inserted literally
# (no sed), so sed metacharacters such as & and \ are safe.
migrate_rewrite_workspace_yaml() {
  local file="$1" new_cwd="$2" new_git_root="$3" tmp line
  [[ -f "$file" ]] || { log_err "workspace.yaml not found: $file"; return 1; }
  tmp="$(mktemp)" || { log_err "mktemp failed"; return 1; }
  while IFS= read -r line || [[ -n "$line" ]]; do
    case "$line" in
      cwd:*)      printf 'cwd: %s\n' "$new_cwd" ;;
      git_root:*) printf 'git_root: %s\n' "$new_git_root" ;;
      *)          printf '%s\n' "$line" ;;
    esac
  done <"$file" >"$tmp"
  # Copy contents back so the original inode + mode are preserved.
  cat "$tmp" >"$file"
  rm -f "$tmp"
}

# Library-mode short-circuit: when sourced for unit testing, define the helper
# functions above and return WITHOUT parsing args or running the migration.
if [[ -n "${AMPLIHACK_MIGRATE_LIB:-}" ]]; then
  # 'return' works when sourced (the tested path); the 'exit' fallback only
  # applies if the file is executed directly with the flag set.
  # shellcheck disable=SC2317
  return 0 2>/dev/null || exit 0
fi

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
  # Resolution precedence (matches Rust resolver, issue #489):
  #   1. AMPLIHACK_AGENT_BINARY env var (allowlist-validated)
  #   2. .claude/runtime/launcher_context.json walked up from cwd
  #   3. parent process chain for a known binary
  #   4. default: copilot
  local allowed_re='^(amplifier|claude|codex|copilot)$'
  if [[ -n "${AMPLIHACK_AGENT_BINARY:-}" ]]; then
    local override
    override="$(printf '%s' "${AMPLIHACK_AGENT_BINARY}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
    if [[ "$override" =~ $allowed_re ]]; then
      echo "$override"
      return
    fi
  fi
  local cur="$PWD"
  local hops=0
  while [[ -n "$cur" && "$cur" != "/" && $hops -lt 32 ]]; do
    local ctx="$cur/.claude/runtime/launcher_context.json"
    if [[ -f "$ctx" ]]; then
      local parsed
      parsed="$(jq -r '.launcher // empty' "$ctx" 2>/dev/null | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]' || true)"
      if [[ "$parsed" =~ ^(amplifier|claude|codex|copilot)$ ]]; then
        echo "$parsed"
        return
      fi
      break
    fi
    cur="$(dirname "$cur")"
    hops=$((hops + 1))
  done
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
  echo copilot
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
  if [[ -n "$SESSION_DIR" && -f "$SESSION_DIR/workspace.yaml" ]]; then
    dr_repo="$(migrate_yaml_field "$SESSION_DIR/workspace.yaml" repository)"
    dr_git_root="$(migrate_yaml_field "$SESSION_DIR/workspace.yaml" git_root)"
    dr_host="$(migrate_yaml_field "$SESSION_DIR/workspace.yaml" host_type)"
    if migrate_should_reconstruct "$dr_repo" "$dr_git_root" "$dr_host"; then
      dr_repo_name="${dr_repo##*/}"
      log_info "Would reconstruct project: clone $dr_repo → \$HOME/src/$dr_repo_name and rewrite cwd/git_root."
    else
      log_info "Would skip project reconstruction (no clonable github repository); resume in \$HOME."
    fi
  fi
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
# Project reconstruction & workspace.yaml rewrite (issue #909)
# ---------------------------------------------------------------------------
# The ~/src/* project trees are NOT shipped in the tarball, so the resumed
# session would otherwise land in $HOME: the cwd/git_root recorded in the
# session's workspace.yaml point at the SOURCE user's home, a path that does
# not exist on the destination. Re-materialize the git checkout on the
# destination and rewrite the persisted paths so resume lands in real context.
#
# Ordering is load-bearing: this runs AFTER the final delta rsync (so rsync
# cannot clobber the rewritten workspace.yaml) and BEFORE the tmux resume (so
# the CLI reads the corrected paths).
if [[ -n "$SESSION_DIR" ]]; then
  WORKSPACE_YAML="$SESSION_DIR/workspace.yaml"
  if [[ -f "$WORKSPACE_YAML" ]]; then
    ws_cwd="$(migrate_yaml_field "$WORKSPACE_YAML" cwd)"
    ws_git_root="$(migrate_yaml_field "$WORKSPACE_YAML" git_root)"
    ws_repo="$(migrate_yaml_field "$WORKSPACE_YAML" repository)"
    ws_host="$(migrate_yaml_field "$WORKSPACE_YAML" host_type)"
    ws_branch="$(migrate_yaml_field "$WORKSPACE_YAML" branch)"

    if migrate_should_reconstruct "$ws_repo" "$ws_git_root" "$ws_host"; then
      # Validate untrusted fields (reject-don't-sanitize) BEFORE they reach the
      # destination shell / filesystem / git. Malformed fields on a session that
      # DOES claim a github repo are a hard failure (exit 11).
      migrate_validate_host_type "$ws_host" \
        || { log_err "invalid host_type in workspace.yaml: $ws_host"; exit 11; }
      migrate_validate_repository "$ws_repo" \
        || { log_err "invalid repository in workspace.yaml: $ws_repo"; exit 11; }
      if [[ -n "$ws_branch" ]]; then
        migrate_validate_branch "$ws_branch" \
          || { log_err "invalid branch in workspace.yaml: $ws_branch"; exit 11; }
      else
        ws_branch="main"
      fi

      # Classify worktree vs plain from the source cwd (never reuse the source
      # path verbatim); the remap is re-derived on the destination where $HOME
      # differs from the source user's home.
      RECON_IS_WORKTREE=0
      [[ "$ws_cwd" == *"/worktrees/"* ]] && RECON_IS_WORKTREE=1

      log_info "Reconstructing project tree on $DEST_HOST (repo=$ws_repo branch=$ws_branch worktree=$RECON_IS_WORKTREE)…"

      RECON_STATUS=0
      # Values pass as positional args into a single-quoted heredoc so they can
      # never be interpreted as command text (shell / git-option injection).
      azlin connect -y "$DEST_HOST" -- bash -s -- \
        "$WORKSPACE_YAML" "$ws_repo" "$ws_branch" "$RECON_IS_WORKTREE" <<'RECON' || RECON_STATUS=$?
set -euo pipefail
ws_yaml="$1"; repository="$2"; branch="$3"; is_worktree="$4"

repo_name="${repository##*/}"
git_root_dest="$HOME/src/$repo_name"
if [ "$is_worktree" = "1" ]; then
  cwd_dest="$git_root_dest/worktrees/$branch"
else
  cwd_dest="$git_root_dest"
fi

# $HOME-containment gate (exit 12): destination paths must be strictly under
# $HOME with no traversal.
assert_under_home() {
  local p="$1"
  [ -n "$p" ] || return 1
  case "$p" in
    ".."|"../"*|*"/../"*|*"/..") return 1 ;;
  esac
  case "$p" in "$HOME"/*) return 0 ;; *) return 1 ;; esac
}
assert_under_home "$git_root_dest" || { echo "[migrate] git_root escapes \$HOME: $git_root_dest" >&2; exit 12; }
assert_under_home "$cwd_dest"      || { echo "[migrate] cwd escapes \$HOME: $cwd_dest" >&2; exit 12; }

mkdir -p "$HOME/src"

# Idempotent main clone into git_root_dest (exit 13 if it cannot be cloned).
if [ -d "$git_root_dest/.git" ]; then
  echo "[migrate] existing checkout at $git_root_dest; fetching"
  git -C "$git_root_dest" fetch --all --prune >/dev/null 2>&1 || true
elif command -v gh >/dev/null 2>&1 && gh repo clone "$repository" "$git_root_dest" >/dev/null 2>&1; then
  echo "[migrate] cloned $repository via gh -> $git_root_dest"
elif git clone "https://github.com/$repository.git" "$git_root_dest" >/dev/null 2>&1; then
  echo "[migrate] cloned $repository via https -> $git_root_dest"
else
  echo "[migrate] clone failed for $repository" >&2
  exit 13
fi

checkout_branch() {
  local dir="$1"
  git -C "$dir" checkout "$branch" >/dev/null 2>&1 && return 0
  git -C "$dir" fetch origin "$branch" >/dev/null 2>&1 || return 1
  git -C "$dir" checkout "$branch" >/dev/null 2>&1
}

if [ "$is_worktree" = "1" ]; then
  if [ ! -e "$cwd_dest/.git" ]; then
    # Best-effort worktree linkage; cwd_dest must still end a real checkout.
    if ! git -C "$git_root_dest" worktree add "$cwd_dest" "$branch" >/dev/null 2>&1; then
      git -C "$git_root_dest" fetch origin "$branch" >/dev/null 2>&1 || true
      if ! git -C "$git_root_dest" worktree add "$cwd_dest" "$branch" >/dev/null 2>&1; then
        echo "[migrate] worktree add failed; falling back to standalone clone at $cwd_dest" >&2
        if [ ! -d "$cwd_dest/.git" ]; then
          if command -v gh >/dev/null 2>&1 && gh repo clone "$repository" "$cwd_dest" >/dev/null 2>&1; then
            :
          else
            git clone "https://github.com/$repository.git" "$cwd_dest" >/dev/null 2>&1 || true
          fi
        fi
        checkout_branch "$cwd_dest" || true
      fi
    fi
  fi
else
  checkout_branch "$git_root_dest" || true
fi

# Atomic, mode-preserving rewrite of cwd + git_root (literal, no sed).
if [ -f "$ws_yaml" ]; then
  tmp="$(mktemp)"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      cwd:*)      printf 'cwd: %s\n' "$cwd_dest" ;;
      git_root:*) printf 'git_root: %s\n' "$git_root_dest" ;;
      *)          printf '%s\n' "$line" ;;
    esac
  done <"$ws_yaml" >"$tmp"
  cat "$tmp" >"$ws_yaml"
  rm -f "$tmp"
  echo "[migrate] rewrote cwd + git_root in $ws_yaml … ok"
else
  echo "[migrate] workspace.yaml missing on destination: $ws_yaml" >&2
fi

# Hard-gate (exit 10): refuse a hollow resume into $HOME.
if [ ! -d "$cwd_dest" ]; then
  echo "[migrate] hard-gate: cwd missing or not a directory: $cwd_dest" >&2
  exit 10
fi
cur_branch="$(git -C "$cwd_dest" rev-parse --abbrev-ref HEAD 2>/dev/null || echo '')"
if [ -z "$cur_branch" ]; then
  echo "[migrate] hard-gate: $cwd_dest is not a git checkout" >&2
  exit 10
fi
if [ "$cur_branch" != "$branch" ]; then
  echo "[migrate] hard-gate: $cwd_dest is on '$cur_branch', expected '$branch'" >&2
  exit 10
fi
echo "[migrate] reconstruction verified: $cwd_dest on $branch"
RECON
      case "$RECON_STATUS" in
        0)  log_info "Project reconstruction complete on $DEST_HOST." ;;
        10) log_err "reconstruction hard-gate failed (cwd missing / not a checkout / wrong branch)"; exit 10 ;;
        11) log_err "workspace.yaml field validation failed on destination"; exit 11 ;;
        12) log_err "cross-user path remap escaped \$HOME on destination"; exit 12 ;;
        13) log_err "project reconstruction (clone) failed for $ws_repo"; exit 13 ;;
        *)  log_err "project reconstruction failed on $DEST_HOST (status $RECON_STATUS)"; exit 13 ;;
      esac
    else
      log_warn "No reconstructable git project (repository=${ws_repo:-none} host_type=${ws_host:-none}); resume will land in \$HOME."
    fi
  else
    log_warn "workspace.yaml not found at $WORKSPACE_YAML; skipping project reconstruction."
  fi
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
