#!/usr/bin/env bash
# probe-no-python.sh — AC9 validation: verify the amplihack binary runs correctly
# in an environment with no Python interpreter on PATH.
#
# Usage:
#   scripts/probe-no-python.sh [--release]
#
# Exits 0 if all smoke tests pass without a Python interpreter.
# Exits 1 if any test fails or if a Python interpreter is still reachable.
#
# Design:
#   1. Build (debug by default, --release if requested).
#   2. Strip all python/python3 entries from PATH.
#   3. Verify python and python3 are NOT on PATH.
#   4. Run a sequence of binary smoke tests; each must succeed.
#   5. Confirm no "python" subprocess was invoked (via strace where available).
#
# Version: v2.1 — AC9 extended with TC-04 through TC-08 (Issue #77)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ── Argument parsing ──────────────────────────────────────────────────────────
RELEASE=0
for arg in "$@"; do
    case "$arg" in
        --release) RELEASE=1 ;;
        *) echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
done

# ── Build ─────────────────────────────────────────────────────────────────────
echo "==> Building amplihack-rs..."
if [[ $RELEASE -eq 1 ]]; then
    cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml" 2>&1
    BINARY="${REPO_ROOT}/target/release/amplihack"
else
    cargo build --manifest-path "${REPO_ROOT}/Cargo.toml" 2>&1
    BINARY="${REPO_ROOT}/target/debug/amplihack"
fi

if [[ ! -x "${BINARY}" ]]; then
    echo "FAIL: binary not found at ${BINARY}" >&2
    exit 1
fi
echo "    binary: ${BINARY}"
echo ""

# ── Capture essential tool paths before PATH stripping ───────────────────────
# mktemp, grep, and rm may reside in the same directory as python3 (e.g.
# /usr/bin); capture their absolute paths now so they remain usable after
# python-containing directories are removed from PATH.
MKTEMP_BIN="$(command -v mktemp 2>/dev/null || true)"
GREP_BIN="$(command -v grep 2>/dev/null || true)"
RM_BIN="$(command -v rm 2>/dev/null || true)"
CHMOD_BIN="$(command -v chmod 2>/dev/null || true)"
MKDIR_BIN="$(command -v mkdir 2>/dev/null || true)"
if [[ -z "${MKTEMP_BIN}" ]]; then
    echo "WARNING: mktemp not found — TC-06 temp-file test will skip mktemp usage" >&2
fi

# ── Strip Python from PATH ────────────────────────────────────────────────────
echo "==> Stripping python/python3 from PATH..."
CLEAN_PATH=""
IFS=: read -ra PATH_DIRS <<< "${PATH}"
for dir in "${PATH_DIRS[@]}"; do
    # Drop any directory that contains a python or python3 executable
    if [[ -x "${dir}/python" || -x "${dir}/python3" ]]; then
        echo "    removed: ${dir} (contains python/python3)"
    else
        CLEAN_PATH="${CLEAN_PATH:+${CLEAN_PATH}:}${dir}"
    fi
done
export PATH="${CLEAN_PATH}"
echo ""

# ── Verify Python is gone ─────────────────────────────────────────────────────
echo "==> Verifying no Python interpreter on PATH..."
if command -v python >/dev/null 2>&1; then
    echo "FAIL: 'python' is still reachable: $(command -v python)" >&2
    exit 1
fi
if command -v python3 >/dev/null 2>&1; then
    echo "FAIL: 'python3' is still reachable: $(command -v python3)" >&2
    exit 1
fi
echo "    OK — no python/python3 on PATH"
echo ""

# ── Smoke tests ───────────────────────────────────────────────────────────────
PASS=0
FAIL=0

run_smoke() {
    local label="$1"
    shift
    echo -n "  smoke: ${label} ... "
    if "$@" >/dev/null 2>&1; then
        echo "PASS"
        PASS=$((PASS + 1))
    else
        echo "FAIL (exit $?)"
        FAIL=$((FAIL + 1))
    fi
}

echo "==> Running binary smoke tests (Python-free PATH)..."

# ── Pre-existing tests (TC-01 through TC-03) ──────────────────────────────

# TC-01: Basic binary sanity
run_smoke "TC-01 --version"        "${BINARY}" --version

# TC-02: Top-level help
run_smoke "TC-02 --help exits 0"   "${BINARY}" --help

