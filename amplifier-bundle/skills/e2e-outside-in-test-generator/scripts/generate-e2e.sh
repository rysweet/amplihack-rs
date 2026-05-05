#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: generate-e2e.sh [plan|generate] [--type web|cli|api|tui|mcp] [--output DIR] [--force]

Creates starter outside-in E2E scenarios using repository-native conventions.
EOF
}

mode="plan"
app_type=""
output_dir="e2e"
force=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        plan|generate) mode="$1"; shift ;;
        --type) app_type="${2:-}"; shift 2 ;;
        --output) output_dir="${2:-}"; shift 2 ;;
        --force) force=1; shift ;;
        -h|--help|help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

detect_type() {
    if [[ -f package.json ]] && rg -q 'playwright|@playwright/test|vite|next|react|vue|svelte' package.json 2>/dev/null; then
        echo web
    elif [[ -f openapi.yaml || -f openapi.yml || -f openapi.json ]]; then
        echo api
    elif [[ -f Cargo.toml || -f package.json || -f go.mod || -f Makefile ]] && { [[ -f src/main.rs ]] || rg -q '\"bin\"|\\[\\[bin\\]\\]|func main\\(' . 2>/dev/null; }; then
        echo cli
    elif find . -maxdepth 3 -type f \( -name '*mcp*.yaml' -o -name '*mcp*.yml' \) -print -quit 2>/dev/null | grep -q .; then
        echo mcp
    else
        echo cli
    fi
}

project_name() {
    basename "$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
}

write_file() {
    local file="$1"
    if [[ -e "$file" && "$force" != 1 ]]; then
        echo "Refusing to overwrite $file (use --force)" >&2
        exit 1
    fi
    mkdir -p "$(dirname "$file")"
    cat > "$file"
    echo "$file"
}

emit_plan() {
    local type="$1"
    cat <<EOF
outside-in e2e plan:
  app: $(project_name)
  detected_type: $type
  output_dir: $output_dir
  journeys:
    - smoke startup/help or first page load
    - primary happy path
    - validation/error path
  validation:
    - run the repository's existing e2e/test command
    - fix harness failures caused by generated tests
EOF
}

generate_cli() {
    local file="$output_dir/cli-smoke.yaml"
    write_file "$file" <<EOF
# Auto-generated outside-in CLI smoke scenario for $(project_name)
scenario:
  name: "CLI smoke"
  type: cli
  steps:
    - action: run
      command: "./$(project_name) --help"
      expected_exit: 0
    - action: verify_output
      contains_any: ["Usage", "USAGE", "help", "--help"]
EOF
}

generate_web() {
    local file="$output_dir/smoke.spec.ts"
    write_file "$file" <<'EOF'
import { test, expect } from '@playwright/test';

test.describe('outside-in smoke', () => {
  test('loads the home page without console errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('console', msg => {
      if (msg.type() === 'error') errors.push(msg.text());
    });

    await page.goto('/');
    await page.waitForLoadState('domcontentloaded');

    await expect(page.locator('body')).toBeVisible();
    expect(errors).toEqual([]);
  });
});
EOF
}

generate_api() {
    local file="$output_dir/api-smoke.yaml"
    write_file "$file" <<EOF
# Auto-generated outside-in API smoke scenario for $(project_name)
scenario:
  name: "API smoke"
  type: api
  steps:
    - action: request
      method: GET
      path: /health
      expected_status_any: [200, 204, 404]
    - action: document
      note: "Replace /health with the repository's real health or root endpoint."
EOF
}

generate_tui() {
    local file="$output_dir/tui-smoke.yaml"
    write_file "$file" <<EOF
# Auto-generated outside-in TUI smoke scenario for $(project_name)
scenario:
  name: "TUI smoke"
  type: tui
  steps:
    - action: launch
      command: "./$(project_name)"
    - action: send_keys
      keys: ["q"]
    - action: verify_exit
      expected_exit: 0
EOF
}

generate_mcp() {
    local file="$output_dir/mcp-smoke.yaml"
    write_file "$file" <<EOF
# Auto-generated outside-in MCP smoke scenario for $(project_name)
scenario:
  name: "MCP smoke"
  type: mcp
  steps:
    - action: initialize
    - action: list_tools
    - action: assert
      condition: "tools response is valid JSON and contains an array"
EOF
}

app_type="${app_type:-$(detect_type)}"

case "$mode:$app_type" in
    plan:*) emit_plan "$app_type" ;;
    generate:cli) generate_cli ;;
    generate:web) generate_web ;;
    generate:api) generate_api ;;
    generate:tui) generate_tui ;;
    generate:mcp) generate_mcp ;;
    *) echo "Unsupported app type: $app_type" >&2; exit 2 ;;
esac
