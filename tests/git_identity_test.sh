#!/usr/bin/env bash
# TDD tests for workflow-created git commit identity resolution.
#
# These tests define the contract for amplifier-bundle/tools/git-identity.sh.
# Expected before implementation: FAIL. Expected after implementation: PASS.
#
# Run: bash tests/git_identity_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HELPER="$REPO_ROOT/amplifier-bundle/tools/git-identity.sh"

pass=0
fail=0
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

assert() {
    local desc="$1"
    shift
    if "$@"; then
        record_pass "$desc"
    else
        record_fail "$desc" "command failed: $*"
    fi
}

assert_eq() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "$actual" = "$expected" ]; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected: <$expected>; actual: <$actual>"
    fi
}

new_repo() {
    local name="$1"
    local remote_url="${2:-}"
    local repo="$TMPROOT/$name"
    mkdir -p "$repo"
    git -C "$repo" init -q
    if [ -n "$remote_url" ]; then
        git -C "$repo" remote add origin "$remote_url"
    fi
    printf '%s\n' "$repo"
}

make_fake_bin() {
    local dir="$TMPROOT/fake-bin"
    mkdir -p "$dir"

    cat >"$dir/gh" <<'GH_EOF'
#!/usr/bin/env bash
set -uo pipefail
if [ "${1:-}" = "auth" ]; then
    exit 0
fi
if [ "${1:-}" = "api" ] && [ "${2:-}" = "user/emails" ]; then
    if [ -n "${AMPLIHACK_TEST_GH_PUBLIC_EMAIL:-}" ]; then
        printf '[{"email":"%s","primary":true,"verified":true,"visibility":"public"}]\n' "$AMPLIHACK_TEST_GH_PUBLIC_EMAIL"
        exit 0
    fi
    printf '[]\n'
    exit 0
fi
if [ "${1:-}" = "api" ] && [ "${2:-}" = "user" ]; then
    case " $* " in
        *" --jq .login "*|*" -q .login "*) printf 'octocat\n' ;;
        *" --jq .id "*|*" -q .id "*) printf '583231\n' ;;
        *" --jq .name "*|*" -q .name "*) printf 'Octo Cat\n' ;;
        *" --jq .email "*|*" -q .email "*) printf '%s\n' "${AMPLIHACK_TEST_GH_PUBLIC_EMAIL:-}" ;;
        *)
            if [ -n "${AMPLIHACK_TEST_GH_PUBLIC_EMAIL:-}" ]; then
                email_json="\"$AMPLIHACK_TEST_GH_PUBLIC_EMAIL\""
            else
                email_json="null"
            fi
            printf '{"login":"octocat","id":583231,"name":"Octo Cat","email":%s}\n' "$email_json"
            ;;
    esac
    exit 0
fi
echo "unexpected gh invocation: $*" >&2
exit 1
GH_EOF

    cat >"$dir/az" <<'AZ_EOF'
#!/usr/bin/env bash
set -uo pipefail
if [ "${1:-}" = "account" ] && [ "${2:-}" = "show" ]; then
    account_email="${AMPLIHACK_TEST_AZ_ACCOUNT_EMAIL:-az.user@example.com}"
    account_type="${AMPLIHACK_TEST_AZ_ACCOUNT_TYPE:-user}"
    case " $* " in
        *" --query user.name "*|*" -q user.name "*) printf '%s\n' "$account_email" ;;
        *" --query user.type "*|*" -q user.type "*) printf '%s\n' "$account_type" ;;
        *) printf '{"user":{"name":"%s","type":"%s"}}\n' "$account_email" "$account_type" ;;
    esac
    exit 0
fi
echo "unexpected az invocation: $*" >&2
exit 1
AZ_EOF

    chmod +x "$dir/gh" "$dir/az"
    printf '%s\n' "$dir"
}

run_prepare() {
    local repo="$1"
    shift
    local fake_bin="$1"
    shift
    (
        cd "$repo" || exit 1
        env -i \
            PATH="$fake_bin:$PATH" \
            HOME="$TMPROOT/home" \
            REPO_PATH="$repo" \
            AMPLIHACK_HOME="$REPO_ROOT" \
            HELPER="$HELPER" \
            "$@" \
            bash -c '
                set -euo pipefail
                . "$HELPER"
                amplihack_prepare_git_commit_identity >/dev/null
                printf "%s|%s|%s|%s\n" \
                    "${GIT_AUTHOR_NAME:-}" \
                    "${GIT_AUTHOR_EMAIL:-}" \
                    "${GIT_COMMITTER_NAME:-}" \
                    "${GIT_COMMITTER_EMAIL:-}"
            '
    )
}