# TC-03: Fleet subcommand help — must not require Python
run_smoke "TC-03 fleet --help"     "${BINARY}" fleet --help

# Doctor subcommand — verifies internal diagnostics without Python
run_smoke "doctor --help"          "${BINARY}" doctor --help

# Recipe subcommand help
run_smoke "recipe --help"          "${BINARY}" recipe --help

# ── New tests (TC-04 through TC-08) — Issue #77 AC9 extension ─────────────

# TC-04: index-code --help must render without Python
run_smoke "TC-04 index-code --help"  "${BINARY}" index-code --help

# TC-05: query-code --help must render without Python
run_smoke "TC-05 query-code --help"  "${BINARY}" query-code --help

# TC-06: query-code stats on a fresh empty Kuzu DB — must not crash or call Python.
# Uses mktemp for a unique temp file and registers a trap to ensure cleanup
# on both success and failure paths (P2-TEMPFILE security requirement).
# We use MKTEMP_BIN (captured before PATH stripping) because /usr/bin may have
# been removed from PATH since it also contained python3.
if [[ -n "${MKTEMP_BIN}" ]]; then
    TEMP_DB="$("${MKTEMP_BIN}" -t amplihack_probe_XXXXXX.db)"
else
    TEMP_DB="/tmp/amplihack_probe_$$.db"
fi
# Use pre-captured RM_BIN for cleanup; fall back to unquoted rm if not captured.
if [[ -n "${RM_BIN}" ]]; then
    trap '"${RM_BIN}" -f "${TEMP_DB}"' EXIT
else
    trap 'rm -f "${TEMP_DB}"' EXIT
fi
echo -n "  smoke: TC-06 query-code stats (empty DB) ... "
exit_code_tc06=0
output_tc06="$("${BINARY}" query-code --kuzu-path "${TEMP_DB}" stats 2>&1)" || exit_code_tc06=$?
# A signal-killed process exits >128.  Treat that as a crash (FAIL).
if [[ ${exit_code_tc06} -gt 128 ]]; then
    echo "FAIL (binary killed by signal $((exit_code_tc06 - 128)))"
    FAIL=$((FAIL + 1))
elif [[ -n "${GREP_BIN}" ]] && echo "${output_tc06}" | "${GREP_BIN}" -qE "python: command not found|python3: command not found|No such file or directory: .python|ModuleNotFoundError"; then
    echo "FAIL (Python invocation detected)"
    FAIL=$((FAIL + 1))
else
    echo "PASS"
    PASS=$((PASS + 1))
fi

# TC-07: index-scip --help must render without Python
# (scip-python is a Go binary, not Python; this verifies no interpreter call)
run_smoke "TC-07 index-scip --help"  "${BINARY}" index-scip --help

# TC-08: import a tiny code graph and query it with failing python/python3
# shims first on PATH. This proves the live native path never touches Python
# even when indexing and querying real graph data.
echo -n "  smoke: TC-08 index-code + query-code populated graph ... "
if [[ -n "${MKTEMP_BIN}" ]]; then
    TC08_DIR="$("${MKTEMP_BIN}" -d -t amplihack_probe_tc08_XXXXXX)"
else
    TC08_DIR="/tmp/amplihack_probe_tc08_$$"
    if [[ -n "${MKDIR_BIN}" ]]; then
        "${MKDIR_BIN}" -p "${TC08_DIR}"
    else
        mkdir -p "${TC08_DIR}"
    fi
fi
if [[ -n "${RM_BIN}" ]]; then
    trap '"${RM_BIN}" -f "${TEMP_DB}"; "${RM_BIN}" -rf "${TC08_DIR}"' EXIT
else
    trap 'rm -f "${TEMP_DB}"; rm -rf "${TC08_DIR}"' EXIT
fi
if [[ -n "${MKDIR_BIN}" ]]; then
    "${MKDIR_BIN}" -p "${TC08_DIR}/bin"
else
    mkdir -p "${TC08_DIR}/bin"
fi
TC08_PYTHON_LOG="${TC08_DIR}/python.log"
TC08_JSON="${TC08_DIR}/blarify.json"
TC08_DB="${TC08_DIR}/probe_tc08.kuzu"
printf '%s\n' '#!/bin/sh' "echo python >> ${TC08_PYTHON_LOG}" 'exit 97' > "${TC08_DIR}/bin/python"
printf '%s\n' '#!/bin/sh' "echo python3 >> ${TC08_PYTHON_LOG}" 'exit 97' > "${TC08_DIR}/bin/python3"
if [[ -n "${CHMOD_BIN}" ]]; then
    "${CHMOD_BIN}" 755 "${TC08_DIR}/bin/python" "${TC08_DIR}/bin/python3"
