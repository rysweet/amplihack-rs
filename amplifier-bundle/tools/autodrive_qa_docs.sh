#!/usr/bin/env bash
# autodrive_qa_docs.sh — is this pull request documentation and nothing else?
# (issue #2219.) autodrive_qa_scope.sh loads this file from its own physical
# directory and from nowhere else: never $PWD, REPO_PATH or the pull
# request's tree. It is a gate file (GATE_FILES in merge_guard.rs). Since
# #2569 its answer is a fact the criterion-1 judging agent may cite, not an
# exemption.
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
# Qualifying, compared BYTE-EXACT: a path ending `.md`, `.markdown` or `.rst`,
# or a root `LICENSE`, `NOTICE` or `COPYING`.
#
# `.txt` qualifies only under a `docs/` component at any depth, or as a root
# `README.txt`, `CHANGELOG.txt`, `CHANGES.txt`, `LICENSE.txt`, `NOTICE.txt`,
# `COPYING.txt`, `AUTHORS.txt`, `CONTRIBUTING.txt` or `INSTALL.txt`. The
# extension is generic — a golden fixture, a page template, a fuzz corpus seed
# and a dependency pin all wear it — so qualifying it everywhere exempted
# `tests/fixtures/expected.txt`, `src/templates/welcome.txt`,
# `fuzz/corpus/seed.txt`, `requirements-dev.txt` and `Makefile.txt` as
# documentation. Naming the places prose lives closes the class; the pin
# disqualifiers below stay, because `docs/requirements.txt` is a Sphinx pin
# sitting in exactly the directory this rule trusts.
#
# Disqualifying whatever else matches, compared CASE-INSENSITIVELY:
#   - anything under a DOT-DIRECTORY at any depth — `.github/`, `.claude/`,
#     `.amplihack/`, and `.cursor/`, `.codex/`, `.devcontainer/` and whatever
#     the next tool invents — or under `amplifier-bundle/`: agent
#     instructions, and the declared build and QA sources, decide what runs
#     and what may merge. One pattern, `*/.*/*`, the same one
#     _autodrive_manifest_dirs uses, so a dot-directory nobody thought to
#     list is covered too. A dot-FILE is not a dot-directory: the pattern
#     needs a `/` after the dotted name, so `docs/.hidden.md` is still prose.
#   - a basename of `AGENTS.md`, `CLAUDE.md` or `SKILL.md`, wherever it sits.
#     A pull request editing only `merge-ready/SKILL.md` edits the rules that
#     decide what may merge, so it does not buy its way out of them.
#   - a `.txt` whose path holds `requirements` or `constraints`, and a
#     basename of `CMakeLists.txt`. A build reads them, not a person: the
#     first two pin the dependencies a suite installs and the third is a
#     build script. The narrowed `.txt` qualifier already stops every pin at
#     the root, so what this rule still catches is the copy under `docs/` —
#     `docs/requirements.txt` is the ordinary Sphinx pin, which is why the
#     rule is not anchored at the root. The two pin patterns match a
#     SUBSTRING rather than a basename, because an exact name is one spelling
#     away from the next: `docs/requirements-dev.txt`,
#     `docs/dev-requirements.txt` and `docs/requirements/base.txt` are all the
#     same build input. The cost is that a prose file named for requirements
#     loses the exemption and its pull request runs the scenarios it would
#     otherwise have skipped, which is the direction this predicate fails in.
#     `requirements.txt` and `CMakeLists.txt` are also in
#     AUTODRIVE_ROOT_PROJECT_FILES (autodrive_trust.sh), where this same
#     repository already calls them project files. That array is NOT read
#     here: it may be undefined in some load order, and a disqualifier that
#     silently disappears is the failure being fixed. The test pins the two
#     lists together instead, so the next `.txt` added there is caught.
#
# Every rule listed is a rule the code runs. There is no extension
# disqualifier: the byte-exact qualifier above already rejects every `.yml`,
# `.yaml`, `.toml`, `.json` and `.sh`, so one here could never be reached.
#
# The asymmetry between the two comparisons is deliberate and fails closed in
# both directions. `Agents.md`, `.GitHub/notes.md` and `docs/Requirements.txt`
# are disqualified on a case-insensitive match; `README.MD` simply costs the
# exemption rather than winning it, since it never qualifies in the first
# place. The `docs/` DIRECTORY qualifies no EXTENSION on its own — it only
# widens where a `.txt` may sit — so `docs/conf.py` and `docs/release.js` are
# code that happens to live under docs/, and `docs/build.SH` is not
# documentation because `.SH` is not `.txt`, `.md`, `.markdown` or `.rst`.
#
# The other arms fail closed too. A diff that changed nothing changed no
# documentation either, so it is `false`, not `true`. A list that could not be
# read — no base given, a base that fails the branch rules, a git failure — is
# `unknown`, and `unknown` is not `true`. autodrive_qa_evidence.sh records the
# answer in qa-inputs.json for the judging agent.
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
  # A `known` list holding nothing is the `empty` answer, not the `unknown`
  # one: the list was read and it named no documentation. _autodrive_qa_scan
  # makes `known` imply non-empty today, so the guard is unreachable; it is
  # here because its absence does not fail the same way twice — bash 5 walks
  # out of the loop and exempts the whole pull request, bash 3.2 stops on an
  # unbound variable — and the production shell is the one that fails open.
  case "$_AD_PATHS_STATE" in
    known) [ -n "${_AD_PATHS[0]+x}" ] || { printf 'false\n'; return 0; } ;;
    empty) printf 'false\n'; return 0 ;;
    *) printf 'unknown\n'; return 0 ;;
  esac
  # The byte-exact rule first, because it needs no lowercased copy and so
  # costs no process. Either rule failing on any path gives `false`, so which
  # of the two sees a path first cannot change the answer.
  for p in "${_AD_PATHS[@]}"; do
    case "$p" in
      *.md | *.markdown | *.rst | LICENSE | NOTICE | COPYING) ;;
      # `.txt` only where prose lives. `*` spans `/` in a case pattern, so the
      # first arm is any depth under a `docs/` component, at the root or below
      # one; the second is the root documentation names, byte-exact like
      # LICENSE above. Everything else ending `.txt` falls through to `false`.
      docs/*.txt | */docs/*.txt) ;;
      README.txt | CHANGELOG.txt | CHANGES.txt | LICENSE.txt | NOTICE.txt \
        | COPYING.txt | AUTHORS.txt | CONTRIBUTING.txt | INSTALL.txt) ;;
      *) printf 'false\n'; return 0 ;;
    esac
  done
  # Now the case-insensitive rules, against a lowercased copy of the list
  # carrying a leading `/` per path, so that a directory rule and a root
  # basename (`/agents.md`) match the same `*/name` pattern. ONE `tr` for the
  # whole list rather than one per path: a fork costs milliseconds and a
  # large pull request has hundreds of paths. Safe because a changed path
  # holding a newline already emptied _AD_PATHS in _autodrive_qa_changed_raw,
  # so no path spans two lines here. A here-string, never a pipe, so that
  # `return` below returns from this function.
  while IFS= read -r l; do
    case "/$l" in
      */.*/* | */amplifier-bundle/* \
        | */agents.md | */claude.md | */skill.md \
        | *requirements*.txt | *constraints*.txt | */cmakelists.txt) printf 'false\n'; return 0 ;;
    esac
  done <<< "$(printf '%s\n' "${_AD_PATHS[@]}" | tr 'A-Z' 'a-z')"
  printf 'true\n'
}
