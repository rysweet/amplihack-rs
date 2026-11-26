# qa-team: reference

## Advanced Patterns [LEVEL 2]

### Conditional Logic

Execute steps based on conditions:

```yaml
steps:
  - action: launch
    target: "./app"

  - action: verify_output
    contains: "Login required"
    id: login_check

  # Only run if login_check passed
  - action: send_input
    value: "login admin password123\n"
    condition: login_check.passed
```

### Variables and Templating [LEVEL 2]

Define variables and use them throughout the scenario:

```yaml
scenario:
  name: "Test with Variables"
  type: cli

  variables:
    username: "testuser"
    api_url: "http://localhost:8080"

  steps:
    - action: launch
      target: "./app"
      args: ["--api", "${api_url}"]

    - action: send_input
      value: "login ${username}\n"

    - action: verify_output
      contains: "Welcome, ${username}!"
```

### Loops and Repetition [LEVEL 2]

Repeat actions multiple times:

```yaml
steps:
  - action: launch
    target: "./app"

  # Repeat action N times
  - action: send_keypress
    value: "down"
    times: 5

  # Loop over list
  - action: send_input
    value: "${item}\n"
    for_each:
      - "apple"
      - "banana"
      - "cherry"
```

### Error Handling [LEVEL 2]

Handle expected errors gracefully:

```yaml
steps:
  - action: send_input
    value: "invalid command\n"

  # Verify error message appears
  - action: verify_output
    contains: "Error: Unknown command"
    expected_failure: true

  # App should still be running
  - action: verify_running
    expected: true
```

### Multi-Step Workflows [LEVEL 2]

Complex scenarios with multiple phases:

```yaml
scenario:
  name: "E-commerce Purchase Flow"
  type: web

  steps:
    # Phase 1: Authentication
    - action: navigate
      url: "http://localhost:3000/login"

    - action: type
      selector: "#username"
      value: "test@example.com"

    - action: type
      selector: "#password"
      value: "password123"

    - action: click
      selector: "button[type=submit]"

    - action: wait_for_url
      contains: "/dashboard"

    # Phase 2: Product Selection
    - action: navigate
      url: "http://localhost:3000/products"

    - action: click
      text: "Add to Cart"
      nth: 1

    - action: verify_element
      selector: ".cart-badge"
      contains: "1"

    # Phase 3: Checkout
    - action: click
      selector: ".cart-icon"

    - action: click
      text: "Proceed to Checkout"

    - action: fill_form
      fields:
        "#shipping-address": "123 Test St"
        "#city": "Testville"
        "#zip": "12345"

    - action: click
      selector: "#place-order"

    - action: wait_for_element
      selector: ".order-confirmation"
      timeout: 10s

    - action: verify_element
      selector: ".order-number"
      exists: true
```

## Level 3: Advanced Topics [LEVEL 3]

### Custom Comprehension Agents

The framework uses AI agents to interpret application output and determine if tests pass. You can customize these agents for domain-specific logic.

**Default Comprehension Agent**:

- Observes raw output (text, HTML, screenshots)
- Applies general reasoning to verify expectations
- Returns pass/fail with explanation

**Custom Comprehension Agent** (see `examples/custom-agents/custom-comprehension-agent.yaml`):

```yaml
scenario:
  name: "Financial Dashboard Test with Custom Agent"
  type: web

  # Define custom comprehension logic
  comprehension_agent:
    # Inherit the runtime model configuration.
    system_prompt: |
      You are a financial data validator. When verifying dashboard content:
      1. All monetary values must use proper formatting ($1,234.56)
      2. Percentages must include % symbol
      3. Dates must be in MM/DD/YYYY format
      4. Negative values must be red
      5. Chart data must be logically consistent

      Be strict about formatting and data consistency.

    examples:
      - input: "Total Revenue: 45000"
        output: "FAIL - Missing currency symbol and comma separator"
      - input: "Total Revenue: $45,000.00"
        output: "PASS - Correctly formatted"

  steps:
    - action: navigate
      url: "http://localhost:3000/financial-dashboard"

    - action: verify_element
      selector: ".revenue-widget"
      use_custom_comprehension: true
      description: "Revenue should be properly formatted"
```

### Visual Regression Testing [LEVEL 3]

