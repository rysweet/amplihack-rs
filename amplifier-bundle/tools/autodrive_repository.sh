#!/usr/bin/env bash
# Source from trusted launch-side tools before any PR lookup. Context is the
# authority; inherited GH_REPO is never used to choose an explicit target.
autodrive_bind_repository() {
  local target="$1" checkout="$2" origin owner_repo
  [[ "$target" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
    echo "ERROR: repository requires canonical OWNER/REPO" >&2; return 1;
  }
  origin="$(git -C "$checkout" remote get-url origin)" || return 1
  case "$origin" in
    https://github.com/*) owner_repo="${origin#https://github.com/}" ;;
    git@github.com:*) owner_repo="${origin#git@github.com:}" ;;
    ssh://git@github.com/*) owner_repo="${origin#ssh://git@github.com/}" ;;
    *) echo "ERROR: cannot bind repository to checkout origin: $origin" >&2; return 1 ;;
  esac
  owner_repo="${owner_repo%/}"; owner_repo="${owner_repo%.git}"
  [ "$(printf '%s' "$owner_repo" | tr '[:upper:]' '[:lower:]')" = "$(printf '%s' "$target" | tr '[:upper:]' '[:lower:]')" ] || {
    echo "ERROR: repository $target does not match checkout origin $owner_repo" >&2; return 1;
  }
  export GH_REPO="$target"
}
