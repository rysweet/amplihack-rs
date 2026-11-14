#!/usr/bin/env bash
# test-issue-1516-qa-command-selection.sh: regression test for #1516.
#
# The merge loop's QA command used to come from root manifests only: a
# repository whose Cargo workspace sits in a subdirectory had no command, and
# a root holding both package.json and pyproject.toml was run as Node. The
# selection order is now:
#   1. AUTODRIVE_QA_COMMAND (run in AUTODRIVE_QA_DIR) / AUTODRIVE_QA_COMMANDS
#   2. a root Cargo.toml                       -> cargo test, in .
#   3. a root pyproject.toml or setup.py       -> pytest, in .  (beats package.json)
#   4. a root package.json                     -> npm test, in .
#   5. otherwise a nested Cargo.toml within depth 3 (target, worktrees,
#      node_modules and hidden directories pruned; symlinks not followed):
#      one hit, or a first sorted hit that is an ancestor of every other hit,
#      runs cargo test in that directory; anything else has no command.
# A root Python or Node project always wins over a nested crate (the
# mistt-repo/jamestown shape: root pytest suite + grid-encode-decode/Cargo.toml).
#
# Issue #1763 scoped step 5 to the pull request's changed paths
# (test-issue-1763-qa-scope.sh covers that); with an empty diff, as here,
# there must be exactly one top-level candidate after same-ecosystem collapse.
# The manifests come from the merge-base tree, never the working tree, so each
# fixture's one commit is also refs/remotes/origin/main and
# AUTODRIVE_QA_BASE=main. Several unrelated candidates, an unsafe directory
# name, or a search error with no root project give declared_qa.status
# "unresolved" with the candidates in declared_qa.candidates, and
# autodrive_qa_target says `unresolved`.
#
# Issue #2569 made criterion 1 an agent judgement: the tool now only measures
# facts and writes qa-inputs.json beside AUTODRIVE_QA_EVIDENCE, issuing no
# verdict and never calling gadugi-test. This test therefore checks which
# command was chosen, from which source, in which directory it ran, its
# status and exit status, the repository type, and that the record is valid
# JSON holding no host path.
#
# It runs amplifier-bundle/tools/autodrive_qa_evidence.sh, unmodified, on
# scratch git repositories, with stub cargo/pytest/npm on PATH that record
# only "name|<dir relative to the repo>|argv". It also calls
# autodrive_qa_target and autodrive_repo_type from autodrive_trust.sh
# directly to pin the helper contract.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1516-qa-command-selection.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_qa_evidence.sh"
TRUST="${REPO_ROOT}/amplifier-bundle/tools/autodrive_trust.sh"
SCOPE="${REPO_ROOT}/amplifier-bundle/tools/autodrive_qa_scope.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

[[ -f "$TOOL" ]] || harness_error "tool not found: $TOOL"
[[ -f "$TRUST" ]] || harness_error "helper not found: $TRUST"
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to validate the JSON strictly"
command -v git >/dev/null 2>&1 || harness_error "git is required to build the fixtures"
valid_json() { python3 -I -c 'import json,sys; json.loads(sys.stdin.buffer.read().decode("utf-8"))' 2>/dev/null; }
# field_of <key> - the value at a dotted path such as declared_qa.command, or
# <absent> when the record lacks it or is not JSON (so a missing field reads
# as a failure, not a harness error). Strings print as they are, null as
# `null`, booleans as true/false and arrays as their items joined by spaces.
field_of() {
    python3 -I -c '
import json, sys
def show(v):
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, list):
        return " ".join(show(x) for x in v)
    return str(v)
try:
    v = json.loads(sys.stdin.buffer.read().decode("utf-8"))
    for k in sys.argv[1].split("."):
        v = v[k] if isinstance(v, dict) and k in v else KeyError
        if v is KeyError:
            break
    v = "<absent>" if v is KeyError else show(v)
except Exception:
    v = "<absent>"
print(v)' "$1"
}

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1516.XXXXXX")" || harness_error "mktemp failed"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

# --- stubs -----------------------------------------------------------------
# Each stub appends "name|<PWD relative to the fixture repo>|argv" to $CALLS
# and exits 0. Nothing from the environment is echoed.
STUBS="$WORK/stubs"
NOCARGO="$WORK/stubs-nocargo"
mkdir -p "$STUBS" "$NOCARGO"
for name in cargo pytest npm; do
    cat > "$STUBS/$name" <<EOF
