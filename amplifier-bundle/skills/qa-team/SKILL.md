---
name: qa-team
description: |
  QA team for outside-in validation, side-by-side parity loops, and A/B behavioral comparison.
  Use when you need behavior-driven tests, legacy-vs-new comparison, or rollout shadow validation.
  Creates executable scenarios and parity workflows that agents can observe, compare, and iterate on.
  Supports local, observable tmux, remote SSH, and shadow-mode divergence logging patterns.
version: 1.1.0
embedded_framework_version: 0.1.0
github_repo: https://github.com/rysweet/gadugi-agentic-test
issue: https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding/issues/1356
---

# QA Team Skill

## Repo-Type Detection

Before running test commands, detect the repository type and use the appropriate tooling:

| Indicator | Repo Type | Test Command | Parity Harness |
|---|---|---|---|
| `Cargo.toml` at root | **Rust CLI** | `cargo test` | Native Rust tests with `tests/parity/scenarios/*.yaml` fixtures |
| `package.json` at root | **Node/Electron** | `gadugi-test run <scenario>.yaml` | gadugi-agentic-test framework |
| `setup.py` / `pyproject.toml` | **Python** | `pytest` or `gadugi-test run` | gadugi-agentic-test framework |

**For Rust CLI repos** (e.g. amplihack-rs): substitute `cargo test` for all `gadugi-test run` invocations below, and use `tests/parity/scenarios/*.yaml` for cross-cutting CLI parity validation. Do **not** require the gadugi-agentic-test framework to be installed.

## Purpose [LEVEL 1]

This skill helps you create **agentic outside-in tests** that verify application behavior from an external user's perspective without any knowledge of internal implementation. Using the gadugi-agentic-test framework (or `cargo test` for Rust CLI repos), you write declarative YAML scenarios that AI agents execute, observe, and validate.

**Key Principle**: Tests describe WHAT should happen, not HOW it's implemented. Agents figure out the execution details.

## When to Use This Skill [LEVEL 1]

### Perfect For

- **Smoke Tests**: Quick validation that critical user flows work
- **Behavior-Driven Testing**: Verify features from user perspective
- **Cross-Platform Testing**: Same test logic for CLI, TUI, Web, Electron
- **Refactoring Safety**: Tests remain valid when implementation changes
- **AI-Powered Testing**: Let agents handle complex interactions
- **Documentation as Tests**: YAML scenarios double as executable specs

### Use This Skill When

- Starting a new project and defining expected behaviors
- Refactoring code and need tests that won't break with internal changes
- Testing user-facing applications (CLI tools, TUIs, web apps, desktop apps)
- Writing acceptance criteria that can be automatically verified
- Need tests that non-developers can read and understand
- Want to catch regressions in critical user workflows
- Testing complex multi-step interactions

### Don't Use This Skill When

- Need unit tests for internal functions (use test-gap-analyzer instead)
- Testing performance or load characteristics
- Need precise timing or concurrency control
- Testing non-interactive batch processes
- Implementation details matter more than behavior

## Core Concepts [LEVEL 1]

### Outside-In Testing Philosophy

**Traditional Inside-Out Testing**:

```python
# Tightly coupled to implementation
def test_calculator_add():
    calc = Calculator()
    result = calc.add(2, 3)
    assert result == 5
    assert calc.history == [(2, 3, 5)]  # Knows internal state
```

**Agentic Outside-In Testing**:

```yaml
# Implementation-agnostic behavior verification
scenario:
  name: "Calculator Addition"
  steps:
    - action: launch
      target: "./calculator"
    - action: send_input
      value: "add 2 3"
    - action: verify_output
      contains: "Result: 5"
```

**Benefits**:

