#!/usr/bin/env bash
# test-issue-1763-qa-scope.sh — regression test for #1763.
#
# The merge loop's QA command now comes from the first of four sources:
#   1. env        AUTODRIVE_QA_COMMAND / AUTODRIVE_QA_COMMANDS (set, even empty)
#   2. repo-file  .amplihack/qa.toml committed at the merge base, a strict
#                 TOML subset, refused when the pull request changes it or a
#                 workflow file, or when the changed paths are unknown
#                 (qa-config-invalid, BLOCKED)
#   3. ci         exactly one clean test step in the merge base's
#                 .github/workflows/*.y(a)ml
#   4. detected   the merge base's manifests, scoped to the changed files
# Nothing in the working tree (untracked, modified or ignored) is read, and an
# unknown merge base keeps gadugi required.
# Detection picks the deepest candidate that contains every changed path,
# after same-ecosystem collapse and after setting aside paths outside every
# candidate; with no usable paths there must be one top-level candidate. Any
# other case is QA_UNRESOLVED (qa-unresolved), and no suite runs.
#
# The contract is docs/reference/auto-drive-to-merge.md, "Choosing the QA
# command" and "Project candidates and path scoping".
#
# This runs amplifier-bundle/tools/autodrive_qa_evidence.sh unmodified on
# scratch git repositories under `env -i` with HOME in this test's own
# directory (no AMPLIHACK_CLAUDE_PROFILE_ROOT, no real HOME), and with stub
# cargo/npm/pytest/gadugi-test on PATH that record "name|<dir>|argv". It also
# calls autodrive_qa_target, autodrive_qa_changed_paths, autodrive_repo_type
# and autodrive_gadugi_required through autodrive_trust.sh.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1763-qa-scope.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
TOOL="${TOOLS}/autodrive_qa_evidence.sh"
TRUST="${TOOLS}/autodrive_trust.sh"
SCOPE="${TOOLS}/autodrive_qa_scope.sh"
SOURCES="${TOOLS}/autodrive_qa_sources.sh"
SNAME="${TOOLS}/autodrive_scenario_name.sh"
GATE="${TOOLS}/autodrive_merge_gate.sh"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }
[[ -f "$TOOL" ]] || harness_error "tool not found: $TOOL"
[[ -f "$TRUST" ]] || harness_error "helper not found: $TRUST"
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to read the JSON strictly"
command -v git >/dev/null 2>&1 || harness_error "git is required to build the fixtures"

# The fixtures are built by this process; keep the developer's git config out.
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE CDPATH

valid_json() { python3 -I -c 'import json,sys; json.loads(sys.stdin.buffer.read().decode("utf-8"))' 2>/dev/null; }
field_of() {
    python3 -I -c '
import json, sys
try:
    v = json.loads(sys.stdin.buffer.read().decode("utf-8")).get(sys.argv[1], "<absent>")
except Exception:
    v = "<absent>"
print(v)' "$1"
}

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1763.XXXXXX")" || harness_error "mktemp failed"
WORK="$(cd "$WORK" && pwd -P)"
trap 'chmod -R u+rwx "$WORK" 2>/dev/null; rm -rf "$WORK"' EXIT

# --- stubs -----------------------------------------------------------------
# Each stub appends "name|<PWD relative to the fixture repo>|argv" to $CALLS.
# gadugi-test exits STUB_GADUGI_RC (default 0); the others exit 0.
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
for name in cargo pytest npm gadugi-test; do
    rc='0'; [ "$name" = gadugi-test ] && rc='${STUB_GADUGI_RC:-0}'
    cat > "$STUBS/$name" <<EOF
#!/usr/bin/env bash
rel="\${PWD#"\${FIXTURE_ROOT}"}"; rel="\${rel#/}"; [ -n "\$rel" ] || rel="."
printf '%s|%s|%s\n' "$name" "\$rel" "\$*" >> "\${CALLS}"
exit $rc
EOF
    chmod +x "$STUBS/$name"
done
SYSPATH="/usr/bin:/bin"
CARGO_ARGS="test --workspace --locked --no-fail-fast"

# --- fixtures --------------------------------------------------------------
N=0
mk() { # mk <relpath>: a directory (trailing /) or a file with one more line
    case "$1" in
        */) mkdir -p "$REPO/$1" ;;
        *) mkdir -p "$(dirname "$REPO/$1")"; printf 'x\n' >> "$REPO/$1" ;;
    esac
}
put() { mkdir -p "$(dirname "$REPO/$1")"; printf '%s' "$2" > "$REPO/$1"; } # put <relpath> <content>
# repo <path>... — a fresh, uncommitted git repository holding each path.
repo() {
    N=$((N + 1))
    REPO="$WORK/r$N/repo"; CALLS="$WORK/r$N/calls"
    mkdir -p "$REPO"; : > "$CALLS"
    git -C "$REPO" init -q . >/dev/null 2>&1 || harness_error "git init failed at $REPO"
    local p; for p in "$@"; do mk "$p"; done
}
# snap — commit the whole work tree on top of HEAD (plumbing: no hooks run).
snap() {
    ( cd "$REPO" && git add -A >/dev/null 2>&1 && t="$(git write-tree)" \
      && p="$(git rev-parse -q --verify HEAD 2>/dev/null || true)" \
      && c="$(git commit-tree "$t" ${p:+-p "$p"} -m c)" && git update-ref HEAD "$c" ) >/dev/null 2>&1 \
      || harness_error "could not commit in $REPO"
}
# base — commit, and make that commit refs/remotes/origin/main.
base() { snap; git -C "$REPO" update-ref refs/remotes/origin/main HEAD || harness_error "update-ref failed"; }
# change <path>... — touch each path and commit it as the pull request.
change() { local p; for p in "$@"; do mk "$p"; done; snap; }
# move <from> <to> — git mv, committed as the pull request.
move() { mkdir -p "$(dirname "$REPO/$2")"; git -C "$REPO" mv -- "$1" "$2" >/dev/null 2>&1 || harness_error "git mv failed"; snap; }

# run_tool [VAR=value]... — the evidence tool on $REPO, scrubbed environment.
run_tool() {
    local stdout
    ERR="$WORK/r$N/err"
    stdout="$(env -i HOME="$WORK" TMPDIR="$WORK" PATH="${STUBS}:${SYSPATH}" REPO_PATH="$REPO" \
        FIXTURE_ROOT="$REPO" CALLS="$CALLS" AUTODRIVE_ROUND_LABEL=round-1 "$@" \
        bash "${TOOL_UNDER_TEST:-$TOOL}" 2>"$ERR" </dev/null)" || true
    OUT="${stdout##*$'\n'}"
}
f() { printf '%s' "$OUT" | field_of "$1"; }
calls_of() { grep "^$1|" "$CALLS" || true; }
errtail() { tail -n 4 "$ERR" 2>/dev/null | tr '\n' '|'; }
expect() { # expect <id> <description> <key>=<value>...
    local id="$1" desc="$2" kv k want got bad=""
    shift 2
    if ! printf '%s' "$OUT" | valid_json; then
        fail "$id" "$desc: record is not valid JSON: $(printf '%q' "${OUT:0:300}") | stderr: $(errtail)"
        return 0
    fi
    for kv in "$@"; do
        k="${kv%%=*}"; want="${kv#*=}"; got="$(f "$k")"
        [ "$got" = "$want" ] || bad="${bad} ${k}='${got}'(want '${want}')"
    done
    if [ -z "$bad" ]; then pass "$id" "$desc"; else fail "$id" "$desc:${bad} | stderr: $(errtail)"; fi
}
expect_calls() { # expect_calls <id> <description> <tool> <expected lines or ''>
    local got
    got="$(calls_of "$3")"
    if [ "$got" = "$4" ]; then pass "$1" "$2"
    else fail "$1" "$2: $3 calls were $(printf '%q' "$got"), want $(printf '%q' "$4")"; fi
}
expect_summary_has() { # expect_summary_has <id> <needle>
    local s; s="$(f qa_summary)"
    case "$s" in *"$2"*) pass "$1" "qa_summary names '$2'" ;; *) fail "$1" "qa_summary '$s' lacks '$2'" ;; esac
}
no_pwned() { # no_pwned <id>
    if [ -z "$(find "$WORK/r$N" -name 'PWNED*' -print -quit 2>/dev/null)" ]; then pass "$1" "no injected command ran"
    else fail "$1" "an injected command ran: $(find "$WORK/r$N" -name 'PWNED*')"; fi
}

