#!/usr/bin/env bash
# autodrive_qa_docs.sh — is this pull request documentation and nothing else?
# (issue #2219.) autodrive_qa_scope.sh loads this file from its own physical
# directory and from nowhere else: never $PWD, REPO_PATH or the pull
# request's tree. It is a gate file (GATE_FILES in merge_guard.rs), because
# the merge gate's gadugi decision passes through it.
#
# It lives beside autodrive_qa_scope.sh rather than inside it because that
# file is at its 400-line brick budget (test-issue-1516-qa-command-selection.sh,
# "static constraints"), the same reason autodrive_qa_sources.sh is its own
# file. The split is physical only: this file reads _AD_PATHS and
# _AD_PATHS_STATE from the scan autodrive_qa_scope.sh already made, in the
# same shell, so it runs no git command of its own.
#
#   autodrive_qa_docs_only <dir>  ->  true | false | unknown
#
# This file only DEFINES a function and runs nothing it reads: no eval, no
# shell on built text, and a changed path is only ever compared with `case`
# and parameter expansion, never executed. LC_ALL=C; bash 3.2+ and tr.
# Contract: docs/reference/auto-drive-to-merge.md, "Documentation-only pull
# requests", which lists each rule and the reason for it.

# autodrive_qa_docs_only <dir> -> true, false or unknown: `true` only when the
# changed-path list is KNOWN and NON-EMPTY and every path is inert
# documentation — prose that no agent and no build reads as input.
#
# Qualifying, compared BYTE-EXACT: a path ending `.md`, `.markdown`, `.rst` or
# `.txt`, or a root `LICENSE`, `NOTICE` or `COPYING`.
#
# Disqualifying whatever else matches, compared CASE-INSENSITIVELY:
#   - anything under `.github/`, `.amplihack/`, `.claude/` or
#     `amplifier-bundle/`, at the root or nested: agent instructions, and the
#     declared build and QA sources, decide what runs and what may merge.
#   - a basename of `AGENTS.md`, `CLAUDE.md` or `SKILL.md`, wherever it sits.
#     A pull request editing only `merge-ready/SKILL.md` edits the rules that
#     decide what may merge, so it does not buy its way out of them.
#   - any `.yml`, `.yaml`, `.toml`, `.json` or `.sh`.
#
# The two asymmetries are deliberate and both fail closed: `docs/build.SH` is
# a script, while `README.MD` simply costs the exemption rather than winning
# it. The `docs/` DIRECTORY qualifies nothing on its own, so `docs/conf.py`
# and `docs/release.js` are code that happens to live under docs/.
#
# The other arms fail closed too. An empty diff changed no documentation, so
# it is `false`, not `true`. A list that could not be read — no base given, a
# base that fails the branch rules, a git failure — is `unknown`, and
# `unknown` is not `true`: autodrive_gadugi_required keeps gadugi required on
# it. That function is the only intended caller, so the merge round and the
# merge gate cannot disagree about the answer.
#
# The paths come from _AD_PATHS, not from the line autodrive_qa_changed_paths
# prints: that line is rendered for people, capped at ten entries with a
# `(+N more)` tail and with every byte outside A-Z a-z 0-9 . _ / - rewritten
# as `_`. A predicate built on it would read a pull request of eleven changed
# paths whose eleventh is `src/index.js` as documentation-only, and would
# judge a rewritten byte rather than the path.
autodrive_qa_docs_only() {
  local -x LC_ALL=C
  local p="" l=""
  _autodrive_qa_scan "${1:-.}"
  case "$_AD_PATHS_STATE" in
    known) ;;
    empty) printf 'false\n'; return 0 ;;
    *) printf 'unknown\n'; return 0 ;;
  esac
  for p in "${_AD_PATHS[@]}"; do
    # One lowercased copy with a leading `/`, so that a directory rule and a
    # root basename (`/agents.md`) match the same `*/name` pattern.
    l="/$(printf '%s' "$p" | tr 'A-Z' 'a-z')"
    case "$l" in
      */.github/* | */.amplihack/* | */.claude/* | */amplifier-bundle/* \
        | */agents.md | */claude.md | */skill.md \
        | *.yml | *.yaml | *.toml | *.json | *.sh) printf 'false\n'; return 0 ;;
    esac
    case "$p" in
      *.md | *.markdown | *.rst | *.txt | LICENSE | NOTICE | COPYING) ;;
      *) printf 'false\n'; return 0 ;;
    esac
  done
  printf 'true\n'
}
