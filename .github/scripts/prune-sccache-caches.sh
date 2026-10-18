#!/usr/bin/env bash
# Delete old sccache disk-cache entries, keeping the newest few per job.
#
#   prune-sccache-caches.sh <ref> <keep> <key-prefix>...
#
# Every save adds an entry keyed <prefix><run_id>-<run_attempt>, and nothing
# else removes them before the repository's 10 GB limit forces eviction. For
# each prefix this lists the entries on <ref> whose key is exactly
# <prefix><digits>-<digits>, prints which it keeps and which it deletes, and
# deletes those literal keys one by one. It never deletes:
#   - the newest <keep> entries for a prefix,
#   - any entry from this run (GITHUB_RUN_ID),
#   - anything not matching the exact sccache key shape (rust-cache entries,
#     other workflows' entries, other prefixes).
#
# Needs GH_TOKEN with actions: write, and GITHUB_REPOSITORY / GITHUB_RUN_ID.
# Problems are reported as ::warning:: and never fail the step.
set -uo pipefail

ref=${1:?ref} keep=${2:?keep}
shift 2
case "$keep" in '' | *[!0-9]*) echo "::warning::keep must be a number, got '$keep'"; exit 0 ;; esac
[ "$keep" -ge 1 ] || { echo "::warning::keep must be at least 1"; exit 0; }

for prefix in "$@"; do
  case "$prefix" in
    sccache-v1-*-) ;;
    *) echo "::warning::Refusing prefix '$prefix': not an sccache-v1-...- prefix"; continue ;;
  esac

  # Newest first. The API's key filter is a prefix match, so the exact shape is
  # checked below.
  listed=$(gh api --paginate \
    "repos/${GITHUB_REPOSITORY}/actions/caches?key=${prefix}&ref=${ref}&sort=created_at&direction=desc&per_page=100" \
    --jq '.actions_caches[].key') || {
    echo "::warning::Could not list caches for ${prefix}; skipping it."
    continue
  }
  matched=$(printf '%s\n' "$listed" | awk -v p="$prefix" \
    'length($0) > length(p) && index($0, p) == 1 && substr($0, length(p) + 1) ~ /^[0-9]+-[0-9]+$/')

  kept=0
  delete=()
  while IFS= read -r key; do
    [ -n "$key" ] || continue
    if [ "$kept" -lt "$keep" ]; then
      echo "keep    ${key}"
      kept=$((kept + 1))
    elif [ "$key" != "${key#"${prefix}${GITHUB_RUN_ID}-"}" ]; then
      echo "keep    ${key} (this run)"
    else
      echo "delete  ${key}"
      delete+=("$key")
    fi
  done <<< "$matched"

  for key in ${delete[@]+"${delete[@]}"}; do
    gh api -X DELETE "repos/${GITHUB_REPOSITORY}/actions/caches?key=${key}&ref=${ref}" > /dev/null \
      && echo "deleted ${key}" \
      || echo "::warning::Could not delete cache ${key}"
  done
done
exit 0
