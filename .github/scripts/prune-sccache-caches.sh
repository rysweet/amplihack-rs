#!/usr/bin/env bash
# Delete old sccache disk-cache entries, keeping the newest few per job.
#
#   prune-sccache-caches.sh <ref> <keep>
#
# Every save adds an entry keyed sccache-v1-<os>-<job>-<target>-<run_id>-<attempt>,
# and nothing else removes them before the repository's 10 GB limit forces
# eviction. This lists the entries on <ref> whose key has exactly that shape,
# groups them by everything before <run_id>-<attempt> (so a new job or build
# target needs no list to maintain), prints which it keeps and which it
# deletes, and deletes those literal keys one by one. It never deletes:
#   - the newest <keep> entries of a group,
#   - any entry from this run (GITHUB_RUN_ID),
#   - anything not shaped like an sccache-v1 entry (rust-cache entries, other
#     workflows' entries).
#
# Needs GH_TOKEN with actions: write, and GITHUB_REPOSITORY / GITHUB_RUN_ID.
# Problems are reported as ::warning:: and never fail the step.
set -uo pipefail

ref=${1:?ref} keep=${2:?keep}
case "$keep" in '' | *[!0-9]*) echo "::warning::keep must be a number, got '$keep'"; exit 0 ;; esac
[ "$keep" -ge 1 ] || { echo "::warning::keep must be at least 1"; exit 0; }

# Newest first. The API's key filter is a prefix match; the exact shape is
# checked below.
listed=$(gh api --paginate \
  "repos/${GITHUB_REPOSITORY}/actions/caches?key=sccache-v1-&ref=${ref}&sort=created_at&direction=desc&per_page=100" \
  --jq '.actions_caches[].key') || {
  echo "::warning::Could not list sccache caches on ${ref}; nothing pruned."
  exit 0
}

# "<keep|delete|this-run> <key>" for every well-shaped key, newest first.
plan=$(printf '%s\n' "$listed" | awk -v keep="$keep" -v run="$GITHUB_RUN_ID" '
  /^sccache-v1-.+-[0-9]+-[0-9]+$/ {
    group = $0
    sub(/[0-9]+-[0-9]+$/, "", group)
    tail = substr($0, length(group) + 1)
    split(tail, part, "-")
    if (++seen[group] <= keep) print "keep", $0
    else if (part[1] == run) print "this-run", $0
    else print "delete", $0
  }')

delete=()
while read -r action key; do
  [ -n "${key:-}" ] || continue
  case "$action" in
    keep) echo "keep    ${key}" ;;
    this-run) echo "keep    ${key} (this run)" ;;
    delete) echo "delete  ${key}"; delete+=("$key") ;;
  esac
done <<< "$plan"

for key in ${delete[@]+"${delete[@]}"}; do
  gh api -X DELETE "repos/${GITHUB_REPOSITORY}/actions/caches?key=${key}&ref=${ref}" > /dev/null \
    && echo "deleted ${key}" \
    || echo "::warning::Could not delete cache ${key}"
done
exit 0
