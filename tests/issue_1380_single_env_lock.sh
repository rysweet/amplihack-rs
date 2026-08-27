#!/usr/bin/env bash
# Issue #1380: one environment lock per crate, and it lives in test_support.
#
# The process environment is global and `cargo test` runs a crate's unit tests
# as threads in ONE process. A mutex declared in some other module serialises
# only the tests that happen to reference it -- tests in sibling modules mutate
# the same variables concurrently. Six such mutexes existed in amplihack-cli;
# each was correct alone and useless against the other five, and the resulting
# flake (#1369) took three diagnosis attempts because the failure always
# surfaced somewhere other than the racing test.
#
# So: a crate's env lock must be `<crate>/src/test_support.rs::env_lock()`, and
# nothing else in `<crate>/src/` may declare one.
#
# Integration tests (`<crate>/tests/*.rs`) are deliberately NOT covered: cargo
# builds each of those files as its own binary, so a file-local mutex there
# really does serialise everything in its process.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

violations=0

report() {
  printf '  %s\n' "$1" >&2
  violations=$((violations + 1))
}

# Every .rs file under a crate's src/, except that crate's test_support module.
mapfile -t files < <(
  find crates -type d -name src -print0 |
    xargs -0 -I{} find {} -name '*.rs' -type f |
    grep -v '/src/test_support\.rs$' |
    sort
)

for file in "${files[@]}"; do
  # Rule 1: a static whose name ends in ENV_LOCK is an environment lock by any
  # other name.
  while IFS=: read -r line _; do
    [ -n "$line" ] || continue
    report "$file:$line: declares a private env mutex (static *ENV_LOCK). Use crate::test_support::env_lock()."
  done < <(grep -nE '^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?static[[:space:]]+[A-Z0-9_]*ENV_LOCK\b' "$file" | cut -d: -f1 | sed 's/$/:/')

  # Rule 2: a function named *env_lock that builds its own mutex. A thin
  # wrapper that forwards to test_support::env_lock() is fine and is exactly
  # how a module gives its lock a local name, so only self-constructed mutexes
  # are rejected.
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    if sed -n "${line},$((line + 5))p" "$file" | grep -q 'Mutex::new('; then
      report "$file:$line: defines its own env_lock() mutex. Use crate::test_support::env_lock()."
    fi
  done < <(grep -nE '^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?fn[[:space:]]+[a-z0-9_]*env_lock[[:space:]]*\(' "$file" | cut -d: -f1)
done

if [ "$violations" -gt 0 ]; then
  echo "" >&2
  echo "FAIL: $violations private environment lock(s) found (issue #1380)." >&2
  echo "The process environment is shared by every test thread in a crate's test" >&2
  echo "binary, so a second mutex does not serialise anything -- it just hides the" >&2
  echo "race behind a lock that looks correct. Route the test through" >&2
  echo "crate::test_support::env_lock(), acquired before the first mutation and" >&2
  echo "held until after the last one (cleanup included)." >&2
  exit 1
fi

echo "PASS: every crate's environment lock lives in its test_support module (#1380)."