run_prepare_expect_failure() {
    local repo="$1"
    shift
    local fake_bin="$1"
    shift
    (
        cd "$repo" || exit 1
        env -i \
            PATH="$fake_bin:$PATH" \
            HOME="$TMPROOT/home" \
            REPO_PATH="$repo" \
            AMPLIHACK_HOME="$REPO_ROOT" \
            HELPER="$HELPER" \
            "$@" \
            bash -c '
                set -euo pipefail
                . "$HELPER"
                amplihack_prepare_git_commit_identity
            '
    ) 2>&1
}

detect_host() {
    local url="$1"
    env -i PATH="$PATH" HELPER="$HELPER" bash -c '
        set -euo pipefail
        . "$HELPER"
        amplihack_detect_remote_host_type "$1"
    ' _ "$url"
}

echo "=== Git identity helper TDD tests ==="
echo "Helper: $HELPER"
echo

fake_bin="$(make_fake_bin)"

# --- Contract: helper file and public functions ------------------------------
assert "git-identity.sh exists" test -f "$HELPER"

for fn in \
    amplihack_prepare_git_commit_identity \
    amplihack_resolve_git_identity \
    amplihack_validate_git_identity \
    amplihack_detect_remote_host_type
do
    assert "exports public function: $fn" \
        bash -c ". '$HELPER' 2>/dev/null && declare -F '$fn' >/dev/null"
done

# --- Contract: remote host detection -----------------------------------------
github_host="$(detect_host "https://github.com/octo/repo.git" 2>/dev/null)"
assert_eq "detects GitHub HTTPS remotes" "github" "$github_host"

azdo_host="$(detect_host "https://dev.azure.com/org/project/_git/repo" 2>/dev/null)"
assert_eq "detects Azure DevOps HTTPS remotes" "azdo" "$azdo_host"

visualstudio_host="$(detect_host "https://org.visualstudio.com/project/_git/repo" 2>/dev/null)"
assert_eq "detects legacy visualstudio.com remotes" "azdo" "$visualstudio_host"

unknown_host="$(detect_host "https://gitlab.com/org/repo.git" 2>/dev/null)"
assert_eq "detects unknown remotes" "unknown" "$unknown_host"

# --- Contract: explicit AMPLIHACK_GIT_* env wins over every other source -----
repo="$(new_repo explicit "https://dev.azure.com/org/project/_git/repo")"
git -C "$repo" config --local user.name "azureuser"
git -C "$repo" config --local user.email "azureuser@vm.internal"
actual="$(run_prepare "$repo" "$fake_bin" \
    AMPLIHACK_GIT_AUTHOR_NAME="Explicit Author" \
    AMPLIHACK_GIT_AUTHOR_EMAIL="explicit.author@example.com" \
    AMPLIHACK_GIT_COMMITTER_NAME="Explicit Committer" \
    AMPLIHACK_GIT_COMMITTER_EMAIL="explicit.committer@example.com" \
    GIT_AUTHOR_NAME="Ignored Author" \
    GIT_AUTHOR_EMAIL="ignored.author@example.com" \
    GIT_COMMITTER_NAME="Ignored Committer" \
    GIT_COMMITTER_EMAIL="ignored.committer@example.com" \
    2>/dev/null)"
assert_eq "explicit AMPLIHACK_GIT_* identity has highest precedence" \
    "Explicit Author|explicit.author@example.com|Explicit Committer|explicit.committer@example.com" \
    "$actual"

# --- Contract: explicit Amplihack config is accepted when env is absent -------
repo="$(new_repo explicit-config "https://dev.azure.com/org/project/_git/repo")"
mkdir -p "$TMPROOT/home/.amplihack"
cat >"$TMPROOT/home/.amplihack/config" <<'CONFIG_EOF'
{
  "git_identity": {
    "author_name": "Config Author",
    "author_email": "config.author@example.com",
    "committer_name": "Config Committer",
    "committer_email": "config.committer@example.com"
  }
}
CONFIG_EOF
actual="$(run_prepare "$repo" "$fake_bin" 2>/dev/null)"
assert_eq "explicit Amplihack config identity is used when env is absent" \
    "Config Author|config.author@example.com|Config Committer|config.committer@example.com" \
    "$actual"
rm -f "$TMPROOT/home/.amplihack/config"