# helper <fn> [VAR=value]... — <fn> from autodrive_trust.sh on $REPO, in a
# scrubbed environment; the whole stdout followed by '#'.
helper() {
    local fn="$1"; shift
    # autodrive_qa_sources.sh is loaded too when it is beside the trust file, as the evidence tool does.
    env -i HOME="$WORK" PATH="$SYSPATH" "$@" bash -c '
        . "$1" || exit 1
        [ ! -f "${1%/*}/autodrive_qa_sources.sh" ] || . "${1%/*}/autodrive_qa_sources.sh" || exit 1
        "$2" "$3"; printf "#"' _ \
        "${TRUST_UNDER_TEST:-$TRUST}" "$fn" "$REPO" 2>/dev/null </dev/null || printf 'ERR#'
}
expect_helper() { # expect_helper <id> <fn> <want, no trailing newline> [VAR=value]...
    local id="$1" fn="$2" want="$3" got
    shift 3
    got="$(helper "$fn" "$@")"
    if [ "$got" = "$want"$'\n#' ]; then pass "$id" "$fn prints $(printf '%q' "$want")"
    else fail "$id" "$fn printed $(printf '%q' "$got"), want $(printf '%q' "$want"$'\n')"; fi
}
B=AUTODRIVE_QA_BASE=main

# ===========================================================================
echo "== #1763 helper: autodrive_qa_changed_paths =="
repo Cargo.toml src/lib.rs; base
expect_helper paths-none autodrive_qa_changed_paths "(none)" "$B"
# Unset (a preflight) means HEAD is the merge base; set but empty is unknown.
expect_helper paths-nobase autodrive_qa_changed_paths "(none)"
expect_helper paths-emptybase autodrive_qa_changed_paths "(unknown)" AUTODRIVE_QA_BASE=
change src/lib.rs crates/x/src/lib.rs
expect_helper paths-known autodrive_qa_changed_paths "crates/x/src/lib.rs src/lib.rs" "$B"
for bad in -main 'ma in' 'a..b' 'main~1' 'main:x' 'ma*n' nope; do
    expect_helper "paths-badbase:$(printf '%q' "$bad")" autodrive_qa_changed_paths "(unknown)" AUTODRIVE_QA_BASE="$bad"
done
mk untracked.rs; mk src/lib.rs
expect_helper paths-worktree "autodrive_qa_changed_paths" "crates/x/src/lib.rs src/lib.rs" "$B"

echo "== #1763 helper: the diff starts at the merge base, and nothing is fetched =="
repo Cargo.toml a/x; base
BASE_SHA="$(git -C "$REPO" rev-parse HEAD)"
change b/y
idx="$WORK/r$N/idx"
GIT_INDEX_FILE="$idx" git -C "$REPO" read-tree "$BASE_SHA"
blob="$(printf 'base\n' | git -C "$REPO" hash-object -w --stdin)"
GIT_INDEX_FILE="$idx" git -C "$REPO" update-index --add --cacheinfo "100644,${blob},a/moved-on" \
    && side="$(git -C "$REPO" commit-tree "$(GIT_INDEX_FILE="$idx" git -C "$REPO" write-tree)" -p "$BASE_SHA" -m side)" \
    && git -C "$REPO" update-ref refs/remotes/origin/main "$side" || harness_error "side commit failed"
expect_helper paths-mergebase autodrive_qa_changed_paths "b/y" "$B"
git init -q --bare "$WORK/r$N/origin.git" && git -C "$REPO" push -q "$WORK/r$N/origin.git" "HEAD:refs/heads/other" >/dev/null 2>&1
git -C "$REPO" remote add origin "$WORK/r$N/origin.git"
expect_helper paths-nofetch autodrive_qa_changed_paths "(unknown)" AUTODRIVE_QA_BASE=other
if git -C "$REPO" rev-parse -q --verify refs/remotes/origin/other >/dev/null; then
    fail paths-nofetch-ref "autodrive_qa_changed_paths fetched refs/remotes/origin/other"
else pass paths-nofetch-ref "autodrive_qa_changed_paths never fetches"; fi

echo "== #1763 helper: changed paths are sanitised, capped, and a newline is (unknown) =="
repo Cargo.toml; base
change "my file.rs"
expect_helper paths-san autodrive_qa_changed_paths "my_file.rs" "$B"
repo Cargo.toml; base
set --; for i in 01 02 03 04 05 06 07 08 09 10 11 12; do set -- "$@" "f$i"; done
change "$@"
expect_helper paths-cap autodrive_qa_changed_paths "f01 f02 f03 f04 f05 f06 f07 f08 f09 f10 (+2 more)" "$B"
repo Cargo.toml; base
change "$(printf 'a\nb')"
expect_helper paths-newline autodrive_qa_changed_paths "(unknown)" "$B"

# ===========================================================================
echo "== #1763 helper: autodrive_qa_target, the reference table =="
repo Cargo.toml crates/x/Cargo.toml crates/x/src/lib.rs; base; change crates/x/src/lib.rs
expect_helper t-root-ws autodrive_qa_target $'rust-cli\t.' "$B"
repo pyproject.toml grid-encode-decode/Cargo.toml grid-encode-decode/src/lib.rs; base
expect_helper t-jamestown-nobase autodrive_qa_target $'python\t.'
change grid-encode-decode/src/lib.rs
expect_helper t-jamestown-crate autodrive_qa_target $'rust-cli\tgrid-encode-decode' "$B"
expect_helper t-jamestown-crate-type autodrive_repo_type "rust-cli" "$B"
repo Cargo.toml web/package.json web/src/app.ts README.md; base; change web/src/app.ts
expect_helper t-web autodrive_qa_target $'node\tweb' "$B"
expect_helper t-web-greq autodrive_gadugi_required "true" "$B"
change README.md
expect_helper t-web-readme autodrive_qa_target $'rust-cli\t.' "$B"
expect_helper t-web-readme-greq autodrive_gadugi_required "false" "$B"
repo ws/Cargo.toml README.md; base
expect_helper t-ws-nobase autodrive_qa_target $'rust-cli\tws'
change README.md .gitignore .github/workflows/ci.yml
expect_helper t-ws-rootfiles autodrive_qa_target $'rust-cli\tws' "$B"
repo a/Cargo.toml b/Cargo.toml a/src/lib.rs b/src/main.rs README.md; base
expect_helper t-ab-nobase autodrive_qa_target $'unresolved\ta b'
expect_helper t-ab-none autodrive_qa_target $'unresolved\ta b' "$B"
expect_helper t-ab-type autodrive_repo_type "unknown"
expect_helper t-ab-greq autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=
expect_helper t-ab-greq-preflight autodrive_gadugi_required "false"
expect_helper t-ab-greq-base autodrive_gadugi_required "false" "$B"
change README.md
expect_helper t-ab-readme autodrive_qa_target $'unresolved\ta b' "$B"
repo a/Cargo.toml b/Cargo.toml a/src/lib.rs b/src/main.rs README.md; base; change b/src/main.rs
expect_helper t-ab-b autodrive_qa_target $'rust-cli\tb' "$B"
change README.md
expect_helper t-ab-b-readme autodrive_qa_target $'rust-cli\tb' "$B"
change a/src/lib.rs
expect_helper t-ab-both autodrive_qa_target $'unresolved\ta b' "$B"
repo a/Cargo.toml b/Cargo.toml a/src/x.rs; base; move a/src/x.rs b/src/x.rs
expect_helper t-ab-move autodrive_qa_target $'unresolved\ta b' "$B"
expect_helper t-ab-move-paths autodrive_qa_changed_paths "a/src/x.rs b/src/x.rs" "$B"
repo svc/package.json docs/svc.md; base; change docs/svc.md
expect_helper t-svc-docs autodrive_qa_target $'node\tsvc' "$B"
repo README.md; base; change README.md
expect_helper t-none autodrive_qa_target $'unknown\t.' "$B"

echo "== #1763 helper: containment is byte-exact on whole components =="
repo svc-a/Cargo.toml svc-ab/Cargo.toml svc-ab/x.rs; base; change svc-ab/x.rs
expect_helper t-prefix autodrive_qa_target $'rust-cli\tsvc-ab' "$B"

echo "== #1763 helper: the deepest containing candidate wins =="
repo Cargo.toml web/package.json web/admin/Cargo.toml web/admin/src.rs; base; change web/admin/src.rs
expect_helper t-deepest autodrive_qa_target $'rust-cli\tweb/admin' "$B"

echo "== #1763 helper: same-ecosystem collapse, different ecosystems stay apart =="
repo ws/Cargo.toml ws/a/Cargo.toml ws/a/src.rs other/Cargo.toml; base; change ws/a/src.rs
expect_helper t-collapse-cargo autodrive_qa_target $'rust-cli\tws' "$B"
repo package.json packages/p/package.json packages/p/x.js; base; change packages/p/x.js
expect_helper t-collapse-npm autodrive_qa_target $'node\t.' "$B"
repo pyproject.toml sub/pyproject.toml sub/x.py; base; change sub/x.py
expect_helper t-collapse-python autodrive_qa_target $'python\t.' "$B"
repo ws/Cargo.toml ws/a/Cargo.toml other/Cargo.toml; base
expect_helper t-collapse-list autodrive_qa_target $'unresolved\tother ws' "$B"