- Tests survive refactoring (internal changes don't break tests)
- Readable by non-developers (YAML is declarative)
- Platform-agnostic (same structure for CLI/TUI/Web/Electron)
- AI agents handle complexity (navigation, timing, screenshots)

### The Gadugi Agentic Test Framework [LEVEL 2]

Gadugi-agentic-test is a Python framework that:

1. **Parses YAML test scenarios** with declarative steps
2. **Dispatches to specialized agents** (CLI, TUI, Web, Electron agents)
3. **Executes actions** (launch, input, click, wait, verify)
4. **Collects evidence** (screenshots, logs, output captures)
5. **Validates outcomes** against expected results
6. **Generates reports** with evidence trails

**Architecture**:

```
YAML Scenario → Scenario Loader → Agent Dispatcher → Execution Engine
                                          ↓
                     [CLI Agent, TUI Agent, Web Agent, Electron Agent]
                                          ↓
                           Observers → Comprehension Agent
                                          ↓
                                   Evidence Report
```

### Progressive Disclosure Levels [LEVEL 1]

This skill teaches testing in four levels:

- **Level 1: Fundamentals** - Basic single-action tests, simple verification
- **Level 2: Intermediate** - Multi-step flows, conditional logic, error handling
- **Level 3: Advanced** - Custom agents, visual regression, performance validation
- **Level 4: Parity & Shadowing** - Side-by-side A/B comparison, remote observable runs, rollout divergence logging

Each example is marked with its level. Start at Level 1 and progress as needed.

## Navigation Guide

Before you do any of the work in this table, read the named file from this skill's folder. The rule of Repo-Type Detection applies in those files too.

| File                   | Read when                                                                          |
| ---------------------- | ---------------------------------------------------------------------------------- |
| `reference.md`         | You need the full list of actions, advanced patterns or custom agents.             |
| `practice.md`          | You run scenarios and need where evidence is written, pitfalls or troubleshooting. |
| `parity-and-shadow.md` | You need side-by-side parity, A/B comparison, shadow mode, tmux or SSH.            |

## Quick Start [LEVEL 1]

### Installation

**Prerequisites (for native module compilation):**

```bash
# macOS
xcode-select --install

# Ubuntu/Debian
sudo apt-get install -y build-essential

# Windows: Install Visual Studio Build Tools with "Desktop development with C++"
```

**Install the framework:**

The gadugi-agentic-test framework is not published to npm. Install it from GitHub:

```bash
# Clone the repository
git clone https://github.com/rysweet/gadugi-agentic-test.git
cd gadugi-agentic-test

# Install dependencies and build
npm install
npm run build

# Verify the build succeeded
node dist/cli.js --version
```

> **Tip**: If you want CLI-style access from anywhere, you can add an alias:
>
> ```bash
> alias gadugi-test="node /path/to/gadugi-agentic-test/dist/cli.js"
> ```

### Your First Test (CLI Example)

Create `test-hello.yaml`:

```yaml
scenario:
  name: "Hello World CLI Test"
  description: "Verify CLI prints greeting"
  type: cli

  prerequisites:
    - "./hello-world executable exists"

  steps:
    - action: launch
      target: "./hello-world"

    - action: verify_output
      contains: "Hello, World!"

    - action: verify_exit_code
      expected: 0
```

Run the test:

```bash
node dist/cli.js run -s test-hello -d ./
```

Output:

```
✓ Scenario: Hello World CLI Test
  ✓ Step 1: Launched ./hello-world
  ✓ Step 2: Output contains "Hello, World!"
  ✓ Step 3: Exit code is 0

PASSED (3/3 steps successful)
Evidence saved to: ./evidence/test-hello-20250116-093045/
```

### Understanding the YAML Structure [LEVEL 1]

Every test scenario has this structure:

```yaml
scenario:
  name: "Descriptive test name"
  description: "What this test verifies"
  type: cli | tui | web | electron

  # Optional metadata
  tags: [smoke, critical, auth]
  timeout: 30s

  # What must be true before test runs
  prerequisites:
    - "Condition 1"
    - "Condition 2"

  # The test steps (executed sequentially)
  steps:
    - action: action_name
      parameter1: value1
      parameter2: value2

    - action: verify_something
      expected: value

  # Optional cleanup
  cleanup:
    - action: stop_application
```

## Application Types and Agents [LEVEL 2]

### CLI Applications [LEVEL 1]

**Use Case**: Command-line tools, scripts, build tools, package managers

**Supported Actions**:

- `launch` - Start the CLI program
- `send_input` - Send text or commands via stdin
- `send_signal` - Send OS signals (SIGINT, SIGTERM)
- `wait_for_output` - Wait for specific text in stdout/stderr
- `verify_output` - Check stdout/stderr contains/matches expected text
- `verify_exit_code` - Validate process exit code
- `capture_output` - Save output for later verification

**Example** (see `examples/cli/calculator-basic.yaml`):

```yaml
scenario:
  name: "CLI Calculator Basic Operations"
  type: cli

  steps:
    - action: launch
      target: "./calculator"
      args: ["--mode", "interactive"]

    - action: send_input
      value: "add 5 3\n"

    - action: verify_output
      contains: "Result: 8"
      timeout: 2s

    - action: send_input
      value: "multiply 4 7\n"

    - action: verify_output
      contains: "Result: 28"

    - action: send_input
      value: "exit\n"

    - action: verify_exit_code
      expected: 0
```

### TUI Applications [LEVEL 1]

**Use Case**: Terminal user interfaces (htop, vim, tmux, custom dashboard TUIs)

**Supported Actions**:

- `launch` - Start TUI application
- `send_keypress` - Send keyboard input (arrow keys, enter, ctrl+c, etc.)
- `wait_for_screen` - Wait for specific text to appear on screen
- `verify_screen` - Check screen contents match expectations
- `capture_screenshot` - Save terminal screenshot (ANSI art)
- `navigate_menu` - Navigate menu structures
- `fill_form` - Fill TUI form fields

**Example** (see `examples/tui/file-manager-navigation.yaml`):

```yaml
scenario:
  name: "TUI File Manager Navigation"
  type: tui

  steps:
    - action: launch
      target: "./file-manager"

    - action: wait_for_screen
      contains: "File Manager v1.0"
      timeout: 3s

    - action: send_keypress
      value: "down"
      times: 3

    - action: verify_screen
      contains: "> documents/"
      description: "Third item should be selected"

    - action: send_keypress
      value: "enter"

    - action: wait_for_screen
      contains: "documents/"
      timeout: 2s

    - action: capture_screenshot
      save_as: "documents-view.txt"
```

### Web Applications [LEVEL 1]

**Use Case**: Web apps, dashboards, SPAs, admin panels

**Supported Actions**:

- `navigate` - Go to URL
- `click` - Click element by selector or text
- `type` - Type into input fields
- `wait_for_element` - Wait for element to appear
- `verify_element` - Check element exists/contains text
- `verify_url` - Validate current URL
- `screenshot` - Capture browser screenshot
- `scroll` - Scroll page or element

**Example** (see `examples/web/dashboard-smoke-test.yaml`):

```yaml
scenario:
  name: "Dashboard Smoke Test"
  type: web

  steps:
    - action: navigate
      url: "http://localhost:3000/dashboard"

    - action: wait_for_element
      selector: "h1.dashboard-title"
      timeout: 5s

    - action: verify_element
      selector: "h1.dashboard-title"
      contains: "Analytics Dashboard"

    - action: verify_element
      selector: ".widget-stats"
      count: 4
      description: "Should have 4 stat widgets"

    - action: click
      selector: "button.refresh-data"

    - action: wait_for_element
      selector: ".loading-spinner"
      disappears: true
      timeout: 10s

    - action: screenshot
      save_as: "dashboard-loaded.png"
```

### Electron Applications [LEVEL 2]

**Use Case**: Desktop apps built with Electron (VS Code, Slack, Discord clones)

**Supported Actions**:

- `launch` - Start Electron app
- `window_action` - Interact with windows (focus, minimize, close)
- `menu_click` - Click application menu items
- `dialog_action` - Handle native dialogs (open file, save, confirm)
- `ipc_send` - Send IPC message to main process
- `verify_window` - Check window state/properties
- All web actions (since Electron uses Chromium)

**Example** (see `examples/electron/single-window-basic.yaml`):

```yaml
scenario:
  name: "Electron Single Window Test"
  type: electron

  steps:
    - action: launch
      target: "./dist/my-app"
      wait_for_window: true
      timeout: 10s

    - action: verify_window
      title: "My Application"
      visible: true

    - action: menu_click
      path: ["File", "New Document"]

    - action: wait_for_element
      selector: ".document-editor"

    - action: type
      selector: ".document-editor"
      value: "Hello from test"

    - action: menu_click
      path: ["File", "Save"]

    - action: dialog_action
      type: save_file
      filename: "test-document.txt"

    - action: verify_window
      title_contains: "test-document.txt"
```

## Test Scenario Anatomy [LEVEL 2]

### Metadata Section

```yaml
scenario:
  name: "Clear descriptive name"
  description: "Detailed explanation of what this test verifies"
  type: cli | tui | web | electron

  # Optional fields
  tags: [smoke, regression, auth, payment]
  priority: high | medium | low
  timeout: 60s # Overall scenario timeout
  retry_on_failure: 2 # Retry count

  # Environment requirements
  environment:
    variables:
      API_URL: "http://localhost:8080"
      DEBUG: "true"
    files:
      - "./config.json must exist"
```

### Prerequisites

Prerequisites are conditions that must be true before the test runs. The framework validates these before execution.

```yaml
prerequisites:
  - "./application binary exists"
  - "Port 8080 is available"
  - "Database is running"
  - "User account test@example.com exists"
  - "File ./test-data.json exists"
```

If prerequisites fail, the test is skipped (not failed).

### Steps

Steps execute sequentially. Each step has:

- **action**: Required - the action to perform
- **Parameters**: Action-specific parameters
- **description**: Optional - human-readable explanation
- **timeout**: Optional - step-specific timeout
- **continue_on_failure**: Optional - don't fail scenario if step fails

```yaml
steps:
  # Simple action
  - action: launch
    target: "./app"

  # Action with multiple parameters
  - action: verify_output
    contains: "Success"
    timeout: 5s
    description: "App should print success message"

  # Continue even if this fails
  - action: click
    selector: ".optional-button"
    continue_on_failure: true
```

### Verification Actions [LEVEL 1]

Verification actions check expected outcomes. They fail the test if expectations aren't met.

**Common Verifications**:

```yaml
# CLI: Check output contains text
- action: verify_output
  contains: "Expected text"

# CLI: Check output matches regex
- action: verify_output
  matches: "Result: \\d+"

# CLI: Check exit code
- action: verify_exit_code
  expected: 0

# Web/TUI: Check element exists
- action: verify_element
  selector: ".success-message"

# Web/TUI: Check element contains text
- action: verify_element
  selector: "h1"
  contains: "Welcome"

# Web: Check URL
- action: verify_url
  equals: "http://localhost:3000/dashboard"

# Web: Check element count
- action: verify_element
  selector: ".list-item"
  count: 5

# Electron: Check window state
- action: verify_window
  title: "My App"
  visible: true
  focused: true
```

### Cleanup Section

Cleanup runs after all steps complete (success or failure). Use for teardown actions.

```yaml
cleanup:
  - action: stop_application
    force: true

  - action: delete_file
    path: "./temp-test-data.json"

  - action: reset_database
    connection: "test_db"
```

## Example Library [LEVEL 1]

This skill includes 15 complete working examples organized by application type and complexity level:

### CLI Examples

1. **calculator-basic.yaml** [LEVEL 1] - Simple CLI arithmetic operations
2. **cli-error-handling.yaml** [LEVEL 2] - Error messages and recovery
3. **cli-interactive-session.yaml** [LEVEL 2] - Multi-turn interactive CLI

### TUI Examples

4. **file-manager-navigation.yaml** [LEVEL 1] - Basic TUI keyboard navigation
5. **tui-form-validation.yaml** [LEVEL 2] - Complex form filling and validation
6. **tui-performance-monitoring.yaml** [LEVEL 3] - TUI performance dashboard testing

### Web Examples

7. **dashboard-smoke-test.yaml** [LEVEL 1] - Simple web dashboard verification
8. **web-authentication-flow.yaml** [LEVEL 2] - Multi-step login workflow
9. **web-visual-regression.yaml** [LEVEL 2] - Screenshot-based visual testing

### Electron Examples

10. **single-window-basic.yaml** [LEVEL 1] - Basic Electron window test
11. **multi-window-coordination.yaml** [LEVEL 2] - Multiple window orchestration
12. **electron-menu-testing.yaml** [LEVEL 2] - Application menu interactions
13. **electron-ipc-testing.yaml** [LEVEL 3] - Main/renderer IPC testing

### Custom Agent Examples

14. **custom-comprehension-agent.yaml** [LEVEL 3] - Domain-specific validation logic
15. **custom-reporter-integration.yaml** [LEVEL 3] - Custom test reporting

See `examples/` directory for full example code with inline documentation.

## Quick Reference: YAML Template [LEVEL 1]

```yaml
scenario:
  # Required fields
  name: "Test Name"
  description: "What this test verifies"
  type: cli | tui | web | electron

  # Optional metadata
  tags: [smoke, critical]
  timeout: 60s

  # What must be true before running
  prerequisites:
    - "Condition 1"
    - "Condition 2"

  # Environment setup
  environment:
    variables:
      VAR_NAME: "value"

  # Variables for templating
  variables:
    username: "testuser"

  # Test steps (executed in order)
  steps:
    - action: launch
      target: "./app"

    - action: send_input
      value: "command\n"

    - action: verify_output
      contains: "Success"
      timeout: 5s

  # Cleanup (always runs)
  cleanup:
    - action: stop_application
```

## Skill Usage Examples [LEVEL 1]

### Invoke This Skill

```
Claude, use the qa-team skill to create a CLI test for my calculator app.

Claude, use qa-team to generate web tests for the login flow.

Claude, create TUI tests using the qa-team skill for file navigation.
```

### What You'll Get

1. **Complete YAML test scenario** matching your requirements
2. **Inline documentation** explaining each section
3. **Best practices applied** (timeouts, waits, verification)
4. **Appropriate complexity level** (1, 2, or 3 based on needs)
5. **Instructions for running** the test

### Example Request and Response

**User**: "Create a web test for user login with valid credentials"

**Claude** (using this skill):

```yaml
scenario:
  name: "User Login - Valid Credentials"
  description: "Verifies user can successfully log in with valid email and password"
  type: web
  tags: [smoke, auth, critical]

  prerequisites:
    - "User test@example.com exists with password 'TestPass123!'"
    - "Application is running on http://localhost:3000"

  steps:
    # Navigate to login page
    - action: navigate
      url: "http://localhost:3000/login"
      description: "Open login page"

    # Wait for page to fully load
    - action: wait_for_element
      selector: "form.login-form"
      timeout: 5s
      description: "Wait for login form to render"

    # Fill in email field
    - action: type
      selector: "input[name='email']"
      value: "test@example.com"
      description: "Enter email address"

    # Fill in password field
    - action: type
      selector: "input[name='password']"
      value: "TestPass123!"
      description: "Enter password"

    # Submit form
    - action: click
      selector: "button[type='submit']"
      description: "Click login button"

    # Wait for redirect to dashboard
    - action: wait_for_url
      contains: "/dashboard"
      timeout: 10s
      description: "Should redirect to dashboard after successful login"

    # Verify user is logged in
    - action: verify_element
      selector: ".user-profile"
      contains: "test@example.com"
      description: "User email should appear in profile widget"

    # Capture evidence
    - action: screenshot
      save_as: "successful-login.png"
      description: "Save screenshot of logged-in state"
# To run this test:
# 1. Save as test-login-valid.yaml
# 2. Run: gadugi-test run test-login-valid.yaml
# 3. Check evidence in ./evidence/user-login-valid-credentials-TIMESTAMP/
```

## Related Resources [LEVEL 1]

### Official Documentation

- **GitHub Repository**: https://github.com/rysweet/gadugi-agentic-test
- **Framework Docs**: See repo README and docs/ folder
- **Issue Tracker**: https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding/issues/1356

---

**Remember**: Outside-in tests verify WHAT your application does, not HOW it does it. Focus on user-visible behavior, and your tests will remain stable across refactorings while providing meaningful validation of critical workflows.

Start at Level 1 with simple smoke tests, and progressively add complexity only when needed. The framework's AI agents handle the hard parts - you just describe what should happen.
