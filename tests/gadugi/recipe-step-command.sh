#!/usr/bin/env bash
# Print the `command:` body of one step of a recipe YAML file, byte for byte,
# with no Python and no YAML library.
#
# The gadugi harnesses (run-step-03b.sh, run-merge-validations.sh) run the
# SHIPPED step body rather than a copy of it. They used to read it with
# python3 and PyYAML. auto-drive's criterion 1 runs this repository's gadugi
# scenarios with the real gadugi-test on whatever host drives a PR, and on a
# host without PyYAML both scenarios failed with ModuleNotFoundError: a
# blocker that no change in the PR under review could clear (PR #1520 review).
#
# Usage: recipe-step-command.sh <recipe.yaml> <step-id>
#
# Supported shape, the one every recipe in amplifier-bundle/recipes uses:
#
#   - id: "<step-id>"            (the step's first key; quotes optional)
#     ...
#     command: |                 (a literal block scalar, clip chomping)
#       <body lines>
#
# The body is printed as YAML defines a literal block scalar: the indentation
# of its first non-blank line is removed from every line, blank lines inside
# are kept, trailing blank lines are dropped and one final newline is kept.
# Any other form (`|-`, `|+`, `>`, an indentation indicator, a quoted or plain
# scalar, a missing step or command) is refused with a named ERROR and exit 2,
# never guessed at. tests/integration/auto_drive_to_merge_test.rs checks this
# output against serde_yaml for every step the harnesses read.
set -uo pipefail

RECIPE="${1:-}"
STEP="${2:-}"
if [ -z "$RECIPE" ] || [ -z "$STEP" ]; then
  echo "usage: recipe-step-command.sh <recipe.yaml> <step-id>" >&2
  exit 2
fi
[ -f "$RECIPE" ] || { echo "ERROR: recipe-not-found: $RECIPE" >&2; exit 2; }

LC_ALL=C awk -v step="$STEP" '
  function indent(s) { match(s, /^ */); return RLENGTH }
  function done_body() {
    while (n > 0 && body[n] == "") n--
    for (i = 1; i <= n; i++) print body[i]
    found = 1
    exit
  }
  BEGIN { state = 0; n = 0; ind = -1 }
  # state 0: looking for "- id: <step>".
  state == 0 {
    line = $0
    if (line ~ /^ *- id:[ ]/) {
      v = line; sub(/^ *- id:[ ]+/, "", v); sub(/[ ]+$/, "", v)
      if (v == step || v == "\"" step "\"" || v == "\047" step "\047") {
        keyind = indent(line) + 2; state = 1
      }
    }
    next
  }
  # state 1: inside the step, looking for its own "command:" key.
  state == 1 {
    if ($0 ~ /^ *$/ || $0 ~ /^ *#/) next
    if (indent($0) < keyind) { bad = "step-has-no-command"; exit }
    if (indent($0) == keyind && substr($0, keyind + 1) ~ /^command:/) {
      hdr = substr($0, keyind + 1); sub(/^command:[ ]*/, "", hdr); sub(/[ ]+#.*$/, "", hdr); sub(/[ ]+$/, "", hdr)
      if (hdr != "|") { bad = "command-is-not-a-literal-block"; exit }
      state = 2
    }
    next
  }
  # state 2: the block body. The first non-blank line sets the indentation.
  state == 2 {
    if ($0 ~ /^ *$/) {
      # A blank line, or one holding only spaces: spaces past the block
      # indentation are content, anything else is an empty line.
      if (ind >= 0 && length($0) > ind) body[++n] = substr($0, ind + 1); else body[++n] = ""
      next
    }
    if (ind < 0) {
      ind = indent($0)
      if (ind <= keyind) { bad = "command-block-is-empty"; exit }
    }
    if (indent($0) < ind) done_body()
    body[++n] = substr($0, ind + 1)
    next
  }
  END {
    if (found) exit 0
    if (state == 2 && bad == "") {
      if (ind < 0) { print "ERROR: command-block-is-empty: " step > "/dev/stderr"; exit 2 }
      while (n > 0 && body[n] == "") n--
      for (i = 1; i <= n; i++) print body[i]
      exit 0
    }
    if (bad == "" && state == 0) bad = "step-not-found"
    if (bad == "") bad = "step-has-no-command"
    print "ERROR: " bad ": " step > "/dev/stderr"
    exit 2
  }
' "$RECIPE"
