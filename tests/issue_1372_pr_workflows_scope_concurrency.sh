#!/usr/bin/env bash
# A workflow that runs on pull_request and cancels in-progress runs must scope
# its concurrency group per ref.
#
# docs.yml used the bare group `pages`, copied from GitHub's Pages template.
# That template assumes a deploy-only workflow. docs.yml also builds on every
# pull_request, so all open PRs shared one global group and each new run
# cancelled the others.
#
# The damage is not the wasted build. GitHub reports a cancelled run as a failed
# required check, so the cancelled PRs sat there looking broken and auto-merge
# would not fire — until someone noticed and re-ran the job by hand. With nine
# PRs open it happened continuously, and it cost three separate diagnoses before
# the cause was found, because "failed" and "cancelled by someone else's push"
# look identical on the PR page.
#
# Every other workflow in the repo already scopes per ref. This keeps it that
# way.

set -uo pipefail

DIR=".github/workflows"
[ -d "$DIR" ] || { echo "missing $DIR (run from repo root)"; exit 1; }

fails=0
checked=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

for f in "$DIR"/*.yml "$DIR"/*.yaml; do
  [ -f "$f" ] || continue
  name="$(basename "$f")"

  # Only workflows that both run on pull_request and cancel in-progress runs
  # can cancel a sibling PR's run.
  # pull_request_target runs per-PR too, so it is covered by the same reasoning.
  grep -qE '^  pull_request(_target)?:' "$f" || continue
  block="$(awk '/^concurrency:/{f=1;next} f && /^[^ ]/{exit} f' "$f")"
  [ -n "$block" ] || continue
  grep -qE '^ *cancel-in-progress: *true' <<<"$block" || continue

  # One awk over the whole block: it reads to EOF, where a `head`-terminated
  # stage would stop early and leave `sed` writing into a closed pipe. Under
  # `pipefail` the group then reads as empty and a correctly-scoped workflow is
  # reported as having no group at all (issue #1434).
  group="$(awk 'n == 0 && match($0, /^ *group: */) { print substr($0, RSTART + RLENGTH); n = 1 }' <<<"$block")"
  checked=$((checked + 1))

  if [ -z "$group" ]; then
    fail "$name declares concurrency with cancel-in-progress but no group"
  elif grep -qE 'github\.(ref|head_ref|event\.pull_request\.number)' <<<"$group"; then
    pass "$name scopes its concurrency group per ref"
  else
    fail "$name uses a constant concurrency group ($group) while running on pull_request with cancel-in-progress: true — open PRs will cancel each other, and a cancelled run reads as a failed required check"
  fi
done

if [ "$checked" -eq 0 ]; then
  echo "  FAIL  no pull_request workflow with cancel-in-progress found — this check would pass vacuously"
  exit 1
fi

echo
if [ "$fails" -gt 0 ]; then
  echo "issue-1372: $fails workflow(s) can cancel a sibling PR's required checks"
  exit 1
fi
echo "issue-1372: all $checked pull_request workflow(s) scope concurrency per ref"
