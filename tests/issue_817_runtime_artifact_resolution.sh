#!/usr/bin/env bash
# Behavior reproduction for issue #817 used by the QA scenario
# (tests/scenarios/issue-817-runtime-artifact-resolution.yaml).
#
# It extracts the *real* RUNTIME_ARTIFACT_HELPER resolution sites from each
# lifecycle recipe and executes EACH site independently in a controlled
# environment, asserting:
#   1. For every resolution site in every lifecycle recipe, with AMPLIHACK_HOME
#      unset and a target repo that has no amplifier-bundle/, the helper resolves
#      from the installed ~/.amplihack bundle.
#   2. An explicit AMPLIHACK_HOME bundle takes precedence over the fallback.
#   3. The ~/.copilot bundle takes precedence over the ~/.amplihack bundle
#      (the documented order: ... -> ~/.copilot -> ~/.amplihack).
#   4. For every resolution site, when no candidate exists anywhere, the site's
#      guard fails loudly with exit 2 instead of sourcing a missing path.
#
# Each site is tested on its own (not concatenated), so a broken site cannot be
# masked by a later one. Snippets run under the same `set -euo pipefail` the
# recipes use, so a chain that aborts under production semantics is caught.
#
# This mirrors crates/amplihack-cli/tests/issue_817_runtime_artifact_resolution.rs
# but runs instantly (no cargo build) so it is usable from the agentic-test
# QA harness.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPES_DIR="$REPO_ROOT/amplifier-bundle/recipes"

# recipe:expected-number-of-resolution-sites
RECIPE_SITES=(
  "workflow-tdd.yaml:1"
  "workflow-finalize.yaml:3"
  "workflow-publish.yaml:2"
  "workflow-refactor-review.yaml:1"
  "workflow-pr-review.yaml:1"
)

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SITES_DIR="$WORK/sites"
mkdir -p "$SITES_DIR"

# Split a recipe into its contiguous RUNTIME_ARTIFACT_HELPER resolution sites.
# A site starts at a bare `RUNTIME_ARTIFACT_HELPER="..."` assignment and runs
# through the consecutive `[ -f "$RUNTIME_ARTIFACT_HELPER" ] ...` fallback and
# guard lines. Each site (chain + guard) is written to "$SITES_DIR/<recipe>.<n>".
# Prints the number of sites found.
extract_sites() {
  local recipe="$1"
  awk -v out="$SITES_DIR/$recipe" '
    function trim(s){ sub(/^[ \t]+/,"",s); return s }
    function finish(){ if (blk != "") { n++; print blk > (out "." n) ; close(out "." n) } ; blk=""; inblk=0 }
    {
      t=trim($0)
      if (t ~ /^RUNTIME_ARTIFACT_HELPER="/) { finish(); blk=t; inblk=1; next }
      if (inblk && t ~ /^\[ -f "\$RUNTIME_ARTIFACT_HELPER" \]/) { blk=blk "\n" t; next }
      if (inblk) finish()
    }
    END { finish(); print n+0 }
  ' "$RECIPES_DIR/$recipe"
}

# Run a resolution snippet under production shell semantics and print the
# resolved RUNTIME_ARTIFACT_HELPER. Args: snippet repo home [AMPLIHACK_HOME]
# stderr is suppressed (benign git/probe noise from a throwaway non-git dir).
run_snippet() {
  local snippet="$1" repo="$2" home="$3" ahome="${4:-}"
  local script
  script=$(printf 'set -euo pipefail\n%s\nprintf "%%s" "$RUNTIME_ARTIFACT_HELPER"\n' "$snippet")
  if [ -n "$ahome" ]; then
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER AMPLIHACK_HOME="$ahome" REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script" 2>/dev/null
  else
    env -u WORKFLOW_RUNTIME_ARTIFACT_HELPER -u AMPLIHACK_HOME REPO_PATH="$repo" HOME="$home" \
      bash -c "cd '$repo' && $script" 2>/dev/null
  fi
}

total_sites=0
SITE_FILES=()

for entry in "${RECIPE_SITES[@]}"; do
  recipe="${entry%%:*}"
  expected="${entry##*:}"
  found="$(extract_sites "$recipe")"
  if [ "$found" -ne "$expected" ]; then
    echo "FAIL: $recipe expected $expected resolution site(s) but found $found" >&2
    exit 1
  fi
  total_sites=$((total_sites + found))

  for n in $(seq 1 "$found"); do
    site_file="$SITES_DIR/$recipe.$n"
    SITE_FILES+=("$site_file")
    site="$(cat "$site_file")"
    site_label="$recipe site $n"
    case "$site" in
      *".amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"*) : ;;
      *) echo "FAIL: $recipe site $n missing ~/.amplihack fallback candidate" >&2; exit 1 ;;
    esac

    # Case 1: site falls back to the installed ~/.amplihack bundle.
    repo="$WORK/$recipe.$n-repo"          # deliberately has no amplifier-bundle/
    home="$WORK/$recipe.$n-home"
    installed="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
    mkdir -p "$repo" "$(dirname "$installed")"
    printf '#!/usr/bin/env bash\npreflight_known_workflow_runtime_artifacts() { :; }\n' > "$installed"
    resolved="$(run_snippet "$site" "$repo" "$home")"
    if [ "$resolved" != "$installed" ]; then
      echo "FAIL: $recipe site $n expected fallback to '$installed' but resolved '$resolved'" >&2
      exit 1
    fi
    [ -f "$resolved" ] || { echo "FAIL: $recipe site $n resolved helper is not a file: $resolved" >&2; exit 1; }

    # Case 4: no candidate anywhere -> this site's guard exits 2.
    frepo="$WORK/$recipe.$n-frepo"   # no bundle
    fhome="$WORK/$recipe.$n-fhome"   # no ~/.copilot or ~/.amplihack bundle
    mkdir -p "$frepo" "$fhome"
    set +e
    run_snippet "$site" "$frepo" "$fhome" >/dev/null 2>&1
    rc=$?
    set -e
    if [ "$rc" -ne 2 ]; then
      echo "FAIL: $recipe site $n must exit 2 when no helper exists, got $rc" >&2
      exit 1
    fi
  done