Compare screenshots against baseline images:

```yaml
scenario:
  name: "Visual Regression - Homepage"
  type: web

  steps:
    - action: navigate
      url: "http://localhost:3000"

    - action: wait_for_element
      selector: ".page-loaded"

    - action: screenshot
      save_as: "homepage.png"

    - action: visual_compare
      screenshot: "homepage.png"
      baseline: "./baselines/homepage-baseline.png"
      threshold: 0.05 # 5% difference allowed
      highlight_differences: true
```

### Performance Validation [LEVEL 3]

Measure and validate performance metrics:

```yaml
scenario:
  name: "Performance - Dashboard Load Time"
  type: web

  performance:
    metrics:
      - page_load_time
      - first_contentful_paint
      - time_to_interactive

  steps:
    - action: navigate
      url: "http://localhost:3000/dashboard"
      measure_timing: true

    - action: verify_performance
      metric: page_load_time
      less_than: 3000 # 3 seconds

    - action: verify_performance
      metric: first_contentful_paint
      less_than: 1500 # 1.5 seconds
```

### Multi-Window Coordination (Electron) [LEVEL 3]

Test applications with multiple windows:

```yaml
scenario:
  name: "Multi-Window Chat Application"
  type: electron

  steps:
    - action: launch
      target: "./chat-app"

    - action: menu_click
      path: ["Window", "New Chat"]

    - action: verify_window
      count: 2

    - action: window_action
      window: 1
      action: focus

    - action: type
      selector: ".message-input"
      value: "Hello from window 1"

    - action: click
      selector: ".send-button"

    - action: window_action
      window: 2
      action: focus

    - action: wait_for_element
      selector: ".message"
      contains: "Hello from window 1"
      timeout: 5s
```

### IPC Testing (Electron) [LEVEL 3]

Test Inter-Process Communication between renderer and main:

```yaml
scenario:
  name: "Electron IPC Communication"
  type: electron

  steps:
    - action: launch
      target: "./my-app"

    - action: ipc_send
      channel: "get-system-info"

    - action: ipc_expect
      channel: "system-info-reply"
      timeout: 3s

    - action: verify_ipc_payload
      contains:
        platform: "darwin"
        arch: "x64"
```

### Custom Reporters [LEVEL 3]

Generate custom test reports:

```yaml
scenario:
  name: "Test with Custom Reporting"
  type: cli

  reporting:
    format: custom
    template: "./report-template.html"
    include:
      - screenshots
      - logs
      - timing_data
      - video_recording

    email:
      enabled: true
      recipients: ["team@example.com"]
      on_failure_only: true

  steps:
    # ... test steps ...
```

## Framework Freshness Check [LEVEL 3]

This skill embeds knowledge of gadugi-agentic-test version 0.1.0. To check if a newer version exists:

```bash
# Check the latest published framework version
gh release view --repo rysweet/gadugi-agentic-test --json tagName,publishedAt

# Output if outdated:
# WARNING: Embedded framework version is 0.1.0
# Latest GitHub version is 0.2.5
#
# New features in 0.2.5:
# - Native Playwright support for web testing
# - Video recording for all test types
# - Parallel test execution
#
# Update with: npm update -g @gadugi/agentic-test
```

The script checks the GitHub repository for releases and compares against the embedded version. This ensures you're aware of new features and improvements.

**When to Update This Skill**:

- New framework version adds significant features
- Breaking changes in YAML schema
- New application types supported
- Agent capabilities expand

## Reference: Action Catalog [LEVEL 3]

### CLI Actions

| Action             | Parameters                       | Description                            |
| ------------------ | -------------------------------- | -------------------------------------- |
| `launch`           | `target`, `args`, `cwd`, `env`   | Start CLI application                  |
| `send_input`       | `value`, `delay`                 | Send text to stdin                     |
| `send_signal`      | `signal`                         | Send OS signal (SIGINT, SIGTERM, etc.) |
| `wait_for_output`  | `contains`, `matches`, `timeout` | Wait for text in stdout/stderr         |
| `verify_output`    | `contains`, `matches`, `stream`  | Check output content                   |
| `verify_exit_code` | `expected`                       | Validate exit code                     |
| `capture_output`   | `save_as`, `stream`              | Save output to file                    |

