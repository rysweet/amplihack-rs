#!/usr/bin/env bash
# Scoped checkpoint commit; every normal hook remains authoritative.
commit_with_pre_commit_guard() {
  amplihack_prepare_git_commit_identity || return $?
  amplihack hygiene workflow checkpoint --repo . "$@"
}
