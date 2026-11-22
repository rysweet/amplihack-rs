#!/usr/bin/env bash
# Self-asserting gadugi-test scenario body for issue #2957: `amplihack update`
# looked releases up anonymously, and on hosts that share one address GitHub's
# anonymous allowance was used up, so the update failed with HTTP 403.
#
# Asserted from outside, against the shipped binary and a stand-in `gh`:
#
#   A. With a logged-in gh (found through GH_CONFIG_DIR, GH_TOKEN unset), the
#      release lookup is `gh api -i repos/rysweet/amplihack-rs/releases/latest`.
#   B. When that release is newer, the archive and its .sha256 file come from
#      `gh release download`, and the existing checksum check still runs: a
#      wrong checksum stops the update and nothing is installed.
#   C. A 403 rate-limit answer through gh is reported with GitHub's own
#      message and the reset time in plain words, and gh is asked once (no
#      retry).
#
# The anonymous path (no gh, or gh not logged in) is not driven here, because
# it would make a real request to api.github.com. The Rust tests in
# crates/amplihack-cli/src/update/tests/gh_release.rs cover it against a
# loopback stand-in API.
#
# Every command runs under `env -i` with a scratch HOME, and PATH holds only
# the stand-in gh and the system folders, so no real gh, login, token or
# network is used. The binary is copied into the scratch folder first, so an
# update could only ever write there. Everything is created under one mktemp
# folder (mode 0700) that the EXIT trap removes.
#
# Where it runs: CI's "Install Smoke Test" job (.github/workflows/ci.yml) runs
# this script against the amplihack binary that job installs.
# tests/gadugi/scenarios/issue-2957-update-with-gh-login.yaml wraps it for
# gadugi-test by hand.
#
# Run by hand: bash tests/gadugi/run-issue-2957-update-with-gh-login-scenario.sh
# It builds amplihack with cargo first. Set AMPLIHACK_2957_QA_BIN to an
# amplihack binary to skip the build. On success it prints ALL_CASES_PASSED
# and exits 0; otherwise SCENARIO_FAILED and exit 1.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd -P)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-2957-qa.XXXXXX")" || {
  echo "FAIL: cannot create a temporary directory"; echo "SCENARIO_FAILED"; exit 1
}
WORK="$(cd "$WORK" && pwd -P)"
chmod 700 "$WORK"
trap 'chmod -R -- u+rwx "$WORK" 2>/dev/null || true; rm -rf -- "$WORK"' EXIT
SYS_PATH="/usr/local/bin:/usr/bin:/bin"

# --- the binary under test --------------------------------------------------
BIN="${AMPLIHACK_2957_QA_BIN:-}"
if [ -z "$BIN" ]; then
  # 14 is the length of `"executable":"`; 15 adds the closing quote.
  if ! BIN="$(cd "$REPO_ROOT" && cargo build -p amplihack --bin amplihack --locked \
      --message-format=json-render-diagnostics 2>"$WORK/build.log" \
      | awk 'match($0, /"executable":"[^"]*\/amplihack"/) { bin = substr($0, RSTART + 14, RLENGTH - 15) } END { print bin }')"; then
    echo "FAIL: cargo build -p amplihack --bin amplihack failed"
    tail -n 40 "$WORK/build.log"
    echo "SCENARIO_FAILED"; exit 1
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "FAIL: no executable amplihack binary (got '${BIN}')"
  echo "SCENARIO_FAILED"; exit 1
fi
mkdir -m 700 -- "$WORK/opt" "$WORK/tools" "$WORK/home" "$WORK/gh-config" "$WORK/logs"
cp -- "$BIN" "$WORK/opt/amplihack"
BIN="$WORK/opt/amplihack"
echo "binary under test: $BIN"
CURRENT="$("$BIN" --version | awk '{ print $2 }')"
# A logged-in gh configuration: the stand-in accepts it when hosts.yml exists.
: > "$WORK/gh-config/hosts.yml"
chmod 600 "$WORK/gh-config/hosts.yml"

