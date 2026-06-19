#!/usr/bin/env bash
# Shared Git commit identity resolver for amplihack-managed workflow commits.

amplihack_detect_remote_host_type() {
    local remote_url="${1:-}"
    local remote_lower

    if [ -z "$remote_url" ]; then
        remote_url="$(git remote get-url origin 2>/dev/null || true)"
    fi

    remote_lower="$(printf '%s' "$remote_url" | tr '[:upper:]' '[:lower:]')"
    case "$remote_lower" in
        *github.com:*|*github.com/*)
            printf 'github\n'
            ;;
        *dev.azure.com/*|*visualstudio.com/*|*ssh.dev.azure.com/*)
            printf 'azdo\n'
            ;;
        *)
            printf 'unknown\n'
            ;;
    esac
}

amplihack_validate_git_identity() {
    local name="${1:-}"
    local email="${2:-}"
    local label="${3:-identity}"
    local lower_email local_part domain lower_name

    name="$(_amplihack_trim "$name")"
    email="$(_amplihack_trim "$email")"
    lower_name="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
    lower_email="$(printf '%s' "$email" | tr '[:upper:]' '[:lower:]')"
    local_part="${lower_email%@*}"
    domain="${lower_email#*@}"

    if [ -z "$name" ]; then
        echo "ERROR: unsafe Git $label: missing name" >&2
        return 1
    fi
    if [ -z "$email" ]; then
        echo "ERROR: unsafe Git $label: missing email" >&2
        return 1
    fi
    if ! printf '%s' "$email" | grep -Eq '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$'; then
        echo "ERROR: unsafe Git $label: malformed email" >&2
        return 1
    fi
    case "$domain" in
        localhost|*.localhost|*.local|*.internal|*.lan|*.invalid|localdomain|*.localdomain)
            echo "ERROR: unsafe Git $label: localhost or private host email domain is not allowed" >&2
            return 1
            ;;
    esac
    case "$local_part" in
        azureuser|root|ubuntu|vsts|runner|agent|build|buildagent|svc|service|defaultuser)
            echo "ERROR: unsafe Git $label: VM/service account email is not allowed" >&2
            return 1
            ;;
    esac
    case "$lower_name" in
        ""|"unknown"|"unknown user"|"root"|"build agent"|"azure pipelines"|"azure devops")
            echo "ERROR: unsafe Git $label: VM/service account name is not allowed" >&2
            return 1
            ;;
    esac

    return 0
}

amplihack_resolve_git_identity() {
    local author_name="" author_email="" committer_name="" committer_email=""
    local reason=""

    if _amplihack_get_explicit_identity author_name author_email committer_name committer_email reason; then
        _amplihack_print_identity "$author_name" "$author_email" "$committer_name" "$committer_email"
        return 0
    fi
    if [ -n "$reason" ]; then
        _amplihack_fail_identity "$reason"
        return 1
    fi

    if _amplihack_get_existing_git_env_identity author_name author_email committer_name committer_email reason; then
        _amplihack_print_identity "$author_name" "$author_email" "$committer_name" "$committer_email"
        return 0
    fi
    if [ -n "$reason" ]; then
        _amplihack_fail_identity "$reason"
        return 1
    fi

    if _amplihack_get_repo_local_identity author_name author_email; then
        _amplihack_print_identity "$author_name" "$author_email" "$author_name" "$author_email"
        return 0
    fi

    if _amplihack_get_provider_identity author_name author_email; then
        _amplihack_print_identity "$author_name" "$author_email" "$author_name" "$author_email"
        return 0
    fi

    _amplihack_fail_identity "no safe explicit, environment, repo-local, or authenticated provider identity was found"
    return 1
}

amplihack_prepare_git_commit_identity() {
    local resolved author_name author_email committer_name committer_email

    resolved="$(amplihack_resolve_git_identity)" || return 1
    IFS=$'\t' read -r author_name author_email committer_name committer_email <<<"$resolved"

    export GIT_AUTHOR_NAME="$author_name"
    export GIT_AUTHOR_EMAIL="$author_email"
    export GIT_COMMITTER_NAME="$committer_name"
    export GIT_COMMITTER_EMAIL="$committer_email"
}

_amplihack_get_explicit_identity() {
    local __author_name_ref="$1" __author_email_ref="$2" __committer_name_ref="$3" __committer_email_ref="$4" __reason_ref="$5"
    local explicit_author_name explicit_author_email explicit_committer_name explicit_committer_email any_explicit

    explicit_author_name="$(_amplihack_explicit_value AMPLIHACK_GIT_AUTHOR_NAME author_name)"
    explicit_author_email="$(_amplihack_explicit_value AMPLIHACK_GIT_AUTHOR_EMAIL author_email)"
    explicit_committer_name="$(_amplihack_explicit_value AMPLIHACK_GIT_COMMITTER_NAME committer_name)"
    explicit_committer_email="$(_amplihack_explicit_value AMPLIHACK_GIT_COMMITTER_EMAIL committer_email)"
    any_explicit="$explicit_author_name$explicit_author_email$explicit_committer_name$explicit_committer_email"

    if [ -z "$any_explicit" ]; then
        printf -v "$__reason_ref" '%s' ""
        return 1
    fi

    if [ -z "$explicit_author_name" ] || [ -z "$explicit_author_email" ]; then
        printf -v "$__reason_ref" '%s' "explicit AMPLIHACK_GIT_AUTHOR_NAME and AMPLIHACK_GIT_AUTHOR_EMAIL must both be set"
        return 1
    fi
    if { [ -n "$explicit_committer_name" ] && [ -z "$explicit_committer_email" ]; } || { [ -z "$explicit_committer_name" ] && [ -n "$explicit_committer_email" ]; }; then
        printf -v "$__reason_ref" '%s' "explicit AMPLIHACK_GIT_COMMITTER_NAME and AMPLIHACK_GIT_COMMITTER_EMAIL must be set together"
        return 1
    fi
    if [ -z "$explicit_committer_name" ] && [ -z "$explicit_committer_email" ]; then
        explicit_committer_name="$explicit_author_name"
        explicit_committer_email="$explicit_author_email"
    fi
    if ! amplihack_validate_git_identity "$explicit_author_name" "$explicit_author_email" "author from AMPLIHACK_GIT_*"; then
        printf -v "$__reason_ref" '%s' "explicit AMPLIHACK_GIT_AUTHOR_* identity is invalid or unsafe"
        return 1
    fi
    if ! amplihack_validate_git_identity "$explicit_committer_name" "$explicit_committer_email" "committer from AMPLIHACK_GIT_*"; then
        printf -v "$__reason_ref" '%s' "explicit AMPLIHACK_GIT_COMMITTER_* identity is invalid or unsafe"
        return 1
    fi

    printf -v "$__author_name_ref" '%s' "$explicit_author_name"
    printf -v "$__author_email_ref" '%s' "$explicit_author_email"
    printf -v "$__committer_name_ref" '%s' "$explicit_committer_name"
    printf -v "$__committer_email_ref" '%s' "$explicit_committer_email"
    printf -v "$__reason_ref" '%s' ""
    return 0
}

_amplihack_get_existing_git_env_identity() {
    local __author_name_ref="$1" __author_email_ref="$2" __committer_name_ref="$3" __committer_email_ref="$4" __reason_ref="$5"
    local env_author_name="${GIT_AUTHOR_NAME:-}" env_author_email="${GIT_AUTHOR_EMAIL:-}"
    local env_committer_name="${GIT_COMMITTER_NAME:-}" env_committer_email="${GIT_COMMITTER_EMAIL:-}"
    local any_git_env="$env_author_name$env_author_email$env_committer_name$env_committer_email"

    if [ -z "$any_git_env" ]; then
        printf -v "$__reason_ref" '%s' ""
        return 1
    fi
    if [ -z "$env_author_name" ] || [ -z "$env_author_email" ] || [ -z "$env_committer_name" ] || [ -z "$env_committer_email" ]; then
        printf -v "$__reason_ref" '%s' "existing GIT_AUTHOR_* and GIT_COMMITTER_* identity must be complete"
        return 1
    fi
    if ! amplihack_validate_git_identity "$env_author_name" "$env_author_email" "author from existing Git environment"; then
        printf -v "$__reason_ref" '%s' "existing GIT_AUTHOR_* identity is invalid or unsafe"
        return 1
    fi
    if ! amplihack_validate_git_identity "$env_committer_name" "$env_committer_email" "committer from existing Git environment"; then
        printf -v "$__reason_ref" '%s' "existing GIT_COMMITTER_* identity is invalid or unsafe"
        return 1
    fi

    printf -v "$__author_name_ref" '%s' "$env_author_name"
    printf -v "$__author_email_ref" '%s' "$env_author_email"
    printf -v "$__committer_name_ref" '%s' "$env_committer_name"
    printf -v "$__committer_email_ref" '%s' "$env_committer_email"
    printf -v "$__reason_ref" '%s' ""
    return 0
}

_amplihack_get_repo_local_identity() {
    local __author_name_ref="$1" __author_email_ref="$2"
    local name email

    name="$(git config --local --get user.name 2>/dev/null || true)"
    email="$(git config --local --get user.email 2>/dev/null || true)"
    if [ -z "$name" ] && [ -z "$email" ]; then
        return 1
    fi
    if amplihack_validate_git_identity "$name" "$email" "from repo-local git config" >/dev/null 2>&1; then
        printf -v "$__author_name_ref" '%s' "$(_amplihack_trim "$name")"
        printf -v "$__author_email_ref" '%s' "$(_amplihack_trim "$email")"
        return 0
    fi
    return 1
}

_amplihack_get_provider_identity() {
    local __author_name_ref="$1" __author_email_ref="$2"
    local host_type="${REMOTE_HOST_TYPE:-}"

    if [ -z "$host_type" ]; then
        host_type="$(amplihack_detect_remote_host_type)"
    fi
    case "$host_type" in
        github)
            _amplihack_get_github_identity "$__author_name_ref" "$__author_email_ref"
            ;;
        azdo|azure-devops)
            _amplihack_get_azdo_identity "$__author_name_ref" "$__author_email_ref"
            ;;
        *)
            return 1
            ;;
    esac
}

_amplihack_get_github_identity() {
    local __author_name_ref="$1" __author_email_ref="$2"
    local login id name email

    command -v gh >/dev/null 2>&1 || return 1
    gh auth status >/dev/null 2>&1 || return 1

    login="$(_amplihack_provider_value gh api user --jq .login)"
    id="$(_amplihack_provider_value gh api user --jq .id)"
    name="$(_amplihack_provider_value gh api user --jq .name)"
    email="$(_amplihack_provider_value gh api user --jq .email)"

    if [ -z "$name" ]; then
        name="$login"
    fi
    if [ -z "$email" ]; then
        if [ -n "$id" ] && [ -n "$login" ]; then
            email="${id}+${login}@users.noreply.github.com"
        fi
    fi

    if amplihack_validate_git_identity "$name" "$email" "from authenticated GitHub account" >/dev/null 2>&1; then
        printf -v "$__author_name_ref" '%s' "$name"
        printf -v "$__author_email_ref" '%s' "$email"
        return 0
    fi
    return 1
}

_amplihack_get_azdo_identity() {
    local __author_name_ref="$1" __author_email_ref="$2"
    local account_name account_type

    command -v az >/dev/null 2>&1 || return 1

    account_name="$(_amplihack_provider_value az account show --query user.name -o tsv)"
    account_type="$(_amplihack_provider_value az account show --query user.type -o tsv)"
    account_type="$(printf '%s' "$account_type" | tr '[:upper:]' '[:lower:]')"
    if [ -n "$account_type" ] && [ "$account_type" != "user" ]; then
        return 1
    fi

    if amplihack_validate_git_identity "$account_name" "$account_name" "from authenticated Azure CLI account" >/dev/null 2>&1; then
        printf -v "$__author_name_ref" '%s' "$account_name"
        printf -v "$__author_email_ref" '%s' "$account_name"
        return 0
    fi
    return 1
}

_amplihack_provider_value() {
    local value

    value="$("$@" 2>/dev/null || true)"
    value="$(printf '%s' "$value" | sed -n '1p')"
    value="$(_amplihack_trim "$value")"
    case "$value" in
        ""|"null"|"None")
            return 0
            ;;
        *)
            printf '%s\n' "$value"
            ;;
    esac
}

_amplihack_explicit_value() {
    local env_name="$1"
    local config_key="$2"
    local value="${!env_name:-}"

    if [ -n "$value" ]; then
        _amplihack_trim "$value"
        return 0
    fi
    _amplihack_config_value "$env_name" "$config_key"
}

_amplihack_config_value() {
    local env_name="$1"
    local config_key="$2"
    local file value

    while IFS= read -r file; do
        [ -f "$file" ] || continue
        value="$(awk -v env_name="$env_name" -v config_key="$config_key" '
            function trim(s) {
                sub(/^[[:space:]]+/, "", s)
                sub(/[[:space:]]+$/, "", s)
                return s
            }
            function unquote(s) {
                s = trim(s)
                if ((s ~ /^".*"$/) || (s ~ /^'\''.*'\''$/)) {
                    s = substr(s, 2, length(s) - 2)
                }
                return s
            }
            /^[[:space:]]*($|#|;)/ { next }
            /^[[:space:]]*\[[^]]+\][[:space:]]*$/ {
                section = $0
                gsub(/^[[:space:]]*\[/, "", section)
                gsub(/\][[:space:]]*$/, "", section)
                section = trim(section)
                next
            }
            index($0, "=") {
                key = substr($0, 1, index($0, "=") - 1)
                value = substr($0, index($0, "=") + 1)
                sub(/[[:space:]]+#.*$/, "", value)
                sub(/[[:space:]]+;.*$/, "", value)
                key = trim(key)
                value = unquote(value)
                if (key == env_name || key == "git_identity." config_key || (section == "git_identity" && key == config_key)) {
                    print value
                    exit
                }
            }
        ' "$file" | sed -n '1p')"
        if [ -n "$value" ]; then
            printf '%s\n' "$value"
            return 0
        fi
        value="$(_amplihack_json_config_value "$file" "$config_key")"
        if [ -n "$value" ]; then
            printf '%s\n' "$value"
            return 0
        fi
    done < <(_amplihack_candidate_config_files)
}

_amplihack_json_config_value() {
    local file="$1"
    local config_key="$2"

    command -v python3 >/dev/null 2>&1 || return 0
    python3 - "$file" "$config_key" <<'PY' 2>/dev/null
import json
import sys

path, key = sys.argv[1], sys.argv[2]
try:
    with open(path, "r", encoding="utf-8") as fh:
        data = json.load(fh)
except Exception:
    sys.exit(0)

value = data.get("git_identity", {}).get(key)
if isinstance(value, str) and value:
    print(value)
PY
}

_amplihack_candidate_config_files() {
    if [ -n "${AMPLIHACK_CONFIG:-}" ]; then
        printf '%s\n' "$AMPLIHACK_CONFIG"
    fi
    if [ -n "${HOME:-}" ]; then
        printf '%s\n' "$HOME/.amplihack/config"
    fi
    if [ -n "${AMPLIHACK_HOME:-}" ]; then
        printf '%s\n' "$AMPLIHACK_HOME/config"
        printf '%s\n' "$AMPLIHACK_HOME/.amplihack/config"
    fi
}

_amplihack_print_identity() {
    printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4"
}

_amplihack_fail_identity() {
    local reason="$1"

    {
        echo "ERROR: Amplihack could not determine a safe Git commit identity."
        echo "Reason: $reason"
        echo
        echo "Set explicit identity before running commit-producing workflows:"
        echo '  export AMPLIHACK_GIT_AUTHOR_NAME="Your Name"'
        echo '  export AMPLIHACK_GIT_AUTHOR_EMAIL="you@example.com"'
        echo
        echo "Optionally set AMPLIHACK_GIT_COMMITTER_NAME and AMPLIHACK_GIT_COMMITTER_EMAIL"
        echo "when the committer should differ from the author. Alternatively configure"
        echo "safe repo-local Git identity with:"
        echo '  git config --local user.name "Your Name"'
        echo '  git config --local user.email "you@example.com"'
        echo
        echo "Amplihack refuses VM/service fallback identities such as azureuser@...,"
        echo "localhost-style emails, malformed emails, and incomplete identities."
    } >&2
}

_amplihack_trim() {
    printf '%s' "${1:-}" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'
}
