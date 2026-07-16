#!/usr/bin/env bats
# ---------------------------------------------------------------------------
# TDD contract for issue #909 — migrate skill project reconstruction.
#
# The resumed session on the destination VM must land in a valid git checkout,
# not $HOME. This suite pins the pure, sourceable helper functions that the
# reconstruction phase of migrate.sh must expose so the cross-user path remap
# and workspace.yaml rewrite are testable off-host (no azlin / ssh / clone).
#
# These tests FAIL until migrate.sh:
#   1. Can be sourced in "library only" mode (AMPLIHACK_MIGRATE_LIB=1) so that
#      sourcing defines functions WITHOUT running the migration main flow.
#   2. Defines the migrate_* helper functions contracted below.
#
# Run:  bats amplifier-bundle/skills/migrate/scripts/tests/test_workspace_rewrite.bats
# ---------------------------------------------------------------------------

setup() {
  SCRIPT_UNDER_TEST="${BATS_TEST_DIRNAME}/../migrate.sh"

  # Deterministic, isolated destination $HOME for remap assertions.
  TEST_HOME="$(mktemp -d)"
  export HOME="$TEST_HOME"

  # Source the script in library-only mode: this MUST define the migrate_*
  # functions and MUST NOT parse args / run the migration / call exit.
  export AMPLIHACK_MIGRATE_LIB=1
  # shellcheck disable=SC1090
  source "$SCRIPT_UNDER_TEST"
}

teardown() {
  [[ -n "${TEST_HOME:-}" && -d "$TEST_HOME" ]] && rm -rf "$TEST_HOME"
}

# Helper: write a workspace.yaml fixture into $1 with given field values.
_write_workspace_yaml() {
  local file="$1" cwd="$2" git_root="$3" repository="$4" host_type="$5" branch="$6"
  cat >"$file" <<YAML
session_id: b127f92a-0000-4000-8000-000000000000
cwd: ${cwd}
git_root: ${git_root}
repository: ${repository}
host_type: ${host_type}
branch: ${branch}
YAML
}

# ===========================================================================
# Library-mode sourcing contract
# ===========================================================================

@test "sourcing in library mode does not run the migration or exit nonzero" {
  # setup() already sourced it; if sourcing had run main it would have exited
  # (no DEST_HOST => exit 2) and this file would never run. Assert the guard
  # variable is honored and a core helper is defined.
  run type -t migrate_yaml_field
  [ "$status" -eq 0 ]
  [ "$output" = "function" ]
}

# ===========================================================================
# migrate_yaml_field  — flat top-level scalar parser (grep/sed, no yq)
# ===========================================================================

@test "migrate_yaml_field extracts a plain scalar value" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork" "/home/rysweet/src/azork" \
    "rysweet/azork" "github" "main"
  run migrate_yaml_field "$f" cwd
  [ "$status" -eq 0 ]
  [ "$output" = "/home/rysweet/src/azork" ]
}

@test "migrate_yaml_field reads each of the five reconstruction fields" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork/worktrees/feat/x" "/home/rysweet/src/azork/worktrees/feat/x" \
    "rysweet/azork" "github" "feat/x"
  run migrate_yaml_field "$f" git_root
  [ "$output" = "/home/rysweet/src/azork/worktrees/feat/x" ]
  run migrate_yaml_field "$f" repository
  [ "$output" = "rysweet/azork" ]
  run migrate_yaml_field "$f" host_type
  [ "$output" = "github" ]
  run migrate_yaml_field "$f" branch
  [ "$output" = "feat/x" ]
}

@test "migrate_yaml_field strips surrounding quotes and trailing whitespace" {
  local f="$TEST_HOME/ws.yaml"
  printf 'repository: "rysweet/azork"   \nbranch: %s\n' "'feat/x'" >"$f"
  run migrate_yaml_field "$f" repository
  [ "$output" = "rysweet/azork" ]
  run migrate_yaml_field "$f" branch
  [ "$output" = "feat/x" ]
}