done

# --- Case 2 + Case 3: precedence ordering, asserted for EVERY resolution site ---
# Case 2: an explicit AMPLIHACK_HOME bundle wins over the installed fallback.
# Case 3: the ~/.copilot bundle wins over the ~/.amplihack bundle.
# Running these against every site (not just tdd) catches a candidate reordering
# in any single recipe, e.g. ~/.amplihack placed before ~/.copilot.
i=0
for site_file in "${SITE_FILES[@]}"; do
  i=$((i + 1))
  site="$(cat "$site_file")"

  # Case 2
  repo="$WORK/case2-$i-repo"
  home="$WORK/case2-$i-home"
  explicit_home="$WORK/case2-$i-explicit"
  explicit="$explicit_home/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
  installed="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
  mkdir -p "$repo" "$(dirname "$explicit")" "$(dirname "$installed")"
  printf '# explicit\n' > "$explicit"
  printf '# installed\n' > "$installed"
  resolved="$(run_snippet "$site" "$repo" "$home" "$explicit_home")"
  if [ "$resolved" != "$explicit" ]; then
    echo "FAIL: $site_file: explicit AMPLIHACK_HOME must win; expected '$explicit' but resolved '$resolved'" >&2
    exit 1
  fi

  # Case 3
  repo="$WORK/case3-$i-repo"
  home="$WORK/case3-$i-home"
  copilot="$home/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
  amplihack="$home/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
  mkdir -p "$repo" "$(dirname "$copilot")" "$(dirname "$amplihack")"
  printf '# copilot\n' > "$copilot"
  printf '# amplihack\n' > "$amplihack"
  resolved="$(run_snippet "$site" "$repo" "$home")"
  if [ "$resolved" != "$copilot" ]; then
    echo "FAIL: $site_file: ~/.copilot must win over ~/.amplihack; expected '$copilot' but resolved '$resolved'" >&2
    exit 1
  fi
done

echo "PASS: issue-817 runtime artifact resolution behaves correctly across $total_sites resolution site(s) in all lifecycle recipes"