#!/usr/bin/env bash
rel="\${PWD#"\${FIXTURE_ROOT}"}"; rel="\${rel#/}"; [ -n "\$rel" ] || rel="."
printf '%s|%s|%s\n' "$name" "\$rel" "\$*" >> "\${CALLS}"
exit 0
EOF
    chmod +x "$STUBS/$name"
    [ "$name" = cargo ] || cp -p "$STUBS/$name" "$NOCARGO/$name"
done
SYSPATH="/usr/bin:/bin"

# --- fixtures --------------------------------------------------------------
N=0
# new_repo <path>... - a fresh git repository holding empty files at each
# relative <path> (a trailing / makes a directory), committed as HEAD and
# refs/remotes/origin/main. Sets REPO and CALLS.
new_repo() {
    N=$((N + 1))
    REPO="$WORK/r$N/repo"
    CALLS="$WORK/r$N/calls"
    mkdir -p "$REPO"
    : > "$CALLS"
    local p
    for p in "$@"; do
        case "$p" in
            */) mkdir -p "$REPO/$p" ;;
            *) mkdir -p "$(dirname "$REPO/$p")"; : > "$REPO/$p" ;;
        esac
    done
    ( cd "$REPO" && git init -q . && git add -A \
      && c="$(GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t \
              GIT_COMMITTER_EMAIL=t@example.invalid git commit-tree "$(git write-tree)" -m init)" \
      && git update-ref HEAD "$c" && git update-ref refs/remotes/origin/main "$c" ) >/dev/null 2>&1 \
      || harness_error "could not create a scratch git repo at $REPO"
}
# recommit - commit the work tree again (symlinks included) as HEAD and origin/main.
recommit() {
    ( cd "$REPO" && git add -A && c="$(GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t \
          GIT_COMMITTER_EMAIL=t@example.invalid git commit-tree "$(git write-tree)" -p HEAD -m again)" \
      && git update-ref HEAD "$c" && git update-ref refs/remotes/origin/main "$c" ) >/dev/null 2>&1 \
      || harness_error "could not commit again in $REPO"
}
export AUTODRIVE_QA_BASE=main

# printed_matches <qa-inputs.json> <stdout record>: the printed record is the
# file with declared_qa.log_tail replaced by log_tail_bytes, its size in
# UTF-8 bytes; the tail itself is never printed.
printed_matches() {
    [ -f "$1" ] && python3 -I -c '
import json, sys
f = json.loads(open(sys.argv[1], "rb").read().decode("utf-8"))
p = json.loads(sys.argv[2])
t = f["declared_qa"].pop("log_tail")
n = p["declared_qa"].pop("log_tail_bytes", None)
sys.exit(0 if f == p and n == len(t.encode("utf-8")) else 1)' "$1" "$2" 2>/dev/null
}
# run_tool [VAR=value]... - runs the tool on $REPO with a scrubbed environment
# (every other AUTODRIVE_QA_* unset unless given), with AUTODRIVE_QA_EVIDENCE
# in a private directory outside the repository. Sets OUT (the record from
# stdout), RC, ERR and INPUTS (the qa-inputs.json path).
STUBPATH="$STUBS"
run_tool() {
    local stdout ev="$WORK/r$N/ev"
    ERR="$WORK/r$N/err"
    INPUTS="$ev/qa-inputs.json"
    mkdir -p "$ev" && chmod 700 "$ev"
    RC=0
    stdout="$(env -i HOME="$WORK" TMPDIR="$WORK" PATH="${STUBPATH}:${SYSPATH}" REPO_PATH="$REPO" \
        FIXTURE_ROOT="$REPO" CALLS="$CALLS" AUTODRIVE_ROUND_LABEL=round-1 AUTODRIVE_QA_BASE=main \
        AUTODRIVE_QA_EVIDENCE="$ev/qa-evidence.json" "$@" \
        bash "$TOOL" 2>"$ERR" </dev/null)" || RC=$?
    OUT="${stdout##*$'\n'}"
}
f() { printf '%s' "$OUT" | field_of "$1"; }
calls_of() { grep "^$1|" "$CALLS" || true; }
errtail() { tail -n 4 "$ERR" 2>/dev/null | tr '\n' '|'; }

