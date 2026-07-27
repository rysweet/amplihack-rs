# Recipe Context Environment Export — Reference

Complete reference for how `amplihack recipe run` exports every recipe context
variable as an environment variable so that bash steps can read them directly —
including under `set -u` and inside nested sub-recipes.

This contract complements the `{{placeholder}}` template substitution already
documented in [Recipe Executor Environment](./recipe-executor-environment.md).
Template substitution rewrites step *text*; context environment export makes the
same values available to the *process environment* of every shell step.

## Contents

- [Why it exists](#why-it-exists)
- [Behavior summary](#behavior-summary)
- [Key transformation rules](#key-transformation-rules)
- [Reserved-name denylist](#reserved-name-denylist)
- [Aggregate environment budget](#aggregate-environment-budget)
- [Precedence and no-regression guarantees](#precedence-and-no-regression-guarantees)
- [Nested and sub-recipe propagation](#nested-and-sub-recipe-propagation)
- [Skip logging](#skip-logging)
- [API: `context_env_pairs` and budget helpers](#api-context_env_pairs-and-budget-helpers)
- [Examples](#examples)
- [Configuration](#configuration)
- [Security model](#security-model)
- [resolve-bundle-asset availability](#resolve-bundle-asset-availability)
- [Related](#related)

---

## Why it exists

Recipe context variables such as `task_description` and `repo_path` were
substituted into bash step text as `{{task_description}}` placeholders but were
**not** present in the process environment of the shell step. A bash step that
referenced `$TASK_DESCRIPTION` or `$REPO_PATH` directly would therefore fail
under `set -u` (the default hardening for recipe shell steps):

```
TASK_DESCRIPTION: unbound variable
REPO_PATH: unbound variable
```

This blocked every multi-workstream campaign that reached
`step-03-create-issue`, because that step reads context from the environment.
Six prior follow-up workstreams all failed at the same step and produced no
pull requests.

Context environment export closes the gap: the values that feed
`{{placeholders}}` are now **also** exported as environment variables, so bash
steps may use either form interchangeably. The export happens once on the
`recipe-runner-rs` subprocess and is inherited by every nested shell step,
including those launched from sub-recipes.

### The aggregate-size failure (issue #1023)

Exporting every context value protected each *individual* value against
`E2BIG` (the per-variable cap, see [rule 6](#key-transformation-rules)), but the
mirror originally had **no aggregate cap**. `recipe-runner-rs` inherits the whole
environment and re-exports it to every bash step (`Command::envs(child_env)`), so
in a long workflow the cumulative environment grows until the total argv+envp
size crosses the kernel's `ARG_MAX` limit. The *first* small bash step can spawn
fine, but a *late* step — for example `step-19d-verification-gate`, after all the
real work is already done — fails at spawn time with:

```
Argument list too long (os error 7)
```

This is a **false failure**: the recipe is reported as failed even though every
meaningful step succeeded. The runner-side analysis is in
[rysweet/amplihack-recipe-runner#130](https://github.com/rysweet/amplihack-recipe-runner/issues/130);
because the runner inherits its environment from `amplihack recipe run`, the
**primary** fix belongs here, on the CLI side, and is described in
[Aggregate environment budget](#aggregate-environment-budget).

---

## Behavior summary

When `amplihack recipe run` launches `recipe-runner-rs`, it applies the merged
recipe context (the same map fed to `{{placeholder}}` substitution) to the child
process environment:

| Aspect | Behavior |
|--------|----------|
| Source map | The merged recipe context (`context` block + `-c/--context` flags + inferred values) |
| Name | Context key, ASCII-uppercased (`task_description` → `TASK_DESCRIPTION`) |
| Value | Context value, unchanged |
| Scope | The `recipe-runner-rs` subprocess and — through normal process-environment inheritance — every shell and sub-recipe step it spawns (see [propagation](#nested-and-sub-recipe-propagation)) |
| Validity | Names must be valid POSIX shell identifiers; invalid keys are skipped |
| Safety | Reserved/dangerous names are never exported (see [denylist](#reserved-name-denylist)) |
| Precedence | Lowest — builder-managed and correlation variables always win |

No recipe YAML changes are required. Existing recipes that only use
`{{placeholders}}` are unaffected; recipes that read `$UPPERCASE_NAME` from the
environment now work.

---

## Key transformation rules

Each context entry `(key, value)` is transformed into a candidate environment
pair `(NAME, value)`:

1. **Uppercase.** `NAME = key.to_ascii_uppercase()`. Only ASCII letters are
   case-folded; non-ASCII characters are left unchanged and consequently fail
   validation in step 2.
2. **Validate as a shell identifier.** `NAME` must match
   `^[A-Z_][A-Z0-9_]*$`. This rejects:
   - empty keys,
   - keys whose uppercased form begins with a digit,
   - keys containing any character outside `[A-Z0-9_]` (spaces, dots, dashes,
     `=`, non-ASCII, etc.).
3. **Reject control characters in the value.** A value containing a NUL byte
   (`\0`) cannot be represented in a process environment and is skipped.
4. **Reject reserved names.** `NAME` must not appear in the
   [reserved-name denylist](#reserved-name-denylist) and must not begin with the
   `AMPLIHACK_` prefix.
5. **Reject oversized values.** A single environment string longer than the
   kernel's `MAX_ARG_STRLEN` (≈128 KB on Linux) makes the spawn fail with
   `E2BIG`. Values above a conservative per-variable byte cap are therefore not
   mirrored into the environment; they are still delivered to the runner via the
   recipe context file for `{{placeholder}}` substitution.
6. **Fit within the aggregate budget.** After all per-key checks pass, the
   surviving pairs are admitted against a single, runtime-derived
   [aggregate environment budget](#aggregate-environment-budget) so the *total*
   environment handed to `recipe-runner-rs` (and inherited by every bash step)
   stays safely under the kernel's `ARG_MAX`. A small set of
   [essential keys](#essential-keys-always-exported) is always admitted; the
   remaining keys are filled **smallest-first** until the budget is exhausted,
   and any key that does not fit is skipped from the *environment mirror only*.
   Skipped values are still delivered via the recipe context file, exactly like
   an oversized value in rule 5.

A candidate that passes all checks is exported. A candidate that fails any
check is **skipped** (never exported, never fatal) and a name-only warning is
emitted (see [Skip logging](#skip-logging)).

### Common mappings

| Context key | Exported environment variable |
|-------------|-------------------------------|
| `task_description` | `TASK_DESCRIPTION` |
| `repo_path` | `REPO_PATH` |
| `issue_number` | `ISSUE_NUMBER` |
| `branch_name` | `BRANCH_NAME` |
| `target_path` | `TARGET_PATH` |

### Collision behavior

If two distinct context keys uppercase to the same environment name (for example
`repo_path` and `REPO_PATH`), the context map is iterated in deterministic
(sorted) order and the last writer wins. This is well-defined but should be
avoided in recipe authoring.

---

## Reserved-name denylist

Some environment variables change how the shell or dynamic loader behaves before
a single line of the step runs. Exporting attacker- or author-controlled values
into those names would be a code-execution vector. The exporter therefore
**never** sets any name in the reserved denylist, regardless of context, and
never sets any name beginning with `AMPLIHACK_` (those are owned by the
subprocess environment builder).

| Category | Reserved names |
|----------|----------------|
| Dynamic linker | `LD_PRELOAD`, `LD_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `GLIBC_TUNABLES` |
| Shell startup / RCE | `BASH_ENV`, `ENV`, `PS4`, `PROMPT_COMMAND`, `SHELLOPTS`, `BASHOPTS` |
| Word splitting | `IFS` |
| Path and identity | `PATH`, `HOME`, `SHELL`, `PWD`, `USER`, `LOGNAME` |
| Interpreter options | `PYTHONPATH`, `NODE_OPTIONS`, `PERL5OPT`, `RUBYOPT` |
| Framework-owned prefix | any name starting with `AMPLIHACK_` |

`BASH_ENV` and `PS4` are the most commonly overlooked code-execution vectors —
`BASH_ENV` names a file sourced before a non-interactive script runs, and `PS4`
is expanded (and can contain command substitution) whenever `set -x` is active.
Both are denied.

The denylist is the **primary** control that makes bare (un-prefixed) export
names acceptable. It is exhaustively covered by tests. See
[Security model](#security-model).

---

## Aggregate environment budget

The per-variable cap ([rule 5](#key-transformation-rules)) stops any *single*
value from triggering `E2BIG`, but it cannot stop the *sum* of many valid values
from overflowing the kernel's total `argv + envp` limit (`ARG_MAX`). Because
`recipe-runner-rs` inherits this environment and re-exports it to **every** bash
step, an unbounded mirror made late steps in long workflows fail with
`Argument list too long (os error 7)` (issue #1023). The aggregate budget is the
fix: the exporter admits context pairs only up to a runtime-derived byte budget
that keeps the whole inherited environment comfortably under `ARG_MAX`.

The budget is **adaptive**, not a fixed magic number. It is derived at spawn time
from the actual kernel limit and the environment this process already carries.

### How the budget is derived

```
budget = ARG_MAX
         − bytes already consumed by the inherited process environment
         − a reservation for argv and the runner's own added variables
         (floored at 0)
```

1. **Query the kernel limit.** On Unix the exporter reads
   `sysconf(_SC_ARG_MAX)`. If the call returns `≤ 0`, or an implausibly small
   value (below 64 KB — smaller than any real platform and a sign of a broken or
   emulated environment), it falls back to a conservative constant of **128 KB**.
   POSIX only guarantees `_POSIX_ARG_MAX` = 4096, which is too small to reason
   about, so the floor and fallback are deliberately generous. Non-Unix targets
   always use the 128 KB fallback.
2. **Subtract the pass-through environment.** The exporter sums
   `name.len() + value.len() + per-entry-overhead` over the current process
   environment (`std::env::vars_os()`) — this is the environment that will pass
   **through** to the runner regardless of the mirror. The per-entry overhead
   (16 bytes) accounts for the `=` separator, the trailing `NUL`, and the
   argv/envp pointer slot on a 64-bit host.
3. **Subtract a reservation.** A single flat reservation (128 KB) covers the
   process argv, the runner's own `RECIPE_VAR_*` re-exports, the `AMPLIHACK_*`
   variables added by the environment builder, and a safety margin.
4. **Floor at zero.** All arithmetic is saturating, so a pathologically large
   inherited environment yields a budget of `0` (essentials only) rather than a
   wrapped, huge number.

The derivation is factored into a pure helper so it is unit-testable without
depending on the host's real `ARG_MAX` (see
[API](#api-context_env_pairs-and-budget-helpers)).

### Operator override

Set `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES` to an explicit non-negative integer to
**override** the derived budget. When set and valid, the override takes
precedence over the derived value, so policy can be pinned explicitly (for
example in CI images with unusual `ARG_MAX`).

| Value | Effect |
|-------|--------|
| unset | Budget is derived adaptively (default). |
| a valid `usize`, e.g. `262144` | That exact byte budget is used for the mirror. |
| `0` | Valid — mirror **only** the essential keys, drop all non-essential mirroring. |
| invalid (non-numeric, negative, overflowing) | Ignored; a name-only `WARN` with reason `invalid_env_budget_override` is logged and the derived budget is used. |

See the [environment variable reference](./environment-variables.md#amplihack_context_env_budget_bytes).

### Essential keys (always exported)

A small, known set of context keys is **always** mirrored into the environment,
regardless of the budget, because bash steps under `set -u` depend on them and
they are tiny:

- exact keys: `task_description`, `repo_path`, `existing_branch`
- prefix keys: any key beginning with `should_` (e.g. `should_create_pr`)

Essential keys are matched on the **original** (lowercased) context key, before
uppercasing. They are counted against the budget first. In the pathological case
where the essential set alone exceeds the budget, the essentials are **still
exported** and a loud `WARN` with reason `essential_env_exceeds_budget` is
emitted — a required variable is never silently dropped. In that case no
non-essential keys are mirrored.

> Essentials are exempt from the **budget** only. They are never exempt from the
> [per-key filters](#key-transformation-rules) or the
> [denylist](#reserved-name-denylist): a key that looks essential but is
> reserved, invalid, NUL-bearing, or oversized is still dropped.

### Smallest-first fill for non-essential keys

After essentials are admitted, the remaining budget is filled with the
non-essential pairs **smallest-first** by exported entry size
(`name.len() + value.len() + overhead`), with a stable tie-break by name for
determinism. This maximizes the number of useful variables that survive: many
small vars are kept in preference to one large var. Each non-essential key that
does not fit is dropped from the mirror with a name-only `WARN` whose reason is
`aggregate_env_budget`.

Every dropped value — essential-exceeds notwithstanding — remains available to
the recipe via the unchanged `--set` / `--context-file` delivery path, so
`{{placeholder}}` substitution is unaffected. **Only the environment mirror is
trimmed**, never the runner's view of the context. This is the same contract as
the per-value cap in [rule 5](#key-transformation-rules) (issue #784).

### Worked example

Given a budget of 4096 bytes and this context (post-filter, post-uppercase):

| Key | Exported size | Class | Admitted? |
|-----|---------------|-------|-----------|
| `task_description` | 40 B | essential | ✅ always |
| `repo_path` | 30 B | essential | ✅ always |
| `SMALL_NOTE` | 100 B | non-essential | ✅ (fits, smallest) |
| `MEDIUM_BLOB` | 2 KB | non-essential | ✅ (fits) |
| `HUGE_BLOB` | 80 KB | non-essential | ❌ `aggregate_env_budget` |

`HUGE_BLOB` is dropped from the environment but still substitutes into
`{{huge_blob}}` and is delivered to the runner via the context file.

---

## Precedence and no-regression guarantees

Context environment variables are applied at the **lowest** precedence. The
spawn seam writes them first, then layers the subprocess environment builder and
the correlation variable on top:

```
1. command.envs(context_env_pairs(context, budget))   // context, lowest priority
2. env_builder.apply_to_command(&mut command)         // AMPLIHACK_*, pager-safe, PATH/HOME fallbacks
3. command.env("AMPLIHACK_RECIPE_RUN_ID", …)          // correlation id, highest priority
```

Guarantees that follow from this ordering:

- **Builder-managed variables always win.** `AMPLIHACK_NONINTERACTIVE`,
  `AMPLIHACK_HOME`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_ASSET_RESOLVER`,
  `AMPLIHACK_GRAPH_DB_PATH`, pager-safe defaults, Python sanitization, and the
  `CLAUDECODE` removal cannot be overridden by recipe context.
- **Correlation is immutable.** `AMPLIHACK_RECIPE_RUN_ID` reflects the real run
  identity even if a context key tried to collide with it (it is also blocked by
  the `AMPLIHACK_` prefix rule).
- **`PATH`/`HOME` are never clobbered.** These are on the denylist, so a context
  key such as `path=/evil` is dropped rather than replacing the process `PATH`.
- **`--set` / `--context-file` placeholder delivery is unchanged.** The
  existing argv- and temp-file-based delivery used for `{{placeholder}}`
  substitution is untouched; environment export is additive.

---

## Nested and sub-recipe propagation

The CLI exports the context **once**, when it spawns the `recipe-runner-rs`
subprocess. `std::process::Command` starts that child with the parent's
environment plus the pairs added by `command.envs(...)`, and never clears it, so
the exported context is guaranteed to reach `recipe-runner-rs` itself. This first
hop — CLI → `recipe-runner-rs` — is owned by code in this repository and is
directly testable.

From there, propagation down to individual steps relies on `recipe-runner-rs`
(an external binary, typically installed in `~/.cargo/bin`) using default,
inheriting process spawning for its shell steps and nested sub-recipes — that is,
it does not clear or rewrite the environment before launching them. Under that
contract:

- the top-level recipe's bash steps see `$TASK_DESCRIPTION` / `$REPO_PATH`;
- a `type: recipe` sub-step's bash steps see them too;
- a `sh -c '…'` grandchild launched by a bash step still sees them.

Because this end-to-end path crosses an external binary, it is treated as a
**verified contract, not an unchecked assumption.** The nested `sh -c` canary in
the [propagation tutorial](../tutorials/recipe-context-env-propagation.md) — a
sub-recipe step whose bash command runs
`sh -c 'set -u; echo "$TASK_DESCRIPTION"'` — exercises the full chain
(CLI → `recipe-runner-rs` → bash step → grandchild shell) and must print the
value rather than abort with `unbound variable`. If a future `recipe-runner-rs`
cleared or rewrote the environment, that canary would fail first, surfacing the
regression at the seam where it occurs.

This is exactly the "parent context propagated to child" behavior that
multi-workstream campaigns require. Per-sub-recipe context *overrides* (the
step-level `context:` dict on a `type: recipe` step) are resolved inside the
recipe runner's `{{placeholder}}` layer and are out of scope for the CLI-level
environment export.

---

## Skip logging

When a context entry is skipped, the exporter emits a `WARN`-level
[trace event](./trace-logging-api.md) naming **only the key**, never the value,
so sensitive context never leaks into logs:

```
WARN recipe context key skipped for env export name=ISSUE TITLE reason=invalid_identifier
WARN recipe context key skipped for env export name=LD_PRELOAD reason=reserved_name
WARN recipe context key skipped for env export name=NOTES reason=value_contains_nul
WARN recipe context key skipped for env export name=HUGE_BLOB reason=aggregate_env_budget
```

> **Visibility (by design).** Skip notices are `WARN`-level `tracing` events
> emitted by the parent `amplihack` process — the same mechanism the rest of the
> recipe-run subsystem uses for diagnostics. The CLI initializes its subscriber
> with `EnvFilter::from_default_env()` and no default directive, so with
> `RUST_LOG` unset **only `ERROR` is shown** and skip notices are suppressed.
> This is a deliberate decision: a skipped key is advisory (the run still
> succeeds), so it is surfaced on demand with `RUST_LOG=warn amplihack recipe
> run …` (or `info`/`debug`) rather than printed on every run. When a skipped key
> later causes an `unbound variable` failure, the
> [troubleshooting guide](../howto/troubleshoot-recipe-execution.md) directs you
> to re-run with `RUST_LOG=warn` to see which key was dropped and why.
>
> **Field rendering.** The fields are recorded as `%`-display values
> (`name = %name`, `reason = %reason`), which produces the unquoted
> `name=…`/`reason=…` rendering shown above. This matches the existing
> recipe-run `tracing::warn!` style in
> `crates/amplihack-cli/src/commands/recipe/resolve.rs`; named (non-`%`) fields
> would render quoted instead. The live subscriber also prefixes a timestamp that
> these examples omit for brevity.

Skip reasons:

| Reason | Meaning |
|--------|---------|
| `invalid_identifier` | Uppercased name is empty, starts with a digit, or contains characters outside `[A-Z0-9_]` |
| `reserved_name` | Name is on the denylist or begins with `AMPLIHACK_` |
| `value_contains_nul` | Value contains a NUL byte and cannot be represented in the environment |
| `value_too_large` | Value exceeds the per-variable byte cap (kept below the kernel's `MAX_ARG_STRLEN` to avoid `E2BIG`); the value is still delivered via the recipe context file for `{{placeholder}}` substitution |
| `aggregate_env_budget` | A non-essential key did not fit within the [aggregate environment budget](#aggregate-environment-budget); the value is still delivered via the recipe context file |
| `essential_env_exceeds_budget` | The [essential keys](#essential-keys-always-exported) alone exceed the budget; they are exported anyway (loud warning) and no non-essential keys are mirrored |
| `invalid_env_budget_override` | `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES` was set to a non-numeric/negative/overflowing value; it is ignored and the derived budget is used |

Skips are never fatal. A recipe with one un-exportable key still runs; only that
single key is omitted from the environment (its `{{placeholder}}` form, if used,
continues to work).

---

## API: `context_env_pairs` and budget helpers

The transform is implemented as a pure, total function so it can be unit-tested
in isolation from process spawning. The budget arithmetic is factored into a
separate pure helper so it can be tested with **injected** values, independent of
the host's real `ARG_MAX`.

**Location:** `crates/amplihack-cli/src/commands/recipe/run/execute.rs`

```rust
/// Transform a recipe context map into the environment pairs to export to
/// `recipe-runner-rs` and its shell steps.
///
/// Each `(key, value)` becomes `(KEY, value)` where `KEY` is the ASCII-
/// uppercased key. Entries are skipped (with a name-only WARN) when the
/// uppercased name is not a valid shell identifier, is a reserved name, begins
/// with `AMPLIHACK_`, the value contains a NUL byte, or the value exceeds the
/// per-variable cap.
///
/// Surviving pairs are admitted against `budget` (total exported bytes):
/// essential keys are always exported; the remaining keys are filled
/// smallest-first until the budget is exhausted. Keys that do not fit are
/// dropped from the env mirror (name-only WARN, reason `aggregate_env_budget`)
/// but remain available via the recipe context file.
///
/// Total: invalid/over-budget entries are skipped, never fatal. Deterministic:
/// input is a sorted `BTreeMap` and fill order is smallest-first with a stable
/// name tie-break.
fn context_env_pairs(
    context: &BTreeMap<String, String>,
    budget: usize,
) -> Vec<(String, String)>;
```

| Property | Guarantee |
|----------|-----------|
| Totality | Never panics, never returns `Err`; invalid/over-budget entries are dropped |
| Determinism | Essentials first; non-essentials smallest-first with a stable name tie-break |
| Purity | No I/O except name-only `WARN` tracing for skipped keys |
| Budget safety | Total exported bytes never exceed `budget`, except when the essential set alone exceeds it (essentials always win, with a loud warning) |

### Budget helpers

```rust
/// Pure budget arithmetic. Deterministic and injectable for tests:
///   arg_max.saturating_sub(inherited_env_bytes + reservation)
/// Floors at 0 (saturating), so a huge inherited env yields "essentials only".
fn context_env_budget(
    arg_max: usize,
    inherited_env_bytes: usize,
    reservation: usize,
) -> usize;

/// Probe the kernel's ARG_MAX. On Unix reads `sysconf(_SC_ARG_MAX)`, guarding
/// `<= 0` and implausibly small (`< 64 KB`) results before the cast; otherwise
/// (and on non-Unix) returns the 128 KB conservative fallback.
fn query_arg_max() -> usize;

/// Sum `name.len() + value.len() + ENV_ENTRY_OVERHEAD_BYTES` over
/// `std::env::vars_os()` with saturating arithmetic.
fn inherited_env_bytes() -> usize;

/// Caller-facing orchestrator. Honors the `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES`
/// override (strict `usize` parse; invalid → name-only WARN + derived budget;
/// `0` is valid) and otherwise returns the derived budget from the helpers
/// above.
fn resolve_context_env_budget() -> usize;
```

Documented constants (all `SCREAMING_SNAKE`, cited to issue #1023):

```rust
/// Per-envp-entry overhead: `=` + trailing NUL + one 64-bit pointer slot + slack.
const ENV_ENTRY_OVERHEAD_BYTES: usize = 16;
/// Conservative ARG_MAX used when sysconf is unavailable or implausible.
const ARG_MAX_FALLBACK_BYTES: usize = 131_072; // 128 KB
/// sysconf results below this are treated as implausible → use the fallback.
const ARG_MAX_MIN_PLAUSIBLE_BYTES: usize = 65_536; // 64 KB
/// Flat reservation for argv, RECIPE_VAR_*/AMPLIHACK_* additions, and margin.
const ENV_BUDGET_RESERVATION_BYTES: usize = 131_072; // 128 KB
/// Operator override for the aggregate budget.
const CONTEXT_ENV_BUDGET_OVERRIDE_ENV: &str = "AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES";
```

The companion constant lists the reserved names:

```rust
/// Environment names that must never be set from recipe context because they
/// alter shell or dynamic-loader behavior, or are owned by the subprocess
/// environment builder.
const RESERVED_ENV_DENYLIST: &[&str];
```

The spawn seam consumes the helper at the lowest precedence. In
`execute_recipe_via_rust` (`execute.rs`), the call is inserted immediately after
`pass_context(&mut command, context)?` and **before**
`env_builder.apply_to_command(&mut command)`, so the builder and correlation id
layer on top:

```rust
// after: let _context_file = pass_context(&mut command, context)?;
let budget = resolve_context_env_budget();               // adaptive or override
command.envs(context_env_pairs(context, budget));        // context, lowest priority
// … then env_builder.apply_to_command(&mut command);   // AMPLIHACK_*, pager-safe, PATH/HOME
// … then command.env("AMPLIHACK_RECIPE_RUN_ID", correlation.run_id());
```

`EnvBuilder::apply_to_command` does **not** clear the environment — it only
`env_remove`s its explicitly unset keys and then `command.envs(...)` its own
pairs — so context variables added first survive except where a builder-managed
name (or the correlation id) intentionally overrides them.

---

## Examples

### Top-level recipe reading the environment under `set -u`

```yaml
name: env-aware-recipe
description: Reads context from the environment, not just {{placeholders}}
version: "1.0"

context:
  task_description: ""
  repo_path: "."

steps:
  - id: announce
    type: bash
    command: |
      set -euo pipefail
      echo "task: $TASK_DESCRIPTION"
      echo "repo: $REPO_PATH"
```

```bash
amplihack recipe run env-aware-recipe \
  -c task_description="Add validation for empty display names" \
  -c repo_path=.
```

Output:

```
task: Add validation for empty display names
repo: .
```

The same step could equivalently use `{{task_description}}` in the command text;
both forms now resolve to the same value.

### Nested sub-recipe inheriting the environment

`parent.yaml`:

```yaml
name: parent
context:
  task_description: ""
  repo_path: "."
steps:
  - id: call-child
    type: recipe
    recipe: child
```

`child.yaml`:

```yaml
name: child
steps:
  - id: read-inherited
    type: bash
    command: |
      set -euo pipefail
      # Even a grandchild shell sees the exported context.
      sh -c 'set -u; echo "child sees: $TASK_DESCRIPTION at $REPO_PATH"'
```

```bash
amplihack recipe run parent \
  -c task_description="Ship the fix" \
  -c repo_path=/work/repo
```

Output:

```
child sees: Ship the fix at /work/repo
```

### A skipped key

The skip notice is a `WARN`-level event, so run with `RUST_LOG=warn` to see it
(it is suppressed under the default error-only filter):

```bash
RUST_LOG=warn amplihack recipe run env-aware-recipe \
  -c task_description="ok" \
  -c "issue title=has spaces" \
  -c repo_path=.
```

`issue title` uppercases to `ISSUE TITLE`, which is not a valid identifier, so it
is skipped:

```
WARN recipe context key skipped for env export name=ISSUE TITLE reason=invalid_identifier
```

The recipe still runs; `TASK_DESCRIPTION` and `REPO_PATH` are exported normally.

### A key dropped by the aggregate budget

A large but otherwise valid value is admitted to the runner (for
`{{placeholder}}` substitution) but may be trimmed from the *environment mirror*
when the aggregate budget is tight. Force a tiny budget with the operator
override to observe it:

```bash
RUST_LOG=warn AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES=256 \
amplihack recipe run env-aware-recipe \
  -c task_description="ok" \
  -c repo_path=. \
  -c huge_blob="$(head -c 20000 /dev/zero | tr '\0' x)"
```

`task_description` and `repo_path` are essential and always exported;
`huge_blob` does not fit the 256-byte budget, so it is dropped from the mirror:

```
WARN recipe context key skipped for env export name=HUGE_BLOB reason=aggregate_env_budget
```

`{{huge_blob}}` still substitutes correctly because the value is delivered via
the context file. To mirror everything regardless of the derived budget, raise
the override (or unset it to use the adaptive default).

---

## Configuration

Context environment export has **no recipe-level flags** and is always active and
additive. The one operator-facing knob is the aggregate-budget override:

| Setting | Effect |
|---------|--------|
| `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES` (unset) | Budget is [derived adaptively](#aggregate-environment-budget) from `ARG_MAX` and the inherited environment. This is the default and the recommended mode. |
| `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES=<usize>` | Pins the aggregate mirror budget to exactly that many bytes; `0` means "essentials only". Takes precedence over the derived budget. |

Additional notes:

- There is no opt-out of export itself. Recipes that do not read environment
  variables are unaffected because the extra variables are simply present and
  unused.
- The set of exported variables is determined by the merged recipe context. To
  change *what* is exported, change the context (`context:` block or
  `-c/--context` flags); the budget only controls *how many* survive the mirror
  when space is tight.
- The reserved-name denylist and the essential-key set are fixed in code; they
  are not user-configurable, by design, because they are safety/security
  controls.

Large context maps continue to use the existing `--context-file` spill path for
`{{placeholder}}` delivery (argv size protection); every value the mirror drops
is still delivered there. The aggregate budget guarantees the mirror never pushes
the inherited environment past `ARG_MAX`, so late shell steps no longer fail with
`Argument list too long (os error 7)`.

---

## Security model

The trust boundary is: untrusted content (issue bodies, task descriptions,
third-party recipes) → environment name and value → inherited by every nested
shell step via the process environment.

| Control | Description |
|---------|-------------|
| V1 — Input validation (allowlist) | Names must match `^[A-Z_][A-Z0-9_]*$`; values must not contain NUL. |
| V2 — Reserved denylist (primary) | Loader, shell-startup, `IFS`, path/identity, interpreter-option, and `AMPLIHACK_`-prefixed names are never exported. |
| V3 — Bounded budget arithmetic | `ARG_MAX` is probed with guarded casts (`≤ 0` and `< 64 KB` rejected); all size sums use saturating arithmetic, so a crafted large value cannot wrap the budget into an over-export. |
| V4 — Filters precede budget | All five per-key filters run **before** budget classification. A denylisted, invalid, NUL-bearing, or oversized key can never re-enter the mirror through the essentials or budget path — even at `budget = usize::MAX` and even if renamed to look essential. |
| G2 — Precedence as a control | Context is applied first (lowest priority); the environment builder and correlation id are applied after, so context can never override security-relevant builder configuration. |
| Name-only logging | Skip warnings (including `aggregate_env_budget`, `essential_env_exceeds_budget`, and `invalid_env_budget_override`) log the key name and a static reason only — never the value or its length — to avoid leaking sensitive context. |

Note on threat shape: at the spawn seam, names and values are passed via
`Command::env`, which performs **no** shell evaluation. The residual risk is
therefore *name clobbering* (replacing a meaningful variable), not value
injection — and clobbering of dangerous names is exactly what the denylist
prevents.

Defense-in-depth note: prefixing every exported name (for example
`AMPLIHACK_CTX_TASK_DESCRIPTION`) would eliminate the entire name-clobber class.
Bare names are retained for ergonomics (`$TASK_DESCRIPTION` is what recipe
authors expect) and are safe **only** while the denylist remains exhaustive and
fully test-covered.

---

## resolve-bundle-asset availability

`smart-orchestrator`'s preflight requires the `amplihack resolve-bundle-asset`
subcommand. That subcommand is implemented and wired in the Rust CLI:

| Surface | Location |
|---------|----------|
| Subcommand definition | `crates/amplihack-cli/src/cli_commands.rs` (`ResolveBundleAsset`, `#[command(name = "resolve-bundle-asset")]`) |
| Dispatch | `crates/amplihack-cli/src/commands/mod.rs` (`Commands::ResolveBundleAsset`) |
| Implementation | `crates/amplihack-cli/src/resolve_bundle_asset/` |
| Standalone binary | `bins/amplihack-asset-resolver/` (same single-argument interface) |

A `cargo build -p amplihack-cli` therefore exposes `resolve-bundle-asset`.
Reports of a "missing" subcommand trace to a **stale installed Python
`amplihack`** earlier on `PATH`, not to the Rust binary. Confirm which binary is
in use:

```bash
command -v amplihack
amplihack resolve-bundle-asset --help
```

If the resolved binary is the legacy Python entry point, reinstall or reorder
`PATH` so the Rust `amplihack` is selected. No code change in this area is
required to unblock multi-workstream execution; this section exists to document
the verification. See
[resolve-bundle-asset command reference](./resolve-bundle-asset-command.md).

---

## Related

- [Recipe Executor Environment](./recipe-executor-environment.md) — Subprocess launch, shell-step and agent-step environment injection
- [Environment Variables](./environment-variables.md) — All variables read or injected by amplihack, including context-derived names
- [Tutorial: Propagate Recipe Context to Bash Steps](../tutorials/recipe-context-env-propagation.md) — Hands-on walkthrough for top-level, nested, and skipped keys
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Diagnosing `TASK_DESCRIPTION: unbound variable` and related failures
- [resolve-bundle-asset Command Reference](./resolve-bundle-asset-command.md) — Native bundle asset resolver
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) — `AMPLIHACK_RECIPE_RUN_ID` and pointer events
