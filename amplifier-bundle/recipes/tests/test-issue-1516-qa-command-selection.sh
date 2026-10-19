#!/usr/bin/env bash
# test-issue-1516-qa-command-selection.sh — regression test for #1516.
#
# The merge loop's QA command used to come from root manifests only: a
# repository whose Cargo workspace sits in a subdirectory got
# qa_status=BLOCKED, and a root holding both package.json and pyproject.toml
# was run as Node. The selection order is now:
#   1. AUTODRIVE_QA_COMMAND (run in AUTODRIVE_QA_DIR) / AUTODRIVE_QA_COMMANDS
#   2. a root Cargo.toml                       -> cargo test, in .
#   3. a root pyproject.toml or setup.py       -> pytest, in .  (beats package.json)
#   4. a root package.json                     -> npm test, in .
#   5. otherwise a nested Cargo.toml within depth 3 (target, worktrees,
#      node_modules and hidden directories pruned; symlinks not followed):
#      one hit, or a first sorted hit that is an ancestor of every other hit,
#      runs cargo test in that directory; anything else is BLOCKED.
# A root Python or Node project always wins over a nested crate (the
# mistt-repo/jamestown shape: root pytest suite + grid-encode-decode/Cargo.toml).
#
# This runs amplifier-bundle/tools/autodrive_qa_evidence.sh, unmodified, on
# scratch git repositories, with stub cargo/pytest/npm/gadugi-test on PATH that
# record only "name|<dir relative to the repo>|argv". It also calls
# autodrive_qa_target / autodrive_repo_type / autodrive_gadugi_required from
# autodrive_trust.sh directly to pin the helper contract.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1516-qa-command-selection.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOL="${REPO_ROOT}/amplifier-bundle/tools/autodrive_qa_evidence.sh"
TRUST="${REPO_ROOT}/amplifier-bundle/tools/autodrive_trust.sh"

harness_error() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

[[ -f "$TOOL" ]] || harness_error "tool not found: $TOOL"
[[ -f "$TRUST" ]] || harness_error "helper not found: $TRUST"
command -v python3 >/dev/null 2>&1 || harness_error "python3 is required to validate the JSON strictly"
command -v git >/dev/null 2>&1 || harness_error "git is required to build the fixtures"
valid_json() { python3 -I -c 'import json,sys; json.loads(sys.stdin.buffer.read().decode("utf-8"))' 2>/dev/null; }
# field_of <key> — the string value, or <absent> when the record lacks the key
# or is not JSON (so a missing field reads as a failure, not a harness error).
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

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1516.XXXXXX")" || harness_error "mktemp failed"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

# --- stubs -----------------------------------------------------------------
# Each stub appends "name|<PWD relative to the fixture repo>|argv" to $CALLS
# and exits 0. Nothing from the environment is echoed.
STUBS="$WORK/stubs"
NOCARGO="$WORK/stubs-nocargo"
mkdir -p "$STUBS" "$NOCARGO"
for name in cargo pytest npm gadugi-test; do
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
# new_repo <path>... — a fresh git repository holding empty files at each
# relative <path> (a trailing / makes a directory). Sets REPO and CALLS.
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
      && git update-ref HEAD "$c" ) >/dev/null 2>&1 \
      || harness_error "could not create a scratch git repo at $REPO"
}

# run_tool [VAR=value]... — runs the tool on $REPO with a scrubbed environment
# (every AUTODRIVE_QA_* unset unless given). Sets OUT (the record) and ERR.
STUBPATH="$STUBS"
run_tool() {
    local stdout
    ERR="$WORK/r$N/err"
    stdout="$(env -i HOME="$WORK" TMPDIR="$WORK" PATH="${STUBPATH}:${SYSPATH}" REPO_PATH="$REPO" \
        FIXTURE_ROOT="$REPO" CALLS="$CALLS" AUTODRIVE_ROUND_LABEL=round-1 "$@" \
        bash "$TOOL" 2>"$ERR" </dev/null)" || true
    OUT="${stdout##*$'\n'}"
}
f() { printf '%s' "$OUT" | field_of "$1"; }
calls_of() { grep "^$1|" "$CALLS" || true; }
errtail() { tail -n 4 "$ERR" 2>/dev/null | tr '\n' '|'; }