@test "migrate_yaml_field returns empty for an absent field (absence is not an error)" {
  local f="$TEST_HOME/ws.yaml"
  printf 'cwd: /home/rysweet/src/azork\n' >"$f"
  run migrate_yaml_field "$f" repository
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

@test "migrate_yaml_field matches only exact top-level keys (no substring bleak)" {
  local f="$TEST_HOME/ws.yaml"
  printf 'git_root: /home/rysweet/src/azork\nroot_cause: nope\n' >"$f"
  # Asking for 'root' must not return the git_root or root_cause value.
  run migrate_yaml_field "$f" root
  [ -z "$output" ]
}

# ===========================================================================
# Untrusted-field validation (reject-don't-sanitize)
# ===========================================================================

@test "migrate_validate_repository accepts owner/name form" {
  run migrate_validate_repository "rysweet/azork"
  [ "$status" -eq 0 ]
  run migrate_validate_repository "rys-weet_1.x/az.ork_2-y"
  [ "$status" -eq 0 ]
}

@test "migrate_validate_repository rejects malformed / injecting values" {
  run migrate_validate_repository "rysweet"          ; [ "$status" -ne 0 ]
  run migrate_validate_repository "rysweet/az/ork"   ; [ "$status" -ne 0 ]
  run migrate_validate_repository "rysweet/azork; rm -rf ~" ; [ "$status" -ne 0 ]
  run migrate_validate_repository "-flag/azork"      ; [ "$status" -ne 0 ]
  run migrate_validate_repository ""                 ; [ "$status" -ne 0 ]
  run migrate_validate_repository "$(printf 'rysweet/azork\nevil')" ; [ "$status" -ne 0 ]
}

@test "migrate_validate_branch accepts normal branch names incl. slashes" {
  run migrate_validate_branch "main"    ; [ "$status" -eq 0 ]
  run migrate_validate_branch "feat/x"  ; [ "$status" -eq 0 ]
  run migrate_validate_branch "release/1.2.3" ; [ "$status" -eq 0 ]
}

@test "migrate_validate_branch rejects leading dash, dotdot, and newlines" {
  run migrate_validate_branch "-force"        ; [ "$status" -ne 0 ]   # git-option injection
  run migrate_validate_branch "feat/../evil"  ; [ "$status" -ne 0 ]   # path traversal
  run migrate_validate_branch ".."            ; [ "$status" -ne 0 ]
  run migrate_validate_branch "feat/x;reboot" ; [ "$status" -ne 0 ]
  run migrate_validate_branch ""              ; [ "$status" -ne 0 ]
  run migrate_validate_branch "$(printf 'main\nevil')" ; [ "$status" -ne 0 ]
}

@test "migrate_validate_host_type allows only github" {
  run migrate_validate_host_type "github" ; [ "$status" -eq 0 ]
  run migrate_validate_host_type "gitlab" ; [ "$status" -ne 0 ]
  run migrate_validate_host_type ""       ; [ "$status" -ne 0 ]
}

# ===========================================================================
# Cross-user path remap  — paths re-derived from validated repo/branch,
# always strictly under the destination $HOME (D1 REWRITE strategy).
# ===========================================================================

@test "migrate_remap_git_root always maps to \$HOME/src/<repo>" {
  run migrate_remap_git_root "rysweet/azork"
  [ "$status" -eq 0 ]
  [ "$output" = "$HOME/src/azork" ]
}

@test "migrate_remap_cwd for a plain (non-worktree) session equals the git root" {
  # Source cwd has no /worktrees/ segment => plain session.
  run migrate_remap_cwd "/home/rysweet/src/azork" "rysweet/azork" "main"
  [ "$status" -eq 0 ]
  [ "$output" = "$HOME/src/azork" ]
}

@test "migrate_remap_cwd for a worktree session appends worktrees/<branch>" {
  run migrate_remap_cwd "/home/rysweet/src/azork/worktrees/feat/x" "rysweet/azork" "feat/x"
  [ "$status" -eq 0 ]
  [ "$output" = "$HOME/src/azork/worktrees/feat/x" ]
}

@test "migrate_remap_cwd normalizes double-nested worktrees to a single level" {
  run migrate_remap_cwd \
    "/home/rysweet/src/azork/worktrees/A/worktrees/B" "rysweet/azork" "feat/deep"
  [ "$status" -eq 0 ]
  # Tail is re-derived from the validated branch, never copied verbatim.
  [ "$output" = "$HOME/src/azork/worktrees/feat/deep" ]
}

@test "migrate_remap_cwd never reuses the source git_root / source home prefix" {
  run migrate_remap_cwd "/home/rysweet/src/azork" "rysweet/azork" "main"
  [[ "$output" != *"/home/rysweet/"* ]]
  [[ "$output" == "$HOME/"* ]]
}

# ===========================================================================
# $HOME-containment gate (exit 12 semantics)
# ===========================================================================

@test "migrate_assert_under_home accepts a path inside \$HOME" {
  run migrate_assert_under_home "$HOME/src/azork"
  [ "$status" -eq 0 ]
}

@test "migrate_assert_under_home rejects paths outside \$HOME and traversal escapes" {
  run migrate_assert_under_home "/etc/passwd"              ; [ "$status" -ne 0 ]
  run migrate_assert_under_home "$HOME/../other/evil"      ; [ "$status" -ne 0 ]
  run migrate_assert_under_home "$HOME"                    ; [ "$status" -ne 0 ]  # must be strictly under
  run migrate_assert_under_home "${HOME}_evil/src"         ; [ "$status" -ne 0 ]  # sibling prefix trap
}

# ===========================================================================
# Reconstruction gating: skip vs. reconstruct vs. hard-fail
# ===========================================================================

@test "migrate_should_reconstruct is true for a github repo with git_root" {
  run migrate_should_reconstruct "rysweet/azork" "/home/rysweet/src/azork" "github"
  [ "$status" -eq 0 ]
}

@test "migrate_should_reconstruct skips when repository or git_root is absent" {
  run migrate_should_reconstruct "" "/home/rysweet/src/azork" "github"
  [ "$status" -ne 0 ]   # skip (resume in \$HOME, no hard-gate)
  run migrate_should_reconstruct "rysweet/azork" "" "github"
  [ "$status" -ne 0 ]
}

@test "migrate_should_reconstruct skips a non-github host_type" {
  run migrate_should_reconstruct "rysweet/azork" "/home/rysweet/src/azork" "gitlab"
  [ "$status" -ne 0 ]   # skip, NOT exit 11
}

# ===========================================================================
# Atomic workspace.yaml rewrite  — cwd + git_root, mode preserved, sed-safe
# ===========================================================================

@test "migrate_rewrite_workspace_yaml rewrites both cwd and git_root" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork/worktrees/feat/x" "/home/rysweet/src/azork" \
    "rysweet/azork" "github" "feat/x"

  run migrate_rewrite_workspace_yaml "$f" \
    "$HOME/src/azork/worktrees/feat/x" "$HOME/src/azork"
  [ "$status" -eq 0 ]

  run migrate_yaml_field "$f" cwd
  [ "$output" = "$HOME/src/azork/worktrees/feat/x" ]
  run migrate_yaml_field "$f" git_root
  [ "$output" = "$HOME/src/azork" ]
}

