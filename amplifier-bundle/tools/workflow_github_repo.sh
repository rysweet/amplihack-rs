#!/usr/bin/env bash
# Shared GitHub repo identity parser (issues #1642, #1643).
#
# parse_github_repo_identity <url>
#   Prints "owner/repo" and returns 0 when <url> names a github.com
#   repository; otherwise prints nothing (stdout or stderr) and returns 1.
#
# This is the ONLY bash copy of this parsing. Every bundle caller sources this
# file (standalone scripts via their SCRIPT_DIR, sourced libraries via
# BASH_SOURCE, workflow-terminal-state.yaml via its helper lookup), and a guard
# test fails on any other copy. The Rust twin in
# crates/amplihack-cli/src/commands/multitask/command_builder.rs follows the
# same contract. See docs/reference/workflow-github-repo-identity.md.
#
# Accepted prefixes (exact, case-sensitive):
#   https://github.com/  http://github.com/  git@github.com:
#   ssh://git@github.com/  http(s)://<userinfo>@github.com/
# The userinfo is the text before the FIRST "@github.com/" and must be
# non-empty RFC 3986 userinfo characters without '@'. Anything else - '/',
# '?', '#', '\', whitespace, control or non-ASCII bytes - means git would
# contact another host (https://evil.example/x@github.com/o/r), so the URL is
# rejected.
#
# This file only DEFINES a function: no set -e/-u/-o, no shopt, no globals,
# nothing printed when sourced. bash 3.2 compatible.

parse_github_repo_identity() {
  # Pin the locale so bracket ranges match ASCII only, exactly as Rust does.
  local LC_ALL=C
  local url="${1:-}" rest userinfo path owner repo
  local userinfo_re="^[A-Za-z0-9._~%!\$&'()*+,;=:-]+\$"
  local segment_re='^[A-Za-z0-9._-]+$'
  case "$url" in
    https://github.com/*) path="${url#https://github.com/}" ;;
    http://github.com/*) path="${url#http://github.com/}" ;;
    git@github.com:*) path="${url#git@github.com:}" ;;
    ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
    https://*@github.com/*|http://*@github.com/*)
      rest="${url#*://}"
      # %% (longest suffix) keeps the text before the FIRST "@github.com/";
      # % would take the last one and let "a@github.com/x@github.com/o/r"
      # smuggle "a@github.com/x" past the check as userinfo.
      userinfo="${rest%%@github.com/*}"
      [[ "$userinfo" =~ $userinfo_re ]] || return 1
      path="${rest#*@github.com/}" ;;
    *) return 1 ;;
  esac
  path="${path%%\?*}"
  path="${path%%#*}"
  path="${path%/}"
  path="${path%.git}"
  case "$path" in */*) ;; *) return 1 ;; esac
  owner="${path%%/*}"
  repo="${path#*/}"
  repo="${repo%%/*}"
  case "$owner" in .|..|-*) return 1 ;; esac
  case "$repo" in .|..) return 1 ;; esac
  [[ "$owner" =~ $segment_re ]] || return 1
  [[ "$repo" =~ $segment_re ]] || return 1
  printf '%s/%s\n' "$owner" "$repo"
}
