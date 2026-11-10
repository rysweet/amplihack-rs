#!/usr/bin/env bash
# test-issue-2219-docs-only-scenarios.sh — criterion 1 does not require a
# gadugi scenario for a documentation-only pull request (issue #2219, AC4).
#
# WHAT IS PINNED HERE
#
# The exemption is MEASURED, not prose. A sentence in SKILL.md cannot clear
# the stop it is meant to clear: with gadugi required and no scenarios,
# qa_status never reaches PASS, and measured_downgrade forces NOT_MERGE_READY
# whatever the assessment text says. So one predicate decides it, in
# autodrive_qa_scope.sh beside autodrive_qa_changed_paths, and both the merge
# round and the merge gate read it through autodrive_gadugi_required — the two
# cannot disagree.
#
#   autodrive_qa_docs_only <dir> -> true | false | unknown
#
# true only when the changed-path list is KNOWN and NON-EMPTY and every path
# qualifies. Qualifying, byte-exact: a path ending `.md`, `.markdown`, `.rst`
# or `.txt`, or a ROOT `LICENSE`, `NOTICE` or `COPYING`. Disqualifying
# whatever else matches, compared case-insensitively: anything under a
# DOT-DIRECTORY at any depth or under `amplifier-bundle/`, a basename of
# `AGENTS.md`, `CLAUDE.md` or `SKILL.md`, and `requirements.txt`,
# `constraints.txt` or `CMakeLists.txt`, which end `.txt` and so qualify
# above but are read by a build rather than by a person.
#
# There is no extension disqualifier for `.yml`, `.toml`, `.json` or `.sh`:
# the byte-exact qualifier has already rejected every one of those, so such a
# rule could never run. `docs/build.SH` is `false` below because `.SH` is not
# one of the four extensions, not because it is a script.
#
# Three properties are deliberate and are pinned below:
#   - The `docs/` DIRECTORY does not qualify a path; the extension does. A
#     `docs/conf.py` or `docs/release.js` is code that happens to live under
#     docs/.
#   - Disqualifiers are matched case-insensitively (`docs/Requirements.txt`
#     is a dependency pin) while qualifiers are byte-exact (`README.MD`
#     simply costs the exemption). Both directions fail closed.
#   - Files consumed as AGENT INSTRUCTIONS are not inert documentation. A
#     pull request editing only `merge-ready/SKILL.md` edits the rules that
#     decide what may merge, so it does not buy its way out of the scenario
#     requirement. The dot-directory rule is one pattern, `*/.*/*`, so
#     `.cursor/` and `.codex/` are covered alongside `.claude/` without
#     being named; a dot-FILE is not a dot-directory, and `docs/.hidden.md`
#     is still prose.
#
# Precedence, all fail-closed: an operator-set AUTODRIVE_QA_SCENARIO_DIR still
# forces gadugi_required=true; a scope helper that did not load still forces
# true; `unknown` is not `true`.
#
# REACH. The exemption changes the answer only where gadugi was required in
# the first place — a Node project, or an unresolved choice with a Node
# candidate. A Rust or Python repository never required it.
#
# HOW. Synthetic git repositories under this test's own temp directory, with
# `env -i`, HOME inside that directory, GIT_CONFIG_GLOBAL=/dev/null and
# GIT_CONFIG_NOSYSTEM=1. No jq, no network, no clock, no real ~/.claude,
# ~/.amplihack or profile root.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-2219-docs-only-scenarios.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
TRUST="${TOOLS}/autodrive_trust.sh"
SCOPE="${TOOLS}/autodrive_qa_scope.sh"
EVIDENCE_TOOL="${TOOLS}/autodrive_qa_evidence.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }
[ -f "$TRUST" ] || harness_error "helper not found: $TRUST"
[ -f "$SCOPE" ] || harness_error "helper not found: $SCOPE"
command -v git >/dev/null 2>&1 || harness_error "git is required to build the fixtures"

export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE CDPATH

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== issue #2219: a documentation-only pull request needs no gadugi scenario ==="

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2219-docs.XXXXXX")" || harness_error "mktemp -d failed"
WORK="$(cd "$WORK" && pwd -P)"
cleanup() { [ -n "${WORK:-}" ] && [ "${WORK}" != "/" ] && { chmod -R u+rwx "${WORK}" 2>/dev/null; rm -rf -- "${WORK}"; }; }
trap cleanup EXIT
chmod 0700 "${WORK}"
TEST_HOME="${WORK}/home"; mkdir -p "${TEST_HOME}"
SYSPATH="/usr/bin:/bin"

