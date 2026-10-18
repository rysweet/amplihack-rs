#!/usr/bin/env bash
# Helpers for the sccache-restore and sccache-save composite actions.
#
# The Actions cache service rate-limits the whole repository. A restore that
# gets HTTP 429 reports "Cache not found", and a save that gets one reports
# "Unable to reserve cache ... another job may be creating this cache". The
# composite actions retry both with jitter, so jobs that were refused together
# do not retry together, and use this script to decide whether to retry.
#
# Needs GH_TOKEN with actions: read, and GITHUB_REPOSITORY / GITHUB_OUTPUT.
#
#   sccache-cache.sh restore-gap <key-prefix> <attempt> <max-attempts>
#   sccache-cache.sh save-check  <key> <attempt> <max-attempts>
#
# Both write retry=true|false to GITHUB_OUTPUT and never fail the step.
set -uo pipefail

jitter() {
  local s=$((15 + RANDOM % 45))
  echo "Waiting ${s}s before the next attempt"
  sleep "$s"
}

# Number of cache entries whose key starts with $1, on any ref; empty if the
# REST call failed. The REST API has its own limit, separate from the cache
# service that is refusing us.
count_entries() {
  gh api "repos/${GITHUB_REPOSITORY}/actions/caches?key=$1&per_page=100" \
    --jq '.total_count' 2>/dev/null
}

# 1 if an entry with exactly key $1 exists, 0 if not; empty if the call failed.
exact_entry() {
  gh api "repos/${GITHUB_REPOSITORY}/actions/caches?key=$1&per_page=100" \
    --jq "[.actions_caches[] | select(.key == \"$1\")] | length" 2>/dev/null
}

cmd=${1:-}
case "$cmd" in
  restore-gap)
    prefix=$2 attempt=$3 max=$4
    n=$(count_entries "$prefix")
    if [ "$n" = "0" ]; then
      echo "No cache entry starts with ${prefix}; nothing to restore, this job builds cold."
      echo "retry=false" >> "$GITHUB_OUTPUT"
    elif [ "$attempt" -ge "$max" ]; then
      echo "::warning title=sccache cache not restored::Restore of ${prefix}* failed ${max} times although entries exist (likely the Actions cache rate limit, HTTP 429). This job is compiling cold."
      echo "retry=false" >> "$GITHUB_OUTPUT"
    else
      echo "Restore attempt ${attempt} of ${max} found nothing, but ${n:-unknown number of} entries exist for ${prefix}; likely rate-limited."
      jitter
      echo "retry=true" >> "$GITHUB_OUTPUT"
    fi
    ;;
  save-check)
    key=$2 attempt=$3 max=$4
    n=$(exact_entry "$key")
    if [ "$n" = "1" ]; then
      echo "Saved: ${key}"
      echo "retry=false" >> "$GITHUB_OUTPUT"
    elif [ -z "$n" ]; then
      echo "::warning title=sccache save unverified::Could not list caches to confirm ${key} was saved; not retrying."
      echo "retry=false" >> "$GITHUB_OUTPUT"
    elif [ "$attempt" -ge "$max" ]; then
      echo "::warning title=sccache cache not saved::Save of ${key} failed ${max} times (likely the Actions cache rate limit, reported as \"Unable to reserve cache\"). The next run restores an older entry."
      echo "retry=false" >> "$GITHUB_OUTPUT"
    else
      echo "Save attempt ${attempt} of ${max} did not create ${key}."
      jitter
      echo "retry=true" >> "$GITHUB_OUTPUT"
    fi
    ;;
  *)
    echo "usage: $0 restore-gap <key-prefix> <attempt> <max> | save-check <key> <attempt> <max>" >&2
    exit 2
    ;;
esac
exit 0