echo "== #1763 helper: a directory with several manifests takes Cargo, Python, Node =="
repo svc/Cargo.toml svc/pyproject.toml svc/package.json; base
expect_helper t-multi-cargo autodrive_qa_target $'rust-cli\tsvc'
repo svc/pyproject.toml svc/package.json; base
expect_helper t-multi-python autodrive_qa_target $'python\tsvc'
repo lib/setup.py; base
expect_helper t-setup-py autodrive_qa_target $'python\tlib'

echo "== #1763 helper: unchanged root rules =="
repo go.mod c/Cargo.toml c/src/lib.rs; base; change c/src/lib.rs
expect_helper t-rootfile autodrive_qa_target $'unknown\t.' "$B"
repo a/b/c/package.json; base
expect_helper t-depth4 autodrive_qa_target $'unknown\t.'
repo x/; mkdir -p "$WORK/r$N/outside"; : > "$WORK/r$N/outside/package.json"
ln -s "$WORK/r$N/outside/package.json" "$REPO/x/package.json"; base
expect_helper t-symlink-manifest autodrive_qa_target $'unknown\t.'

echo "== #1763 helper: an unsafe chosen directory is unresolved, sanitised =="
repo "my crate/Cargo.toml" "my crate/x.rs"; base; change "my crate/x.rs"
expect_helper t-unsafe autodrive_qa_target $'unresolved\tmy_crate' "$B"

echo "== #1763 helper: gadugi_required under unresolved =="
repo a/Cargo.toml b/package.json a/x.rs; base
expect_helper g-unres-node autodrive_gadugi_required "true"
expect_helper g-unres-node-type autodrive_repo_type "unknown"
change a/x.rs
expect_helper g-scoped-cargo autodrive_gadugi_required "false" "$B"
repo a/Cargo.toml b/Cargo.toml; base
expect_helper g-unres-cargo autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=
expect_helper g-unres-cargo-base autodrive_gadugi_required "false" "$B"
expect_helper g-unres-optin autodrive_gadugi_required "true" AUTODRIVE_QA_SCENARIO_DIR=tests/agentic

echo "== #1763 helper: a search error with a root project falls back to the root =="
# A missing tree object makes git ls-tree fail part way through the search.
repo Cargo.toml web/package.json z/q/package.json; base
zt="$(git -C "$REPO" rev-parse HEAD:z)" && rm -f "$REPO/.git/objects/${zt:0:2}/${zt:2}" || harness_error "could not drop tree z"
expect_helper search-error-root autodrive_qa_target $'rust-cli\t.' "$B"

# ===========================================================================
echo "== #1763 evidence: detection scoped to the changed paths =="
repo Cargo.toml crates/x/Cargo.toml crates/x/src/lib.rs; base; change crates/x/src/lib.rs
run_tool "$B"
expect ev-root-ws "root workspace, change in crates/x" qa_status=PASS qa_repo_type=rust-cli qa_dir=. \
    qa_command_source=detected qa_command_paths=crates/x/src/lib.rs qa_candidates="" \
    qa_command="cargo $CARGO_ARGS"
expect_calls ev-root-ws-calls "cargo runs at the root" cargo "cargo|.|$CARGO_ARGS"

repo a/Cargo.toml b/Cargo.toml a/src/lib.rs b/src/main.rs; base
run_tool AUTODRIVE_QA_BASE=
expect ev-unres-nobase "unknown base, two projects" qa_status=QA_UNRESOLVED qa_reason=qa-unresolved \
    qa_candidates="a b" qa_command="" qa_dir="" qa_command_source="" qa_repo_type=unknown \
    qa_command_paths="(unknown)" gadugi_required=true gadugi_status=NO_SCENARIOS qa_suite_commands_count=0
expect_summary_has ev-unres-nobase-summary "unresolved QA project candidates: a b"
expect_calls ev-unres-nobase-calls "no suite runs" cargo ""
run_tool "$B"
expect ev-unres-none "an empty diff is unresolved too" qa_status=QA_UNRESOLVED qa_command_paths="(none)" \
    gadugi_required=false gadugi_status=NOT_REQUIRED
change b/src/main.rs
: > "$CALLS"; run_tool "$B"
expect ev-b "a change inside b runs b" qa_status=PASS qa_repo_type=rust-cli qa_dir=b \
    qa_command_source=detected qa_command_paths=b/src/main.rs
expect_calls ev-b-calls "cargo runs in b" cargo "cargo|b|$CARGO_ARGS"

repo a/Cargo.toml b/Cargo.toml a/src/x.rs; base; move a/src/x.rs b/src/x.rs
run_tool "$B"
expect ev-move "a move across projects is unresolved" qa_status=QA_UNRESOLVED qa_reason=qa-unresolved \
    qa_candidates="a b" qa_command_paths="a/src/x.rs b/src/x.rs"
expect_calls ev-move-calls "no suite runs" cargo ""

repo ws/Cargo.toml README.md; base; change README.md
run_tool "$B"
expect ev-ws-readme "nested-only ws, README.md change" qa_status=PASS qa_repo_type=rust-cli qa_dir=ws
expect_calls ev-ws-readme-calls "cargo runs in ws" cargo "cargo|ws|$CARGO_ARGS"

repo svc/package.json docs/svc.md svc/tests/agentic/s.yaml; base; change docs/svc.md
run_tool "$B"
# #2219: a Node project whose change is documentation only needs no scenario.
expect ev-svc "svc Node, docs change" qa_repo_type=node qa_dir=svc gadugi_required=false docs_only=true
expect_calls ev-svc-calls "npm runs in svc" npm "npm|svc|test"

repo Cargo.toml web/package.json web/src/app.ts; base; change web/src/app.ts
run_tool "$B"
expect ev-web "root Cargo, change only under web" qa_repo_type=node qa_dir=web qa_command="npm test" gadugi_required=true
expect_calls ev-web-npm "npm runs in web" npm "npm|web|test"
expect_calls ev-web-cargo "cargo is never called" cargo ""

echo "== #1763 evidence: qa_command_paths is sanitised and capped =="
repo Cargo.toml; base
set --; for i in 01 02 03 04 05 06 07 08 09 10 11 12; do set -- "$@" "f$i"; done
change "$@" "my file.rs"
run_tool "$B"
expect ev-paths-cap "eleven or more paths are capped" qa_status=PASS \
    qa_command_paths="f01 f02 f03 f04 f05 f06 f07 f08 f09 f10 (+3 more)"
repo Cargo.toml; base; change "my file.rs"
run_tool "$B"
expect ev-paths-san "a space becomes _" qa_command_paths="my_file.rs"

echo "== #1763 evidence: qa_candidates is capped and holds no host path =="
set --; for i in 01 02 03 04 05 06 07 08 09 10 11 12; do set -- "$@" "c$i/Cargo.toml"; done
repo "$@"; base
run_tool
expect ev-cand-cap "twelve candidates" qa_status=QA_UNRESOLVED \
    qa_candidates="c01 c02 c03 c04 c05 c06 c07 c08 c09 c10 (+2 more)"
if [[ "$OUT" != *"$WORK"* ]]; then pass ev-cand-nohost "the QA_UNRESOLVED record holds no host path"
else fail ev-cand-nohost "host path leaked: $OUT"; fi

echo "== #1763 evidence: QA_UNRESOLVED with a package.json candidate runs gadugi =="
repo a/Cargo.toml b/package.json tests/agentic/s.yaml; put tests/agentic/s.yaml $'name: smoke\n'; base
run_tool
expect ev-unres-gadugi-pass "gadugi passes; the round stays unresolved" qa_status=QA_UNRESOLVED qa_reason=qa-unresolved \
    gadugi_required=true gadugi_status=PASS gadugi_scenarios_run=1 qa_candidates="a b"
if calls_of gadugi-test | grep -q '|validate ' && calls_of gadugi-test | grep -q '|run '; then
    pass ev-unres-gadugi-ran "gadugi-test validate and run were called"
else fail ev-unres-gadugi-ran "gadugi-test calls: $(calls_of gadugi-test | tr '\n' '|')"; fi
expect_calls ev-unres-gadugi-npm "no suite runs" npm ""
: > "$CALLS"; run_tool STUB_GADUGI_RC=1
expect ev-unres-gadugi-fail "gadugi fails; still QA_UNRESOLVED/qa-unresolved" qa_status=QA_UNRESOLVED \
    qa_reason=qa-unresolved gadugi_required=true gadugi_status=VALIDATE_FAILED
expect_summary_has ev-unres-gadugi-fail-summary "gadugi"
expect_summary_has ev-unres-gadugi-fail-cands "unresolved QA project candidates: a b"
rm -rf "${REPO:?}/tests"; snap
: > "$CALLS"; run_tool
expect ev-unres-noscen "no scenarios; still QA_UNRESOLVED" qa_status=QA_UNRESOLVED qa_reason=qa-unresolved \
    gadugi_status=NO_SCENARIOS