# --- stubs for the evidence-tool cases -------------------------------------
# Each records "name|<dir relative to the fixture>|argv" and exits 0.
STUBS="${WORK}/stubs"; mkdir -p "${STUBS}"
for name in npm cargo pytest gadugi-test; do
  cat > "${STUBS}/${name}" <<EOF
#!/usr/bin/env bash
rel="\${PWD#"\${FIXTURE_ROOT}"}"; rel="\${rel#/}"; [ -n "\$rel" ] || rel="."
printf '%s|%s|%s\n' "${name}" "\$rel" "\$*" >> "\${CALLS:-/dev/null}"
exit 0
EOF
  chmod +x "${STUBS}/${name}"
done

# --- fixtures --------------------------------------------------------------
N=0; REPO=""; CALLS=""
mk() { # mk <relpath>: a directory (trailing /) or a file with one more line
  case "$1" in
    */) mkdir -p "${REPO}/$1" ;;
    *) mkdir -p "$(dirname "${REPO}/$1")"; printf 'x\n' >> "${REPO}/$1" ;;
  esac
}
repo() { # repo <path>...: a fresh git repository holding each path
  N=$((N + 1))
  REPO="${WORK}/r${N}/repo"; CALLS="${WORK}/r${N}/calls"
  mkdir -p "${REPO}"; : > "${CALLS}"
  git -C "${REPO}" init -q . >/dev/null 2>&1 || harness_error "git init failed at ${REPO}"
  local p; for p in "$@"; do mk "$p"; done
}
snap() { # commit the whole work tree with plumbing, so no hook runs
  ( cd "${REPO}" && git add -A >/dev/null 2>&1 && t="$(git write-tree)" \
    && p="$(git rev-parse -q --verify HEAD 2>/dev/null || true)" \
    && c="$(git commit-tree "$t" ${p:+-p "$p"} -m c)" && git update-ref HEAD "$c" ) >/dev/null 2>&1 \
    || harness_error "could not commit in ${REPO}"
}
base() { snap; git -C "${REPO}" update-ref refs/remotes/origin/main HEAD || harness_error "update-ref failed"; }
change() { local p; for p in "$@"; do mk "$p"; done; snap; }

# helper <fn> [VAR=value]... — <fn> from autodrive_trust.sh against $REPO, in
# a scrubbed environment; the whole stdout followed by '#'.
helper() {
  local fn="$1"; shift
  env -i HOME="${TEST_HOME}" TMPDIR="${WORK}" PATH="${SYSPATH}" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null "$@" bash -c '
      . "$1" || exit 1
      "$2" "$3"; printf "#"' _ "${TRUST}" "${fn}" "${REPO}" 2>/dev/null </dev/null || printf 'ERR#'
}
expect_helper() { # expect_helper <id> <fn> <want> [VAR=value]...
  local id="$1" fn="$2" want="$3" got
  shift 3
  got="$(helper "${fn}" "$@")"
  if [ "${got}" = "${want}"$'\n#' ]; then pass "${id}" "${fn} prints ${want}"
  else fail "${id}" "${fn} printed $(printf '%q' "${got}"), want $(printf '%q' "${want}")"; fi
}
# both <id> <docs_only> <gadugi_required> <why> [VAR=value]...
# Two assertions and one sentence of context; the sentence is not a third
# assertion and is not counted.
both() {
  local id="$1" d="$2" g="$3" why="$4"
  shift 4
  echo "  -- ${id}: ${why}"
  expect_helper "${id}:docs-only" autodrive_qa_docs_only "${d}" "$@"
  expect_helper "${id}:gadugi" autodrive_gadugi_required "${g}" "$@"
}
no_pwned() { # no_pwned <id>: nothing in a changed path was ever executed
  if [ -z "$(find "${WORK}/r${N}" -name 'PWNED*' -print 2>/dev/null)" ]; then
    pass "$1" "no path name was executed while it was measured"
  else
    fail "$1" "a path name was executed: $(find "${WORK}/r${N}" -name 'PWNED*' -print 2>/dev/null)"
  fi
}
B=AUTODRIVE_QA_BASE=main