# --- Contract: existing complete safe Git env is preserved -------------------
repo="$(new_repo existing-env "https://dev.azure.com/org/project/_git/repo")"
git -C "$repo" config --local user.name "azureuser"
git -C "$repo" config --local user.email "azureuser@vm.internal"
actual="$(run_prepare "$repo" "$fake_bin" \
    GIT_AUTHOR_NAME="Existing Author" \
    GIT_AUTHOR_EMAIL="existing.author@example.com" \
    GIT_COMMITTER_NAME="Existing Committer" \
    GIT_COMMITTER_EMAIL="existing.committer@example.com" \
    2>/dev/null)"
assert_eq "complete safe GIT_AUTHOR_* and GIT_COMMITTER_* env is preserved" \
    "Existing Author|existing.author@example.com|Existing Committer|existing.committer@example.com" \
    "$actual"

# --- Contract: safe repo-local config is used without global config ----------
repo="$(new_repo repo-local "https://dev.azure.com/org/project/_git/repo")"
git -C "$repo" config --local user.name "Repo Local User"
git -C "$repo" config --local user.email "repo.local@example.com"
actual="$(run_prepare "$repo" "$fake_bin" 2>/dev/null)"
assert_eq "safe repo-local git config populates both author and committer" \
    "Repo Local User|repo.local@example.com|Repo Local User|repo.local@example.com" \
    "$actual"

# --- Contract: GitHub provider fallback uses authenticated noreply identity ---
repo="$(new_repo github-public "https://github.com/octo/repo.git")"
actual="$(run_prepare "$repo" "$fake_bin" \
    AMPLIHACK_TEST_GH_PUBLIC_EMAIL="octo.public@example.com" \
    2>/dev/null)"
assert_eq "GitHub fallback prefers authenticated public email when available" \
    "Octo Cat|octo.public@example.com|Octo Cat|octo.public@example.com" \
    "$actual"

repo="$(new_repo github-fallback "https://github.com/octo/repo.git")"
actual="$(run_prepare "$repo" "$fake_bin" 2>/dev/null)"
assert_eq "GitHub fallback uses authenticated noreply email when public email is unavailable" \
    "Octo Cat|583231+octocat@users.noreply.github.com|Octo Cat|583231+octocat@users.noreply.github.com" \
    "$actual"

# --- Contract: Azure provider fallback accepts safe authenticated email -------
repo="$(new_repo azdo-fallback "https://dev.azure.com/org/project/_git/repo")"
actual="$(run_prepare "$repo" "$fake_bin" \
    AMPLIHACK_TEST_AZ_ACCOUNT_EMAIL="az.user@example.com" \
    2>/dev/null)"
case "$actual" in
    *"|az.user@example.com|"*"|az.user@example.com") record_pass "Azure CLI fallback accepts safe authenticated email" ;;
    *) record_fail "Azure CLI fallback accepts safe authenticated email" "actual: <$actual>" ;;
esac

# --- Contract: unsafe VM/service identities fail closed ----------------------
repo="$(new_repo azdo-unsafe "https://dev.azure.com/org/project/_git/repo")"
git -C "$repo" config --local user.name "azureuser"
git -C "$repo" config --local user.email "azureuser@vm.internal"
out="$(run_prepare_expect_failure "$repo" "$fake_bin" \
    AMPLIHACK_TEST_AZ_ACCOUNT_EMAIL="azureuser@buildvm.local" \
    2>&1)"
rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "AMPLIHACK_GIT_AUTHOR_NAME" && printf '%s' "$out" | grep -q "AMPLIHACK_GIT_AUTHOR_EMAIL"; then
    record_pass "unsafe azureuser identity fails with explicit configuration guidance"
else
    record_fail "unsafe azureuser identity fails with explicit configuration guidance" "rc=$rc output: $out"
fi

repo="$(new_repo malformed "https://github.com/octo/repo.git")"
out="$(run_prepare_expect_failure "$repo" "$fake_bin" \
    AMPLIHACK_GIT_AUTHOR_NAME="Bad Author" \
    AMPLIHACK_GIT_AUTHOR_EMAIL="not-an-email" \
    AMPLIHACK_GIT_COMMITTER_NAME="Bad Committer" \
    AMPLIHACK_GIT_COMMITTER_EMAIL="committer@example.com" \
    2>&1)"
rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -Eq "AMPLIHACK_GIT|invalid|malformed|unsafe"; then
    record_pass "malformed explicit identity fails closed"
else
    record_fail "malformed explicit identity fails closed" "rc=$rc output: $out"
fi

if [ ! -f "$TMPROOT/home/.gitconfig" ]; then
    record_pass "identity preparation does not mutate global git config"
else
    record_fail "identity preparation does not mutate global git config" "$TMPROOT/home/.gitconfig was created"
fi

echo
echo "=== Results: $pass passed, $fail failed ==="
if [ "$fail" -ne 0 ]; then
    exit 1
fi