# expect <id> <description> <key>=<value>... — checks fields of $OUT.
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
# target_of <dir> — autodrive_qa_target's whole stdout, with a sentinel so a
# missing or extra trailing newline is visible.
target_of() { (. "$TRUST" && autodrive_qa_target "$1"; printf '#') 2>/dev/null || printf 'ERR#'; }
# shellcheck source=/dev/null
type_req_of() { (. "$TRUST" && autodrive_repo_type "$1" && autodrive_gadugi_required "$1") 2>/dev/null | tr '\n' ' '; }
expect_target() { # expect_target <id> <want stdout, without trailing newline>
    local got
    got="$(target_of "$REPO")"
    if [ "$got" = "$2"$'\n#' ]; then pass "$1" "autodrive_qa_target prints $(printf '%q' "$2")"
    else fail "$1" "autodrive_qa_target printed $(printf '%q' "$got"), want $(printf '%q' "$2"$'\n')"; fi
}
expect_type_req() { # expect_type_req <id> <type> <gadugi_required>
    local got
    got="$(type_req_of "$REPO")"
    if [ "$got" = "$2 $3 " ]; then pass "$1" "autodrive_repo_type/gadugi_required = $2/$3"
    else fail "$1" "autodrive_repo_type/gadugi_required gave '$got', want '$2 $3 '"; fi
}

CARGO_ARGS="test --workspace --locked --no-fail-fast"

echo "== #1516 (a): AUTODRIVE_QA_COMMAND runs in AUTODRIVE_QA_DIR =="
new_repo ws/Cargo.toml ws/tests/parity/scenarios/s.yaml sub/
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=sub
expect a "configured command, run in sub" qa_status=PASS qa_repo_type=configured qa_dir=sub qa_command="cargo test"
expect_calls a-calls "cargo runs once, in sub" cargo "cargo|sub|test"
expect a-parity "parity scenarios still come from the detected workspace ws" \
    qa_scenarios=ws/tests/parity/scenarios/s.yaml

echo "== #1516: AUTODRIVE_QA_COMMANDS only records qa_dir '.' =="
new_repo x/y/Cargo.toml
run_tool AUTODRIVE_QA_COMMANDS="cargo check"
expect commands "COMMANDS entries run from the root" qa_status=PASS qa_repo_type=configured qa_dir=.
expect_calls commands-calls "the entry runs at the root" cargo "cargo|.|check"

echo "== #1516: set-but-empty AUTODRIVE_QA_COMMAND still means no command =="
new_repo Cargo.toml
run_tool AUTODRIVE_QA_COMMAND=
expect empty "empty override is BLOCKED, not detected" qa_status=BLOCKED qa_reason=qa-command-missing \
    qa_repo_type=configured qa_dir=.
expect_calls empty-calls "cargo is never called" cargo ""

echo "== #1516 (b): root Cargo.toml =="
new_repo Cargo.toml tests/parity/scenarios/r.yaml
run_tool
expect b "root crate" qa_status=PASS qa_repo_type=rust-cli qa_dir=. gadugi_required=false \
    qa_command="cargo $CARGO_ARGS" qa_scenarios=tests/parity/scenarios/r.yaml
expect_calls b-calls "cargo runs at the root" cargo "cargo|.|$CARGO_ARGS"
expect_target b-target $'rust-cli\t.'

echo "== #1516 (c): a single nested x/y/Cargo.toml runs in x/y =="
new_repo x/y/Cargo.toml
run_tool
expect c "nested crate is not BLOCKED" qa_status=PASS qa_repo_type=rust-cli qa_dir=x/y gadugi_required=false \
    qa_command="cargo $CARGO_ARGS"
