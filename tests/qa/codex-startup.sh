#!/usr/bin/env bash
# Outside-in startup/install acceptance checks; all clients and HOME are fixtures.
set -euo pipefail
: "${AMPLIHACK_QA_BINARY:?set AMPLIHACK_QA_BINARY to the built candidate binary}"
candidate=$(realpath "$AMPLIHACK_QA_BINARY")
source_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
scenario=${1:?expected claude, copilot, codex, or install}
case "$scenario" in claude|copilot|codex|install) ;; *) exit 2;; esac
fixture=$(mktemp -d "${TMPDIR:-/tmp}/amplihack-qa.XXXXXX")
trap 'rm -rf -- "$fixture"' EXIT
fixture_home="$fixture/home"
tools="$fixture/tools"
mkdir -p "$tools" "$fixture_home"/{.codex,.amplihack/codex,.cargo,.config/gh,.cache,.local/share,.local/state,.claude,claude-data,.copilot,tmp}
printf '{"schema_version":1,"codex_home":"%s/.codex","package_digest":"legacy-unframed-digest","hooks":{}}\n' "$fixture_home" > "$fixture_home/.amplihack/codex/ownership.json"
printf 'foreign package bytes\n' > "$fixture_home/.amplihack/codex/foreign.txt"
printf 'model = "user-owned"\n' > "$fixture_home/.codex/config.toml"
printf '{"foreign":true}\n' > "$fixture_home/.codex/hooks.json"
for provider in claude copilot; do
    cat > "$tools/$provider" <<'CLIENT'
#!/bin/sh
provider=${0##*/}
case "$*" in
    *--version*) if [ "$provider" = claude ]; then echo '2.1.0 (Claude Code)'; else echo '0.0.400'; fi;;
    *plugin*) echo "$provider:plugin" >> "$HOME/events"; echo '{}';;
    *) echo "$provider:launch" >> "$HOME/events";;
esac
CLIENT
done
cat > "$tools/codex" <<'CLIENT'
#!/bin/sh
case "$*" in
    *--version*) echo 'codex-cli 0.160.0';;
    plugin*) echo 'codex:plugin' >> "$HOME/events"; echo '{"installed":[]}';;
    *) echo 'codex:launch' >> "$HOME/events";;
esac
CLIENT
printf '#!/bin/sh\necho v24.1.0\n' > "$tools/node"
printf '#!/bin/sh\necho "{}"\n' > "$tools/amplihack-hooks"
printf '#!/bin/sh\necho '\''{"schema_version":1,"version":"fixture","capabilities":["codex_exec"]}'\''\n' > "$tools/recipe-runner-rs"
for tool in "$tools"/*; do
    # Installer binary-size validation should accept these observable fixtures.
    printf '# %1200s\n' '' >> "$tool"
    chmod 755 "$tool"
done
snapshot() {
    tar --sort=name -cf "$1" -C "$fixture_home" .codex .amplihack/codex
}
snapshot "$fixture/before.tar"
inherited=claude
args=("$scenario" --no-reflection --subprocess-safe)
if [ "$scenario" = claude ] || [ "$scenario" = copilot ]; then inherited=codex; fi
if [ "$scenario" = install ]; then args=(install --local "$source_root"); fi
status=0
env -i HOME="$fixture_home" CODEX_HOME="$fixture_home/.codex" \
    PATH="$tools:/usr/bin:/bin" AMPLIHACK_HOME="$source_root" \
    AMPLIHACK_AGENT_BINARY="$inherited" AMPLIHACK_NONINTERACTIVE=1 \
    AMPLIHACK_NO_UPDATE_CHECK=1 AMPLIHACK_NO_FRESHNESS_CHECK=1 IS_SANDBOX=1 \
    AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH="$tools/amplihack-hooks" \
    AMPLIHACK_CODEX_BINARY_PATH="$tools/codex" RECIPE_RUNNER_RS_PATH="$tools/recipe-runner-rs" \
    CARGO_HOME="$fixture_home/.cargo" XDG_CONFIG_HOME="$fixture_home/.config" \
    XDG_CACHE_HOME="$fixture_home/.cache" XDG_DATA_HOME="$fixture_home/.local/share" \
    XDG_STATE_HOME="$fixture_home/.local/state" CLAUDE_CONFIG_DIR="$fixture_home/.claude" \
    CLAUDE_PLUGIN_DATA="$fixture_home/claude-data" COPILOT_HOME="$fixture_home/.copilot" \
    COPILOT_CONFIG_DIR="$fixture_home/.copilot" GH_CONFIG_DIR="$fixture_home/.config/gh" \
    TMPDIR="$fixture_home/tmp" "$candidate" "${args[@]}" > "$fixture/output" 2>&1 || status=$?
snapshot "$fixture/after.tar"
if ! cmp -s "$fixture/before.tar" "$fixture/after.tar"; then
    cat "$fixture/output"
    echo 'FAIL: protected Codex state changed' >&2
    exit 1
fi
if [ "$scenario" = claude ] || [ "$scenario" = copilot ]; then
    test "$status" -eq 0 || { cat "$fixture/output"; exit 1; }
    rg -qx "$scenario:launch" "$fixture_home/events"
    ! rg -q '^codex:' "$fixture_home/events"
    test -f "$fixture_home/.amplihack/.installed-version"
    test -f "$fixture_home/.amplihack/amplifier-bundle/recipes/workflow-tdd.yaml"
else
    test "$status" -ne 0
    rg -q 'legacy or changed Codex ownership proof' "$fixture/output"
    rg -qx 'codex:plugin' "$fixture_home/events"
    ! rg -q '^codex:launch$' "$fixture_home/events"
fi
printf 'PASS: %s preserves foreign Codex state and respects selected provider\n' "$scenario"
