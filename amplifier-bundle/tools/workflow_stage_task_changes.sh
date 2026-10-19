#!/usr/bin/env bash
# Stage the task's changes for step-15-commit-push (issue #1575).
#
# Step 15 used to run a blanket `git add -A`, which swept in a crate version
# bump the repository's release automation owns and amplihack-generated
# `.github/hooks/` stubs. This helper stages the worktree, then takes back out
# of the index:
#
#   * every path under a generated directory (GENERATED_PATHS), with a WARNING
#     per path. Untracked ones are also added to info/exclude so the later
#     `git add -A` steps (18c, 20b) do not sweep them in.
#   * version-only churn in Cargo.toml / Cargo.lock / package.json (any
#     depth), unless ALLOW_VERSION_BUMP permits it and step 14 asked for a
#     bump (new_version != current_version). Excluded churn is saved as a
#     patch, then the files are restored. A file mixing a version change with
#     other edits keeps every version file staged (all-or-nothing keeps
#     Cargo.toml and Cargo.lock consistent for --locked builds).
#
# Exit 0 when staging finished (the index may be empty); exit 2 when it could
# not finish safely, after restoring the index saved at the start.
# Contract: docs/reference/workflow-publish-helpers.md.

set -uo pipefail
export GIT_LITERAL_PATHSPECS=1 GIT_PAGER=cat

GENERATED_PATHS=(".github/hooks/")
VERSION_FILES=(Cargo.toml Cargo.lock package.json)
EXCLUDE_HEADER="# amplihack #1575: generated paths"
SAFE_PATH_RE='^\.github/hooks/[A-Za-z0-9._/-]+$'

[ $# -eq 0 ] || { echo "usage: $(basename "$0") (takes no arguments)" >&2; exit 2; }
top="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "ERROR: step-15 staging: $(pwd) is not inside a git work tree" >&2; exit 2; }
cd "$top" || { echo "ERROR: step-15 staging: cannot enter the work tree top" >&2; exit 2; }
pre_tree="$(git write-tree 2>/dev/null)" || { echo "ERROR: step-15 staging: git write-tree failed; the index cannot be saved (unmerged paths?)" >&2; exit 2; }
scratch="$(mktemp -d -t stage-task-changes-XXXXXX)" || { echo "ERROR: step-15 staging: mktemp failed" >&2; exit 2; }
trap 'rm -rf "$scratch"' EXIT

rollback() {
  echo "ERROR: step-15 staging failed: $1; restoring the index to its state before staging" >&2
  if ! git read-tree "$pre_tree"; then
    echo "ERROR: step-15 staging: git read-tree $pre_tree failed; the index needs manual repair" >&2
  fi
  # Re-stat the restored index; a non-zero status here only means files differ.
  git update-index -q --refresh >/dev/null 2>&1 || :
  exit 2
}

has_head=false
git rev-parse -q --verify 'HEAD^{commit}' >/dev/null 2>&1 && has_head=true
in_head() { [ "$has_head" = true ] && git cat-file -e "HEAD:$1" 2>/dev/null; }

git add -A || rollback "git add -A failed"

# --- generated paths -------------------------------------------------------
exclude_file="$(git rev-parse --git-path info/exclude)" || rollback "cannot locate info/exclude"
add_exclude() {
  mkdir -p "$(dirname "$exclude_file")" || return 1
  touch "$exclude_file" || return 1
  grep -Fxq -- "$EXCLUDE_HEADER" "$exclude_file" || printf '%s\n' "$EXCLUDE_HEADER" >>"$exclude_file" || return 1
  grep -Fxq -- "$1" "$exclude_file" || printf '%s\n' "$1" >>"$exclude_file"
}
git diff --cached --name-only -z --no-renames -- "${GENERATED_PATHS[@]}" >"$scratch/generated" ||
  rollback "cannot list staged generated paths"
while IFS= read -r -d '' path; do
  if in_head "$path"; then
    git restore --staged -- "$path" || rollback "cannot unstage $path"
    echo "WARNING: step-15 unstaged generated path $path (tracked; modification left in the working tree)" >&2
    continue
  fi
  git rm --cached -q -- "$path" >/dev/null || rollback "cannot unstage $path"
  if [[ "$path" =~ $SAFE_PATH_RE ]] && [[ "$path" != *..* ]]; then
    add_exclude "/$path" || rollback "cannot write $exclude_file"
    echo "WARNING: step-15 unstaged generated path $path (added to info/exclude)" >&2
  else
    echo "WARNING: step-15 unstaged generated path $path (not added to info/exclude: path contains unsupported characters)" >&2
  fi
done <"$scratch/generated"

# --- version churn ---------------------------------------------------------
# Print the line numbers of the project's own version lines in one file.
own_version_lines() {
  case "$1" in
    Cargo.toml) awk '
      /^[[:space:]]*\[/ { s = $0; gsub(/[[:space:]]/, "", s); own = (s ~ /^\[(package|workspace\.package)\]/); next }
      own && /^[[:space:]]*version[[:space:]]*=/ { print NR }' ;;
    Cargo.lock) awk '
      function flush() { if (pkg && ver && !src) print ver; pkg = 0; ver = 0; src = 0 }
      /^\[\[package\]\][[:space:]]*$/ { flush(); pkg = 1; next }
      /^[[:space:]]*\[/ { flush(); next }
      pkg && /^[[:space:]]*version[[:space:]]*=/ { ver = NR }
      pkg && /^[[:space:]]*source[[:space:]]*=/ { src = 1 }
      END { flush() }' ;;
    package.json) awk '
      { if (depth == 1 && $0 ~ /^[[:space:]]*"version"[[:space:]]*:/) print NR
        n = length($0)
        for (i = 1; i <= n; i++) {
          c = substr($0, i, 1)
          if (instr) { if (esc) esc = 0; else if (c == "\\") esc = 1; else if (c == "\"") instr = 0 }
          else if (c == "\"") instr = 1
          else if (c == "{" || c == "[") depth++
          else if (c == "}" || c == "]") depth--
        } }' ;;
  esac
}