expect_calls c-calls "cargo runs in x/y" cargo "cargo|x/y|$CARGO_ARGS"
expect_target c-target $'rust-cli\tx/y'
expect_type_req c-type rust-cli false
qd="$(f qa_dir)"
if [[ "$qd" != /* && "$OUT" != *"$WORK"* ]]; then pass c-sec-d3 "qa_dir is relative and the record holds no host path"
else fail c-sec-d3 "host path leaked: qa_dir='$qd' record=$OUT"; fi

echo "== #1516 (c'): a nested workspace root with member crates picks the ancestor =="
new_repo ws/Cargo.toml ws/crates/a/Cargo.toml
run_tool
expect c2 "workspace root ws wins over its member" qa_status=PASS qa_repo_type=rust-cli qa_dir=ws
expect_calls c2-calls "cargo runs once, in ws" cargo "cargo|ws|$CARGO_ARGS"
expect_target c2-target $'rust-cli\tws'

echo "== #1516: depth boundary — a/b/Cargo.toml (depth 3) is found =="
new_repo a/b/Cargo.toml
run_tool
expect depth3 "depth 3 is in range" qa_status=PASS qa_repo_type=rust-cli qa_dir=a/b

echo "== #1516 (d): Cargo.toml only under pruned directories is BLOCKED/unknown =="
for p in target/Cargo.toml worktrees/w/Cargo.toml node_modules/m/Cargo.toml .git/x/Cargo.toml \
         .hidden/Cargo.toml a/target/Cargo.toml a/node_modules/Cargo.toml; do
    new_repo "$p"
    run_tool
    expect "d:$p" "$p is pruned" qa_status=BLOCKED qa_reason=qa-command-missing qa_repo_type=unknown qa_dir=.
    expect_calls "d-calls:$p" "cargo is never called for $p" cargo ""
done
new_repo target/Cargo.toml
expect_target d-target $'unknown\t.'
expect_type_req d-type unknown false

echo "== #1516 (e): two unrelated nested crates are BLOCKED and both are named =="
new_repo alpha/Cargo.toml beta/Cargo.toml
run_tool
expect e "two siblings" qa_status=BLOCKED qa_reason=qa-command-missing qa_repo_type=unknown qa_dir=. gadugi_required=false
expect_calls e-calls "cargo is never called" cargo ""
sum="$(f qa_summary)"
if [[ "$sum" == *"ambiguous nested Cargo"* && "$sum" == *alpha* && "$sum" == *beta* ]]; then
    pass e-summary "summary names the ambiguity and both candidates"
else fail e-summary "summary is '$sum'"; fi
got="$(target_of "$REPO")"
want_re="^ambiguous"$'\t'"alpha(/Cargo\\.toml)? beta(/Cargo\\.toml)?"$'\n'"#\$"
if [[ "$got" =~ $want_re ]]; then
    pass e-target "autodrive_qa_target: ambiguous, space-separated sorted candidates"
else fail e-target "autodrive_qa_target printed $(printf '%q' "$got")"; fi
expect_type_req e-type unknown false
if [[ "$OUT" != *"$WORK"* ]]; then pass e-sec-d3 "the ambiguous record holds no host path"
else fail e-sec-d3 "host path leaked: $OUT"; fi

echo "== #1516: a string prefix is not an ancestor (ws + ws2) =="
new_repo ws/Cargo.toml ws2/Cargo.toml
run_tool
expect prefix "ws is not an ancestor of ws2" qa_status=BLOCKED qa_repo_type=unknown qa_dir=.
expect_calls prefix-calls "cargo is never called" cargo ""

echo "== #1516: the first sorted hit must be an ancestor of every other hit =="
new_repo m/Cargo.toml n/o/Cargo.toml
run_tool
expect not-ancestor "m and n/o are unrelated" qa_status=BLOCKED qa_repo_type=unknown

echo "== #1516 (e'): the candidate list is capped at 10 =="
set --
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do set -- "$@" "c$i/Cargo.toml"; done
new_repo "$@"
run_tool
expect e2 "twelve siblings" qa_status=BLOCKED qa_repo_type=unknown
sum="$(f qa_summary)"
if [[ "$sum" == *"(+2 more)"* && "$sum" == *c01* && "$sum" == *c10* && "$sum" != *c11* && "$sum" != *c12* ]]; then
    pass e2-summary "first 10 candidates then '(+2 more)'"
else fail e2-summary "summary is '$sum'"; fi

echo "== #1516 (f): package.json + pyproject.toml is Python =="
new_repo package.json pyproject.toml
run_tool
expect f "mixed root" qa_status=PASS qa_repo_type=python qa_dir=. qa_command=pytest \
    gadugi_required=false gadugi_status=NOT_REQUIRED
expect_calls f-pytest "pytest runs at the root" pytest "pytest|.|"
expect_calls f-npm "npm is never called" npm ""
expect_calls f-gadugi "gadugi-test is never called" gadugi-test ""
expect_target f-target $'python\t.'
expect_type_req f-type python false

echo "== #1516 (g): setup.py + package.json is Python =="
new_repo setup.py package.json
run_tool
expect g "setup.py beats package.json" qa_status=PASS qa_repo_type=python qa_dir=. gadugi_required=false
expect_calls g-npm "npm is never called" npm ""

echo "== #1516 (h): package.json alone is Node =="
new_repo package.json
run_tool
expect h "node root" qa_repo_type=node qa_dir=. qa_command="npm test" gadugi_required=true
expect_calls h-calls "npm runs at the root" npm "npm|.|test"
expect_type_req h-type node true

echo "== #1516 (i): the jamestown shape runs the root pytest suite =="
new_repo pyproject.toml tests/test_x.py grid-encode-decode/Cargo.toml grid-encode-decode/src/lib.rs
run_tool
expect i "root Python wins over a nested crate" qa_status=PASS qa_repo_type=python qa_dir=. \
    qa_command=pytest gadugi_required=false
expect_calls i-pytest "pytest runs at the root" pytest "pytest|.|"
expect_calls i-cargo "cargo is never called" cargo ""
expect_target i-target $'python\t.'

echo "== #1516 (j): package.json + a nested Cargo.toml is Node =="
new_repo package.json sub/Cargo.toml
run_tool
expect j "root Node wins over a nested crate" qa_repo_type=node qa_dir=. gadugi_required=true
expect_calls j-npm "npm runs at the root" npm "npm|.|test"
expect_calls j-cargo "cargo is never called" cargo ""

echo "== #1516 (k): a/b/c/Cargo.toml (depth 4) is out of range =="
new_repo a/b/c/Cargo.toml
run_tool
expect k "depth 4 is BLOCKED" qa_status=BLOCKED qa_reason=qa-command-missing qa_repo_type=unknown qa_dir=.
expect_calls k-calls "cargo is never called" cargo ""

echo "== #1516: no manifest at all stays BLOCKED/unknown =="
new_repo README.md
run_tool
expect none "empty repo" qa_status=BLOCKED qa_reason=qa-command-missing qa_repo_type=unknown qa_dir=.
expect_target none-target $'unknown\t.'

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
if [ "$(f qa_reason)" != "qa-command-not-installed" ]; then pass installed "bin/x in sub is found"
else fail installed "bin/x in sub was reported not installed | stderr: $(errtail)"; fi
expect installed-status "the command runs" qa_status=PASS qa_dir=sub
expect_calls installed-calls "bin/x runs in sub" x "x|sub|"

echo "== #1516: parity fixtures of a nested workspace are listed under its directory =="
new_repo x/y/Cargo.toml x/y/tests/parity/scenarios/s.yaml tests/parity/scenarios/root.yaml
run_tool
expect parity "qa_scenarios is relative to the repository root" qa_dir=x/y \
    qa_scenarios=x/y/tests/parity/scenarios/s.yaml

echo "== #1516 SEC-V4: symlinks are never followed =="
mkdir -p "$WORK/outside"
: > "$WORK/outside/Cargo.toml"
new_repo README.md
ln -s "$WORK/outside" "$REPO/x"
run_tool
expect sec-v4-dir "a symlinked directory holding a crate is not searched" qa_status=BLOCKED qa_repo_type=unknown qa_dir=.
expect_calls sec-v4-dir-calls "cargo is never called" cargo ""
new_repo y/
ln -s "$WORK/outside/Cargo.toml" "$REPO/y/Cargo.toml"
run_tool
expect sec-v4-file "a symlinked Cargo.toml is not a hit" qa_status=BLOCKED qa_repo_type=unknown qa_dir=.
expect_calls sec-v4-file-calls "cargo is never called" cargo ""

echo "== #1516 SEC-V1/D2: unsafe directory names are BLOCKED and never reach a shell =="
# shellcheck disable=SC2016 # the literal $(...) is the payload; it must not expand
for d in 'has space' '$(touch PWNED)' 'q"uote' 'back\slash' "$(printf 'tab\tdir')"; do
    new_repo "$d/Cargo.toml"
    run_tool
    expect "sec-v1:$(printf '%q' "$d")" "unsafe name is BLOCKED with valid JSON" \
        qa_status=BLOCKED qa_reason=qa-command-missing qa_repo_type=unknown qa_dir=.
    expect_calls "sec-v1-calls:$(printf '%q' "$d")" "cargo is never called" cargo ""
    if [ ! -e "$REPO/PWNED" ] && [ ! -e "$REPO/$d/PWNED" ]; then
        pass "sec-v1-exec:$(printf '%q' "$d")" "the name was never executed"
    else fail "sec-v1-exec:$(printf '%q' "$d")" "a command substitution in a directory name ran"; fi
    got="$(target_of "$REPO")"
    case "$got" in
        ambiguous$'\t'*) pass "sec-v1-target:$(printf '%q' "$d")" "autodrive_qa_target reports ambiguous" ;;
        *) fail "sec-v1-target:$(printf '%q' "$d")" "autodrive_qa_target printed $(printf '%q' "$got")" ;;
    esac
    cand="${got#*$'\t'}"; cand="${cand%$'\n#'}"
    if [[ "$cand" =~ ^[A-Za-z0-9._/\ +\(\)-]*$ ]]; then
        pass "sec-v1-sanitised:$(printf '%q' "$d")" "the candidate list holds only safe bytes"
    else fail "sec-v1-sanitised:$(printf '%q' "$d")" "unsanitised candidate: $(printf '%q' "$cand")"; fi
done

echo "== #1516 SEC-V8: a planted ./cargo in the workspace does not pass the installed check =="
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
    expect sec-v8 "detected cargo needs cargo on PATH" qa_status=BLOCKED qa_reason=qa-command-not-installed \
        qa_repo_type=rust-cli qa_dir=x/y
    expect_calls sec-v8-calls "the planted binary never runs" planted ""
fi

echo "== #1516 A5: an AUTODRIVE_QA_DIR that does not exist is reported as such =="
new_repo Cargo.toml
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=nope
expect a5 "missing QA_DIR is BLOCKED/qa-command-missing" qa_status=BLOCKED qa_reason=qa-command-missing \
    qa_repo_type=configured qa_dir=nope
sum="$(f qa_summary)"
if [[ "$sum" == *"AUTODRIVE_QA_DIR nope not found"* && "$sum" != *"not installed"* ]]; then
    pass a5-summary "summary names the missing directory, not a missing command"
else fail a5-summary "summary is '$sum'"; fi
expect_calls a5-calls "cargo is never called" cargo ""
# The cause is sanitised: an operator-supplied name with control bytes and a
# command substitution never reaches the record raw and never runs.
# shellcheck disable=SC2016 # the literal $(...) is the payload; it must not expand
run_tool AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR="$(printf 'no\033[31mpe$(touch PWNED)')"
expect a5-san "an unsafe missing QA_DIR still gives valid JSON and BLOCKED" \
    qa_status=BLOCKED qa_reason=qa-command-missing
sum="$(f qa_summary)"
if [[ "$sum" != *$'\033'* && "$sum" == *"not found"* ]]; then
    pass a5-san-summary "the missing-directory cause carries no escape byte"
else fail a5-san-summary "summary is $(printf '%q' "$sum")"; fi
if [ ! -e "$REPO/PWNED" ]; then pass a5-san-exec "the directory name was never executed"
else fail a5-san-exec "a command substitution in AUTODRIVE_QA_DIR ran"; fi

echo "== #1516 R1: a detected command never resolves to a file inside the repository =="
# PATH begins with an empty entry (= the current directory) and the stub PATH
# holds no cargo, so the only cargo reachable is the one planted in x/y.
if PATH="$SYSPATH" command -v cargo >/dev/null 2>&1; then
    echo "  SKIP[r1]: a cargo exists in $SYSPATH on this host"
else
    for shape in empty dot; do
        new_repo x/y/Cargo.toml
        MARK="$WORK/r$N/planted-ran"
        cat > "$REPO/x/y/cargo" <<EOF
#!/usr/bin/env bash
: > "$MARK"
printf 'planted|%s\n' "\$*" >> "\${CALLS}"
exit 0
EOF
        chmod +x "$REPO/x/y/cargo"
        if [ "$shape" = empty ]; then STUBPATH=":$NOCARGO"; else STUBPATH=".:$NOCARGO"; fi
        run_tool
        STUBPATH="$STUBS"
        expect "r1-$shape" "PATH with '${shape}' entry: planted ./cargo is not installed" \
            qa_status=BLOCKED qa_reason=qa-command-not-installed qa_repo_type=rust-cli qa_dir=x/y
        expect_calls "r1-$shape-calls" "the planted cargo never runs" planted ""
        if [ ! -e "$MARK" ]; then pass "r1-$shape-marker" "no marker from the planted cargo"
        else fail "r1-$shape-marker" "the planted cargo ran"; fi
    done
    # A planted root-level cargo with an in-repo absolute PATH entry is refused too.
    new_repo Cargo.toml bin/
    MARK="$WORK/r$N/planted-ran"
    printf '#!/usr/bin/env bash\n: > "%s"\nexit 0\n' "$MARK" > "$REPO/bin/cargo"
    chmod +x "$REPO/bin/cargo"
    STUBPATH="$REPO/bin:$NOCARGO"
    run_tool
    STUBPATH="$STUBS"
    expect r1-abs "a cargo under \$ROOT on an absolute PATH entry is not installed" \
        qa_status=BLOCKED qa_reason=qa-command-not-installed qa_repo_type=rust-cli qa_dir=.
    if [ ! -e "$MARK" ]; then pass r1-abs-marker "the in-repo cargo never runs"
    else fail r1-abs-marker "the in-repo cargo ran"; fi
fi
# The resolved stub, outside the repository, still runs with its arguments.
new_repo x/y/Cargo.toml
STUBPATH=":$STUBS"
run_tool
STUBPATH="$STUBS"
expect r1-ok "an out-of-repo cargo is still used when PATH holds an empty entry" \
    qa_status=PASS qa_dir=x/y
expect_calls r1-ok-calls "the out-of-repo cargo runs in x/y with full argv" cargo "cargo|x/y|$CARGO_ARGS"

echo "== #1516 security review: cd never resolves outside the checked directory =="
# A crate in a directory named `-`: `cd -- -` would mean $OLDPWD.
new_repo -/Cargo.toml
run_tool
expect cd-dash "a crate in '-' runs there" qa_status=PASS qa_repo_type=rust-cli qa_dir=-
expect_calls cd-dash-calls "cargo runs in '-', not \$OLDPWD" cargo "cargo|-|$CARGO_ARGS"
# An exported CDPATH holding the same relative names must not redirect cd.
mkdir -p "$WORK/cdpath/x/y" "$WORK/cdpath/sub"
new_repo x/y/Cargo.toml sub/
run_tool CDPATH="$WORK/cdpath"
expect cdpath-detected "detected command ignores CDPATH" qa_status=PASS qa_dir=x/y
expect_calls cdpath-detected-calls "cargo runs in the repository's x/y" cargo "cargo|x/y|$CARGO_ARGS"
: > "$CALLS"
run_tool CDPATH="$WORK/cdpath" AUTODRIVE_QA_COMMAND="cargo test" AUTODRIVE_QA_DIR=sub
expect cdpath-word "configured command ignores CDPATH" qa_status=PASS qa_dir=sub
expect_calls cdpath-word-calls "cargo runs in the repository's sub" cargo "cargo|sub|test"
# shellcheck source=/dev/null
got="$(cd "$REPO" && CDPATH="$WORK/cdpath" && export CDPATH && (. "$TRUST" && autodrive_qa_target x/y/..) 2>/dev/null)" || got=""
if [ "$got" = $'rust-cli\ty' ]; then pass cdpath-target "autodrive_qa_target searches the given directory under CDPATH"
else fail cdpath-target "autodrive_qa_target under CDPATH printed $(printf '%q' "$got")"; fi

echo "== #1516: static constraints =="
for t in "$TOOL" "$TRUST"; do
    n="$(wc -l < "$t" | tr -d ' ')"
    if [ "$n" -le 400 ]; then pass "lines:${t##*/}" "$n lines (<= 400)"
    else fail "lines:${t##*/}" "$n lines (> 400)"; fi
done
if grep -n 'Cargo\.toml\|package\.json\|pyproject\.toml' "$TOOL" | grep -vE '^[0-9]+:[[:space:]]*#' >/dev/null; then
    fail markers "autodrive_qa_evidence.sh names a manifest outside comments; marker detection belongs to autodrive_trust.sh"
else pass markers "autodrive_qa_evidence.sh leaves manifest names to autodrive_trust.sh"; fi
if grep -q 'autodrive_qa_target()' "$TRUST" && grep -q 'autodrive_qa_target' "$TOOL"; then
    pass helper "autodrive_qa_target is defined in autodrive_trust.sh and used by the tool"
else fail helper "autodrive_qa_target must be defined in autodrive_trust.sh and called by autodrive_qa_evidence.sh"; fi
if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck -S warning "$TOOL" "$TRUST" >/dev/null 2>&1; then pass shellcheck "clean at -S warning"
    else fail shellcheck "shellcheck -S warning reports problems"; fi
else
    echo "  SKIP[shellcheck]: shellcheck not installed"
fi

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
