# Outside-In Scenario YAML Format

Outside-in test scenarios live in `tests/outside-in/` and are validated by
`gadugi-test validate`. Each file describes one end-to-end scenario as a
YAML document.

## Required top-level fields

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `agents` | list | **yes** | One or more agent definitions the scenario relies on |
| `scenario` | object | **yes** | Scenario metadata (name, description, type, tags, …) |
| `steps` | list | **yes** | Ordered list of test actions |

`gadugi-test validate` fails with a schema error when `agents:` is absent.
Every scenario file must declare at least one agent entry.

## `agents` section

```yaml
agents:
  - name: amplihack-cli
    type: cli
    command: amplihack
```

Each entry in the `agents` list describes one test agent.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | string | **yes** | Unique name for the agent within this scenario |
| `type` | string | **yes** | Agent type — `cli` for command-line tools |
| `command` | string | yes (for `cli`) | The executable to invoke |

### Supported agent types

| `type` | Description |
| --- | --- |
| `cli` | Runs a command-line executable. The `command` field specifies the binary name (resolved via `$PATH`) or a relative path. |

## `scenario` section

```yaml
scenario:
  name: "Human-readable scenario title"
  description: |
    Multi-line description of what the scenario validates.
  type: cli
  tags: [smoke, feature-name, issue-NNN]
  prerequisites:
    - "Description of what must be true before the scenario runs"
```

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `name` | string | **yes** | Human-readable scenario title |
| `description` | string | **yes** | What the scenario validates |
| `type` | string | **yes** | Scenario type — matches the primary agent type |
| `tags` | list | no | Labels for filtering and categorisation |
| `prerequisites` | list | no | Human-readable preconditions |

## `steps` section

Each step is an action the test runner executes in order.

```yaml
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

  - action: verify_output
    matches: "regex\\s+pattern"
```

### Step actions

| `action` | Required fields | Description |
| --- | --- | --- |
| `launch` | `target` | Execute a binary. `args` and `env` are optional. |
| `run` | `target` | Run a full command string (e.g. `cargo test ...`). |
| `verify_exit_code` | `expected` | Assert the last launched process exited with the given code. |
| `verify_output` | `contains` or `matches` | Assert the last process output contains a literal string or matches a regex. |

## `cleanup` section

```yaml
cleanup: []
```

Optional list of cleanup actions to run after the scenario completes.
Use an empty list `[]` when no cleanup is needed.

## Complete example

```yaml
agents:
  - name: amplihack-cli
    type: cli
    command: amplihack

scenario:
  name: "Example scenario"
  description: |
    Verifies that `amplihack --version` exits 0 and prints a semver string.
  type: cli
  tags: [smoke]
  prerequisites:
    - "amplihack release binary exists at ./target/release/amplihack"

steps:
  - action: launch
    target: "./target/release/amplihack"
    args: ["--version"]
    description: "Print version"

  - action: verify_exit_code
    expected: 0

  - action: verify_output
    matches: "\\d+\\.\\d+\\.\\d+"
    description: "Output must contain a semver version string"

cleanup: []
```

## Validation

`gadugi-test validate` checks all scenario files in `tests/outside-in/`:

```sh
gadugi-test validate tests/outside-in/
```

Validation fails when:

- `agents:` is absent or empty.
- `scenario:` is absent or missing required sub-fields.
- `steps:` is absent or empty.
- A `verify_output` step specifies neither `contains` nor `matches`.
- A `launch`/`run` step is missing `target`.

## Existing scenarios

| File | Validates |
| --- | --- |
| `scenario1-fleet-status-basic.yaml` | `amplihack fleet` status command |
| `scenario2-code-graph-no-python.yaml` | Native code-graph query without Python on `PATH` (Issue #77 AC6/AC8/AC9) |
| `scenario3-domain-agents-routing.yaml` | Domain agent routing (CodeReview, MeetingSynthesizer) |
| `scenario4-teaching-session.yaml` | Teaching session flow |
| `scenario5-adaptive-context-uvx.yaml` | Adaptive context with UVX |
| `scenario5-eval-modules.yaml` | Eval module scenarios |
| `scenario6-cross-crate-parity.yaml` | Cross-crate parity checks |
| `scenario7-kuzu-to-ladybugdb-migration.yaml` | KuzuDB → LadybugDB migration path |

## Related

- [`docs/reference/install-completeness.md`](install-completeness.md) —
  integration test contract that exercises the CLI via `cargo run --package amplihack`
- [`docs/reference/amplihack-package-binaries.md`](amplihack-package-binaries.md) —
  multiple-binary layout and `default-run` directive