mkdir -p "$WORK/nogadugi"; for s in cargo npm pytest; do ln -sf "$STUBS/$s" "$WORK/nogadugi/$s"; done
stdout="$(env -i HOME="$WORK" TMPDIR="$WORK" PATH="$WORK/nogadugi:${SYSPATH}" REPO_PATH="$REPO" \
    FIXTURE_ROOT="$REPO" CALLS="$CALLS" bash "$TOOL" 2>/dev/null </dev/null)" || true
OUT="${stdout##*$'\n'}"
expect ev-unres-notinst "gadugi-test missing; still QA_UNRESOLVED" qa_status=QA_UNRESOLVED \
    qa_reason=qa-unresolved gadugi_status=NOT_INSTALLED

# ===========================================================================
echo "== #1763 evidence: source precedence =="
QA_OK=$'# the suite\ncommand = "cargo nextest run --workspace --locked"\ndir = "engine"\n'
repo Cargo.toml engine/Cargo.toml engine/src.rs src/lib.rs tests/parity/scenarios/r.yaml
put .amplihack/qa.toml "$QA_OK"; base; change src/lib.rs
run_tool "$B"
expect rf-ok "a valid qa.toml" qa_status=PASS qa_command_source=repo-file qa_repo_type=configured qa_dir=engine \
    qa_command="cargo nextest run --workspace --locked" qa_command_paths=src/lib.rs \
    qa_scenarios=tests/parity/scenarios/r.yaml
expect_calls rf-ok-calls "cargo runs in engine with the declared words" cargo "cargo|engine|nextest run --workspace --locked"
: > "$CALLS"; run_tool "$B" AUTODRIVE_QA_COMMAND="cargo check"
expect env-wins "AUTODRIVE_QA_COMMAND beats qa.toml" qa_status=PASS qa_command_source=env qa_repo_type=configured \
    qa_dir=. qa_command_paths=src/lib.rs
expect_calls env-wins-calls "only the env command runs" cargo "cargo|.|check"
: > "$CALLS"; run_tool "$B" AUTODRIVE_QA_COMMAND=
expect env-empty "an empty env value still wins, blocks, and records env" qa_status=BLOCKED \
    qa_reason=qa-command-missing qa_command_source=env
expect_calls env-empty-calls "nothing runs" cargo ""
: > "$CALLS"; run_tool "$B" AUTODRIVE_QA_COMMANDS="cargo check"
expect env-commands "AUTODRIVE_QA_COMMANDS is the env source" qa_status=PASS qa_command_source=env
expect_calls env-commands-calls "the entry runs at the root" cargo "cargo|.|check"
: > "$CALLS"; run_tool AUTODRIVE_QA_BASE= AUTODRIVE_QA_COMMAND="cargo check"
expect env-nobase "the env source runs without a base; gadugi is then required" qa_status=FAIL \
    qa_command_source=env qa_command_paths="(unknown)" gadugi_required=true gadugi_status=NO_SCENARIOS
expect_calls env-nobase-calls "the env command still runs" cargo "cargo|.|check"

echo "== #1763 evidence: repo-file beats ci, and an invalid file stops there =="
CI_CLEAN=$'jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo nextest run --workspace\n        working-directory: engine\n'
repo Cargo.toml engine/Cargo.toml src/lib.rs
put .amplihack/qa.toml $'command = "cargo test --lib"\n'; put .github/workflows/ci.yml "$CI_CLEAN"; base; change src/lib.rs
run_tool "$B"
expect rf-beats-ci "qa.toml beats CI" qa_status=PASS qa_command_source=repo-file qa_dir=.
expect_calls rf-beats-ci-calls "the qa.toml command runs" cargo "cargo|.|test --lib"
put .amplihack/qa.toml $'command = "cargo test"\ncommand = "cargo test"\n'; snap
base; change src/lib.rs
: > "$CALLS"; run_tool "$B"
expect rf-invalid-stops "an invalid qa.toml blocks; CI and detection are not tried" qa_status=BLOCKED \
    qa_reason=qa-config-invalid qa_command_source="" qa_dir=.
expect_calls rf-invalid-stops-calls "cargo is never called" cargo ""
: > "$CALLS"; run_tool "$B" AUTODRIVE_QA_COMMAND="cargo check"
expect rf-invalid-env "the env source does not read an invalid qa.toml" qa_status=PASS qa_command_source=env

echo "== #1763 evidence: presence of .amplihack/qa.toml =="
repo Cargo.toml engine/Cargo.toml src/lib.rs .amplihack/notes.md
put .github/workflows/ci.yml "$CI_CLEAN"; base; change engine/src/lib.rs
run_tool "$B"
expect present-dir-only "a real .amplihack without qa.toml falls through to CI" qa_status=PASS \
    qa_command_source=ci qa_repo_type=configured qa_dir=engine qa_command="cargo nextest run --workspace"
expect_calls present-dir-only-calls "the CI command runs in engine" cargo "cargo|engine|nextest run --workspace"

# rf_case <id> <token> — the last built repository must give qa-config-invalid
# with that token, run nothing, and print no qa.toml content.
rf_case() {
    : > "$CALLS"; run_tool "$B"
    expect "$1" "qa.toml problem $2" qa_status=BLOCKED qa_reason=qa-config-invalid qa_command_source=""
    expect_summary_has "$1-token" ".amplihack/qa.toml: $2"
    expect_calls "$1-calls" "nothing runs" cargo ""
    case "$(f qa_summary)" in *nextest*|*PWNED*|*touch*) fail "$1-content" "file content reached the summary: $(f qa_summary)" ;;
        *) pass "$1-content" "no qa.toml content in the summary" ;; esac
}
# rf_repo <qa.toml content> — root Cargo, engine/, the file, a base, a PR change.
rf_repo() { repo Cargo.toml engine/Cargo.toml src/lib.rs; put .amplihack/qa.toml "$1"; base; change src/lib.rs; }

repo Cargo.toml engine/ src/lib.rs real/; put real/qa.toml "$QA_OK"; ln -s real "$REPO/.amplihack"; base; change src/lib.rs
rf_case tok-symlink-dir symlink
repo Cargo.toml engine/ src/lib.rs .amplihack/; ln -s ../nowhere.toml "$REPO/.amplihack/qa.toml"; base; change src/lib.rs
rf_case tok-symlink-dangling symlink
repo Cargo.toml engine/ src/lib.rs; put .amplihack/real.toml "$QA_OK"; ln -s real.toml "$REPO/.amplihack/qa.toml"; base; change src/lib.rs
rf_case tok-symlink-file symlink
repo Cargo.toml engine/ src/lib.rs; put .amplihack "$QA_OK"; base; change src/lib.rs
rf_case tok-not-regular-file not-regular
repo Cargo.toml engine/ src/lib.rs .amplihack/qa.toml/x; base; change src/lib.rs
rf_case tok-not-regular-dir not-regular

