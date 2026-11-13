#!/usr/bin/env bash
# test-issue-2219-docs-only-scenarios.sh: whether a pull request changes
# documentation only is measured, and the measurement is recorded as a fact in
# qa-inputs.json for the criterion-1 judgement (issues #2219 and #2569).
#
# WHAT IS PINNED HERE
#
# One predicate decides the fact, in autodrive_qa_docs.sh, loaded through
# autodrive_trust.sh beside autodrive_qa_changed_paths:
#
#   autodrive_qa_docs_only <dir> -> true | false | unknown
#
# true only when the changed-path list is KNOWN and NON-EMPTY and every path
# qualifies. Qualifying, byte-exact: a path ending `.md`, `.markdown` or
# `.rst`, or a ROOT `LICENSE`, `NOTICE` or `COPYING`. A `.txt` qualifies only
# under a `docs/` component at any depth or as a root documentation name
# (`README.txt`, `CHANGELOG.txt` and the rest), because the extension is worn
# by golden fixtures, page templates, corpus seeds and dependency pins as
# readily as by prose. Disqualifying whatever else matches, compared
# case-insensitively: anything under a DOT-DIRECTORY at any depth or under
# `amplifier-bundle/`, a basename of `AGENTS.md`, `CLAUDE.md` or `SKILL.md`,
# and `requirements.txt`, `constraints.txt` or `CMakeLists.txt`, which a build
# reads rather than a person. The narrowed qualifier stops all three at the
# root, so what the rule still catches is the copy under `docs/`.
#
# There is no extension disqualifier for `.yml`, `.toml`, `.json` or `.sh`:
# the byte-exact qualifier has already rejected every one of those, so such a
# rule could never run. `docs/build.SH` is `false` below because `.SH` is not
# one of the four extensions, not because it is a script.
#
# Three properties are deliberate and are pinned below:
#   - The `docs/` DIRECTORY qualifies no EXTENSION; it only widens where a
#     `.txt` may sit. A `docs/conf.py` or `docs/release.js` is still code that
#     happens to live under docs/.
#   - Disqualifiers are matched case-insensitively (`docs/Requirements.txt`
#     is a dependency pin) while qualifiers are byte-exact (`README.MD` is
#     simply not documentation). Both directions answer false, never true.
#   - Files consumed as AGENT INSTRUCTIONS are not inert documentation. A
#     pull request editing only `merge-ready/SKILL.md` edits the rules that
#     decide what may merge. The dot-directory rule is one pattern, `*/.*/*`,
#     so `.cursor/` and `.codex/` are covered alongside `.claude/` without
#     being named; a dot-FILE is not a dot-directory, and `docs/.hidden.md`
#     is still prose.
#
# Nothing measured is not `true`: an empty diff is `false`, and a missing or
# invalid base, or a predicate that did not load, is `unknown`. The answer
# does not depend on the repository type or on AUTODRIVE_QA_SCENARIO_DIR.
#
# The fact is recorded, not acted on. autodrive_qa_evidence.sh writes
# docs_only into qa-inputs.json beside AUTODRIVE_QA_EVIDENCE and prints the
# same JSON on stdout; no step stops or is exempted because of it. The last
# section checks that the record carries the measured value.
#
# HOW. Synthetic git repositories under this test's own temp directory, with
# `env -i`, HOME inside that directory, GIT_CONFIG_GLOBAL=/dev/null and
# GIT_CONFIG_NOSYSTEM=1. No jq, no network, no clock, no real ~/.claude,
# ~/.amplihack or profile root. python3 parses the recorded JSON.
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
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to read the recorded JSON"