# --- the stand-in gh ---------------------------------------------------------
# Records its argv (never its environment) and answers like gh does.
cat > "$WORK/tools/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "gh $*" >> "$GH_STANDIN_LOG"
asset="amplihack-${GH_STANDIN_TARGET}.tar.gz"
case "$1 $2" in
  "auth status")
    if [ -n "${GH_CONFIG_DIR:-}" ] && [ -f "$GH_CONFIG_DIR/hosts.yml" ]; then exit 0; fi
    echo "You are not logged into any GitHub hosts." >&2; exit 1 ;;
  "api -i")
    if [ "${GH_STANDIN_MODE:-}" = ratelimit ]; then
      printf 'HTTP/2.0 403 Forbidden\r\nContent-Type: application/json\r\nX-Ratelimit-Remaining: 0\r\nX-Ratelimit-Reset: %s\r\n\r\n' "$GH_STANDIN_RESET"
      printf '{"message":"API rate limit exceeded for user ID 1.","documentation_url":"https://docs.github.com/rest"}\n'
      echo "gh: API rate limit exceeded for user ID 1. (HTTP 403)" >&2; exit 1
    fi
    tag="v$GH_STANDIN_VERSION"
    base="https://github.com/rysweet/amplihack-rs/releases/download/$tag"
    printf 'HTTP/2.0 200 OK\r\nContent-Type: application/json\r\n\r\n'
    printf '{"tag_name":"%s","draft":false,"prerelease":false,"assets":[{"name":"%s","browser_download_url":"%s/%s"},{"name":"%s.sha256","browser_download_url":"%s/%s.sha256"}]}\n' \
      "$tag" "$asset" "$base" "$asset" "$asset" "$base" "$asset"
    exit 0 ;;
  "release download")
    dir=""; prev=""
    for arg in "$@"; do [ "$prev" = "--dir" ] && dir="$arg"; prev="$arg"; done
    printf 'stand-in archive bytes\n' > "$dir/$asset"
    printf '%064d  %s\n' 0 "$asset" > "$dir/$asset.sha256"
    exit 0 ;;
esac
echo "stand-in gh: unexpected arguments: $*" >&2; exit 2
EOF
chmod 700 "$WORK/tools/gh"

case "$(uname -m)-$(uname -s)" in
  x86_64-Linux) TARGET="x86_64-unknown-linux-gnu" ;;
  aarch64-Linux) TARGET="aarch64-unknown-linux-gnu" ;;
  x86_64-Darwin) TARGET="x86_64-apple-darwin" ;;
  arm64-Darwin) TARGET="aarch64-apple-darwin" ;;
  *) echo "FAIL: no release target for $(uname -m)-$(uname -s)"; echo "SCENARIO_FAILED"; exit 1 ;;
esac

FAILED=0
fail() { echo "FAIL: $*"; FAILED=1; }

# run_update <case> [VAR=value...]: runs `amplihack update --skip-install`.
run_update() {
  local name="$1"; shift
  : > "$WORK/logs/$name.gh"
  set +e
  env -i PATH="$WORK/tools:$SYS_PATH" HOME="$WORK/home" TMPDIR="$WORK/home" \
    GH_CONFIG_DIR="$WORK/gh-config" GH_STANDIN_LOG="$WORK/logs/$name.gh" \
    GH_STANDIN_TARGET="$TARGET" "$@" \
    "$BIN" update --skip-install </dev/null >"$WORK/logs/$name.out" 2>&1
  RC=$?
  set -e
  echo "--- case $name: exit $RC"
  sed 's/^/    /' "$WORK/logs/$name.out"
  echo "    gh calls:"
  sed 's/^/      /' "$WORK/logs/$name.gh"
}
has() { grep -qF -- "$2" "$WORK/logs/$1"; }

# A. Lookup through the logged-in gh; the release is the current version.
run_update A-lookup GH_STANDIN_VERSION="$CURRENT"
[ "$RC" -eq 0 ] || fail "A: expected exit 0, got $RC"
has A-lookup.out "Already at the latest version" || fail "A: no 'Already at the latest version'"
has A-lookup.gh "gh api -i repos/rysweet/amplihack-rs/releases/latest" || fail "A: gh api was not used"

# B. Newer release: download through gh, and the checksum check still applies.
run_update B-download GH_STANDIN_VERSION=999.0.0
[ "$RC" -ne 0 ] || fail "B: a wrong checksum must stop the update"
has B-download.gh "gh release download v999.0.0 --repo rysweet/amplihack-rs --pattern amplihack-$TARGET.tar.gz --pattern amplihack-$TARGET.tar.gz.sha256 --dir" \
  || fail "B: gh release download was not used"
has B-download.out "SHA-256 checksum mismatch" || fail "B: no checksum mismatch reported"
[ ! -e "$WORK/home/.local/bin/amplihack" ] || fail "B: something was installed"

# C. 403 through gh: GitHub's message and the reset time, and no retry.
RESET=$(( $(date +%s) + 23 * 60 + 30 ))
run_update C-ratelimit GH_STANDIN_MODE=ratelimit GH_STANDIN_RESET="$RESET"
[ "$RC" -ne 0 ] || fail "C: a 403 must fail the update"
has C-ratelimit.out "GitHub refused the request (HTTP 403): API rate limit exceeded for user ID 1." \
  || fail "C: GitHub's own message is missing"
has C-ratelimit.out "The limit resets at " || fail "C: no reset time"
has C-ratelimit.out "(in 23 minutes)" || fail "C: the reset time is not in plain words"
[ "$(grep -c '^gh api ' "$WORK/logs/C-ratelimit.gh")" -eq 1 ] || fail "C: gh api was asked more than once"

if [ "$FAILED" -eq 0 ]; then
  echo "ALL_CASES_PASSED"
else
  echo "SCENARIO_FAILED"; exit 1
fi