# expect <id> <description> <key>=<value>... - checks fields of $OUT.
expect() {
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
# expect_calls <id> <description> <tool> <expected lines, newline-joined, or ''>
expect_calls() {
    local got
    got="$(calls_of "$3")"
    if [ "$got" = "$4" ]; then pass "$1" "$2"
    else fail "$1" "$2: $3 calls were $(printf '%q' "$got"), want $(printf '%q' "$4")"; fi
}
# shellcheck source=/dev/null # the helper under test, sourced in a subshell
# target_of <dir> - autodrive_qa_target's whole stdout, with a sentinel so a
# missing or extra trailing newline is visible.
target_of() { (. "$TRUST" && autodrive_qa_target "$1"; printf '#') 2>/dev/null || printf 'ERR#'; }
# shellcheck source=/dev/null
type_of() { (. "$TRUST" && autodrive_repo_type "$1") 2>/dev/null | tr '\n' ' '; }
expect_target() { # expect_target <id> <want stdout, without trailing newline>
    local got
    got="$(target_of "$REPO")"
    if [ "$got" = "$2"$'\n#' ]; then pass "$1" "autodrive_qa_target prints $(printf '%q' "$2")"
    else fail "$1" "autodrive_qa_target printed $(printf '%q' "$got"), want $(printf '%q' "$2"$'\n')"; fi
}
expect_type() { # expect_type <id> <type>
    local got
    got="$(type_of "$REPO")"
    if [ "$got" = "$2 " ]; then pass "$1" "autodrive_repo_type = $2"
    else fail "$1" "autodrive_repo_type gave '$got', want '$2 '"; fi
}

CARGO_ARGS="test --workspace --locked --no-fail-fast"

echo "== #1516 (a): AUTODRIVE_QA_COMMAND runs in AUTODRIVE_QA_DIR =="
new_repo ws/Cargo.toml sub/
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=sub
expect a "configured command, run in sub" declared_qa.source=env declared_qa.dir=sub \
    declared_qa.command="cargo test" declared_qa.status=ran declared_qa.exit_status=0
expect_calls a-calls "cargo runs once, in sub" cargo "cargo|sub|test"

echo "== #1516: AUTODRIVE_QA_COMMANDS entries run from the root and record no dir =="
new_repo x/y/Cargo.toml
run_tool AUTODRIVE_QA_COMMANDS="cargo check"
expect commands "COMMANDS entries run from the root" declared_qa.source=env declared_qa.dir="" \
    declared_qa.command="cargo check" declared_qa.status=ran declared_qa.exit_status=0
expect_calls commands-calls "the entry runs at the root" cargo "cargo|.|check"

echo "== #1516: set-but-empty AUTODRIVE_QA_COMMAND still means no command =="
new_repo Cargo.toml
run_tool AUTODRIVE_QA_COMMAND=
expect empty "empty override is recorded as missing, not detected" declared_qa.source=env \
    declared_qa.status=missing declared_qa.command="" declared_qa.exit_status=null
expect_calls empty-calls "cargo is never called" cargo ""

echo "== #1516 (b): root Cargo.toml =="
new_repo Cargo.toml
run_tool
expect b "root crate" repo_type=rust-cli declared_qa.source=detected declared_qa.dir=. \
    declared_qa.command="cargo $CARGO_ARGS" declared_qa.status=ran declared_qa.exit_status=0
expect_calls b-calls "cargo runs at the root" cargo "cargo|.|$CARGO_ARGS"
if [ "$RC" -eq 0 ] && printed_matches "$INPUTS" "$OUT" && [ ! -e "${INPUTS%/*}/qa-evidence.json" ]; then
    pass b-file "exit 0, qa-inputs.json matches stdout and no qa-evidence.json is written"
else fail b-file "rc=$RC inputs=$(cat "$INPUTS" 2>/dev/null | head -c 200)"; fi
expect_target b-target $'rust-cli\t.'

echo "== #1516 (c): a single nested x/y/Cargo.toml runs in x/y =="
new_repo x/y/Cargo.toml
run_tool
expect c "nested crate gets a command" repo_type=rust-cli declared_qa.source=detected declared_qa.dir=x/y \
    declared_qa.command="cargo $CARGO_ARGS" declared_qa.status=ran declared_qa.exit_status=0
expect_calls c-calls "cargo runs in x/y" cargo "cargo|x/y|$CARGO_ARGS"
expect_target c-target $'rust-cli\tx/y'
expect_type c-type rust-cli
qd="$(f declared_qa.dir)"
if [[ "$qd" != /* && "$OUT" != *"$WORK"* ]]; then pass c-sec-d3 "declared_qa.dir is relative and the record holds no host path"
else fail c-sec-d3 "host path leaked: declared_qa.dir='$qd' record=$OUT"; fi

echo "== #1516 (c'): a nested workspace root with member crates picks the ancestor =="
# Two members within depth 3, so the ancestor rule (not a single hit) decides.
new_repo ws/Cargo.toml ws/a/Cargo.toml ws/b/Cargo.toml
run_tool
expect c2 "workspace root ws wins over its member" repo_type=rust-cli declared_qa.dir=ws declared_qa.status=ran declared_qa.exit_status=0
expect_calls c2-calls "cargo runs once, in ws" cargo "cargo|ws|$CARGO_ARGS"
expect_target c2-target $'rust-cli\tws'

echo "== #1516: depth boundary: a/b/Cargo.toml (depth 3) is found =="
new_repo a/b/Cargo.toml
run_tool
expect depth3 "depth 3 is in range" repo_type=rust-cli declared_qa.dir=a/b declared_qa.status=ran declared_qa.exit_status=0

echo "== #1516 (d): Cargo.toml only under pruned directories gives no command =="
for p in target/Cargo.toml worktrees/w/Cargo.toml node_modules/m/Cargo.toml .git/x/Cargo.toml \
         .hidden/Cargo.toml a/target/Cargo.toml a/node_modules/Cargo.toml; do
    new_repo "$p"
    run_tool
    expect "d:$p" "$p is pruned" repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command=""
    expect_calls "d-calls:$p" "cargo is never called for $p" cargo ""
done
new_repo target/Cargo.toml
expect_target d-target $'unknown\t.'
expect_type d-type unknown

echo "== #1516 (e), changed by #1763: two unrelated nested crates are unresolved and both are named =="
new_repo alpha/Cargo.toml beta/Cargo.toml
run_tool
expect e "two siblings" repo_type=unknown declared_qa.source=none declared_qa.status=unresolved declared_qa.dir="" \
    declared_qa.command="" declared_qa.candidates="alpha beta" declared_qa.exit_status=null
expect_calls e-calls "cargo is never called" cargo ""
expect_target e-target $'unresolved\talpha beta'
expect_type e-type unknown
if [[ "$OUT" != *"$WORK"* ]]; then pass e-sec-d3 "the unresolved record holds no host path"
else fail e-sec-d3 "host path leaked: $OUT"; fi

echo "== #1516, changed by #1763: a string prefix is not an ancestor (ws + ws2) =="
new_repo ws/Cargo.toml ws2/Cargo.toml
run_tool
expect prefix "ws is not an ancestor of ws2" repo_type=unknown declared_qa.source=none declared_qa.status=unresolved \
    declared_qa.candidates="ws ws2"
expect_calls prefix-calls "cargo is never called" cargo ""

echo "== #1516, changed by #1763: unrelated top-level candidates are unresolved =="
new_repo m/Cargo.toml n/o/Cargo.toml
run_tool
expect not-ancestor "m and n/o are unrelated" repo_type=unknown declared_qa.source=none declared_qa.status=unresolved \
    declared_qa.candidates="m n/o"

echo "== #1516 (e'), changed by #1763: the candidate list is capped at 10 =="
set --
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do set -- "$@" "c$i/Cargo.toml"; done
new_repo "$@"
run_tool
expect e2 "twelve siblings, first 10 then '(+2 more)'" repo_type=unknown declared_qa.source=none declared_qa.status=unresolved \
    declared_qa.candidates="c01 c02 c03 c04 c05 c06 c07 c08 c09 c10 (+2 more)"

echo "== #1516 (f): package.json + pyproject.toml is Python =="
new_repo package.json pyproject.toml
run_tool
expect f "mixed root" repo_type=python declared_qa.source=detected declared_qa.dir=. \
    declared_qa.command=pytest declared_qa.status=ran declared_qa.exit_status=0
expect_calls f-pytest "pytest runs at the root" pytest "pytest|.|"
expect_calls f-npm "npm is never called" npm ""
expect_target f-target $'python\t.'
expect_type f-type python

echo "== #1516 (g): setup.py + package.json is Python =="
new_repo setup.py package.json
run_tool
expect g "setup.py beats package.json" repo_type=python declared_qa.dir=. declared_qa.command=pytest declared_qa.status=ran declared_qa.exit_status=0
expect_calls g-npm "npm is never called" npm ""

echo "== #1516 (h): package.json alone is Node =="
new_repo package.json
run_tool
expect h "node root" repo_type=node declared_qa.dir=. declared_qa.command="npm test" declared_qa.status=ran declared_qa.exit_status=0
expect_calls h-calls "npm runs at the root" npm "npm|.|test"
expect_type h-type node

echo "== #1516 (i): the jamestown shape runs the root pytest suite =="
new_repo pyproject.toml tests/test_x.py grid-encode-decode/Cargo.toml grid-encode-decode/src/lib.rs
run_tool
expect i "root Python wins over a nested crate" repo_type=python declared_qa.dir=. \
    declared_qa.command=pytest declared_qa.status=ran declared_qa.exit_status=0
expect_calls i-pytest "pytest runs at the root" pytest "pytest|.|"
expect_calls i-cargo "cargo is never called" cargo ""
expect_target i-target $'python\t.'

echo "== #1516 (j): package.json + a nested Cargo.toml is Node =="
new_repo package.json sub/Cargo.toml
run_tool
expect j "root Node wins over a nested crate" repo_type=node declared_qa.dir=. declared_qa.command="npm test"
expect_calls j-npm "npm runs at the root" npm "npm|.|test"
expect_calls j-cargo "cargo is never called" cargo ""

echo "== #1516 (k): a/b/c/Cargo.toml (depth 4) is out of range =="
new_repo a/b/c/Cargo.toml
run_tool
expect k "depth 4 gives no command" repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command=""
expect_calls k-calls "cargo is never called" cargo ""

echo "== #1516: no manifest at all gives no command and type unknown =="
new_repo README.md
run_tool
expect none "empty repo" repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command="" declared_qa.exit_status=null
expect_target none-target $'unknown\t.'

echo "== #1516: a root project with no runner beats a nested helper crate (no wrong suite runs) =="
for rf in requirements.txt setup.cfg tox.ini Pipfile go.mod pom.xml build.gradle build.gradle.kts \
          Makefile makefile GNUmakefile CMakeLists.txt meson.build build.sbt Gemfile composer.json \
          mix.exs flake.nix; do
    new_repo "$rf" c/Cargo.toml c/src/lib.rs
    run_tool
    expect "rootfile:$rf" "$rf + c/Cargo.toml gives no command, not cargo in c" \
        repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command=""
    expect_calls "rootfile-calls:$rf" "cargo is never called beside $rf" cargo ""
done
new_repo go.mod tools/x/Cargo.toml
expect_target rootfile-target $'unknown\t.'
expect_type rootfile-type unknown
new_repo Makefile Cargo.toml
expect_target rootfile-cargo-root $'rust-cli\t.'
new_repo requirements.txt pyproject.toml c/Cargo.toml
expect_target rootfile-pyproject $'python\t.'

echo "== #1516, changed by #1763: a search error with no root project is unresolved, never the readable crate =="
# The search reads the merge-base tree, so a mode-000 directory hides nothing;
# a missing tree object makes git ls-tree fail part way instead.
new_repo a/Cargo.toml a/src/lib.rs z/b/Cargo.toml
zt="$(git -C "$REPO" rev-parse HEAD:z)" && rm -f "$REPO/.git/objects/${zt:0:2}/${zt:2}" || harness_error "could not drop tree z"
expect_target unreadable-target $'unresolved\t(search-error) a'
expect_type unreadable-type unknown
run_tool
expect unreadable "a crate hidden behind an unreadable tree is unresolved and names the search error" \
    repo_type=unknown declared_qa.source=none declared_qa.status=unresolved declared_qa.candidates="(search-error) a"
expect_calls unreadable-calls "cargo is never called when the search failed" cargo ""
new_repo a/Cargo.toml a/src/lib.rs z/b/Cargo.toml
chmod 000 "$REPO/z"
expect_target unreadable-dir $'unresolved\ta z/b'
chmod 755 "$REPO/z"

echo "== #1516: installed check accepts ./<command> relative to AUTODRIVE_QA_DIR =="
new_repo sub/bin/
cat > "$REPO/sub/bin/x" <<'EOF'
#!/usr/bin/env bash
rel="${PWD#"${FIXTURE_ROOT}"}"; rel="${rel#/}"
printf 'x|%s|%s\n' "${rel:-.}" "$*" >> "${CALLS}"
exit 0
EOF
chmod +x "$REPO/sub/bin/x"
run_tool AUTODRIVE_QA_COMMAND=bin/x AUTODRIVE_QA_DIR=sub
if [ "$(f declared_qa.status)" != "not-installed" ]; then pass installed "bin/x in sub is found"
else fail installed "bin/x in sub was reported not installed | stderr: $(errtail)"; fi
expect installed-status "the command runs" declared_qa.dir=sub declared_qa.command=bin/x declared_qa.status=ran declared_qa.exit_status=0
expect_calls installed-calls "bin/x runs in sub" x "x|sub|"

echo "== #1516 SEC-V4: symlinks are never followed =="
mkdir -p "$WORK/outside"
: > "$WORK/outside/Cargo.toml"
new_repo README.md
ln -s "$WORK/outside" "$REPO/x"; recommit
run_tool
expect sec-v4-dir "a symlinked directory holding a crate is not searched" repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command=""
expect_calls sec-v4-dir-calls "cargo is never called" cargo ""
new_repo y/
ln -s "$WORK/outside/Cargo.toml" "$REPO/y/Cargo.toml"; recommit
run_tool
expect sec-v4-file "a symlinked Cargo.toml is not a hit" repo_type=unknown declared_qa.source=none declared_qa.status=missing declared_qa.dir="" declared_qa.command=""
expect_calls sec-v4-file-calls "cargo is never called" cargo ""

echo "== #1516 SEC-V1/D2, changed by #1763: unsafe directory names are unresolved and never reach a shell =="
# shellcheck disable=SC2016 # the literal $(...) is the payload; it must not expand
for d in 'has space' '$(touch PWNED)' 'q"uote' 'back\slash' "$(printf 'tab\tdir')"; do
    new_repo "$d/Cargo.toml"
    run_tool
    expect "sec-v1:$(printf '%q' "$d")" "unsafe name is unresolved with valid JSON" \
        repo_type=unknown declared_qa.source=none declared_qa.status=unresolved declared_qa.dir="" declared_qa.command=""
    expect_calls "sec-v1-calls:$(printf '%q' "$d")" "cargo is never called" cargo ""
    if [ ! -e "$REPO/PWNED" ] && [ ! -e "$REPO/$d/PWNED" ]; then
        pass "sec-v1-exec:$(printf '%q' "$d")" "the name was never executed"
    else fail "sec-v1-exec:$(printf '%q' "$d")" "a command substitution in a directory name ran"; fi
    got="$(target_of "$REPO")"
    case "$got" in
        unresolved$'\t'*) pass "sec-v1-target:$(printf '%q' "$d")" "autodrive_qa_target reports unresolved" ;;
        *) fail "sec-v1-target:$(printf '%q' "$d")" "autodrive_qa_target printed $(printf '%q' "$got")" ;;
    esac
    cand="${got#*$'\t'}"; cand="${cand%$'\n#'}"
    if [[ "$cand" =~ ^[A-Za-z0-9._/\ +\(\)-]*$ ]]; then
        pass "sec-v1-sanitised:$(printf '%q' "$d")" "the candidate list holds only safe bytes"
    else fail "sec-v1-sanitised:$(printf '%q' "$d")" "unsanitised candidate: $(printf '%q' "$cand")"; fi
done

echo "== #1516 SEC-V8: a ./cargo in the workspace, off PATH, does not pass the installed check =="
if PATH="$SYSPATH" command -v cargo >/dev/null 2>&1; then
    echo "  SKIP[sec-v8]: a cargo exists in $SYSPATH on this host"
else
    new_repo x/y/Cargo.toml
    cat > "$REPO/x/y/cargo" <<'EOF'
#!/usr/bin/env bash
printf 'planted|%s\n' "$*" >> "${CALLS}"
exit 0
EOF
    chmod +x "$REPO/x/y/cargo"
    STUBPATH="$NOCARGO"
    run_tool
    STUBPATH="$STUBS"
    expect sec-v8 "detected cargo needs cargo on PATH" declared_qa.status=not-installed \
        declared_qa.cause="cargo not installed" declared_qa.exit_status=null repo_type=rust-cli declared_qa.dir=x/y
    expect_calls sec-v8-calls "the planted binary never runs" planted ""
fi

echo "== #1516 A5: an AUTODRIVE_QA_DIR that does not exist is reported as such =="
new_repo Cargo.toml
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=nope
expect a5 "missing QA_DIR is recorded as missing" declared_qa.source=env declared_qa.status=missing \
    declared_qa.dir=nope declared_qa.exit_status=null
sum="$(f declared_qa.cause)"
if [[ "$sum" == *"nope not found"* && "$sum" != *"not installed"* ]]; then
    pass a5-cause "the cause names the missing directory, not a missing command"
else fail a5-cause "cause is '$sum'"; fi
expect_calls a5-calls "cargo is never called" cargo ""
# The cause is sanitised: an operator-supplied name with control bytes and a
# command substitution never reaches the record raw and never runs.
# shellcheck disable=SC2016 # the literal $(...) is the payload; it must not expand
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR="$(printf 'no\033[31mpe$(touch PWNED)')"
expect a5-san "an unsafe missing QA_DIR still gives valid JSON and status missing" \
    declared_qa.status=missing
sum="$(f declared_qa.cause)"
if [[ "$sum" != *$'\033'* && "$sum" == *"not found"* ]]; then
    pass a5-san-cause "the missing-directory cause holds no escape byte"
else fail a5-san-cause "cause is $(printf '%q' "$sum")"; fi
if [ ! -e "$REPO/PWNED" ]; then pass a5-san-exec "the directory name was never executed"
else fail a5-san-exec "a command substitution in AUTODRIVE_QA_DIR ran"; fi

echo "== #1516 R1: the installed check is a PATH lookup, not a sandbox =="
# Branch code runs anyway (build.rs, conftest.py, scripts.test), so a program
# found on PATH runs wherever it lives, including inside the repository.
if PATH="$SYSPATH" command -v cargo >/dev/null 2>&1; then
    echo "  SKIP[r1]: a cargo exists in $SYSPATH on this host"
else
    new_repo Cargo.toml bin/
    cp -p "$STUBS/cargo" "$REPO/bin/cargo"
    STUBPATH="$REPO/bin:$NOCARGO"
    run_tool
    STUBPATH="$STUBS"
    expect r1-inrepo "a cargo under \$ROOT on PATH is installed" \
        repo_type=rust-cli declared_qa.dir=. declared_qa.status=ran declared_qa.exit_status=0
    expect_calls r1-inrepo-calls "the PATH cargo runs at the root" cargo "cargo|.|$CARGO_ARGS"
fi
new_repo x/y/Cargo.toml
STUBPATH=":$STUBS"
run_tool
STUBPATH="$STUBS"
expect r1-ok "a cargo on PATH is used when PATH also holds an empty entry" \
    declared_qa.dir=x/y declared_qa.status=ran declared_qa.exit_status=0
expect_calls r1-ok-calls "cargo runs in x/y with full argv" cargo "cargo|x/y|$CARGO_ARGS"

echo "== #1516 R2: a .venv pytest on PATH runs, detected or configured =="
if PATH="$SYSPATH" command -v pytest >/dev/null 2>&1; then
    echo "  SKIP[r2]: a pytest exists in $SYSPATH on this host"
else
    new_repo pyproject.toml .venv/bin/
    cp -p "$STUBS/pytest" "$REPO/.venv/bin/pytest"
    STUBPATH="$REPO/.venv/bin"
    run_tool
    expect r2-detected "a detected pytest in .venv on PATH runs" \
        repo_type=python declared_qa.source=detected declared_qa.dir=. declared_qa.status=ran declared_qa.exit_status=0
    expect_calls r2-calls "the .venv pytest runs at the root" pytest "pytest|.|"
    : > "$CALLS"
    run_tool AUTODRIVE_QA_COMMAND=pytest
    STUBPATH="$STUBS"
    expect r2-configured "AUTODRIVE_QA_COMMAND=pytest runs the .venv pytest" declared_qa.source=env declared_qa.dir=. declared_qa.status=ran declared_qa.exit_status=0
    expect_calls r2-configured-calls "the .venv pytest runs at the root" pytest "pytest|.|"
    # Without .venv/bin on PATH the detected pytest is not installed.
    mkdir -p "$WORK/emptypath"
    STUBPATH="$WORK/emptypath"
    run_tool
    STUBPATH="$STUBS"
    expect r2-offpath "a .venv pytest off PATH is not installed" \
        declared_qa.status=not-installed repo_type=python declared_qa.dir=.
fi

echo "== #1516 security review: cd never resolves outside the checked directory =="
# A crate in a directory named `-`: `cd -- -` would mean $OLDPWD.
new_repo -/Cargo.toml
run_tool
expect cd-dash "a crate in '-' runs there" repo_type=rust-cli declared_qa.dir=- declared_qa.status=ran declared_qa.exit_status=0
expect_calls cd-dash-calls "cargo runs in '-', not \$OLDPWD" cargo "cargo|-|$CARGO_ARGS"
# The same for a configured AUTODRIVE_QA_DIR=- : run in the repository's `-`.
: > "$CALLS"
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=-
expect cd-dash-word "AUTODRIVE_QA_DIR=- runs in the repository's '-'" declared_qa.dir=- declared_qa.status=ran declared_qa.exit_status=0
expect_calls cd-dash-word-calls "configured cargo runs in '-', not \$OLDPWD" cargo "cargo|-|test"
# An exported CDPATH holding the same relative names must not redirect cd.
mkdir -p "$WORK/cdpath/x/y" "$WORK/cdpath/sub"
new_repo x/y/Cargo.toml sub/
run_tool CDPATH="$WORK/cdpath"
expect cdpath-detected "detected command ignores CDPATH" declared_qa.dir=x/y declared_qa.status=ran declared_qa.exit_status=0
expect_calls cdpath-detected-calls "cargo runs in the repository's x/y" cargo "cargo|x/y|$CARGO_ARGS"
: > "$CALLS"
run_tool CDPATH="$WORK/cdpath" AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=sub
expect cdpath-word "configured command ignores CDPATH" declared_qa.dir=sub declared_qa.status=ran declared_qa.exit_status=0
expect_calls cdpath-word-calls "cargo runs in the repository's sub" cargo "cargo|sub|test"
# shellcheck source=/dev/null
got="$(cd "$REPO" && CDPATH="$WORK/cdpath" && export CDPATH && (. "$TRUST" && autodrive_qa_target x/y/..) 2>/dev/null)" || got=""
# <dir> is only where git runs; the tree listed is always the repository's.
if [ "$got" = $'rust-cli\tx/y' ]; then pass cdpath-target "autodrive_qa_target lists the repository's tree under CDPATH"
else fail cdpath-target "autodrive_qa_target under CDPATH printed $(printf '%q' "$got")"; fi

echo "== #1516: static constraints =="
for t in "$TOOL" "$TRUST" "$SCOPE"; do
    [ -f "$t" ] || { fail "lines:${t##*/}" "missing"; continue; }
    n="$(wc -l < "$t" | tr -d ' ')"
    if [ "$n" -le 400 ]; then pass "lines:${t##*/}" "$n lines (<= 400)"
    else fail "lines:${t##*/}" "$n lines (> 400)"; fi
done
if grep -n 'Cargo\.toml\|package\.json\|pyproject\.toml' "$TOOL" | grep -vE '^[0-9]+:[[:space:]]*#' >/dev/null; then
    fail markers "autodrive_qa_evidence.sh names a manifest outside comments; marker detection belongs to autodrive_trust.sh"
else pass markers "autodrive_qa_evidence.sh leaves manifest names to autodrive_trust.sh"; fi
SOURCES="${REPO_ROOT}/amplifier-bundle/tools/autodrive_qa_sources.sh"
if grep -q '^autodrive_qa_target()' "$SCOPE" 2>/dev/null && grep -q 'autodrive_qa_target' "$SOURCES" 2>/dev/null \
   && grep -q 'autodrive_qa_report' "$TOOL"; then
    pass helper "autodrive_qa_target is defined in autodrive_qa_scope.sh and reaches the tool through autodrive_qa_report"
else fail helper "autodrive_qa_target must be defined in autodrive_qa_scope.sh and reach autodrive_qa_evidence.sh through autodrive_qa_report"; fi
if command -v shellcheck >/dev/null 2>&1; then
    if [ -f "$SCOPE" ] && shellcheck -S warning "$TOOL" "$TRUST" "$SCOPE" >/dev/null 2>&1; then pass shellcheck "clean at -S warning"
    else fail shellcheck "shellcheck -S warning reports problems"; fi
else
    echo "  SKIP[shellcheck]: shellcheck not installed"
fi

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
