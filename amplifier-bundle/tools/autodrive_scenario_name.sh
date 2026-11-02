#!/usr/bin/env bash
# autodrive_scenario_name.sh <file> — print the name of one qa-team (gadugi)
# scenario file, for `gadugi-test run --scenario <name>`. The name is a
# top-level `name:`, or a `name:` directly under a top-level `scenario:`,
# with quotes and a trailing comment removed. A block scalar, flow value,
# anchor, alias or tag is no name. Prints nothing when there is none.
#
# autodrive_qa_evidence.sh runs this file from its own directory and decides
# what an empty or unsafe name means. It reads only <file>, and awk never
# calls a command. Exit 0, or 2 for a usage error.
set -uo pipefail
[ "$#" -eq 1 ] || { echo "usage: autodrive_scenario_name.sh <file>" >&2; exit 2; }
LC_ALL=C awk '
  { sub(/\r$/, "") }
  # A top-level key: remember whether it is `scenario:`, and stop at `name:`.
  /^[^[:space:]#]/ {
    top = $0
    sub(/[[:space:]]*:.*$/, "", top)
    insc = (top == "scenario")
    ind = -1
    if ($0 ~ /^name[[:space:]]*:/) {
      v = $0
      got = 1
      exit
    }
    next
  }
  # Under `scenario:`, only a key at the first indentation seen counts.
  insc && /^[[:space:]]+[^[:space:]#]/ {
    match($0, /^[[:space:]]+/)
    if (ind < 0) ind = RLENGTH
    if (RLENGTH == ind && $0 ~ /^[[:space:]]+name[[:space:]]*:/) {
      v = $0
      got = 1
      exit
    }
  }
  END {
    if (!got) exit
    sub(/^[[:space:]]*name[[:space:]]*:[[:space:]]*/, "", v)
    q = substr(v, 1, 1)
    if (q == "\"" || q == "\047") {
      v = substr(v, 2)
      n = index(v, q)
      v = (n > 0) ? substr(v, 1, n - 1) : ""
    } else {
      sub(/[[:space:]]+#.*$/, "", v)
      sub(/[[:space:]]+$/, "", v)
      if (v ~ /^[|>\[{&*!]/) v = ""
    }
    print v
  }' "$1"