# ===========================================================================
echo "== the predicate: which changed paths are documentation =="

repo package.json src/index.js README.md; base
change README.md docs/guide.md
both D1-node-docs-only true false \
  "a Node pull request touching only Markdown needs no gadugi scenario" "$B"

repo package.json src/index.js; base
change src/index.js
both D2-node-code false true "a code change still requires the scenarios" "$B"

repo package.json src/index.js README.md; base
change README.md src/index.js
both D3-mixed false true "one code file costs the whole pull request its exemption" "$B"

repo package.json docs/conf.py; base
change docs/conf.py
both D4-docs-dir-python false true \
  "the docs/ DIRECTORY does not qualify a path; docs/conf.py is code" "$B"

repo package.json docs/release.js; base
change docs/release.js
both D4b-docs-dir-js false true "docs/release.js is code that happens to live under docs/" "$B"

repo package.json docs/Requirements.txt; base
change docs/Requirements.txt
both D5-case-insensitive-disqualifier false true \
  "disqualifiers are matched case-insensitively: docs/Requirements.txt is a dependency pin" "$B"

repo package.json docs/build.SH; base
change docs/build.SH
both D5b-uppercase-sh false true \
  "docs/build.SH never qualifies: .SH is not one of the four documentation extensions" "$B"

repo package.json .github/workflows/ci.yml README.md; base
change .github/workflows/ci.yml
both D6-github false true "a workflow file decides what CI runs; it is never documentation" "$B"

repo package.json .amplihack/qa.toml README.md; base
change .amplihack/qa.toml
both D7-amplihack false true "the declared QA sources are not documentation" "$B"

repo package.json .claude/agents/reviewer.md README.md; base
change .claude/agents/reviewer.md
both D8-claude-dir false true \
  "a file under .claude/ is read as agent instructions, not as prose" "$B"

repo package.json AGENTS.md CLAUDE.md README.md; base
change AGENTS.md
both D9a-agents-md false true "AGENTS.md is an instruction surface" "$B"
repo package.json AGENTS.md CLAUDE.md README.md; base
change CLAUDE.md
both D9b-claude-md false true "CLAUDE.md is an instruction surface" "$B"

repo package.json amplifier-bundle/skills/merge-ready/SKILL.md README.md; base
change amplifier-bundle/skills/merge-ready/SKILL.md
both D10-skill-md false true \
  "a pull request editing only the merge-ready rules does not skip the scenario requirement" "$B"

repo package.json LICENSE NOTICE COPYING src/index.js; base
change LICENSE NOTICE COPYING
both D11-root-licences true false "root LICENSE, NOTICE and COPYING qualify by name" "$B"

repo package.json vendor/LICENSE src/index.js; base
change vendor/LICENSE
both D12-nested-licence false true "LICENSE qualifies at the ROOT only" "$B"

repo package.json README.MD src/index.js; base
change README.MD
both D13-uppercase-md false true \
  "qualifiers are byte-exact: unusual casing costs the exemption rather than granting it" "$B"

repo package.json notes.txt spec.rst readme.markdown src/index.js; base
change notes.txt spec.rst readme.markdown
both D14-other-doc-extensions true false ".markdown, .rst and .txt qualify beside .md" "$B"

echo "== a .txt a build reads is not documentation =="
# The same repository lists requirements.txt and CMakeLists.txt in
# AUTODRIVE_ROOT_PROJECT_FILES, as files the QA target cannot run. Without
# these rules a dependency bump would pass criterion 1 with no scenario and
# no blocker, because the round reads docs_only as a measured field.

repo package.json requirements.txt README.md; base
change requirements.txt
both D22-root-requirements false true \
  "requirements.txt ends .txt but pins what the suite installs; it is a build input" "$B"

repo package.json docs/requirements.txt README.md; base
change docs/requirements.txt
both D23-docs-requirements false true \
  "the rule is not anchored at the root: docs/requirements.txt is the ordinary Sphinx pin" "$B"

repo package.json CMakeLists.txt README.md; base
change CMakeLists.txt
both D24-cmakelists false true "CMakeLists.txt ends .txt and is a build script" "$B"

