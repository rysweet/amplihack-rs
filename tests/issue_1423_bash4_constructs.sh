#!/usr/bin/env bash
# Guard (issue #1423): shipped shell must run on bash 3.2.
#
# macOS has shipped GNU bash 3.2.57 as /bin/bash since 2007 and will not update
# it (bash 4 is GPLv3). Every recipe `type: bash` step, every tool under
# amplifier-bundle/tools/, and every shell test therefore executes under bash
# 3.2 on a stock Mac. A bash-4-only construct there is not a style problem: it
# is a hard `bad substitution` / `command not found` failure.
#
# That is how #1423 happened. `${REMOTE_URL,,}` in workflow-prep.yaml aborted
# step-02d-detect-host-type, workflow-prep is a sub-recipe of default-workflow,
# and the entire development workflow was unusable on macOS. CI is
# ubuntu-latest with bash 5, so CI was green the whole time.
#
# This guard is the thing that would have caught it. It is a static scan: it
# does not need a bash 3.2 to run, which is exactly why it can run on every PR.
#
# Portable replacements:
#   ${v,,} / ${v^^}      ->  $(printf '%s' "$v" | tr '[:upper:]' '[:lower:]')
#   mapfile -t a < <(x)  ->  a=(); while IFS= read -r l; do a+=("$l"); done < <(x)
#   mapfile -d '' -t a   ->  a=(); while IFS= read -r -d '' l; do a+=("$l"); done
#   declare -A m         ->  a case lookup, or two parallel indexed arrays
#   shopt -s globstar    ->  find -print0 piped into `read -r -d ''`
#   cmd &>>f             ->  cmd >>f 2>&1
#   wait -n              ->  wait on a recorded pid
#
# Scope: amplifier-bundle/ (everything shipped to users), tests/, scripts/.
#
# NOT covered, because no static pattern separates them from the ~200 benign
# look-alikes in this repo: bash 3.2's command-substitution scanner does not
# understand heredocs or comments, so a lone apostrophe in a comment inside
# `$( ... )`, or a bare `#` inside a heredoc inside `$( ... )`, swallows the
# rest of the file. Two of those were found and fixed alongside #1423. The
# only reliable detector is a real bash 3.2:
#   ./configure && make   # from ftp.gnu.org/gnu/bash/bash-3.2.57.tar.gz
#   find amplifier-bundle tests scripts -name '*.sh' -exec ./bash -n {} +
#
# Comments are scanned too, deliberately: a commented example is where the next
# copy of the construct comes from. Describe it in prose ('bash 4 lowercase
# expansion') rather than quoting it. This file is the one exemption — it has
# to name what it forbids — and it excludes itself by path below.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

SELF="tests/issue_1423_bash4_constructs.sh"

# Directories scanned. `skills` at the repo root is a symlink into
# amplifier-bundle/skills, so it is deliberately not listed: grep -r does not
# descend into symlinks it meets, and naming it would double-report every hit.
SCAN_DIRS=(amplifier-bundle tests scripts)

# Only files that are, or embed, shell. .md is included because a shipped skill
# document is a copy-paste source for an agent writing shell.
INCLUDES=(--include='*.sh' --include='*.bash' --include='*.yaml' --include='*.yml' --include='*.md')

fails=0
found_total=0

# report <bash-version-introduced> <what> <ERE>
report() {
  local since="$1" what="$2" pattern="$3" hits
  hits="$(grep -rnE "${INCLUDES[@]}" -- "$pattern" "${SCAN_DIRS[@]}" 2>/dev/null \
    | grep -v "^${SELF}:" || true)"
  if [ -n "$hits" ]; then
    printf '  FAIL  %s (bash %s+, absent from macOS /bin/bash 3.2)\n' "$what" "$since"
    printf '%s\n' "$hits" | sed 's/^/          /'
    found_total=$((found_total + $(printf '%s\n' "$hits" | grep -c '')))
    fails=$((fails + 1))
  else
    printf '  ok    no %s\n' "$what"
  fi
}

echo "issue #1423: bash-4-only constructs in shipped shell"
echo "scanning: ${SCAN_DIRS[*]}"
echo ""

# 1. Case-modification expansion — the construct that broke #1423.
report 4.0 'case-modifying expansion ${v,,} / ${v^^}' \
  '\$\{[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?(\^\^|,,|\^|,)'

# 2. Associative arrays.
report 4.0 'associative array (declare/local/typeset -A)' \
  '(^|[;&|[:space:]])(declare|local|typeset)[[:space:]]+(-[A-Za-z]+[[:space:]]+)*-[A-Za-z]*A'

# 3. mapfile / readarray.
report 4.0 'mapfile / readarray' \
  '(^|[;&|(`[:space:]])(mapfile|readarray)[[:space:]]'

# 4. Append-both redirection.
report 4.0 '&>> append redirection' '&>>'

# 5. Recursive ** globbing.
report 4.0 'globstar (** recursive glob)' 'shopt[[:space:]]+-[su][[:space:]]+([A-Za-z_]+[[:space:]]+)*globstar'

# 6. case fall-through terminator.
report 4.0 ';;& case fall-through' ';;&'

# 7. Coprocesses.
report 4.0 'coproc' '(^|[;&|[:space:]])coproc[[:space:]]'

# 8. Namerefs.
report 4.3 'nameref (declare/local/typeset -n)' \
  '(^|[;&|[:space:]])(declare|local|typeset)[[:space:]]+(-[A-Za-z]+[[:space:]]+)*-[A-Za-z]*n'

# 9. wait -n.
report 4.3 'wait -n' '(^|[;&|[:space:]])wait[[:space:]]+-n([[:space:]]|$)'

# 10. Parameter transformations ${v@Q} and friends.
report 4.4 'parameter transformation ${v@Q}' \
  '\$\{[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?@[QEPAKakLUu]\}'

echo ""
if [ "$fails" -ne 0 ]; then
  cat <<'MSG'
FAIL: shipped shell uses constructs macOS /bin/bash 3.2 cannot parse.

A recipe step that hits one of these does not degrade — it aborts, and it takes
its parent recipe down with it (issue #1423 took down default-workflow on every
Mac). Replace with the portable form listed at the top of this file; do not
suppress this check.
MSG
  printf '\n%s construct class(es), %s line(s).\n' "$fails" "$found_total"
  exit 1
fi

echo "PASS: no bash-4-only constructs in ${SCAN_DIRS[*]}"
exit 0
