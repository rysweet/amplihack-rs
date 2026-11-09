#!/usr/bin/env bash
# workflow_worktree_source.sh — decide which commit a task worktree starts from,
# check the worktree after it is created, and tell later steps which base
# branch was verified (issue #1957).
#
# Contract: amplifier-bundle/tools/workflow_worktree_source.md.
#
# Usage (executed, never sourced):
#   workflow_worktree_source.sh resolve --recipe default|consensus --repo <abs-path> \
#       [--base-branch <branch>] [--base-sha <sha>]
#   workflow_worktree_source.sh verify-worktree <worktree-path> <sha> created|reused \
#       <main-repo-path> <origin-fingerprint> [--branch <branch>] [--cleanup-created]
#   workflow_worktree_source.sh verified-base
#   workflow_worktree_source.sh requested-inputs
#
# stdout carries only machine output; INFO, WARNING, RULE and ERROR lines go to
# stderr. Exit codes for resolve and verify-worktree: 0 success, 1 refused (one
# ERROR line, always the last line on stderr), 2 usage error.
#
# verified-base (read by the PR tools, decision D3):
#   0  prints <b>: worktree_setup.source_verified is exactly `verified`, <b>
#      passes the branch rule and refs/remotes/origin/<b> resolves
#   1  one ERROR line: verified but the name is invalid or the ref does not
#      resolve, or the recipe context cannot be read
#   2  usage error (any argument)
#   3  no verified base: legacy, not-applicable, any other value, an absent
#      field, no context, or a SHA-only run (base_branch empty)
#
# requested-inputs: 0 base_branch or base_sha was given, 3 neither, 1 the
# context cannot be read (one ERROR line), 2 usage error. Prints nothing on
# stdout and never prints a value.
#
# Rules this file keeps: no eval, every expansion quoted, `git -C` on every git
# call, and the origin URL is only ever piped into `git hash-object`, never
# printed (a URL can hold a token).

set -u
set -o pipefail
export LC_ALL=C
unset GIT_TRACE GIT_TRACE_PACKET GIT_TRACE_CURL GIT_CURL_VERBOSE

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHA_RE='^([0-9a-f]{40}|[0-9a-f]{64})$'
BRANCH_RE='^[A-Za-z0-9._/+-]+$'

# show <value>: the first 64 bytes, every byte outside the branch allow-list
# replaced by `?`, so a refused value cannot forge a RULE:/ERROR: line or send
# control bytes to the terminal.
show() {
    printf '%s' "${1:0:64}" | tr -c 'A-Za-z0-9._/+-' '?'
}

# show_path <path>: the path with every control byte (newline, CR, escape)
# replaced by `?`. Not truncated, so the path stays usable for diagnosis, but
# it cannot start a new RULE:/ERROR: line (SR-6).
show_path() {
    printf '%s' "$1" | tr '[:cntrl:]' '?'
}

usage() {
    echo "ERROR: workflow_worktree_source.sh: $1 (see workflow_worktree_source.md)." >&2
    exit 2
}

# refuse <rule> <message>: the one RULE line, then the one ERROR line, last.
refuse() {
    echo "RULE: $1" >&2
    echo "ERROR: $2" >&2
    exit 1
}