pad="$(printf '#%.0s' $(seq 1 4200))"
body=$'command = "cargo test"\n'
fill=$((4096 - ${#body} - 1))
rf_repo "${pad:0:$fill}"$'\n'"$body"
run_tool "$B"
if [ "$(wc -c < "$REPO/.amplihack/qa.toml")" -eq 4096 ]; then
    expect tok-4096 "a 4096-byte file is accepted" qa_status=PASS qa_command_source=repo-file
else harness_error "the 4096-byte fixture is $(wc -c < "$REPO/.amplihack/qa.toml") bytes"; fi
rf_repo "${pad:0:$((fill + 1))}"$'\n'"$body"
rf_case tok-too-large too-large

for c in '[tool]\ncommand = "cargo test"\n' \
         "command = 'cargo test'\n" \
         'command = "cargo test\n' \
         'command = "cargo\\\\test"\n' \
         'command = "cargo test" # trailing\n' \
         'command = "cargo test"\r\n' \
         'command = "cargo test"\n\001\n' \
         'command = "cargo t\303\251st"\n' \
         'command\n' \
         '  = "cargo test"\n'; do
    # shellcheck disable=SC2059 # each case is a printf format on purpose
    rf_repo "$(printf "$c")"
    rf_case "tok-syntax:$(printf '%q' "$c")" syntax
done
repo Cargo.toml engine/ src/lib.rs; printf 'command = "cargo test"\n\000\n' > "$WORK/nul.toml"
mkdir -p "$REPO/.amplihack"; cp "$WORK/nul.toml" "$REPO/.amplihack/qa.toml"; base; change src/lib.rs
rf_case tok-syntax-nul syntax

rf_repo $'command = "cargo test"\ncmd = "x"\n';                      rf_case tok-unknown-key unknown-key
rf_repo $'command = "cargo test"\ncommand = "cargo test"\n';         rf_case tok-duplicate-key duplicate-key
rf_repo $'dir = "engine"\n';                                         rf_case tok-missing-command missing-command
rf_repo $'command = "   "\n';                                        rf_case tok-missing-spaces missing-command
rf_repo '';                                                          rf_case tok-missing-empty missing-command
rf_repo $'# only a comment\n\n';                                     rf_case tok-missing-comment missing-command
# shellcheck disable=SC2016 # the literal payloads must not expand
for cmd in 'cargo test $(touch PWNED)' 'cargo test `touch PWNED`' 'cargo test; touch PWNED' \
           'cargo test | tee PWNED' 'cargo test && touch PWNED' 'cargo test > PWNED' 'cargo test *' \
           'cargo test ~' 'cargo test {a,b}' 'cargo test !x' 'cargo test #x' 'cargo test ?' '$HOME/x'; do
    rf_repo "command = \"$cmd\""$'\n'
    rf_case "tok-unsafe-command:$(printf '%q' "$cmd")" unsafe-command
    no_pwned "tok-unsafe-command-exec:$(printf '%q' "$cmd")"
done
for d in /tmp -engine .. engine/.. engine//x 'en gine' 'engine/' 'eng$ine'; do
    rf_repo "$(printf 'command = "cargo test"\ndir = "%s"\n' "$d")"
    rf_case "tok-unsafe-dir:$(printf '%q' "$d")" unsafe-dir
done
rf_repo $'command = "cargo test"\ndir = "nope"\n';                   rf_case tok-dir-not-found dir-not-found
mkdir -p "$WORK/escape"
repo Cargo.toml src/lib.rs; ln -s "$WORK/escape" "$REPO/engine"
put .amplihack/qa.toml $'command = "cargo test"\ndir = "engine"\n'; base; change src/lib.rs
rf_case tok-dir-escape dir-not-found

rf_repo "$QA_OK"; put .amplihack/qa.toml $'command = "true"\n'; snap
rf_case tok-changed-qa changed-in-pr
# qa.toml is read from the merge base, so a workflow edit cannot change it.
rf_repo "$QA_OK"; change .github/workflows/x.yml
: > "$CALLS"; run_tool "$B"
expect tok-changed-ci "a workflow edit leaves qa.toml in force" qa_command_source=repo-file qa_dir=engine \
    qa_command="cargo nextest run --workspace --locked"
expect_calls tok-changed-ci-calls "the declared command runs" cargo "cargo|engine|nextest run --workspace --locked"
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .amplihack/qa.toml "$QA_OK"; base
: > "$CALLS"; run_tool "$B"
expect tok-paths-none "an empty diff is paths-empty" qa_status=BLOCKED qa_reason=qa-config-invalid
expect_summary_has tok-paths-none-token ".amplihack/qa.toml: paths-empty"
expect_helper tok-paths-none-helper autodrive_qa_config $'invalid\tpaths-empty' "$B"
: > "$CALLS"; run_tool
expect tok-paths-unknown "no base is paths-unknown" qa_status=BLOCKED qa_reason=qa-config-invalid
expect_summary_has tok-paths-unknown-token ".amplihack/qa.toml: paths-unknown"
expect_calls tok-paths-unknown-calls "nothing runs" cargo ""

rf_repo $'command="cargo test"\n'
run_tool "$B"
expect rf-nospace "spaces around = are optional" qa_status=PASS qa_command_source=repo-file
rf_repo $'command = "cargo test -p a_b:c,d@e%f+g=h"\n'
run_tool "$B"
expect rf-alphabet "every byte of the command alphabet is allowed" qa_status=PASS qa_command_source=repo-file
expect_calls rf-alphabet-calls "the words run as given" cargo "cargo|.|test -p a_b:c,d@e%f+g=h"

# ===========================================================================
echo "== #1763 evidence: the CI source =="
# ci_repo <workflow content> [file name] — root Cargo, engine/, the workflow, a base, a PR change.
ci_repo() { repo Cargo.toml engine/Cargo.toml src/lib.rs; put ".github/workflows/${2:-ci.yml}" "$1"; base; change src/lib.rs; }
ci_detected() { # ci_detected <id> <why> [qa_ci_skip]: CI gave nothing; detection ran cargo test at the root
    : > "$CALLS"; run_tool "$B"
    expect "$1" "$2: detection is used" qa_status=PASS qa_command_source=detected qa_repo_type=rust-cli qa_dir=.
    # The reason the CI source gave none is recorded, so an operator can see why it was skipped.
    [ -z "${3:-}" ] || expect "$1-skip" "$2: qa_ci_skip names the reason" qa_ci_skip="$3"
    expect_calls "$1-calls" "$2: cargo test runs at the root" cargo "cargo|.|$CARGO_ARGS"
    if ! grep -qE 'workflow|\.github|(^|[^A-Za-z])CI([^A-Za-z]|$)' "$ERR" 2>/dev/null; then
        pass "$1-silent" "$2: nothing is printed about CI"
    else fail "$1-silent" "$2: CI fallthrough printed: $(awk 'n < 2 && /workflow|\.github|CI/ { printf "%s|", $0; n++ }' "$ERR")"; fi
}
steps() { printf 'on: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n%s' "$1"; }

repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"; base; change engine/src/lib.rs
run_tool "$B"
expect ci-clean "one clean step with working-directory" qa_status=PASS qa_command_source=ci qa_repo_type=configured \
    qa_dir=engine qa_command="cargo nextest run --workspace"
expect_calls ci-clean-calls "cargo runs in engine" cargo "cargo|engine|nextest run --workspace"
ci_repo "$(steps $'      - run: cargo test --workspace\n')" ci.yaml
run_tool "$B"
expect ci-yaml-root "a .yaml file, no working-directory" qa_status=PASS qa_command_source=ci qa_dir=. qa_command="cargo test --workspace"
expect_calls ci-yaml-root-calls "cargo runs at the root" cargo "cargo|.|test --workspace"
ci_repo "$(steps $'      - name: tests\n        run: npm run test\n')"
run_tool "$B"
expect ci-npm "npm run test" qa_command_source=ci qa_command="npm run test"
expect_calls ci-npm-calls "npm runs at the root" npm "npm|.|run test"
ci_repo "$(steps $'      - run: pytest -q\n')"
run_tool "$B"
expect ci-pytest "pytest" qa_status=PASS qa_command_source=ci qa_command="pytest -q"
expect_calls ci-pytest-calls "pytest runs at the root" pytest "pytest|.|-q"

ci_repo "$(steps $'      - run: cargo test --workspace\n      - run: cargo nextest run\n')"
ci_detected ci-two-clean "two clean matches" several-matches
ci_repo "$(steps $'      - run: cargo test --workspace\n      - run: |\n          cargo nextest run --workspace\n')"
ci_detected ci-clean-plus-block "a clean match beside a block scalar" not-clean
ci_repo "$(steps $'      - run: >\n          cargo test\n')"
ci_detected ci-folded "a folded block scalar alone" not-clean
ci_repo "$(steps $'      - run: RUST_LOG=debug cargo test\n')"
ci_detected ci-env-prefix "an env-assignment prefix" not-clean
ci_repo "$(steps $'      - run: cargo test ${{ matrix.flags }}\n')"
ci_detected ci-expr "a \${{ expression" not-clean
ci_repo "$(steps $'      - run: cargo test "x"\n')"
ci_detected ci-quote "a quote" not-clean
# shellcheck disable=SC2016 # the literal payload must not expand
ci_repo "$(steps '      - run: cargo test $(touch PWNED)'$'\n')"
ci_detected ci-subst "a command substitution" not-clean
no_pwned ci-subst-exec
ci_repo "$(steps $'      - run: cargo test && touch PWNED\n')"
ci_detected ci-and "an &&" not-clean
no_pwned ci-and-exec
ci_repo "$(printf 'defaults:\n  run:\n    working-directory: engine\n%s' "$(steps $'      - run: cargo test\n')")"
ci_detected ci-defaults "a workflow-level defaults: run: working-directory" not-clean
ci_repo "$(printf 'on: [push]\njobs:\n  test:\n    defaults:\n      run:\n        working-directory: engine\n    steps:\n      - run: cargo test\n')"
ci_detected ci-job-defaults "a job-level defaults: run: working-directory" not-clean
ci_repo "$(steps $'      - run: cargo test\n        working-directory: ../x\n')"
ci_detected ci-wd-unsafe "an unsafe working-directory" unsafe-dir
ci_repo "$(steps $'      - run: cargo test\n        working-directory: nope\n')"
ci_detected ci-wd-missing "a working-directory that does not exist" dir-not-found
ci_repo "$(steps $'      - run: echo hi\n')"
ci_detected ci-none "no match" no-match

# The step must cover the change (crusty round 3): two sibling Cargo projects,
# one clean CI step in a/.
CI_A="$(steps $'      - run: cargo test\n        working-directory: a\n')"
repo a/Cargo.toml a/src/lib.rs b/Cargo.toml b/src/lib.rs; put .github/workflows/ci.yml "$CI_A"; base; change b/src/lib.rs
: > "$CALLS"; run_tool "$B"
expect ci-out-of-scope "a CI step in a/ does not test a change in b/" qa_status=PASS \
    qa_command_source=detected qa_repo_type=rust-cli qa_dir=b qa_ci_skip=out-of-scope
expect_calls ci-out-of-scope-calls "detection runs cargo test in b" cargo "cargo|b|$CARGO_ARGS"
expect_helper ci-out-of-scope-helper autodrive_qa_ci $'none\tout-of-scope' "$B"
repo a/Cargo.toml a/src/lib.rs b/Cargo.toml b/src/lib.rs; put .github/workflows/ci.yml "$CI_A"; base; change a/src/lib.rs
: > "$CALLS"; run_tool "$B"
expect ci-in-scope "a CI step in a/ covers a change in a/" qa_status=PASS \
    qa_command_source=ci qa_dir=a qa_command="cargo test" qa_ci_skip=""
expect_calls ci-in-scope-calls "the CI command runs in a" cargo "cargo|a|test"
repo a/Cargo.toml a/src/lib.rs b/Cargo.toml b/src/lib.rs; put .github/workflows/ci.yml "$CI_A"; base; change a/src/lib.rs docs/x.md
: > "$CALLS"; run_tool "$B"
expect ci-in-scope-docs "a path outside every candidate does not count against the step" \
    qa_command_source=ci qa_dir=a qa_ci_skip=""
# A `./` working-directory names the same directory (crusty round 4): sibling
# engine/ and other/ Cargo projects, one clean step in ./engine, a change in engine/.
for wdf in ./engine ././engine engine/. ./engine/./.; do
    repo engine/Cargo.toml engine/src/lib.rs other/Cargo.toml other/src/lib.rs
    put .github/workflows/ci.yml "$(steps "      - run: cargo nextest run --workspace
        working-directory: $wdf
")"; base; change engine/src/lib.rs
    : > "$CALLS"; run_tool "$B"
    expect "ci-wd-dot-slash[$wdf]" "working-directory $wdf is engine" qa_status=PASS \
        qa_command_source=ci qa_dir=engine qa_command="cargo nextest run --workspace" qa_ci_skip=""
    expect_calls "ci-wd-dot-slash-calls[$wdf]" "the declared command runs in engine" cargo "cargo|engine|nextest run --workspace"
done
expect_helper ci-wd-dot-slash-helper autodrive_qa_ci $'ok\tengine\tcargo nextest run --workspace' "$B"
# Removing `.` components does not open the rule: `.//engine` and `./../x` are still unsafe.
ci_repo "$(steps $'      - run: cargo test\n        working-directory: .//engine\n')"
ci_detected ci-wd-dot-double-slash "a .// working-directory" unsafe-dir
ci_repo "$(steps $'      - run: cargo test\n        working-directory: ./../x\n')"
ci_detected ci-wd-dot-parent "a ./.. working-directory" unsafe-dir
# jamestown shape: a root pyproject.toml, a nested Rust crate, a root pytest step.
repo pyproject.toml tests/test_x.py grid-encode-decode/Cargo.toml grid-encode-decode/src/lib.rs
put .github/workflows/ci.yml "$(steps $'      - run: pytest\n')"; base; change grid-encode-decode/src/lib.rs
: > "$CALLS"; run_tool "$B"
expect ci-root-nested "a root pytest step does not test a change in a nested crate" \
    qa_command_source=detected qa_repo_type=rust-cli qa_dir=grid-encode-decode qa_ci_skip=out-of-scope
expect_calls ci-root-nested-calls "pytest never runs" pytest ""
repo pyproject.toml tests/test_x.py grid-encode-decode/Cargo.toml grid-encode-decode/src/lib.rs
put .github/workflows/ci.yml "$(steps $'      - run: pytest\n')"; base; change tests/test_x.py
: > "$CALLS"; run_tool "$B"
expect ci-root-python "the same root step covers a root Python change" qa_command_source=ci qa_dir=. qa_command=pytest

repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"; base
put .github/workflows/ci.yml "$(steps $'      - run: cargo test --lib\n')"; snap
ci_detected ci-changed-in-pr "a workflow changed by the pull request" changed-in-pr
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"; base
: > "$CALLS"; run_tool
expect ci-nobase "no base: CI gives nothing" qa_command_source=detected qa_dir=. qa_ci_skip=paths-unknown
ci_repo "$CI_CLEAN" "c i.yml"
ci_detected ci-badname "a file name outside the allowlist" unsafe-name
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/sub/ci.yml "$CI_CLEAN"; base; change src/lib.rs
ci_detected ci-nested "a workflow below the top level" no-workflows
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"
for i in $(seq 1 50); do put ".github/workflows/w$i.yml" $'on: [push]\n'; done; base; change src/lib.rs
ci_detected ci-too-many "51 workflow files" too-many-files
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"
for i in $(seq 1 49); do put ".github/workflows/w$i.yml" $'on: [push]\n'; done; base; change engine/src/lib.rs
: > "$CALLS"; run_tool "$B"
expect ci-fifty "50 workflow files are still read" qa_status=PASS qa_command_source=ci qa_dir=engine
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"
{ printf 'on: [push]\n'; head -c 270000 /dev/zero | tr '\0' '#'; printf '\n'; } > "$REPO/.github/workflows/big.yml"; base; change src/lib.rs
ci_detected ci-too-large "a workflow over 256 KiB" too-large
repo Cargo.toml engine/Cargo.toml src/lib.rs real/workflows/; put real/workflows/ci.yml "$CI_CLEAN"
ln -s real "$REPO/.github"; base; change src/lib.rs
ci_detected ci-symlink-github "a symlinked .github" no-workflows
repo Cargo.toml engine/Cargo.toml src/lib.rs .github/; put real/ci.yml "$CI_CLEAN"
ln -s ../real "$REPO/.github/workflows"; base; change src/lib.rs
ci_detected ci-symlink-workflows "a symlinked .github/workflows" no-workflows
repo Cargo.toml engine/Cargo.toml src/lib.rs .github/workflows/; put real.yml "$CI_CLEAN"
ln -s ../../real.yml "$REPO/.github/workflows/ci.yml"; base; change src/lib.rs
ci_detected ci-symlink-entry "a symlinked workflow file" not-regular

echo "== #1763 evidence: amplihack-rs's own CI shape falls through to cargo test at the root =="
ci_repo "$(steps $'      - run: cargo nextest run --workspace --locked\n      - name: hook env\n        shell: bash\n        run: |\n          export GIT_DIR=x\n          cargo nextest run --locked \\\n            -p amplihack-git\n      - run: cargo test --workspace --doc --locked\n')"
ci_detected ci-own-shape "the amplihack-rs ci.yml shape" not-clean
repo Cargo.toml crates/a/Cargo.toml src/lib.rs; mkdir -p "$REPO/.github/workflows"
cp "$REPO_ROOT"/.github/workflows/*.yml "$REPO/.github/workflows/" 2>/dev/null || true
base; change src/lib.rs
ci_detected ci-own-real "a copy of this repository's own workflows"

# ===========================================================================
echo "== #1763 merge-base tree: the working tree never chooses the command =="
# Untracked, modified and ignored files, and a different case, cannot change
# qa.toml, the CI command or the candidate manifests.
repo Cargo.toml engine/Cargo.toml src/lib.rs; base; change src/lib.rs
put .amplihack/qa.toml $'command = "true"\n'
expect_helper mb-untracked-qa autodrive_qa_config "none" "$B"
: > "$CALLS"; run_tool "$B"
expect mb-untracked-qa-ev "an untracked qa.toml is not read" qa_status=PASS qa_command_source=detected qa_dir=.
expect_calls mb-untracked-qa-calls "detection runs cargo test, not the untracked command" cargo "cargo|.|$CARGO_ARGS"
rf_repo "$QA_OK"; put .amplihack/qa.toml $'command = "true"\n'
expect_helper mb-modified-qa autodrive_qa_config $'ok\tengine\tcargo nextest run --workspace --locked' "$B"
: > "$CALLS"; run_tool "$B"
expect mb-modified-qa-ev "a modified tracked qa.toml gives the merge-base command" qa_status=PASS qa_command_source=repo-file \
    qa_command="cargo nextest run --workspace --locked"
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .gitignore $'.amplihack/\n'; base; change src/lib.rs
put .amplihack/qa.toml $'command = "true"\n'
expect_helper mb-ignored-qa autodrive_qa_config "none" "$B"
repo Cargo.toml engine/Cargo.toml src/lib.rs; base; put .Amplihack/QA.toml $'command = "true"\n'; change src/lib.rs
expect_helper mb-case-qa autodrive_qa_config "none" "$B"
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .amplihack/real.toml "$QA_OK"; ln -s real.toml "$REPO/.amplihack/qa.toml"; base; change src/lib.rs
if git -C "$REPO" ls-tree HEAD -- .amplihack/qa.toml | grep -q '^120000 '; then
    expect_helper mb-symlink-qa autodrive_qa_config $'invalid\tsymlink' "$B"
else harness_error "the qa.toml symlink fixture is not mode 120000"; fi
repo Cargo.toml engine/Cargo.toml src/lib.rs; base; change src/lib.rs
put .github/workflows/x.yml "$(steps $'      - run: cargo test\n')"
expect_helper mb-untracked-ci autodrive_qa_ci $'none\tno-workflows' "$B"
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"; base; change engine/src/lib.rs
put .github/workflows/x.yml "$(steps $'      - run: cargo test\n')"
put .github/workflows/ci.yml "$(steps $'      - run: cargo test --lib\n')"
expect_helper mb-untracked-ci-base autodrive_qa_ci $'ok\tengine\tcargo nextest run --workspace' "$B"

echo "== #1763 merge-base tree: a manifest the pull request adds never makes gadugi optional =="
repo package.json src/x.js; base; put src/Cargo.toml $'[package]\nname = "x"\n'; change src/x.js
expect_helper h1-nested-greq autodrive_gadugi_required "true" "$B"
expect_helper h1-nested-target autodrive_qa_target $'node\t.' "$B"
repo package.json src/x.js; base; put Cargo.toml $'[package]\nname = "x"\n'; change src/x.js
expect_helper h1-root-greq autodrive_gadugi_required "true" "$B"
expect_helper h1-root-target autodrive_qa_target $'node\t.' "$B"
repo package.json src/x.js; base; change src/x.js; put src/Cargo.toml $'[package]\nname = "x"\n'
expect_helper h1-untracked-target autodrive_qa_target $'node\t.' "$B"
repo Cargo.toml a/x.rs; base; change a/x.rs; mk a/package.json
expect_helper h1-untracked-node autodrive_qa_target $'rust-cli\t.' "$B"

echo "== #1763 merge-base tree: an unknown merge base keeps gadugi required =="
repo Cargo.toml src/lib.rs; base; change src/lib.rs
expect_helper mb-empty-greq autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=
expect_helper mb-bad-greq autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=nope
expect_helper mb-noref-greq autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=other
expect_helper mb-unknown-target autodrive_qa_target $'rust-cli\t.' AUTODRIVE_QA_BASE=
expect_helper mb-known-greq autodrive_gadugi_required "false" "$B"
# Unset is a preflight (no pull request yet): HEAD's committed tree decides.
expect_helper mb-preflight-greq autodrive_gadugi_required "false"
repo package.json src/x.js; base
expect_helper mb-preflight-node autodrive_gadugi_required "true"

echo "== #1763 evidence: an invalid qa.toml outranks the gadugi reasons =="
repo package.json src/x.js; put .amplihack/qa.toml $'command = "npm test"\ncommand = "npm test"\n'; base; change src/x.js
: > "$CALLS"; run_tool "$B"
expect pick-config-invalid "gadugi required, broken qa.toml, no scenarios" qa_status=BLOCKED \
    qa_reason=qa-config-invalid gadugi_required=true gadugi_status=NO_SCENARIOS

# ===========================================================================
echo "== #1763 fallback: a missing or symlinked scope file blocks =="
for mode in missing symlink; do
    T="$WORK/tools-$mode"; mkdir -p "$T"
    for t in autodrive_qa_evidence.sh autodrive_state.sh autodrive_trust.sh autodrive_unmerged.sh git-repo-env.sh; do
        [ -f "$TOOLS/$t" ] && cp "$TOOLS/$t" "$T/"
    done
    [ "$mode" = symlink ] && [ -f "$SCOPE" ] && ln -s "$SCOPE" "$T/autodrive_qa_scope.sh"
    repo Cargo.toml src/lib.rs; base; change src/lib.rs
    TRUST_UNDER_TEST="$T/autodrive_trust.sh"
    expect_helper "fallback-$mode-target" autodrive_qa_target $'unknown\t.' "$B"
    expect_helper "fallback-$mode-greq" autodrive_gadugi_required "true" "$B"
    unset TRUST_UNDER_TEST
    TOOL_UNDER_TEST="$T/autodrive_qa_evidence.sh"; run_tool "$B"; unset TOOL_UNDER_TEST
    expect "fallback-$mode-evidence" "the evidence tool blocks without the scope helper" \
        qa_status=BLOCKED qa_reason=qa-command-missing
    expect_calls "fallback-$mode-calls" "cargo is never called" cargo ""
done
# The scope file is there but the sources file is not: detection works, yet
# qa.toml cannot be read, so the evidence blocks rather than ignore it.
T="$WORK/tools-nosources"; mkdir -p "$T"
for t in autodrive_qa_evidence.sh autodrive_state.sh autodrive_trust.sh autodrive_unmerged.sh autodrive_qa_scope.sh git-repo-env.sh; do
    cp "$TOOLS/$t" "$T/"
done
repo Cargo.toml src/lib.rs; base; change src/lib.rs
TRUST_UNDER_TEST="$T/autodrive_trust.sh"
expect_helper fallback-nosources-target autodrive_qa_target $'rust-cli\t.' "$B"
unset TRUST_UNDER_TEST
TOOL_UNDER_TEST="$T/autodrive_qa_evidence.sh"; run_tool "$B"; unset TOOL_UNDER_TEST
expect fallback-nosources-evidence "the evidence tool blocks without the sources file" \
    qa_status=BLOCKED qa_reason=qa-command-missing
expect_calls fallback-nosources-calls "cargo is never called" cargo ""

echo "== #1763 S3: one evidence run computes the diff and the tree walk once =="
GITBIN="$(command -v git)"; GSTUB="$WORK/gitcount"; mkdir -p "$GSTUB"; GITLOG="$WORK/gitcount.log"
cat > "$GSTUB/git" <<GITSTUB
#!/bin/sh
case " \$* " in *" diff "*) echo diff >> "$GITLOG" ;; *" ls-tree -r "*) echo walk >> "$GITLOG" ;; esac
exec "$GITBIN" "\$@"
GITSTUB
chmod +x "$GSTUB/git"
repo Cargo.toml engine/Cargo.toml src/lib.rs; put .github/workflows/ci.yml "$CI_CLEAN"; base; change engine/src/lib.rs
: > "$GITLOG"; : > "$CALLS"; run_tool "$B" PATH="${GSTUB}:${STUBS}:${SYSPATH}"
expect once-evidence "the counted run still uses the CI source" qa_status=PASS qa_command_source=ci
if [ "$(grep -c '^diff$' "$GITLOG")" = 1 ] && [ "$(grep -c '^walk$' "$GITLOG")" = 1 ]; then
    pass once-count "one git diff and one tree walk per evidence run"
else fail once-count "git diff ran $(grep -c '^diff$' "$GITLOG") times and the tree walk $(grep -c '^walk$' "$GITLOG")"; fi

echo "== #1763 gate wiring: the gate's own base decides gadugi in a mixed repository =="
repo Cargo.toml web/package.json web/src/app.ts; base; change web/src/app.ts
expect_helper gate-mixed-web autodrive_gadugi_required "true" "$B"
expect_helper gate-mixed-preflight autodrive_gadugi_required "false"
expect_helper gate-mixed-emptybase autodrive_gadugi_required "true" AUTODRIVE_QA_BASE=
repo Cargo.toml web/package.json src/lib.rs; base; change src/lib.rs
expect_helper gate-mixed-root autodrive_gadugi_required "false" "$B"
# The gate runs in the launch checkout, normally on the base branch, so it
# decides from headRefOid (AUTODRIVE_QA_HEAD), never the checkout's HEAD.
repo a/Cargo.toml web/package.json a/src/lib.rs; base; change a/src/lib.rs
PRH="$(git -C "$REPO" rev-parse HEAD)"; git -C "$REPO" update-ref HEAD refs/remotes/origin/main
expect_helper gate-head-crate autodrive_gadugi_required "false" "$B" AUTODRIVE_QA_HEAD="$PRH"
expect_helper gate-head-crate-paths autodrive_qa_changed_paths "a/src/lib.rs" "$B" AUTODRIVE_QA_HEAD="$PRH"
expect_helper gate-head-missing autodrive_gadugi_required "true" "$B" AUTODRIVE_QA_HEAD="$(printf '%040d' 0)"
expect_helper gate-head-short autodrive_gadugi_required "true" "$B" AUTODRIVE_QA_HEAD="${PRH:0:12}"
expect_helper gate-head-empty autodrive_gadugi_required "true" "$B" AUTODRIVE_QA_HEAD=
repo Cargo.toml web/package.json web/src/app.ts; base; change web/src/app.ts
PRH="$(git -C "$REPO" rev-parse HEAD)"; git -C "$REPO" update-ref HEAD refs/remotes/origin/main
expect_helper gate-head-web autodrive_gadugi_required "true" "$B" AUTODRIVE_QA_HEAD="$PRH"
# The one statement that asks, from `GADUGI_REQ="$(` to its `|| GADUGI_REQ="true"`.
GCOUNT="$(grep -v '^[[:space:]]*#' "$GATE" | grep -c 'autodrive_gadugi_required')"
GLINE="$(awk '/^[[:space:]]*GADUGI_REQ="\$\(/ { on = 1 } on { print } on && /GADUGI_REQ="true"/ { exit }' "$GATE")"
if [ "$GCOUNT" = 1 ] && printf '%s' "$GLINE" | grep -q 'autodrive_gadugi_required' \
   && printf '%s' "$GLINE" | grep -qF 'AUTODRIVE_QA_BASE=' \
   && printf '%s' "$GLINE" | grep -qF 'baseRefName' \
   && ! printf '%s' "$GLINE" | grep -qF 'AUTODRIVE_QA_BASE:-' \
   && printf '%s' "$GLINE" | grep -qF 'AUTODRIVE_QA_HEAD="$HEAD_SHA"' \
   && printf '%s' "$GLINE" | grep -qF 'GADUGI_REQ="true"'; then
    pass gate-line "the gate sets AUTODRIVE_QA_BASE and AUTODRIVE_QA_HEAD from its own baseRefName and headRefOid, fail-closed"
else fail gate-line "gate line is: ${GLINE:-<none>}"; fi

# ===========================================================================
echo "== #1763 static constraints =="
for t in "$TOOL" "$TRUST" "$SCOPE" "$SOURCES" "$SNAME" "$GATE"; do
    if [ ! -f "$t" ]; then fail "lines:${t##*/}" "missing"; continue; fi
    n="$(wc -l < "$t" | tr -d ' ')"
    if [ "$n" -le 400 ]; then pass "lines:${t##*/}" "$n lines (<= 400)"; else fail "lines:${t##*/}" "$n lines (> 400)"; fi
done
for y in autodrive-merge-round autodrive-merge-evidence autodrive-merge-loop; do
    n="$(wc -l < "$RECIPES/$y.yaml" | tr -d ' ')"
    if [ "$n" -le 400 ]; then pass "lines:$y" "$n lines (<= 400)"; else fail "lines:$y" "$n lines (> 400)"; fi
done
code() { grep -v '^[[:space:]]*#' "$1" 2>/dev/null || true; }
for f in "$SCOPE" "$SOURCES" "$SNAME"; do
    n="${f##*/}"
    if [ ! -f "$f" ]; then fail "file:$n" "$n does not exist"; continue; fi
    code "$f" | grep -qE '(^|[;&|[:space:](])eval[[:space:]]' && fail "eval:$n" "$n uses eval" || pass "eval:$n" "no eval"
    code "$f" | grep -qE 'bash -c|sh -c' && fail "shell:$n" "$n starts a shell" || pass "shell:$n" "no bash -c"
    code "$f" | grep -qE '(^|[[:space:]])fetch([[:space:]]|$)' && fail "fetch:$n" "$n fetches" || pass "fetch:$n" "never fetches"
    code "$f" | grep -qE 'system\(|\|[[:space:]]*getline' && fail "awk:$n" "an awk parser runs commands" \
        || pass "awk:$n" "awk never runs commands"
    grep -q 'LC_ALL=C' "$f" && pass "lcall:$n" "runs under LC_ALL=C" || fail "lcall:$n" "no LC_ALL=C"
    code "$f" | grep -q 'ambiguous' && fail "token:$n" "$n still says ambiguous" || pass "token:$n" "the token is unresolved"
done
for fn in autodrive_qa_target autodrive_qa_changed_paths; do
    grep -q "^${fn}()" "$SCOPE" && pass "scope-fn:$fn" "defined in autodrive_qa_scope.sh" \
        || fail "scope-fn:$fn" "not defined in autodrive_qa_scope.sh"
done
for fn in autodrive_qa_config autodrive_qa_ci autodrive_qa_report; do
    grep -q "^${fn}()" "$SOURCES" && pass "sources-fn:$fn" "defined in autodrive_qa_sources.sh" \
        || fail "sources-fn:$fn" "not defined in autodrive_qa_sources.sh"
done
# The merge gate needs only the scope file; the declared sources stay out of it.
code "$TRUST" | grep -q 'autodrive_qa_sources' && fail trust-sources "autodrive_trust.sh loads the sources file" \
    || pass trust-sources "the gate path never loads autodrive_qa_sources.sh"
grep -q 'shellcheck source=autodrive_qa_scope.sh' "$TRUST" && pass trust-directive "autodrive_trust.sh has the shellcheck source= directive" \
    || fail trust-directive "autodrive_trust.sh lacks '# shellcheck source=autodrive_qa_scope.sh'"
grep -q 'BASH_SOURCE' "$TRUST" && grep -q '_AD_SCOPE_OK' "$TRUST" && pass trust-loader "the loader uses BASH_SOURCE and _AD_SCOPE_OK" \
    || fail trust-loader "autodrive_trust.sh has no BASH_SOURCE loader with _AD_SCOPE_OK"
code "$TRUST" | grep -q '_autodrive_cargo_dirs' && fail trust-moved "_autodrive_cargo_dirs is still in autodrive_trust.sh" || pass trust-moved "the search moved out"
code "$TRUST" | grep -q 'ambiguous' && fail trust-token "autodrive_trust.sh still says ambiguous" || pass trust-token "no ambiguous token"
code "$TOOL" | grep -q 'ambiguous' && fail tool-token "autodrive_qa_evidence.sh still says ambiguous" || pass tool-token "no ambiguous token"
grep -q 'declare -F' "$TOOL" && pass tool-declare "the evidence tool checks the scope functions with declare -F" || fail tool-declare "no declare -F guard"
for k in qa_command_source qa_command_paths qa_candidates qa-unresolved qa-config-invalid QA_UNRESOLVED; do
    grep -q "$k" "$TOOL" && pass "tool-has:$k" "the evidence tool knows $k" || fail "tool-has:$k" "the evidence tool lacks $k"
done
if [ "$(code "$TOOL" | grep -c 'bash -c')" -le 1 ]; then pass tool-bash-c "bash -c only for AUTODRIVE_QA_COMMANDS entries"
else fail tool-bash-c "bash -c appears more than once in autodrive_qa_evidence.sh"; fi
if code "$TOOL" | grep -E 'Cargo\.toml|package\.json|pyproject\.toml' >/dev/null; then
    fail tool-markers "autodrive_qa_evidence.sh names a manifest outside comments"
else pass tool-markers "manifest names stay in the scope helper"; fi
STEP02="$(awk '/- id: "step-02-qa-team-scenarios"/{on=1} on && /- id: "step-03/{on=0} on' "$RECIPES/autodrive-merge-evidence.yaml")"
if printf '%s' "$STEP02" | grep -q 'RECIPE_VAR_merge_sync' && printf '%s' "$STEP02" | grep -q 'AUTODRIVE_QA_BASE='; then
    pass evidence-step02 "evidence step-02 reads base from merge_sync and passes AUTODRIVE_QA_BASE"
else fail evidence-step02 "evidence step-02 does not wire AUTODRIVE_QA_BASE from RECIPE_VAR_merge_sync"; fi
ROUND="$RECIPES/autodrive-merge-round.yaml"
if grep -n 'qa-unresolved' "$ROUND" | grep -q 'qa-config-invalid' \
   || { grep -q 'qa-unresolved' "$ROUND" && grep -q 'qa-config-invalid' "$ROUND"; }; then
    if grep -E 'qa-unresolved|qa-config-invalid' -A1 "$ROUND" | grep -q 'AUTODRIVE_QA_COMMAND' \
       && grep -E 'qa-unresolved|qa-config-invalid' -A1 "$ROUND" | grep -q 'qa.toml'; then
        pass round-map "the round maps qa-unresolved and qa-config-invalid to the operator action"
    else fail round-map "the round names the tokens without the AUTODRIVE_QA_COMMAND / qa.toml action"; fi
else fail round-map "autodrive-merge-round.yaml does not name qa-unresolved and qa-config-invalid"; fi
if grep -q '"autodrive_qa_scope.sh"' "$REPO_ROOT/crates/amplihack-cli/src/commands/recipe/run/merge_guard.rs"; then
    pass gate-files "GATE_FILES lists autodrive_qa_scope.sh"
else fail gate-files "GATE_FILES does not list autodrive_qa_scope.sh"; fi
if grep -q 'test-issue-1763-qa-scope.sh' "$REPO_ROOT/.github/workflows/ci.yml"; then pass ci-wired "CI runs this test"
else fail ci-wired ".github/workflows/ci.yml does not run test-issue-1763-qa-scope.sh"; fi
if command -v shellcheck >/dev/null 2>&1; then
    if [ -f "$SCOPE" ] && (cd "$TOOLS" && shellcheck -x -S warning autodrive_qa_scope.sh autodrive_trust.sh autodrive_qa_evidence.sh \
            autodrive_merge_gate.sh autodrive_qa_sources.sh autodrive_scenario_name.sh) >/dev/null 2>&1; then
        pass shellcheck "the scope file, autodrive_trust.sh, the evidence tool and the gate are clean at -S warning"
    else fail shellcheck "shellcheck -S warning reports problems (or the scope file is missing)"; fi
else
    echo "  SKIP[shellcheck]: shellcheck not installed"
fi

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
