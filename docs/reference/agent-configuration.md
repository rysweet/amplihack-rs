# Agent Configuration Reference

Complete reference for configuring agents, harness overflow, memory backends,
evaluation, and hive deployments in amplihack-rs.

## Agent Configuration

### Environment Variables

| Variable                    | Type     | Default             | Description                            |
|-----------------------------|----------|---------------------|----------------------------------------|
| `AMPLIHACK_AGENT_MODEL`     | `String` | `claude-sonnet-4-5` | Default LLM model for agents           |
| `AMPLIHACK_AGENT_BINARY`    | `String` | `copilot`           | Active AI binary override; precedence: env var (unless tagged as a default guess) → session marker → `<repo>/.claude/runtime/launcher_context.json` → `copilot`. Allowlist: `claude` \| `copilot` \| `codex` \| `amplifier`. See [Active Agent Binary](./active-agent-binary.md). |
| `AMPLIHACK_MEMORY_BACKEND`  | `String` | `cognitive`         | Memory backend: `cognitive`, `hierarchical`, `memory` |
| `AMPLIHACK_MEMORY_TOPOLOGY` | `String` | `single`            | Memory topology: `single`, `distributed` |
| `AMPLIHACK_MEMORY_STORAGE_PATH` | `String` | `~/.amplihack/memory.db` | Memory storage path          |
| `AMPLIHACK_MAX_DEPTH`       | `u32`    | `3`                 | Max recursion depth for nested sessions |
| `AMPLIHACK_NONINTERACTIVE`  | `bool`   | `false`             | Skip interactive prompts               |
| `AMPLIHACK_OVERFLOW_HARNESSES` | `String` | (unset: off)     | Comma-separated harnesses a recipe run may overflow to. See [Harness Overflow](#harness-overflow). |
| `AMPLIHACK_OVERFLOW_MIN_PRIORITY` | `String` | `low`         | Lowest work priority allowed to overflow: `low`, `normal`, `high`. |
| `EVAL_MODEL`                | `String` | `claude-opus-4-6`   | Model used for evaluation grading      |

### Project Configuration File

Create `.amplihack/config.toml` in your project root:

```toml
[agent]
model = "claude-sonnet-4-5"
max_turns = 100
timeout_secs = 300

[memory]
backend = "cognitive"
topology = "single"
storage_path = ".amplihack/memory.db"

[memory.kuzu]  # backward-compatible config section name
buffer_pool_mb = 64
max_db_size_mb = 1024

[memory.distributed]
replication_factor = 3
query_fanout = 2
gossip_enabled = true
gossip_rounds = 3

[eval]
model = "claude-opus-4-6"
output_dir = "eval/results"

[hive]
num_containers = 20
agents_per_container = 5
location = "eastus"
topic_name = "hive-graph"
```

### User Configuration File

Create `~/.amplihack/config.toml` for user-level defaults:

```toml
[agent]
model = "claude-sonnet-4-5"

[memory]
backend = "cognitive"
```

## Memory Backend Configuration

### Cognitive Backend (Default)

```toml
[memory]
backend = "cognitive"
storage_path = "~/.amplihack/memory.db"
```

The cognitive backend stores memories using graph-based cognitive modeling with:
- Thread-safe access via `RwLock`
- Automatic schema migrations
- `0o600` file permissions
- Full-text search support

### LadybugDB Graph Backend

```toml
[memory]
backend = "kuzu"  # backward-compatible alias for the LadybugDB graph backend

[memory.kuzu]  # backward-compatible config section name
buffer_pool_mb = 64
max_db_size_mb = 1024
db_path = "~/.amplihack/kuzu_db"  # legacy path, backward-compatible
```

The LadybugDB (formerly Kuzu) backend provides graph storage for:
- Relationship-aware memory queries
- Code graph integration
- Cypher query support

**Requires**: Build with `lbug` feature flag.

### In-Memory Backend

```toml
[memory]
backend = "memory"
```

Volatile storage for testing. All data lost on restart.

### Distributed Topology

```toml
[memory]
topology = "distributed"

[memory.distributed]
replication_factor = 3
query_fanout = 2
gossip_enabled = true
gossip_rounds = 3
transport = "azure_service_bus"
connection_string = "Endpoint=sb://..."
```

Distributed topology enables:
- Content-based sharding via consistent hash ring
- Bloom filter gossip for cross-shard discovery
- Configurable replication and query fanout

## Evaluation Configuration

### Harness Settings

```toml
[eval]
model = "claude-opus-4-6"
output_dir = "eval/results"

[eval.teaching]
max_turns = 20
student_level = "intermediate"

[eval.long_horizon]
total_turns = 1000
checkpoint_interval = 100
categories = ["factual", "procedural", "episodic"]

[eval.self_improve]
iterations = 5
strategy = "prompt-refinement"
```

### Environment Variables for Eval

| Variable          | Type     | Default             | Description                    |
|-------------------|----------|---------------------|--------------------------------|
| `EVAL_MODEL`      | `String` | `claude-opus-4-6`   | Model for grading              |
| `EVAL_OUTPUT_DIR`  | `String` | `eval/results`     | Output directory               |
| `EVAL_MAX_TURNS`  | `u32`    | `1000`              | Max turns for long-horizon     |
| `EVAL_TIMEOUT`    | `u64`    | `3600`              | Per-evaluation timeout (secs)  |

## Hive Configuration

### Deployment Settings

```toml
[hive]
num_containers = 20
agents_per_container = 5
location = "eastus"
topic_name = "hive-graph"
cpu = 1.0
memory_gb = 4

[hive.azure]
resource_group = "hive-rg"
subscription_id = "your-sub-id"
acr_name = "myacr"
```

### Environment Variables for Hive

| Variable                                  | Type     | Default        | Description                    |
|-------------------------------------------|----------|----------------|--------------------------------|
| `SERVICE_BUS_CONN_STR`                    | `String` | (required)     | Service Bus connection string  |
| `AZURE_SUBSCRIPTION_ID`                   | `String` | (required)     | Azure subscription             |
| `AMPLIHACK_HIVE_NUM_CONTAINERS`           | `u32`    | `20`           | Container count override       |
| `AMPLIHACK_HIVE_AGENTS_PER_CONTAINER`     | `u32`    | `5`            | Agents per container override  |
| `AMPLIHACK_MEMORY_TRANSPORT`              | `String` | `local`        | Transport: `local`, `azure_service_bus` |

## Configuration Precedence

Configuration is resolved in this order (highest to lowest priority):

1. **Explicit API parameters** — passed directly to constructors
2. **Environment variables** — `AMPLIHACK_*` prefixed
3. **Project config** — `.amplihack/config.toml` in the working directory
4. **User config** — `~/.amplihack/config.toml`
5. **Compiled defaults** — hardcoded in the crate

## Agent Binary Resolution

The active agent binary (`claude` | `copilot` | `codex` | `amplifier`) follows its **own** precedence, distinct from the general config precedence above:

1. `AMPLIHACK_AGENT_BINARY` env var (explicit override; CI / testing / back-compat), skipped while tagged `AMPLIHACK_AGENT_BINARY_SOURCE=default:<same binary>`, `session:<same binary>` or `overflow:<same binary>`
2. A live session marker exported by the hosting CLI (`CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `COPILOT_CLI`, ...). An `AMPLIHACK_AGENT_BINARY` tagged `session:<binary>`, which a launcher or `recipe run` exported to describe a session, ranks here, after the markers of the CLI it names
3. `<repo>/.claude/runtime/launcher_context.json` `launcher` field (persisted, possibly by a different session; used only while fresh)
4. Built-in default: **`copilot`**

This precedence is implemented once in `amplihack_utils::agent_binary::resolve(&cwd)` and consumed by every Rust read site. The shell helper in `amplifier-bundle/skills/migrate/scripts/migrate.sh` approximates it; the Rust resolver is authoritative where they differ.

See [Active Agent Binary](./active-agent-binary.md) for the full algorithm, allowlist, and security notes, and [Agent Binary Routing](../concepts/agent-binary-routing.md) for the architectural rationale.

[Harness Overflow](#harness-overflow) runs after this resolution and never overrides an explicit choice (layer 1 untagged).

## Harness Overflow

Harness overflow lets a recipe run continue under another agent harness when
every Claude profile is near its limit. It is off by default. Nothing about a
run changes until you list at least one overflow harness: no capacity probe is
made, no file is written under `~/.amplihack/runtime/overflow/`, and recipe
output is identical to a run without the feature.

### Contents

- [Enabling overflow](#enabling-overflow)
- [Overflow settings](#overflow-settings)
- [Work priority](#work-priority)
- [When a run or step overflows](#when-a-run-or-step-overflows)
- [Which harness takes over](#which-harness-takes-over)
- [How capacity is measured](#how-capacity-is-measured)
- [Step arguments on a redirected step](#step-arguments-on-a-redirected-step)
- [Variables exported to steps](#variables-exported-to-steps)
- [Per-run harness record](#per-run-harness-record)
- [Harness per step in the run summary](#harness-per-step-in-the-run-summary)
- [Errors](#errors)
- [Limitations](#limitations)

### Enabling overflow

Add an `overflow` object to `~/.amplihack/config`. This file is JSON, with no
extension, and is the same file the launcher reads for memory settings; it is
not the TOML `config.toml` described above.

```json
{
  "overflow": {
    "harnesses": ["copilot", "codex"],
    "min_priority": "normal"
  }
}
```

Or set the environment variables for one shell or one command:

```sh
export AMPLIHACK_OVERFLOW_HARNESSES=copilot,codex
export AMPLIHACK_OVERFLOW_MIN_PRIORITY=normal
amplihack recipe run default-workflow -c task_description="Fix the flaky test" -c repo_path=.
```

Overflow is enabled only by a non-empty harness list. Setting
`AMPLIHACK_OVERFLOW_MIN_PRIORITY` or `min_priority` on its own does nothing.

To switch overflow off for one command while the config file enables it, set
the harness variable to `off`, `none` or an empty string:

```sh
AMPLIHACK_OVERFLOW_HARNESSES=off amplihack recipe run default-workflow -c task_description="..."
```

### Overflow settings

| Config key | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `overflow.harnesses` | `AMPLIHACK_OVERFLOW_HARNESSES` | none (off) | Harnesses that may take over, in order of preference. The variable is comma-separated. Allowed names: `claude`, `codex`, `copilot`. `off`, `none` or a blank value (any case) disables overflow. |
| `overflow.min_priority` | `AMPLIHACK_OVERFLOW_MIN_PRIORITY` | `low` | Lowest [work priority](#work-priority) that may overflow: `low`, `normal` or `high`. |

Each field is resolved on its own, and the environment variable wins over the
config file. With the file above and `AMPLIHACK_OVERFLOW_MIN_PRIORITY=high`,
the harness list comes from the file and the minimum is `high`.

Validation rules:

- Names are trimmed and lower-cased with the same rules as
  `AMPLIHACK_AGENT_BINARY` (see
  [Allowlist & Validation](./active-agent-binary.md#allowlist--validation)).
- An unknown name, a duplicate, an empty entry (`copilot,,codex`) and
  `amplifier`, which has no overflow adapter, are errors.
- The run's own primary harness may appear in the list; it is skipped.
- The `overflow` value must be a JSON object. Any key other than `harnesses`
  and `min_priority` is an error naming that key, and so is a value of the
  wrong type.
- A missing config file, a file that is not valid JSON, or a file larger than
  1 MiB supplies no overflow settings and prints nothing. A value of
  `AMPLIHACK_OVERFLOW_HARNESSES` longer than 256 bytes is an error.

JSON Schema for the `overflow` object:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "harnesses": {
      "type": "array",
      "items": { "enum": ["claude", "codex", "copilot"] },
      "uniqueItems": true
    },
    "min_priority": { "enum": ["low", "normal", "high"] }
  }
}
```

The schema is case-sensitive; amplihack itself also accepts surrounding
whitespace and upper case in names.

### Work priority

Every recipe run has a priority of `low`, `normal` or `high`
(`low < normal < high`). It is taken from the first of:

1. The `--priority` flag on `amplihack recipe run`.
2. The context value `priority`, from `-c priority=...` or the recipe's own
   `context:` block.
3. The default, `normal`.

```sh
amplihack recipe run default-workflow --priority low \
  -c task_description="Tidy the changelog" -c repo_path=.
```

`--priority` is appended after your `-c` arguments, so it wins over
`-c priority=...` given on the same command line.

The priority is checked only while overflow is enabled. A recipe that already
uses a context value called `priority` for something else keeps working when
overflow is off; with overflow on, a value other than `low`, `normal` or
`high` stops the run (see [Errors](#errors)).

A run may overflow only when its priority is at or above `min_priority`. With
`min_priority` set to `normal`, a `--priority low` run waits on the primary
harness and its record shows `priority-below-minimum`.

### When a run or step overflows

The decision is taken at two points, never in the middle of an agent step:

1. **Run start.** `amplihack recipe run` resolves the agent binary as usual
   (see [Agent Binary Resolution](#agent-binary-resolution)), then decides
   whether to start the run under an overflow harness. The root-sandbox
   pre-flight then checks the harness that will actually run.
2. **Each agent step.** The recipe runner starts every agent step as
   `amplihack <cli> -- <args>`. Before that nested launcher spawns the harness,
   it decides once, against the run's primary harness, whether this step
   should run elsewhere. A step that has started is never interrupted,
   restarted or moved.

At each point the rule is the same: use the primary harness if it has
capacity, otherwise the first overflow harness that qualifies. A later step
returns to the primary once its capacity recovers.

A run or step is eligible only when all of these hold:

- The agent binary was not an explicit choice. An untagged
  `AMPLIHACK_AGENT_BINARY` is your choice and is never overridden; so is a
  value handed on with `AMPLIHACK_AGENT_BINARY_SOURCE=tmux_server:...`. A
  session marker, a launcher context, the built-in default, and values tagged
  `default:`, `session:` or `overflow:` are eligible. See
  [What amplihack exports is tagged](./active-agent-binary.md#what-amplihack-exports-is-tagged).
- The process belongs to a recipe run: `AMPLIHACK_RECIPE_RUN_ID` is non-empty
  and `AMPLIHACK_NONINTERACTIVE` is exactly `1`. Interactive launches and a
  direct `amplihack claude`, `amplihack codex` or `amplihack copilot` outside a
  recipe run never overflow.
- At step level, the launcher invoked is the run's agent binary. A step that
  names a different CLI on purpose is left alone.
- The run's priority is at or above `min_priority`.

### Which harness takes over

Candidates are checked in the configured order, skipping the primary. A
candidate qualifies when it is:

1. **Installed**: its binary is found on `PATH`.
2. **Logged in**: a credential exists in the place listed below.
3. **Not exhausted**: its capacity reading is not `Exhausted`. A reading of
   `Unknown` counts as available.

Neither check runs the harness CLI. The logged-in check reads file metadata
or tests that a variable is non-empty; credential contents and token values
are never read into amplihack's records, logs or messages.

| Harness | Logged in when | Capacity |
| --- | --- | --- |
| `claude` | `$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`, is a non-empty regular file | Measured (see below) |
| `codex` | `$CODEX_HOME/auth.json`, else `~/.codex/auth.json`, is a non-empty regular file | `Unknown` |
| `copilot` | `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN` is non-empty, else `${COPILOT_HOME:-~/.copilot}/config.json` is a non-empty regular file | `Unknown` |

When no candidate qualifies, the work stays on the primary harness and the
record lists each candidate with the reason it was passed over:
`not-installed`, `not-logged-in` or `exhausted`. Work is never dropped.

### How capacity is measured

Only Claude has a measured capacity. The probe is the one
`amplihack profile rotate` uses: one usage reading for the live login and one
for each saved profile (see [Rotate Claude Logins](../howto/claude-profile-rotation.md)).
It does not take the rotator's lock and never swaps a login.

| Readings | Capacity |
| --- | --- |
| At least one profile is under both thresholds | `Available` |
| None under threshold, and at least one reading failed | `Unknown` |
| Every reading is exhausted, or at or above a threshold | `Exhausted` |

The thresholds are rotate's: `AMPLIHACK_ROTATE_SESSION_AT` (default `85`) and
`AMPLIHACK_ROTATE_WEEK_AT` (default `90`), in per cent.

Overflow happens only on `Exhausted`. A primary whose capacity is `Unknown`
keeps the work, with the reason `capacity-unknown`: a failed probe, such as a
network fault, is not evidence that the primary is out of capacity. Because
`codex` and `copilot` always report `Unknown`, a run whose primary is one of
them never overflows.

Readings are cached for 60 seconds in
`~/.amplihack/runtime/overflow/capacity.json`, keyed by harness and, for
Claude, by a hash of the sorted profile names. `Unknown` results are cached
too, so a failing probe is not repeated for every step. A corrupt, expired or
future-dated entry is probed again. The file is safe to delete at any time.

### Step arguments on a redirected step

The runner passes each agent step a fixed set of arguments. A redirected step
is re-dispatched to the target harness's own launcher with those arguments
translated by this table. Values are passed as single argv entries; nothing is
joined or handed to a shell. `--flag=value` is read as `--flag value`.

| Runner argument | To `claude` | To `copilot` | To `codex` |
| --- | --- | --- | --- |
| `-p <prompt>` | kept | kept | kept |
| `--add-dir <dir>` | kept | kept | kept |
| `--model <name>` | dropped | dropped | dropped |
| `--allow-all-tools` | dropped | added, unless `AMPLIHACK_NO_ALLOW_ALL_TOOLS=1` | dropped |
| `--system-prompt <text>` | kept | step stays put | step stays put |
| `--append-system-prompt <text>` | kept | step stays put | step stays put |
| `--append-system-prompt-file <path>` | kept | step stays put | step stays put |

`--model` is dropped because model names belong to one harness; the target
uses its own default model. "Step stays put" means the step runs on the
harness it was invoked as, with the reason `untranslatable-args`. The same
applies to any other argument, a flag missing its value, and a bare `--`.

### Variables exported to steps

While overflow is enabled, `amplihack recipe run` exports these to its steps.
None of them is exported while overflow is off.

| Variable | Value |
| --- | --- |
| `AMPLIHACK_OVERFLOW_HARNESSES` | The resolved harness list, comma-separated. |
| `AMPLIHACK_OVERFLOW_MIN_PRIORITY` | The resolved minimum priority. |
| `AMPLIHACK_RUN_PRIORITY` | The run's [work priority](#work-priority). |
| `AMPLIHACK_OVERFLOW_PRIMARY` | The run's primary harness. Exported only for an eligible run; without it no step overflows. |
| `AMPLIHACK_AGENT_BINARY` and `AMPLIHACK_AGENT_BINARY_SOURCE` | When the run started under an overflow harness: that harness, tagged `overflow:<harness>`. |

A step's launcher reads the harness list from `AMPLIHACK_OVERFLOW_HARNESSES`.
When `~/.amplihack/config` also lists harnesses, a step uses only names on both
lists, so a value injected into a step's environment cannot add a harness the
config file does not allow. Every harness name read from these variables is
validated against `claude`, `codex` and `copilot` again at each read.

The intersection has one consequence: if you enable overflow through the
variable with a harness that the config file's list does not contain, the run
start can choose it but no step can. Add the harness to the file, or remove
the file's `harnesses` key.

### Per-run harness record

While overflow is enabled, each decision is appended as one JSON line to
`~/.amplihack/runtime/overflow/runs/<run_id>.jsonl`, where `<run_id>` is the
run's `AMPLIHACK_RECIPE_RUN_ID`. The first line describes the run start; one
line follows for each agent step.

```json
{"seq":1,"kind":"run-start","harness":"claude","primary":"claude","overflow":false,"reason":"primary-has-capacity","priority":"normal","timestamp":"2026-10-19T09:12:41Z"}
{"seq":2,"kind":"step","harness":"claude","primary":"claude","overflow":false,"reason":"primary-has-capacity","priority":"normal","timestamp":"2026-10-19T09:12:44Z"}
{"seq":3,"kind":"step","harness":"copilot","primary":"claude","overflow":true,"reason":"overflow:copilot","priority":"normal","rejected":[{"harness":"codex","why":"not-installed"}],"timestamp":"2026-10-19T09:31:07Z"}
```

| Field | Meaning |
| --- | --- |
| `seq` | Per-run counter, assigned under a file lock, so parallel steps get distinct numbers. |
| `kind` | `run-start` or `step`. |
| `harness` | The harness that ran. |
| `primary` | The run's primary harness. |
| `overflow` | `true` when `harness` is an overflow harness. |
| `reason` | Why this harness was chosen (see below). |
| `priority` | The run's work priority. |
| `rejected` | Candidates passed over, each with `why`: `not-installed`, `not-logged-in` or `exhausted`. Omitted when empty. |
| `timestamp` | RFC 3339, UTC. |

| `reason` | Meaning |
| --- | --- |
| `primary-has-capacity` | The primary is `Available`. |
| `capacity-unknown` | The primary's capacity could not be measured; it keeps the work. |
| `priority-below-minimum` | The run's priority is below `min_priority`. |
| `explicit-choice` | The agent binary was chosen explicitly and is not overridden. |
| `no-candidate-qualified` | The primary is exhausted and no overflow harness qualified. |
| `untranslatable-args` | The step's arguments cannot be carried to the target harness. |
| `overflow:<harness>` | The work moved to `<harness>`. |

The record never holds prompt text, step arguments, credential paths or token
values. The `runs/` directory is created with mode `0700` and each file with
`0600`; a symlink in either place is refused. A record that cannot be written
produces one warning on stderr and the step runs as decided.

### Harness per step in the run summary

When a run has a record, `amplihack recipe run` adds a section after the
steps, in record order:

```text
Harness per step:
  1. claude (run start)
  2. claude
  3. copilot (overflow from claude)
  4. claude
```

The suffix is `(run start)` for the run-start line, `(overflow from <primary>)`
for an overflowed line, and `(<reason>)` when the primary kept the work for any
reason other than `primary-has-capacity`. With `--output-format json` or
`yaml`, the same entries appear as a top-level `harness_steps` array, with the
fields of the record. At most 1,000 entries are attached; beyond that the last
entry is followed by `... (truncated)` in the table and the result carries
`"harness_steps_truncated": true`.

When overflow is enabled, the run start also prints one line on stdout:

```text
amplihack: overflow: agent steps start under 'copilot' instead of 'claude' (overflow:copilot; priority normal)
amplihack: overflow: agent steps start under 'claude' (primary-has-capacity; priority normal)
```

### Errors

Invalid settings stop `amplihack recipe run` with exit code 1 before any step
runs. Values are shown quoted and escaped, cut to 32 characters, so control
characters cannot reach the terminal or a log.

```text
invalid overflow setting AMPLIHACK_OVERFLOW_HARNESSES "gemini": expected claude, codex or copilot
invalid overflow setting AMPLIHACK_OVERFLOW_HARNESSES "copilot": duplicate harness
invalid overflow setting AMPLIHACK_OVERFLOW_HARNESSES "amplifier": no overflow adapter; expected claude, codex or copilot
invalid overflow setting overflow "max_steps": unknown key; expected harnesses or min_priority
invalid overflow setting overflow.min_priority "urgent": expected low|normal|high
invalid overflow setting priority "urgent": expected low|normal|high
```

A step-level launcher never fails because of overflow: any error there leaves
the step on the harness it was invoked as.

### Limitations

- `codex` and `copilot` report `Unknown` capacity until they gain usage probes
  (umbrella issue #1617), so an overflow target may itself be rate-limited.
  Supporting another pair of harnesses needs only an adapter; the policy is
  harness-neutral.
- There is no queue (issue #1620). A run with no qualifying candidate
  continues on the primary, as it would without overflow.
- `recipe-runner-rs` currently turns context and output keys prefixed
  `amplihack_` into environment variables for steps (runner issue #146), so a
  recipe can influence the overflow variables of its own steps. The
  config-file intersection and the closed set of harness names limit what such
  a value can do.
- A step redirected to `copilot` runs with `--allow-all-tools`, as copilot
  steps launched by the runner already do, unless
  `AMPLIHACK_NO_ALLOW_ALL_TOOLS=1` is set.
- Parallel steps that miss the cache at the same moment may each run the
  Claude probe once.
- The logged-in check is evidence that credentials exist, not that they are
  valid. A step that fails on a stale login is decided afresh when retried.

## Related

- [Environment Variables](./environment-variables.md) — Full env var reference
- [Active Agent Binary](./active-agent-binary.md) — Resolver API, allowlist, hook resolution
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — Architecture and propagation model
- [Memory Backend](./memory-backend.md) — Backend selection details
- [Hive Orchestration](../concepts/hive-orchestration.md) — Hive architecture
- [Rotate Claude Logins](../howto/claude-profile-rotation.md): the capacity probe and thresholds overflow reuses