@test "migrate_rewrite_workspace_yaml leaves other fields untouched" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork" "/home/rysweet/src/azork" \
    "rysweet/azork" "github" "main"
  migrate_rewrite_workspace_yaml "$f" "$HOME/src/azork" "$HOME/src/azork"
  run migrate_yaml_field "$f" repository
  [ "$output" = "rysweet/azork" ]
  run migrate_yaml_field "$f" branch
  [ "$output" = "main" ]
  run migrate_yaml_field "$f" session_id
  [ "$output" = "b127f92a-0000-4000-8000-000000000000" ]
}

@test "migrate_rewrite_workspace_yaml preserves the original file mode" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork" "/home/rysweet/src/azork" \
    "rysweet/azork" "github" "main"
  chmod 600 "$f"
  local before after
  before="$(stat -c '%a' "$f")"
  migrate_rewrite_workspace_yaml "$f" "$HOME/src/azork" "$HOME/src/azork"
  after="$(stat -c '%a' "$f")"
  [ "$before" = "$after" ]
}

@test "migrate_rewrite_workspace_yaml treats replacement as literal (sed metachars safe)" {
  local f="$TEST_HOME/ws.yaml"
  _write_workspace_yaml "$f" \
    "/home/rysweet/src/azork" "/home/rysweet/src/azork" \
    "rysweet/azork" "github" "main"
  # Replacement containing sed-special chars & and \ must be inserted literally.
  local weird="$HOME/src/a&b\\c"
  migrate_rewrite_workspace_yaml "$f" "$weird" "$weird"
  run migrate_yaml_field "$f" cwd
  [ "$output" = "$weird" ]
}

# ===========================================================================
# Static hygiene
# ===========================================================================

@test "migrate.sh passes shellcheck (skipped if shellcheck unavailable)" {
  if ! command -v shellcheck >/dev/null 2>&1; then
    skip "shellcheck not installed"
  fi
  run shellcheck -x "$SCRIPT_UNDER_TEST"
  [ "$status" -eq 0 ]
}
