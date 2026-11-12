#!/usr/bin/env bash
# autodrive_minor_findings.sh - the one parser for the autodrive_minor_findings
# recipe context value (issue #2391).
#
# Usage:
#   autodrive_minor_findings.sh [<value>]
#
# Removes surrounding whitespace, then prints the canonical value on stdout:
#   follow_up          -> follow_up
#   address, empty     -> address
#   absent             -> address
#   anything else      -> address, with one WARNING line on stderr (control
#                         characters removed from the echoed value)
# The match is exact and case-sensitive. Exit 0; exit 2, printing nothing on
# stdout, when given more than one value. Reads no state, makes no network call.
#
# Callers: autodrive-crusty-loop.yaml step-02 (once per crusty loop) and
# autodrive_crusty_rereview.sh (once per re-review). Contract:
# docs/reference/auto-drive-to-merge.md, "How the value is read".

set -uo pipefail

if [ $# -gt 1 ]; then
  echo "ERROR: autodrive_minor_findings.sh takes at most one value" >&2
  exit 2
fi
MF="${1:-}"
MF="${MF#"${MF%%[![:space:]]*}"}"; MF="${MF%"${MF##*[![:space:]]}"}"
case "$MF" in
  follow_up|address) ;;
  '') MF=address ;;
  *) printf "WARNING: autodrive_minor_findings='%s' is not recognised; using 'address'\n" "$(printf '%s' "$MF" | tr -d '[:cntrl:]')" >&2
     MF=address ;;
esac
printf '%s\n' "$MF"