# Print churn, mixed or none for one staged, modified version file.
classify() {
  local path="$1" base="${1##*/}"
  git cat-file blob "HEAD:$path" >"$scratch/old" 2>/dev/null || return 1
  git cat-file blob ":0:$path" >"$scratch/new" 2>/dev/null || return 1
  own_version_lines "$base" <"$scratch/old" >"$scratch/old.lines" || return 1
  own_version_lines "$base" <"$scratch/new" >"$scratch/new.lines" || return 1
  git diff --cached --no-ext-diff --no-textconv --no-color -U0 -- "$path" >"$scratch/diff" || return 1
  awk -v oldf="$scratch/old.lines" -v newf="$scratch/new.lines" '
    BEGIN { while ((getline l < oldf) > 0) old[l] = 1; while ((getline l < newf) > 0) new[l] = 1 }
    /^@@ / { split($2, o, ","); split($3, n, ","); ol = substr(o[1], 2) + 0; nl = substr(n[1], 2) + 0; hunk = 1; next }
    !hunk || /^\\/ { next }
    /^-/ { if (ol in old) own++; else other++; ol++; next }
    /^\+/ { if (nl in new) own++; else other++; nl++; next }
    END { print (own && !other) ? "churn" : (own ? "mixed" : "none") }' "$scratch/diff"
}

git diff --cached --name-only -z --no-renames --diff-filter=M >"$scratch/modified" ||
  rollback "cannot list staged files"
churn=()
mixed=()
while IFS= read -r -d '' path; do
  is_version_file=false
  for name in "${VERSION_FILES[@]}"; do [ "${path##*/}" = "$name" ] && is_version_file=true; done
  [ "$is_version_file" = true ] || continue
  kind="$(classify "$path")" || rollback "cannot classify $path"
  case "$kind" in churn) churn+=("$path") ;; mixed) mixed+=("$path") ;; esac
done <"$scratch/modified"
[ $((${#churn[@]} + ${#mixed[@]})) -gt 0 ] || exit 0

allow="${ALLOW_VERSION_BUMP:-}"
allow="${allow,,}"
new_version="${VERSION_BUMP_NEW_VERSION:-${RECIPE_VAR_version_bump__new_version:-}}"
current_version="${VERSION_BUMP_CURRENT_VERSION:-${RECIPE_VAR_version_bump__current_version:-}}"
# Version strings only reach log lines; anything outside the allow-list is not echoed.
clean() { [[ $1 =~ ^[0-9A-Za-z.+-]{1,64}$ ]] && printf '%s' "$1" || printf '<invalid>'; }
requested=false
[ -n "$new_version" ] && [ -n "$current_version" ] && [ "$new_version" != "$current_version" ] && requested=true
case "$allow" in true | 1 | yes) allowed=true ;; *) allowed=false ;; esac

if [ "$allowed" = true ] && [ "$requested" = true ]; then
  echo "INFO: step-15 keeping version change $(clean "$current_version") -> $(clean "$new_version") (allow_version_bump=$allow)" >&2
  exit 0
fi
if [ ${#mixed[@]} -gt 0 ]; then
  for path in "${mixed[@]}"; do
    echo "WARNING: step-15 kept version files staged: $path mixes a version change with other edits" >&2
  done
  exit 0
fi
if [ "$requested" = true ]; then why="allow_version_bump=$(clean "${allow:-false}")"; else why="no version bump requested by step 14"; fi

# Save the churn before touching the working tree: mktemp + rename, mode 0600,
# so a planted symlink at the patch path is replaced, never followed.
# Not --path-format=absolute: that resolves symlinks, so it would name the target.
patch="$(git rev-parse --git-path amplihack/excluded-version-churn.patch)" || rollback "cannot locate the patch path"
case "$patch" in /*) ;; *) patch="$top/$patch" ;; esac
patch_dir="$(dirname "$patch")"
mkdir -p "$patch_dir" 2>/dev/null || rollback "cannot create $patch_dir"
tmp_patch="$(umask 077 && mktemp "$patch_dir/.excluded-version-churn.XXXXXX" 2>/dev/null)" || rollback "cannot create a temporary patch in $patch_dir"
if ! git diff --cached --binary --no-ext-diff --no-textconv --no-color -- "${churn[@]}" >"$tmp_patch"; then
  rm -f -- "$tmp_patch"
  rollback "cannot write the version churn patch"
fi
if [ ! -s "$tmp_patch" ] || ! git apply --check -R --cached "$tmp_patch" 2>/dev/null; then
  rm -f -- "$tmp_patch"
  rollback "the version churn patch is empty or does not match the index"
fi
[ -L "$patch" ] && { rm -f -- "$patch" || rollback "cannot replace the symlink at $patch"; }
[ -d "$patch" ] && { rm -f -- "$tmp_patch"; rollback "$patch is a directory"; }
mv -fT -- "$tmp_patch" "$patch" || { rm -f -- "$tmp_patch"; rollback "cannot move the patch to $patch"; }
git restore --staged --worktree -- "${churn[@]}" || rollback "cannot restore ${churn[*]}"
for path in "${churn[@]}"; do
  echo "WARNING: step-15 excluded version-only change in $path ($why); saved to $patch" >&2
done
exit 0