### TUI Actions

| Action               | Parameters                        | Description              |
| -------------------- | --------------------------------- | ------------------------ |
| `launch`             | `target`, `args`, `terminal_size` | Start TUI application    |
| `send_keypress`      | `value`, `times`, `modifiers`     | Send keyboard input      |
| `wait_for_screen`    | `contains`, `timeout`             | Wait for text on screen  |
| `verify_screen`      | `contains`, `matches`, `region`   | Check screen content     |
| `capture_screenshot` | `save_as`                         | Save terminal screenshot |
| `navigate_menu`      | `path`                            | Navigate menu structure  |
| `fill_form`          | `fields`                          | Fill TUI form fields     |

### Web Actions

| Action             | Parameters                                | Description            |
| ------------------ | ----------------------------------------- | ---------------------- |
| `navigate`         | `url`, `wait_for_load`                    | Go to URL              |
| `click`            | `selector`, `text`, `nth`                 | Click element          |
| `type`             | `selector`, `value`, `delay`              | Type into input        |
| `wait_for_element` | `selector`, `timeout`, `disappears`       | Wait for element       |
| `verify_element`   | `selector`, `contains`, `count`, `exists` | Check element state    |
| `verify_url`       | `equals`, `contains`, `matches`           | Validate URL           |
| `screenshot`       | `save_as`, `selector`, `full_page`        | Capture screenshot     |
| `scroll`           | `selector`, `direction`, `amount`         | Scroll page/element    |
| `select_option`    | `selector`, `value`                       | Select dropdown option |
| `checkbox`         | `selector`, `checked`                     | Check/uncheck checkbox |

### Electron Actions

| Action          | Parameters                             | Description                |
| --------------- | -------------------------------------- | -------------------------- |
| `launch`        | `target`, `args`, `wait_for_window`    | Start Electron app         |
| `window_action` | `window`, `action`                     | Interact with windows      |
| `menu_click`    | `path`                                 | Click menu items           |
| `dialog_action` | `type`, `action`, `filename`           | Handle dialogs             |
| `ipc_send`      | `channel`, `data`                      | Send IPC message           |
| `ipc_expect`    | `channel`, `timeout`                   | Wait for IPC message       |
| `verify_window` | `title`, `visible`, `focused`, `count` | Check window state         |
| All web actions |                                        | Electron includes Chromium |

### Common Parameters

| Parameter             | Type       | Description                          |
| --------------------- | ---------- | ------------------------------------ |
| `timeout`             | Duration   | Maximum wait time (e.g., "5s", "2m") |
| `description`         | String     | Human-readable step explanation      |
| `continue_on_failure` | Boolean    | Don't fail scenario if step fails    |
| `id`                  | String     | Step identifier for conditionals     |
| `condition`           | Expression | Execute step only if condition true  |

## Changelog [LEVEL 3]

### Version 1.2.0 (2026-03-11)

- **NEW**: Level 4 - A/B Comparison Harness & Self-Improving Audit
- Generic `ab_comparison_harness.py` for any side-by-side CLI comparison
  (migration parity, A/B testing, feature flags, version regression, canary)
- Generic `ab_audit_cycle.py` self-improvement loop
- `SCENARIO_FORMAT.md` YAML specification
- Supports `--a`/`--b` flags (also `--legacy`/`--candidate` for backward compat)
- Example scenario file
- Extracted from amplihack-rs parity validation (118/118 tests)

### Version 1.1.0 (2026-01-29)

- **NEW**: Level 4 - Shadow Environment Integration
- Added complete shadow testing workflow patterns
- Integration examples for Amplifier native and standalone CLI
- Multi-repo integration test patterns
- Web app testing in shadow environments
- Complete workflow example for library change validation
- References to shadow-testing skill for deep-dive documentation

### Version 1.0.0 (2025-11-16)

- Initial skill release
- Support for CLI, TUI, Web, and Electron applications
- 15 complete working examples
- Progressive disclosure levels (1, 2, 3)
- Embedded gadugi-agentic-test framework documentation (v0.1.0)
- Freshness check script for version monitoring
- Full integration with amplihack philosophy
- Comprehensive troubleshooting guide
- Action reference catalog
