# Outside-In Scenario YAML Format

Outside-in test scenarios live in `tests/outside-in/` and are validated by
`gadugi-test validate`. Each file describes one end-to-end scenario as a
YAML document.

Two top-level layout variants exist in the corpus. Both are valid. Choose the
one that best matches the scenario's complexity.

---

## Format A — Wrapped metadata (`scenario:` object)

Used by scenarios that exercise a binary via `launch` / `verify_output` steps.
Metadata is nested under a `scenario:` key; `agents:` and `steps:` are
siblings at the top level.

### Required top-level fields (Format A)

| Field | Type | Description |
| --- | --- | --- |
| `agents` | list | One or more agent definitions |
| `scenario` | object | Scenario metadata (name, description, type, …) |
| `steps` | list | Ordered list of test actions |

### `scenario` object fields

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | string | **yes** | Human-readable scenario title |
| `description` | string | **yes** | What the scenario validates |
| `type` | string | **yes** | Scenario type — matches the primary agent type (`cli`, …) |
| `tags` | list | no | Labels for filtering and categorisation |
| `prerequisites` | list | no | Human-readable preconditions |

### Format A skeleton

```yaml
agents:
  - name: amplihack-cli
    type: cli
    command: amplihack

scenario:
  name: "Human-readable scenario title"
  description: |
    Multi-line description of what the scenario validates.
  type: cli
  tags: [smoke, feature-name, issue-NNN]
  prerequisites:
    - "Description of what must be true before the scenario runs"

steps:
  - action: launch
    target: "./target/release/amplihack"
    args: ["subcommand", "--flag"]
    env:
      KEY: "value"
    description: "Optional human-readable description"

  - action: verify_exit_code
    expected: 0

  - action: verify_output
    contains: "expected substring"

cleanup: []
```

---

## Format B — Flat metadata (bare top-level keys)

Used by scenarios that drive test execution via `run` steps (e.g. `cargo test`
invocations) and collect results in a separate `verifications:` block. All
metadata fields are bare top-level keys; there is no `scenario:` wrapper.

### Required top-level fields (Format B)

| Field | Type | Description |
| --- | --- | --- |
| `id` | string | Machine-readable scenario identifier (kebab-case) |
| `name` | string | Human-readable scenario title |
| `description` | string | What the scenario validates |
| `agents` | list | One or more agent definitions |
| `steps` | list | Ordered list of test actions |

### Optional top-level fields (Format B)

| Field | Type | Description |
| --- | --- | --- |
| `priority` | string | `HIGH`, `MEDIUM`, or `LOW` |
| `interface` | string | Primary interface under test (`CLI`, `HTTP`, …) |
| `tags` | list | Labels for filtering and categorisation |
| `prerequisites` | list | Human-readable preconditions |
| `verifications` | list | Post-run output assertions (see below) |
| `estimatedDuration` | integer | Expected wall-clock time in milliseconds |
| `enabled` | boolean | Set to `false` to skip the scenario (default `true`) |

### Format B skeleton

```yaml
id: my-scenario
name: "Human-readable scenario title"
description: |
  Multi-line description of what the scenario validates.
priority: HIGH
interface: CLI
tags: [smoke, feature-name]
prerequisites:
  - "Rust toolchain available on PATH"

agents:
  - name: cargo-test-runner
    type: cli
    command: cargo

steps:
  - action: run
    target: "cargo test -p my-crate --lib -- my_module --no-fail-fast"
    description: "Run unit tests for my_module"
    expected: success
    timeout: 60000

verifications:
  - type: output
    target: stdout
    expected: "test result: ok"
    operator: contains
    description: "All tests must pass"

estimatedDuration: 60
enabled: true
```

---

## `agents` section (both formats)

```yaml
agents:
  - name: amplihack-cli
    type: cli
    command: amplihack
```

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | string | **yes** | Unique name for the agent within this scenario |
| `type` | string | **yes** | Agent type — `cli` for command-line tools |
| `command` | string | yes (for `cli`) | The executable to invoke |

### Supported agent types

| `type` | Description |
| --- | --- |
| `cli` | Runs a command-line executable. `command` is the binary name (resolved via `$PATH`) or a relative path. |

---

## `steps` section (both formats)

Each step is executed in order. The supported `action` values differ slightly
between the two formats:

| `action` | Required fields | Optional fields | Description |
| --- | --- | --- | --- |
| `launch` | `target` | `args`, `env`, `description` | Execute a binary directly (Format A). |
| `run` | `target` | `description`, `expected`, `timeout` | Run a full command string, e.g. `cargo test …` (Format B). |
| `verify_exit_code` | `expected` | `description` | Assert the last process exited with the given code (Format A). |
| `verify_output` | `contains` or `matches` | `description` | Assert output contains a literal string or matches a regex (Format A). |

**`run` step fields:**

| Field | Type | Description |
| --- | --- | --- |
| `target` | string | Full command string to execute |
| `expected` | string | `success` asserts exit code 0; omit to allow any exit code |
| `timeout` | integer | Timeout in milliseconds before the step is aborted |
| `description` | string | Human-readable label for the step |

---

## `verifications` section (Format B only)

Post-run assertions collected after all `steps` complete.

```yaml
verifications:
  - type: output
    target: stdout
    expected: "test result: ok"
    operator: contains
    description: "All test suites must pass"
```

| Field | Type | Description |
| --- | --- | --- |
| `type` | string | `output` — asserts against captured output |
| `target` | string | `stdout` or `stderr` |
| `expected` | string | The string or pattern to match |
| `operator` | string | `contains`, `matches` (regex), or `equals` |
| `description` | string | Human-readable label |

---

## `cleanup` section (Format A only)

```yaml
cleanup: []
```

Optional list of cleanup actions to run after the scenario completes.
Use an empty list `[]` when no cleanup is needed.

---

## Validation

`gadugi-test validate` checks all scenario files in `tests/outside-in/`:

```sh
gadugi-test validate tests/outside-in/
```

Validation fails when:

- `agents:` is absent or empty.
- `steps:` is absent or empty.
- A Format A file is missing the `scenario:` block or its required sub-fields.
- A Format B file is missing `id`, `name`, or `description`.
- A `verify_output` step specifies neither `contains` nor `matches`.
- A `launch` or `run` step is missing `target`.

---

## Existing scenarios

| File | Format | Validates |
| --- | --- | --- |
| `scenario1-fleet-status-basic.yaml` | A | `amplihack fleet` status command |
| `scenario2-code-graph-no-python.yaml` | A | Native code-graph query without Python on `PATH` (Issue #77 AC6/AC8/AC9) |
| `scenario3-domain-agents-routing.yaml` | B | Domain agent routing (CodeReview, MeetingSynthesizer) |
| `scenario4-teaching-session.yaml` | — | Teaching session flow |
| `scenario5-adaptive-context-uvx.yaml` | — | Adaptive context with UVX |
| `scenario5-eval-modules.yaml` | — | Eval module scenarios |
| `scenario6-cross-crate-parity.yaml` | — | Cross-crate parity checks |
| `scenario7-kuzu-to-ladybugdb-migration.yaml` | — | KuzuDB → LadybugDB migration path |

---

## Related

- [`docs/reference/install-completeness.md`](install-completeness.md) —
  integration test contract that exercises the CLI via `cargo run --package amplihack`
- [`docs/reference/amplihack-package-binaries.md`](amplihack-package-binaries.md) —
  multiple-binary layout and `default-run` directive