repo package.json constraints.txt README.md; base
change constraints.txt
both D25-constraints false true "constraints.txt pins dependencies the same way" "$B"

repo package.json docs/notes.txt README.md; base
change docs/notes.txt
both D25b-plain-txt-still-qualifies true false \
  "an ordinary .txt is still prose: the rule names three basenames, not the extension" "$B"

# Drift: the two lists are not read from one another at runtime, because
# autodrive_qa_docs.sh reads nothing outside the scan variables and an array
# that may be undefined in some load order would fail open. They are pinned
# together here instead, so a .txt added to AUTODRIVE_ROOT_PROJECT_FILES
# without a disqualifier fails this test rather than exempting a build input.
TXT_PROJECT_FILES="$(env -i HOME="${TEST_HOME}" PATH="${SYSPATH}" bash -c '
  . "$1" >/dev/null 2>&1 || exit 1
  printf "%s\n" "${AUTODRIVE_ROOT_PROJECT_FILES[@]}"' _ "${TRUST}" 2>/dev/null \
  | grep -i '\.txt$' || true)"
if [ -z "${TXT_PROJECT_FILES}" ]; then
  fail D26-project-file-drift "AUTODRIVE_ROOT_PROJECT_FILES could not be read, or holds no .txt entry; this check would pass vacuously"
else
  for f in ${TXT_PROJECT_FILES}; do
    repo package.json "$f" README.md; base
    change "$f"
    expect_helper "D26-project-file-drift:${f}" autodrive_qa_docs_only false "$B"
  done
fi

echo "== a dot-directory is an instruction surface; a dot-file is not =="

repo package.json .cursor/notes.md README.md; base
change .cursor/notes.md
both D27-cursor-dir false true \
  "one pattern covers every dot-directory, so .cursor/ is disqualified without being named" "$B"

repo package.json .codex/plan.md README.md; base
change .codex/plan.md
both D27b-codex-dir false true "the same pattern covers .codex/" "$B"

repo package.json docs/.hidden.md README.md; base
change docs/.hidden.md
both D28-dot-file-is-prose true false \
  "the pattern needs a / after the dotted name: a dot-FILE under docs/ is still documentation" "$B"

echo "== fail-closed: nothing measured is not 'documentation only' =="

repo package.json src/index.js README.md; base
both D15-empty-diff false true "an empty diff has changed no documentation; it is not a docs-only change" "$B"

repo package.json src/index.js README.md; base
change README.md
both D16-no-base unknown true \
  "with no base the head stands in for the merge base: nothing was compared, so the answer is unknown"

both D17-bad-base unknown true \
  "a base name that fails the branch rules leaves the paths unreadable" AUTODRIVE_QA_BASE='ma in'

both D18-scenario-dir-wins true true \
  "an operator who set AUTODRIVE_QA_SCENARIO_DIR gets the scenarios run in any repository" \
  "$B" AUTODRIVE_QA_SCENARIO_DIR="${WORK}/scenarios"