trim() {
    local v="$1"
    v="${v#"${v%%[![:space:]]*}"}"
    v="${v%"${v##*[![:space:]]}"}"
    printf '%s' "$v"
}

# valid_branch <b>: the S-1 branch rule. No side effects; needs no repository.
valid_branch() {
    local b="$1" out
    [ -n "$b" ] || return 1
    [ "${#b}" -le 255 ] || return 1
    [[ "$b" =~ $BRANCH_RE ]] || return 1
    case "$b" in -*|refs/*|@|HEAD) return 1 ;; esac
    # The output must equal the input: this refuses @{-N} and similar forms
    # that git would expand to a different branch.
    out="$(git check-ref-format --branch "$b" 2>/dev/null)" || return 1
    [ "$out" = "$b" ]
}

valid_sha() {
    [[ "$1" =~ $SHA_RE ]]
}

# origin_fingerprint <repo>: hash of the origin URL. The URL goes straight into
# hash-object and is never printed.
origin_fingerprint() {
    git -C "$1" remote get-url origin 2>/dev/null | git -C "$1" hash-object --stdin 2>/dev/null
}

# physical <path>: the path with every symlink resolved, or nothing.
physical() {
    (cd -P -- "$1" 2>/dev/null && pwd -P)
}

# run_fetch <repo> [--strict] [refspec...]: workflow_fetch_origin.sh in <repo>,
# or one plain `git fetch` when the tool is missing. Git output goes to stderr.
run_fetch() {
    local repo="$1"; shift
    if [ -f "$SCRIPT_DIR/workflow_fetch_origin.sh" ]; then
        (cd "$repo" && bash "$SCRIPT_DIR/workflow_fetch_origin.sh" "$@") >&2
        return $?
    fi
    local strict=0 refspecs=() a
    for a in "$@"; do
        if [ "$a" = --strict ]; then strict=1; else refspecs+=("$a"); fi
    done
    if [ "${#refspecs[@]}" -gt 0 ]; then
        git -C "$repo" fetch origin --no-tags -- "${refspecs[@]}" >&2
    else
        git -C "$repo" fetch origin --no-tags >&2
    fi
    local rc=$?
    [ "$strict" -eq 1 ] || return 0
    return "$rc"
}

# print_result: the six key=value lines, printed once, after every check passed.
print_result() {
    printf 'base_ref=%s\nbase_branch=%s\nbase_sha=%s\nsource_verified=%s\nsource_rule=%s\norigin_fingerprint=%s\n' \
        "$1" "$2" "$3" "$4" "$5" "$6"
}

# --- legacy chains (moved verbatim from the recipes, #858) --------------------

legacy_default() {
    local repo="$1" ref
    run_fetch "$repo"
    ref="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    if [ -z "$ref" ] || ! git -C "$repo" rev-parse --verify --quiet "${ref}^{commit}" >/dev/null; then
        git -C "$repo" remote set-head origin -a >/dev/null 2>&1 || true
        ref="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    fi
    for ref in $ref origin/master origin/develop; do
        if git -C "$repo" rev-parse --verify --quiet "${ref}^{commit}" >/dev/null; then
            printf '%s\n' "$ref"
            return 0
        fi
    done
    return 1
}

legacy_consensus() {
    local repo="$1" candidate
    run_fetch "$repo"
    candidate="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    if [ -n "$candidate" ] && git -C "$repo" rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
        printf '%s\n' "$candidate"
        return 0
    fi
    git -C "$repo" remote set-head origin -a >/dev/null 2>&1 || true
    candidate="$(git -C "$repo" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    if [ -n "$candidate" ] && git -C "$repo" rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
        printf '%s\n' "$candidate"
        return 0
    fi
    for candidate in origin/main origin/master origin/develop; do
        if git -C "$repo" rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

resolve_legacy() {
    local recipe="$1" repo="$2" ref
    if [ "$recipe" = consensus ]; then
        if ! git -C "$repo" remote get-url origin >/dev/null 2>&1; then
            refuse refuse-no-base-ref "no 'origin' remote configured — refusing to create a task worktree off the caller's HEAD; a recipe task worktree must branch off a fetched remote base ref, never the caller checkout (issue #858)."
        fi
        ref="$(legacy_consensus "$repo")" \
            || refuse refuse-no-base-ref "no supported remote base ref found. Expected origin/HEAD, origin/main, origin/master, or origin/develop; refusing to fall back to caller HEAD (issue #858)."
    else
        ref="$(legacy_default "$repo")" \
            || refuse refuse-no-base-ref "no supported remote base ref found. Expected origin/HEAD, origin/master, or origin/develop."
    fi
    echo "WARNING: no base_branch/base_sha given; legacy base resolution (${ref}), source not verified" >&2
    echo "RULE: legacy-unverified" >&2
    print_result "$ref" "${ref#origin/}" "" legacy legacy-unverified ""
}

# --- resolve ------------------------------------------------------------------

cmd_resolve() {
    local recipe="" repo="" bb="" bs="" seen=" " flag val
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --recipe|--repo|--base-branch|--base-sha)
                flag="$1"
                [ "$#" -ge 2 ] || usage "$flag needs a value"
                val="$2"
                case "$val" in -*) usage "the value for $flag starts with '-'" ;; esac
                case "$seen" in *" $flag "*) usage "$flag given twice" ;; esac
                seen="$seen$flag "
                case "$flag" in
                    --recipe) recipe="$val" ;;
                    --repo) repo="$val" ;;
                    --base-branch) bb="$val" ;;
                    --base-sha) bs="$val" ;;
                esac
                shift 2
                ;;
            *) usage "unknown resolve argument '$(show "$1")'" ;;
        esac
    done
    case "$recipe" in default|consensus) ;; *) usage "--recipe must be default or consensus" ;; esac
    [ -n "$repo" ] || usage "--repo is required"
    case "$repo" in /*) ;; *) usage "--repo must be an absolute path" ;; esac

    bb="$(trim "$bb")"
    bs="$(trim "$bs" | tr '[:upper:]' '[:lower:]')"

    if [ -z "$bb" ] && [ -z "$bs" ]; then
        resolve_legacy "$recipe" "$repo"
        return 0
    fi

    # 1. Validate the inputs (no side effects).
    if [ -n "$bb" ] && ! valid_branch "$bb"; then
        refuse refuse-invalid-input "base_branch '$(show "$bb")' is not a valid branch name (issue #1957)."
    fi
    if [ -n "$bs" ] && ! valid_sha "$bs"; then
        refuse refuse-invalid-input "base_sha '$(show "$bs")' is not a full 40- or 64-character hex SHA; abbreviated SHAs are refused (issue #1957)."
    fi

    # 2. Identity check before the fetch.
    if ! git -C "$repo" remote get-url origin >/dev/null 2>&1; then
        refuse refuse-origin-identity "no 'origin' remote configured in $(show_path "$repo"); refusing to resolve a worktree source (issue #1957)."
    fi
    local phys top fp
    phys="$(physical "$repo")"
    top="$(git -C "$repo" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -z "$phys" ] || [ -z "$top" ] || [ "$phys" != "$top" ]; then
        refuse refuse-origin-identity "--repo $(show_path "$repo") is not the repository top (show-toplevel: $(show_path "${top:-none}")) (issue #1957)."
    fi
    fp="$(origin_fingerprint "$repo")"
    [ -n "$fp" ] || refuse refuse-origin-identity "cannot read the origin remote of $(show_path "$repo") (issue #1957)."

    # 3. Strict fetch. A plain fetch first, so a failure means origin is
    # unreachable; then the explicit refspec, so a narrowed remote.origin.fetch
    # cannot leave origin/<b> stale. A refspec fetch that fails after a plain
    # fetch worked means the branch is not on origin.
    local fetched=1 branch_fetched=1
    run_fetch "$repo" --strict || fetched=0
    if [ "$fetched" -eq 1 ] && [ -n "$bb" ]; then
        run_fetch "$repo" --strict "+refs/heads/${bb}:refs/remotes/origin/${bb}" || branch_fetched=0
    fi

    # 4. D1: with an explicit input, a failed fetch always refuses. Cached
    # refs, or an object already in the clone, never stand in for a fetch.
    if [ "$fetched" -eq 0 ]; then
        refuse refuse-fetch-failed "git fetch origin failed in $(show_path "$repo") (requested base_branch '$(show "$bb")', base_sha '$(show "$bs")'); cached refs cannot prove freshness, so no worktree is created (issue #1957)."
    fi

    # 5. Result checks.
    local tip=""
    if [ -n "$bb" ]; then
        [ "$branch_fetched" -eq 1 ] \
            && tip="$(git -C "$repo" rev-parse --verify --quiet "refs/remotes/origin/${bb}^{commit}" 2>/dev/null)" \
            && [ -n "$tip" ] \
            || refuse refuse-missing-branch "base_branch '$bb' not found on origin in $(show_path "$repo") (refs/remotes/origin/$bb missing after fetch) (issue #1957)."
    fi

    if [ -n "$bs" ]; then
        # The commit check is rev-parse; cat-file only tells a missing object
        # from one that is not a commit.
        local peeled
        peeled="$(git -C "$repo" rev-parse --verify --quiet "${bs}^{commit}" 2>/dev/null || true)"
        if [ "$peeled" != "$bs" ]; then
            git -C "$repo" cat-file -e "$bs" 2>/dev/null \
                || refuse refuse-missing-sha "base_sha $bs not found in $(show_path "$repo") after a successful fetch (actual: none) (issue #1957)."
            [ -n "$peeled" ] || peeled="a $(git -C "$repo" cat-file -t "$bs" 2>/dev/null || echo object)"
            refuse refuse-not-commit "base_sha $bs is not a commit in $(show_path "$repo") (resolves to $peeled) (issue #1957)."
        fi
        if [ -n "$bb" ] && ! git -C "$repo" merge-base --is-ancestor "$bs" "$tip" 2>/dev/null; then
            refuse refuse-not-contained "base_sha $bs is not contained in origin/$bb (tip $tip) in $(show_path "$repo") (issue #1957)."
        fi
    fi

    echo "RULE: fetched" >&2
    if [ -n "$bb" ]; then
        print_result "origin/$bb" "$bb" "${bs:-$tip}" verified fetched "$fp"
    else
        print_result "$bs" "" "$bs" verified fetched "$fp"
    fi
}

# --- verify-worktree ----------------------------------------------------------

cmd_verify_worktree() {
    [ "$#" -ge 5 ] || usage "verify-worktree needs <worktree> <sha> created|reused <main-repo> <fingerprint>"
    local wt="$1" sha="$2" mode="$3" main="$4" fp="$5" branch="" cleanup=0
    shift 5
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --branch)
                [ "$#" -ge 2 ] || usage "--branch needs a value"
                branch="$2"; shift 2 ;;
            --cleanup-created) cleanup=1; shift ;;
            *) usage "unknown verify-worktree argument '$(show "$1")'" ;;
        esac
    done
    case "$mode" in created|reused) ;; *) usage "mode must be created or reused" ;; esac
    valid_sha "$sha" || usage "<sha> must be a full 40- or 64-character hex SHA"
    [ -n "$wt" ] && [ -n "$main" ] || usage "<worktree> and <main-repo> must be non-empty"
    if [ -n "$branch" ] && ! valid_branch "$branch"; then usage "--branch is not a valid branch name"; fi

    local msg="" wt_phys top common main_common actual wt_confirmed=0
    local wt_s main_s
    wt_s="$(show_path "$wt")"
    main_s="$(show_path "$main")"
    wt_phys="$(physical "$wt")"
    top="$(git -C "$wt" rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -z "$wt_phys" ] || [ "$top" != "$wt_phys" ]; then
        msg="worktree $wt_s has toplevel $(show_path "${top:-none}"), expected $wt_s (issue #1957)."
    fi
    if [ -z "$msg" ]; then
        common="$(cd "$wt" 2>/dev/null && physical "$(git rev-parse --git-common-dir 2>/dev/null)")"
        main_common="$(cd "$main" 2>/dev/null && physical "$(git rev-parse --git-common-dir 2>/dev/null)")"
        if [ -z "$common" ] || [ "$common" != "$main_common" ]; then
            msg="worktree $wt_s does not share the git common dir of $main_s (issue #1957)."
        fi
    fi
    # SR-4: only a path whose toplevel and common dir both match is this run's
    # worktree; cleanup never touches anything else.
    [ -n "$msg" ] || wt_confirmed=1
    if [ -z "$msg" ] && [ "$(origin_fingerprint "$main")" != "$fp" ]; then
        msg="origin URL changed during setup in $main_s (issue #1957)."
    fi
    if [ -z "$msg" ]; then
        actual="$(git -C "$wt" rev-parse --verify --quiet HEAD 2>/dev/null || echo none)"
        if [ "$mode" = created ] && [ "$actual" != "$sha" ]; then
            msg="new worktree $wt_s HEAD is $actual, expected base_sha $sha (issue #1957)."
        elif [ "$mode" = reused ] && ! git -C "$wt" merge-base --is-ancestor "$sha" HEAD 2>/dev/null; then
            msg="reused worktree $wt_s HEAD $actual does not contain verified base $sha; delete branch ${branch:-<branch>} or rerun with existing_branch (issue #1957)."
        fi
    fi
    [ -n "$msg" ] || return 0

    # --cleanup-created: remove only what this run made. The recipe passes it
    # for a worktree it created or attached in this run, never for a reused one,
    # and nothing is removed unless the path was confirmed above (SR-4).
    if [ "$cleanup" -eq 1 ] && [ "$wt_confirmed" -eq 0 ]; then
        echo "WARNING: not removing $wt_s: it was not confirmed as this run's worktree (issue #1957)." >&2
    fi
    if [ "$cleanup" -eq 1 ] && [ "$wt_confirmed" -eq 1 ] && [ -n "$branch" ]; then
        if git -C "$main" worktree remove --force "$wt" >/dev/null 2>&1; then
            echo "INFO: removed worktree $wt_s created by this run (issue #1957)." >&2
        else
            echo "WARNING: could not remove worktree $wt_s created by this run (issue #1957)." >&2
        fi
        if [ "$mode" = created ]; then
            if git -C "$main" branch -D "$branch" >/dev/null 2>&1; then
                echo "INFO: deleted branch $branch created by this run (issue #1957)." >&2
            else
                echo "WARNING: could not delete branch $branch created by this run (issue #1957)." >&2
            fi
        fi
    fi
    echo "ERROR: $msg" >&2
    exit 1
}

# --- verified-base ------------------------------------------------------------

# context_value <key>: the helper's value on stdout, its exit code returned
# (0 value, 3 key absent, 1/2 cannot read). A missing helper returns 1: the
# context cannot be read, which must never count as "not requested" (SR-1).
# The helper's own stderr is dropped: the caller writes the one ERROR line.
context_value() {
    local helper="$SCRIPT_DIR/recipe_context_value.sh"
    [ -f "$helper" ] || return 1
    bash "$helper" "$1" - 2>/dev/null
}

cmd_verified_base() {
    [ "$#" -eq 0 ] || usage "verified-base takes no arguments"
    local sv="" bb="" rc_sv=0 rc_bb=0
    sv="$(context_value worktree_setup.source_verified)" || rc_sv=$?
    bb="$(context_value worktree_setup.base_branch)" || rc_bb=$?
    case "$rc_sv:$rc_bb" in
        [12]:*|*:[12])
            echo "ERROR: cannot read worktree_setup from the recipe context (helper exit $rc_sv/$rc_bb); refusing to guess a base branch (issue #1957)." >&2
            return 1 ;;
        3:3)
            # No context at all: the environment, and only as a pair.
            if [ -n "${WORKTREE_SETUP_SOURCE_VERIFIED+set}" ] && [ -n "${WORKTREE_SETUP_BASE_BRANCH+set}" ]; then
                sv="$WORKTREE_SETUP_SOURCE_VERIFIED"
                bb="$WORKTREE_SETUP_BASE_BRANCH"
            else
                return 3
            fi ;;
        0:0) ;;
        *)
            # One field from this run's context and the other absent: never
            # completed from the environment.
            [ "$rc_sv" -eq 0 ] || sv=""
            [ "$rc_bb" -eq 0 ] || bb="" ;;
    esac
    # Only the literal `verified` is trusted; a SHA-only run names no branch.
    [ "$sv" = verified ] || return 3
    [ -n "$bb" ] || return 3
    if ! valid_branch "$bb"; then
        echo "ERROR: verified base_branch '$(show "$bb")' is not a valid branch name; refusing to use it (issue #1957)." >&2
        return 1
    fi
    if ! git rev-parse --verify --quiet "refs/remotes/origin/${bb}^{commit}" >/dev/null 2>&1; then
        echo "ERROR: verified base_branch '$bb' has no refs/remotes/origin/$bb in $(show_path "$(pwd -P)"); refusing to fall back (issue #1957)." >&2
        return 1
    fi
    printf '%s\n' "$bb"
}

# --- requested-inputs ---------------------------------------------------------

cmd_requested_inputs() {
    [ "$#" -eq 0 ] || usage "requested-inputs takes no arguments"
    local key v rc any=0
    for key in base_branch base_sha; do
        rc=0
        v="$(context_value "$key")" || rc=$?
        case "$rc" in
            0) [ -z "$(trim "$v")" ] || any=1 ;;
            3) ;;
            *)
                echo "ERROR: cannot read $key from the recipe context (helper exit $rc); refusing to treat it as not given (issue #1957)." >&2
                return 1 ;;
        esac
    done
    [ "$any" -eq 1 ] || return 3
    return 0
}

# --- main ---------------------------------------------------------------------

[ "$#" -ge 1 ] || usage "a subcommand is required (resolve, verify-worktree, verified-base or requested-inputs)"
sub="$1"; shift
case "$sub" in
    resolve) cmd_resolve "$@" ;;
    verify-worktree) cmd_verify_worktree "$@" ;;
    verified-base) cmd_verified_base "$@"; exit $? ;;
    requested-inputs) cmd_requested_inputs "$@"; exit $? ;;
    *) usage "unknown subcommand '$(show "$sub")'" ;;
esac