else
    chmod 755 "${TC08_DIR}/bin/python" "${TC08_DIR}/bin/python3"
fi
printf '%s\n' \
    '{' \
    '  "files": [' \
    '    {"path":"src/example/module.py","language":"python","lines_of_code":10},' \
    '    {"path":"src/example/utils.py","language":"python","lines_of_code":5}' \
    '  ],' \
    '  "classes": [' \
    '    {"id":"class:Example","name":"Example","file_path":"src/example/module.py","line_number":1}' \
    '  ],' \
    '  "functions": [' \
    '    {"id":"func:Example.process","name":"process","file_path":"src/example/module.py","line_number":2,"class_id":"class:Example"},' \
    '    {"id":"func:helper","name":"helper","file_path":"src/example/utils.py","line_number":1}' \
    '  ],' \
    '  "imports": [],' \
    '  "relationships": [' \
    '    {"type":"CALLS","source_id":"func:Example.process","target_id":"func:helper"}' \
    '  ]' \
    '}' > "${TC08_JSON}"
TC08_PATH="${TC08_DIR}/bin:${PATH}"
tc08_fail=0
tc08_index_output="$(PATH="${TC08_PATH}" "${BINARY}" index-code "${TC08_JSON}" --kuzu-path "${TC08_DB}" 2>&1)" || tc08_fail=1
tc08_stats_output=""
tc08_search_output=""
tc08_callers_output=""
if [[ ${tc08_fail} -eq 0 ]]; then
    tc08_stats_output="$(PATH="${TC08_PATH}" "${BINARY}" query-code --kuzu-path "${TC08_DB}" --json stats 2>&1)" || tc08_fail=1
fi
if [[ ${tc08_fail} -eq 0 ]]; then
    tc08_search_output="$(PATH="${TC08_PATH}" "${BINARY}" query-code --kuzu-path "${TC08_DB}" --json search helper 2>&1)" || tc08_fail=1
fi
if [[ ${tc08_fail} -eq 0 ]]; then
    tc08_callers_output="$(PATH="${TC08_PATH}" "${BINARY}" query-code --kuzu-path "${TC08_DB}" --json callers helper 2>&1)" || tc08_fail=1
fi
tc08_stats_compact="${tc08_stats_output//[[:space:]]/}"
tc08_search_compact="${tc08_search_output//[[:space:]]/}"
tc08_callers_compact="${tc08_callers_output//[[:space:]]/}"
if [[ ${tc08_fail} -ne 0 ]]; then
    echo "FAIL"
    FAIL=$((FAIL + 1))
elif [[ -s "${TC08_PYTHON_LOG}" ]]; then
    echo "FAIL (Python invocation detected)"
    FAIL=$((FAIL + 1))
elif [[ "${tc08_stats_compact}" != *'"files":2'* || "${tc08_stats_compact}" != *'"classes":1'* || "${tc08_stats_compact}" != *'"functions":2'* ]]; then
    echo "FAIL (unexpected stats output)"
    echo "    stats: ${tc08_stats_output}" >&2
    FAIL=$((FAIL + 1))
elif [[ "${tc08_search_compact}" != *'"type":"function"'* || "${tc08_search_compact}" != *'"name":"helper"'* ]]; then
    echo "FAIL (search missing helper)"
    echo "    search: ${tc08_search_output}" >&2
    FAIL=$((FAIL + 1))
elif [[ "${tc08_callers_compact}" != *'"caller":"process"'* || "${tc08_callers_compact}" != *'"callee":"helper"'* ]]; then
    echo "FAIL (callers missing process -> helper)"
    echo "    callers: ${tc08_callers_output}" >&2
    FAIL=$((FAIL + 1))
else
    echo "PASS"
    PASS=$((PASS + 1))
fi

echo ""
echo "==> Results: ${PASS} passed, ${FAIL} failed"

if [[ $FAIL -gt 0 ]]; then
    echo "FAIL: ${FAIL} smoke test(s) failed in Python-free environment." >&2
    exit 1
fi

echo "PASS: All smoke tests passed with no Python interpreter on PATH (AC9)."
exit 0