echo "== a missing gate file is loud, not silent =="
# autodrive_qa_docs.sh is a gate file. If it is deleted or made unreadable the
# predicate degrades to `unknown`, which is safe but otherwise invisible: the
# only symptom would be gadugi staying required for every documentation-only
# pull request, indefinitely, with nothing to read. The load failure says so
# on stderr, the same way round_record reports a measurement it could not make.
NODOCS="${WORK}/tools-no-docs"; mkdir -p "${NODOCS}"; chmod 0700 "${NODOCS}"
cp "${TOOLS}"/*.sh "${NODOCS}/" 2>/dev/null || harness_error "could not copy the tools to ${NODOCS}"
rm -f "${NODOCS}/autodrive_qa_docs.sh"
repo package.json src/index.js README.md; base
change README.md
ND_ERR="${WORK}/r${N}/nodocs.err"
nd() { # nd <fn>: <fn> against $REPO with the gate file absent
  env -i HOME="${TEST_HOME}" TMPDIR="${WORK}" PATH="${SYSPATH}" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null AUTODRIVE_QA_BASE=main \
    bash -c '. "$1" >/dev/null || exit 1
             "$3" "$2"' _ "${NODOCS}/autodrive_trust.sh" "${REPO}" "$1" 2>>"${ND_ERR}"
}
: > "${ND_ERR}"
got="$(nd autodrive_qa_docs_only)"
if [ "${got}" = "unknown" ]; then
  pass D29-missing-gate-file-unknown "a deleted autodrive_qa_docs.sh degrades the predicate to unknown, not to true"
else
  fail D29-missing-gate-file-unknown "printed $(printf '%q' "${got}"), want unknown"
fi
got="$(nd autodrive_gadugi_required)"
if [ "${got}" = "true" ]; then
  pass D29b-missing-gate-file-blocks "gadugi stays required while the predicate cannot be measured"
else
  fail D29b-missing-gate-file-blocks "autodrive_gadugi_required printed $(printf '%q' "${got}"), want true"
fi
if grep -q '^WARNING: autodrive_qa_docs\.sh did not load' "${ND_ERR}" 2>/dev/null; then
  pass D29c-missing-gate-file-warns "the load failure names the file on stderr instead of degrading in silence"
else
  fail D29c-missing-gate-file-warns "no WARNING naming autodrive_qa_docs.sh: $(tr '\n' '|' < "${ND_ERR}" 2>/dev/null)"
fi

echo "== reach: only where gadugi was required in the first place =="

repo Cargo.toml src/lib.rs README.md; base
change README.md
both D19-rust-docs-only true false "a Rust repository never required gadugi; the answer is unchanged" "$B"

repo a/package.json b/package.json README.md; base
change README.md
both D20-unresolved-node-docs-only true false \
  "an unresolved choice with a Node candidate also drops the requirement for a docs-only change" "$B"

repo a/package.json b/package.json src/index.js README.md; base
change a/README.md src/index.js
both D20b-unresolved-node-code false true \
  "the same repository with a code file still requires the scenarios" "$B"

echo "== a changed path is data, never a command =="

repo package.json README.md; base
change 'docs/$(touch PWNED).md' 'docs/`touch PWNED2`.md'
both D21-hostile-path-names true false \
  "a hostile path name that still ends .md is measured as documentation and never evaluated" "$B"
no_pwned D21-no-execution

# ===========================================================================
echo "== the measurement reaches the round as a field, not as an inference =="
# #1517/#1518: the round agent must key criterion 1 off a MEASURED field, not
# guess why gadugi_required is false. The qa evidence line carries docs_only.
ev_run() { # ev_run [VAR=value]...
  EV_ERR="${WORK}/r${N}/ev.err"
  EV_OUT="$(env -i HOME="${TEST_HOME}" TMPDIR="${WORK}" PATH="${STUBS}:${SYSPATH}" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    REPO_PATH="${REPO}" FIXTURE_ROOT="${REPO}" CALLS="${CALLS}" \
    AUTODRIVE_ROUND_LABEL=round-1 "$@" bash "${EVIDENCE_TOOL}" 2>"${EV_ERR}" </dev/null)" || true
  EV_OUT="${EV_OUT##*$'\n'}"
}
ev_field() { printf '%s' "${EV_OUT}" | sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p"; }
ev_expect() { # ev_expect <id> <why> <field=value>...
  local id="$1" why="$2" kv bad="" got
  shift 2
  for kv in "$@"; do
    got="$(ev_field "${kv%%=*}")"
    [ "${got}" = "${kv#*=}" ] || bad="${bad} ${kv%%=*}='${got}'(want '${kv#*=}')"
  done
  if [ -z "${bad}" ]; then pass "${id}" "${why}"
  else fail "${id}" "${why}:${bad} | $(tr '\n' '|' < "${EV_ERR}" 2>/dev/null)"; fi
}

repo package.json src/index.js README.md; base
change README.md
ev_run "$B"
ev_expect E1-evidence-docs-only "the evidence line reports docs_only=true beside gadugi_required=false" \
  docs_only=true gadugi_required=false

repo package.json src/index.js README.md; base
change src/index.js
ev_run "$B"
ev_expect E2-evidence-code "a code change is reported as docs_only=false with gadugi still required" \
  docs_only=false gadugi_required=true

repo package.json src/index.js README.md; base
change README.md
ev_run
ev_expect E3-evidence-unknown "an unmeasurable diff is reported as docs_only=unknown, and gadugi stays required" \
  docs_only=unknown gadugi_required=true

echo ""
echo "passed: ${PASS_COUNT}  failed: ${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ] || exit 1
exit 0
