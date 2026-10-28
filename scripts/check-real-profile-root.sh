#!/usr/bin/env bash
# Issue #1699 — notice when a test run changed the user's real profile root.
#
#   scripts/check-real-profile-root.sh snapshot <file>   before the tests
#   scripts/check-real-profile-root.sh compare <file>    after them
#
# Watches every root among $AMPLIHACK_CLAUDE_PROFILE_ROOT, the first line of
# ~/.amplihack/profile-root (relative values ignored), ~/.claude-profiles and
# the default ~/.amplihack/claude-profiles, each resolved and deduplicated.
# Every entry is recorded without following symlinks: regular files by
# SHA-256 and mtime, symlinks by target and mtime, directories by mtime.
#
# It only reports and never changes a root. Output names paths (shell-quoted,
# control characters removed) and the kind of change, never file contents or
# hashes: profile roots hold credentials and CI logs are public.
#
# Exit: 0 clean, 1 a difference, 2 the scan was impossible or a usage error.
set -u
export LC_ALL=C

usage() {
  echo "usage: $0 snapshot|compare <file>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
cmd="$1" file="$2"
case "$cmd" in snapshot | compare) ;; *) usage ;; esac
[ -n "$file" ] || usage

if command -v sha256sum >/dev/null 2>&1; then
  hash_file() { sha256sum <"$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  hash_file() { shasum -a 256 <"$1" | cut -d' ' -f1; }
else
  echo "check-real-profile-root: no sha256sum or shasum; cannot scan" >&2
  exit 2
fi

# Pick the stat flavour once: falling back per call would run BSD syntax on
# GNU stat whenever a file vanished mid-scan.
if stat -c %Y -- / >/dev/null 2>&1; then
  mtime() { stat -c %Y -- "$1" 2>/dev/null; }
else
  mtime() { stat -f %m -- "$1" 2>/dev/null; }
fi

# GNU-style find and sha256sum let one find and one batched hash pass record a
# whole root; otherwise each entry costs several processes. Both paths write
# the same records, so a snapshot and a compare may use either.
fast=0
if [ "${BASH_VERSINFO[0]}" -ge 4 ] &&
  find -P / -maxdepth 0 -printf '' >/dev/null 2>&1 &&
  printf '' | sha256sum -z >/dev/null 2>&1; then
  fast=1
fi

# Fails closed: an existing path that neither tool resolves could hide / or
# HOME behind a symlink or "." component, so the scan stops. A path that does
# not exist cannot be either, and is recorded as given. Callers run this in
# $(...), so each one checks the status itself.
resolve() {
  realpath -m -- "$1" 2>/dev/null && return 0
  readlink -f -- "$1" 2>/dev/null && return 0
  if [ ! -e "$1" ] && [ ! -L "$1" ]; then
    printf '%s\n' "$1"
    return 0
  fi
  printf 'check-real-profile-root: cannot resolve %q\n' "$1" | clean >&2
  return 2
}

clean() { tr -d '\000-\011\013-\037\177'; }

home="${HOME:-}"
case "$home" in
  /*) ;;
  *)
    echo "check-real-profile-root: HOME is unset or relative; cannot scan" >&2
    exit 2
    ;;
esac
home_real="$(resolve "$home")" || exit 2

candidates=()
env_root="${AMPLIHACK_CLAUDE_PROFILE_ROOT:-}"
if [ -n "$env_root" ]; then
  case "$env_root" in
    /*) candidates+=("$env_root") ;;
    *)
      echo "check-real-profile-root: AMPLIHACK_CLAUDE_PROFILE_ROOT is relative; cannot scan" >&2
      exit 2
      ;;
  esac
fi
saved=""
if [ -f "$home/.amplihack/profile-root" ]; then
  IFS= read -r saved <"$home/.amplihack/profile-root"
fi
case "$saved" in /*) candidates+=("$saved") ;; esac
candidates+=("$home/.claude-profiles" "$home/.amplihack/claude-profiles")

roots=()
for c in "${candidates[@]}"; do
  r="$(resolve "$c")" || exit 2
  if [ "$r" = "/" ] || [ "$r" = "$home_real" ]; then
    printf 'check-real-profile-root: refusing to scan %q (it is / or HOME)\n' "$r" | clean >&2
    exit 2
  fi
  seen=0
  for k in "${roots[@]+"${roots[@]}"}"; do
    [ "$k" = "$r" ] && seen=1
  done
  [ "$seen" -eq 1 ] || roots+=("$r")
done

# One tab-separated record per line; %q leaves no tab or newline in a path.
#   R <present|absent> <root>
#   E <path> <file|link|dir> <signature>
scan_fast() {
  local r="$1" line p head target sig kind
  local -A sums=()
  # A file that vanishes or cannot be read before it is hashed gets an empty
  # hash (same as the slow path).
  while IFS= read -r -d '' line; do
    sums["${line#*  }"]="${line%%  *}"
  done < <(find -P "$r" -type f -print0 2>/dev/null | xargs -0 -r sha256sum -z -- 2>/dev/null)
  while IFS= read -r -d '' head && IFS= read -r -d '' target && IFS= read -r -d '' p; do
    case "${head%% *}" in
      l)
        kind="link"
        printf -v sig '%q %s' "$target" "${head#* }"
        ;;
      f)
        kind="file"
        sig="${sums[$p]:-} ${head#* }"
        ;;
      d)
        kind="dir"
        sig="- ${head#* }"
        ;;
      *)
        kind="other"
        sig="- ${head#* }"
        ;;
    esac
    printf 'E\t%q\t%s\t%s\n' "$p" "$kind" "$sig"
  done < <(find -P "$r" -printf '%y %Ts\0%l\0%p\0')
  wait "$!"
}

scan() {
  local r p sig kind status
  for r in "${roots[@]}"; do
    if [ ! -e "$r" ] && [ ! -L "$r" ]; then
      printf 'R\tabsent\t%q\n' "$r"
      continue
    fi
    printf 'R\tpresent\t%q\n' "$r"
    if [ "$fast" -eq 1 ]; then
      scan_fast "$r"
      status=$?
      if [ "$status" -ne 0 ]; then
        printf 'check-real-profile-root: could not walk %q\n' "$r" | clean >&2
        return 2
      fi
      continue
    fi
    while IFS= read -r -d '' p; do
      if [ -L "$p" ]; then
        kind="link"
        sig="$(printf '%q' "$(readlink -- "$p")") $(mtime "$p")"
      elif [ -f "$p" ]; then
        kind="file"
        sig="$(hash_file "$p") $(mtime "$p")"
      elif [ -d "$p" ]; then
        kind="dir"
        sig="- $(mtime "$p")"
      else
        kind="other"
        sig="- $(mtime "$p")"
      fi
      printf 'E\t%q\t%s\t%s\n' "$p" "$kind" "$sig"
    done < <(find -P "$r" -print0)
    wait "$!"
    status=$?
    if [ "$status" -ne 0 ]; then
      printf 'check-real-profile-root: could not walk %q\n' "$r" | clean >&2
      return 2
    fi
  done
}

if [ "$cmd" = snapshot ]; then
  records="$(scan)" || exit 2
  # The snapshot lists every profile path and hashes credential files: keep
  # it private. chmod covers a reused file, whose old mode survives ">".
  umask 077
  if ! printf '%s\n' "$records" >"$file" || ! chmod 600 -- "$file"; then
    echo "check-real-profile-root: cannot write the snapshot file" >&2
    exit 2
  fi
  exit 0
fi

if [ ! -f "$file" ]; then
  echo "check-real-profile-root: no snapshot file to compare against" >&2
  exit 2
fi
records="$(scan)" || exit 2

report="$(
  printf '%s\n' "$records" | awk -F'\t' '
    FNR == NR {
      if ($1 == "R") { old_root[$3] = $2; roots[$3] = 1 }
      if ($1 == "E") { old[$2] = $3 "\t" $4; paths[$2] = 1 }
      next
    }
    {
      if ($1 == "R") { new_root[$3] = $2; roots[$3] = 1 }
      if ($1 == "E") { cur[$2] = $3 "\t" $4; paths[$2] = 1 }
    }
    END {
      for (r in roots) {
        was = (r in old_root) ? old_root[r] : "absent"
        now = (r in new_root) ? new_root[r] : "absent"
        if (was == "absent" && now == "absent") print "note: " r " absent"
        else if (was == "absent") print "created during run: " r
      }
      for (p in paths) {
        if (!(p in cur)) print "removed: " p
        else if (!(p in old)) print "added: " p
        else if (old[p] != cur[p]) { split(cur[p], k, "\t"); print "changed:" k[1] " " p }
      }
    }
  ' "$file" - | sort | clean
)"

[ -n "$report" ] && printf '%s\n' "$report"
while IFS= read -r line; do
  case "$line" in
    "" | "note: "*) ;;
    *) exit 1 ;;
  esac
done <<<"$report"
exit 0