export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE CDPATH

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== issue #2219/#2569: documentation-only is measured and recorded for criterion 1 ==="

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
for name in npm cargo pytest; do
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
# measure <id> <docs_only> <why> [VAR=value]...
# One assertion and one sentence of context; the sentence is not counted.
measure() {
  local id="$1" d="$2" why="$3"
  shift 3
  echo "  -- ${id}: ${why}"
  expect_helper "${id}:docs-only" autodrive_qa_docs_only "${d}" "$@"
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
measure D1-node-docs-only true \
  "a Node pull request touching only Markdown is documentation only" "$B"

repo package.json src/index.js; base
change src/index.js
measure D2-node-code false "a code change is not documentation" "$B"

repo package.json src/index.js README.md; base
change README.md src/index.js
measure D3-mixed false "one code file makes the whole pull request not documentation only" "$B"

repo package.json docs/conf.py; base
change docs/conf.py
measure D4-docs-dir-python false \
  "the docs/ DIRECTORY does not qualify a path; docs/conf.py is code" "$B"

repo package.json docs/release.js; base
change docs/release.js
measure D4b-docs-dir-js false "docs/release.js is code that happens to live under docs/" "$B"

repo package.json docs/Requirements.txt; base
change docs/Requirements.txt
measure D5-case-insensitive-disqualifier false \
  "disqualifiers are matched case-insensitively: docs/Requirements.txt is a dependency pin" "$B"

repo package.json docs/build.SH; base
change docs/build.SH
measure D5b-uppercase-sh false \
  "docs/build.SH never qualifies: .SH is not one of the four documentation extensions" "$B"

repo package.json .github/workflows/ci.yml README.md; base
change .github/workflows/ci.yml
measure D6-github false "a workflow file decides what CI runs; it is never documentation" "$B"

repo package.json .amplihack/qa.toml README.md; base
change .amplihack/qa.toml
measure D7-amplihack false "the declared QA sources are not documentation" "$B"

repo package.json .claude/agents/reviewer.md README.md; base
change .claude/agents/reviewer.md
measure D8-claude-dir false \
  "a file under .claude/ is read as agent instructions, not as prose" "$B"

repo package.json AGENTS.md CLAUDE.md README.md; base
change AGENTS.md
measure D9a-agents-md false "AGENTS.md is an instruction surface" "$B"
repo package.json AGENTS.md CLAUDE.md README.md; base
change CLAUDE.md
measure D9b-claude-md false "CLAUDE.md is an instruction surface" "$B"

repo package.json amplifier-bundle/skills/merge-ready/SKILL.md README.md; base
change amplifier-bundle/skills/merge-ready/SKILL.md
measure D10-skill-md false \
  "a pull request editing only the merge-ready rules is not documentation only" "$B"

repo package.json LICENSE NOTICE COPYING src/index.js; base
change LICENSE NOTICE COPYING
measure D11-root-licences true "root LICENSE, NOTICE and COPYING qualify by name" "$B"

repo package.json vendor/LICENSE src/index.js; base
change vendor/LICENSE
measure D12-nested-licence false "LICENSE qualifies at the ROOT only" "$B"

repo package.json README.MD src/index.js; base
change README.MD
measure D13-uppercase-md false \
  "qualifiers are byte-exact: unusual casing answers false rather than true" "$B"

repo package.json docs/notes.txt spec.rst readme.markdown src/index.js; base
change docs/notes.txt spec.rst readme.markdown
measure D14-other-doc-extensions true ".markdown, .rst and a docs/ .txt qualify beside .md" "$B"

echo "== a .txt a build reads is not documentation =="
# The same repository lists requirements.txt and CMakeLists.txt in
# AUTODRIVE_ROOT_PROJECT_FILES, as files the QA target cannot run. Without
# these rules a dependency bump would be recorded as documentation only, and
# the criterion-1 judgement reads docs_only as a measured fact.

repo package.json requirements.txt README.md; base
change requirements.txt
measure D22-root-requirements false \
  "requirements.txt ends .txt but pins what the suite installs; it is a build input" "$B"

repo package.json docs/requirements.txt README.md; base
change docs/requirements.txt
measure D23-docs-requirements false \
  "the rule is not anchored at the root: docs/requirements.txt is the ordinary Sphinx pin" "$B"

repo package.json CMakeLists.txt README.md; base
change CMakeLists.txt
measure D24-cmakelists false "CMakeLists.txt ends .txt and is a build script" "$B"

repo package.json constraints.txt README.md; base
change constraints.txt
measure D25-constraints false "constraints.txt pins dependencies the same way" "$B"

repo package.json docs/notes.txt README.md; base
change docs/notes.txt
measure D25b-plain-txt-still-qualifies true \
  "an ordinary .txt under docs/ is still prose, whatever its basename" "$B"

echo "== .txt qualifies only where prose lives =="
# The extension is generic. Qualifying it everywhere counted a golden
# fixture, a page template, a fuzz corpus seed and a dependency pin whose
# basename is not one of the three named above, as documentation, because
# each ends .txt and nothing else disqualified it. The qualifier now names
# the places prose lives: under a docs/ component at any depth, or a root
# documentation name.

for f in tests/fixtures/expected.txt src/templates/welcome.txt \
         fuzz/corpus/seed.txt testdata/golden.txt scripts/deploy.txt; do
  repo package.json "$f" README.md; base
  change "$f"
  measure "D27-txt-outside-docs:${f}" false \
    "a .txt outside docs/ and outside the root names is not prose" "$B"
done

# The three disqualifying basenames are exact, so every near-miss spelling of
# a dependency pin used to qualify. They are stopped by the narrowed
# qualifier now rather than by a list that can never be complete.
for f in requirements-dev.txt dev-requirements.txt constraints-dev.txt Makefile.txt; do
  repo package.json "$f" README.md; base
  change "$f"
  measure "D28-pin-near-miss:${f}" false \
    "a root .txt outside the documentation names is not prose, however it is spelt" "$B"
done

repo package.json requirements/base.txt README.md; base
change requirements/base.txt
measure D28b-pin-in-directory false \
  "requirements/base.txt is a pin in a directory, and the directory is not docs/" "$B"

# The narrowed qualifier stops the pins at the root, but docs/ is exactly
# where a Sphinx pin sits, and an exact basename there is one spelling away
# from the next. The pin disqualifiers match a substring of the path, so
# every spelling of the pin under docs/ is caught rather than the one that
# happened to be named.
for f in docs/requirements-dev.txt docs/dev-requirements.txt \
         docs/requirements/base.txt docs/constraints-dev.txt; do
  repo package.json "$f" README.md; base
  change "$f"
  measure "D28c-pin-under-docs:${f}" false \
    "a dependency pin under docs/ is a build input however it is spelt" "$B"
done

repo package.json docs/guide.txt docs/notes.txt README.md; base
change docs/guide.txt docs/notes.txt
measure D28d-pin-rule-leaves-prose-alone true \
  "the substring pin rule disqualifies pins, not every .txt under docs/" "$B"

for f in README.txt CHANGELOG.txt CONTRIBUTING.txt; do
  repo package.json "$f" src/index.js; base
  change "$f"
  measure "D29-root-doc-name:${f}" true \
    "a root documentation name ending .txt is prose" "$B"
done

repo package.json docs/guide/install.txt sub/docs/notes.txt src/index.js; base
change docs/guide/install.txt sub/docs/notes.txt
measure D30-docs-component-any-depth true \
  "a docs/ component qualifies a .txt at any depth, at the root or below one" "$B"

repo package.json notdocs/notes.txt README.md; base
change notdocs/notes.txt
measure D30b-docs-is-a-component false \
  "docs/ must be a whole path component: notdocs/ does not qualify a .txt" "$B"

# Drift: the two lists are not read from one another at runtime, because
# autodrive_qa_docs.sh reads nothing outside the scan variables and an array
# that may be undefined in some load order would fail open. They are pinned
# together here instead, so a .txt added to AUTODRIVE_ROOT_PROJECT_FILES
# without a disqualifier fails this test rather than calling a build input prose.
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
measure D27-cursor-dir false \
  "one pattern covers every dot-directory, so .cursor/ is disqualified without being named" "$B"

repo package.json .codex/plan.md README.md; base
change .codex/plan.md
measure D27b-codex-dir false "the same pattern covers .codex/" "$B"

repo package.json docs/.hidden.md README.md; base
change docs/.hidden.md
measure D28-dot-file-is-prose true \
  "the pattern needs a / after the dotted name: a dot-FILE under docs/ is still documentation" "$B"

echo "== fail-closed: nothing measured is not 'documentation only' =="

repo package.json src/index.js README.md; base
measure D15-empty-diff false "an empty diff has changed no documentation; it is not a docs-only change" "$B"

repo package.json src/index.js README.md; base
change README.md
measure D16-no-base unknown \
  "with no base the head stands in for the merge base: nothing was compared, so the answer is unknown"

measure D17-bad-base unknown \
  "a base name that fails the branch rules leaves the paths unreadable" AUTODRIVE_QA_BASE='ma in'

measure D18-scenario-dir-irrelevant true \
  "AUTODRIVE_QA_SCENARIO_DIR does not change what the changed paths are" \
  "$B" AUTODRIVE_QA_SCENARIO_DIR="${WORK}/scenarios"

echo "== a missing gate file is loud, not silent =="
# autodrive_qa_docs.sh is a gate file. If it is deleted or made unreadable the
# predicate degrades to `unknown`, which is safe but otherwise invisible: the
# only symptom would be every documentation-only pull request recorded as
# unknown, indefinitely, with nothing to read. The load failure says so
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
if grep -q '^WARNING: autodrive_qa_docs\.sh did not load' "${ND_ERR}" 2>/dev/null; then
  pass D29c-missing-gate-file-warns "the load failure names the file on stderr instead of degrading in silence"
else
  fail D29c-missing-gate-file-warns "no WARNING naming autodrive_qa_docs.sh: $(tr '\n' '|' < "${ND_ERR}" 2>/dev/null)"
fi

echo "== the repository type does not change the measurement =="

repo Cargo.toml src/lib.rs README.md; base
change README.md
measure D19-rust-docs-only true "a Rust repository is measured the same way" "$B"

repo a/package.json b/package.json README.md; base
change README.md
measure D20-unresolved-node-docs-only true \
  "an unresolved choice of project root is measured the same way" "$B"

repo a/package.json b/package.json src/index.js README.md; base
change a/README.md src/index.js
measure D20b-unresolved-node-code false \
  "the same repository with a code file is not documentation only" "$B"

echo "== a changed path is data, never a command =="

repo package.json README.md; base
change 'docs/$(touch PWNED).md' 'docs/`touch PWNED2`.md'
measure D21-hostile-path-names true \
  "a hostile path name that still ends .md is measured as documentation and never evaluated" "$B"
no_pwned D21-no-execution

# ===========================================================================
echo "== the measurement is recorded in qa-inputs.json as a fact =="
# #1517/#1518/#2569: the criterion-1 judgement reads docs_only as a MEASURED
# field of qa-inputs.json, not as an inference. The record is written beside
# AUTODRIVE_QA_EVIDENCE and printed on stdout; both must carry the value.
ev_run() { # ev_run [VAR=value]...
  EV_DIR="${WORK}/r${N}/out"; mkdir -p "${EV_DIR}"; chmod 0700 "${EV_DIR}"
  EV_ERR="${WORK}/r${N}/ev.err"
  EV_OUT="$(env -i HOME="${TEST_HOME}" TMPDIR="${WORK}" PATH="${STUBS}:${SYSPATH}" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    REPO_PATH="${REPO}" FIXTURE_ROOT="${REPO}" CALLS="${CALLS}" \
    AUTODRIVE_QA_EVIDENCE="${EV_DIR}/qa-evidence.json" \
    AUTODRIVE_ROUND_LABEL=round-1 "$@" bash "${EVIDENCE_TOOL}" 2>"${EV_ERR}" </dev/null)" || true
  EV_OUT="${EV_OUT##*$'\n'}"
}
# ev_field <source> <field>: <source> is "stdout" or a file path; prints the
# field as a JSON value (strings unquoted), or ERR when it cannot be read.
ev_field() {
  local src="$1" field="$2"
  if [ "${src}" = stdout ]; then printf '%s' "${EV_OUT}"; else cat -- "${src}" 2>/dev/null; fi \
    | python3 -I -c '
import json, sys
try:
    v = json.load(sys.stdin)
    for k in sys.argv[1].split("."):
        v = v[k]
except Exception:
    print("ERR"); sys.exit(0)
print(v if isinstance(v, str) else json.dumps(v))' "${field}"
}
ev_expect() { # ev_expect <id> <why> <docs_only>
  local id="$1" why="$2" want="$3" out file
  out="$(ev_field stdout docs_only)"
  file="$(ev_field "${EV_DIR}/qa-inputs.json" docs_only)"
  if [ "${out}" = "${want}" ] && [ "${file}" = "${want}" ]; then pass "${id}" "${why}"
  else fail "${id}" "${why}: stdout docs_only='${out}', qa-inputs.json docs_only='${file}' (want '${want}') | $(tr '\n' '|' < "${EV_ERR}" 2>/dev/null)"; fi
}

repo package.json src/index.js README.md; base
change README.md
ev_run "$B"
ev_expect E1-record-docs-only "a Markdown-only change is recorded as docs_only=true" true

repo package.json src/index.js README.md; base
change src/index.js
ev_run "$B"
ev_expect E2-record-code "a code change is recorded as docs_only=false" false

repo package.json src/index.js README.md; base
change README.md
ev_run
ev_expect E3-record-unknown "an unmeasurable diff is recorded as docs_only=unknown" unknown

echo ""
echo "passed: ${PASS_COUNT}  failed: ${FAIL_COUNT}"
[ "${FAIL_COUNT}" -eq 0 ] || exit 1
exit 0
