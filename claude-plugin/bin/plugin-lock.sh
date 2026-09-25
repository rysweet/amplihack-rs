# Sourced by bootstrap and install-runtime: the one install lock they share.
#
# The lock is a directory, $1, created with mkdir (atomic), holding the
# installer's pid. When the evidence about a holder is missing, these helpers
# keep the lock rather than guess: reclaiming a live installer's lock starts a
# second install, which is worse than waiting for a stale lock to age out.

# lock_holder_alive <lockdir>: succeeds while an installer may still hold it.
lock_holder_alive() {
  _pid=$(cat "$1/pid" 2>/dev/null)
  if [ -z "$_pid" ]; then
    # Just created; its installer may not have written its pid yet.
    [ -z "$(find "$1" -maxdepth 0 -mmin +10 2>/dev/null)" ]
    return
  fi
  kill -0 "$_pid" 2>/dev/null || return 1
  # The pid is alive, but after a SIGKILL or a reboot it can belong to an
  # unrelated process. Trust it only if ps shows an installer; if ps cannot
  # tell (no procps, BusyBox ps without -p), keep the lock until it is old.
  if _args=$(ps -o args= -p "$_pid" 2>/dev/null) && [ -n "$_args" ]; then
    case "$_args" in *install-runtime*) return 0 ;; *) return 1 ;; esac
  fi
  [ -z "$(find "$1" -maxdepth 0 -mmin +180 2>/dev/null)" ]
}

# lock_acquire <lockdir>: take the lock, reclaiming it only from a dead holder.
# Reclaims are serialised by a second mkdir mutex, $1.reclaim, and the holder is
# re-checked inside it, so two processes can never both reclaim, and neither
# can remove a lock another has just taken. A reclaim mutex left by a crashed
# process is itself cleared after ten minutes.
lock_acquire() {
  mkdir "$1" 2>/dev/null && return 0
  lock_holder_alive "$1" && return 1
  if ! mkdir "$1.reclaim" 2>/dev/null; then
    [ -n "$(find "$1.reclaim" -maxdepth 0 -mmin +10 2>/dev/null)" ] || return 1
    rm -rf "$1.reclaim"
    mkdir "$1.reclaim" 2>/dev/null || return 1
  fi
  _got=1
  if ! lock_holder_alive "$1"; then
    rm -rf "$1"
    mkdir "$1" 2>/dev/null && _got=0
  fi
  rmdir "$1.reclaim" 2>/dev/null
  return "$_got"
}
