---
title: Effort and Model Inheritance for Recipe Steps
description: How agent steps started by `amplihack recipe run` inherit the parent session's effort and model, and how a step overrides them.
issue: 1883
related: [1421, 1527, 1872]
last_updated: 2026-11-12
---

# Effort and Model Inheritance for Recipe Steps

Every agent step started by `amplihack recipe run` uses the effort and model of
the session that started the run, unless the step sets its own. This applies to
`smart-orchestrator`, `default-workflow` and every sub-recipe they call.

## Contents

- [Quick start](#quick-start)
- [How it works](#how-it-works)
- [Where the run's values come from](#where-the-runs-values-come-from)
- [Precedence inside each step](#precedence-inside-each-step)
- [Support for each agent binary](#support-for-each-agent-binary)
- [Variables](#variables)
- [Log lines](#log-lines)
- [Value checks](#value-checks)
- [Changes in behaviour](#changes-in-behaviour)
- [Limitations](#limitations)
- [Relation to per-step roles (#1872)](#relation-to-per-step-roles-1872)
- [Design labels](#design-labels)

## Quick start

Start a session with an effort and a model, then run a recipe from it:

```sh
amplihack claude -- --effort high --model claude-opus-5-5
# inside that session, an agent runs:
amplihack recipe run default-workflow -c task_description="Fix issue #1883"
```

stderr shows one line for the run:

```text
amplihack: steps inherit effort=high model=claude-opus-5-5 (from the parent amplihack launch)
```

Each `claude` step is then started with:

```text
claude ... --model claude-opus-5-5 --effort high ...
```

and its environment holds `ANTHROPIC_MODEL=claude-opus-5-5` and
`CLAUDE_CODE_EFFORT_LEVEL=high`.

To set the values for one run without changing the session, pass them as
context:

```sh
amplihack recipe run default-workflow -c effort=medium -c model=claude-sonnet-4-5 \
  -c task_description="Update the changelog"
```

stderr then shows:

```text
amplihack: steps inherit effort=medium model=claude-sonnet-4-5 (from the recipe context)
```

## How it works

1. `amplihack recipe run` works out the effort and model **once per run**
   (see [Where the run's values come from](#where-the-runs-values-come-from)).
2. It checks each value against the run's agent binary
   (`AMPLIHACK_AGENT_BINARY`) and drops what that binary cannot take, with one
   log line per dropped setting.
3. It sets `AMPLIHACK_PARENT_EFFORT` and `AMPLIHACK_PARENT_MODEL` in the recipe
   runner's environment, each with `AMPLIHACK_PARENT_EFFORT_BINARY` or
   `AMPLIHACK_PARENT_MODEL_BINARY` naming the run's agent binary, the binary
   the value was checked for. A variable is set only when it has a value. All
   four are removed first, so a stale value from the caller's environment
   never reaches the runner.
4. The runner passes its environment to every step unchanged. Each step's
   `amplihack <binary> -- …` launch reads only these variables and the step's
   own arguments. Steps do not read `/proc` or the fallback variables.
5. Each launch exports `AMPLIHACK_PARENT_EFFORT` and `AMPLIHACK_PARENT_MODEL`
   again for the agent it starts, each with its `_BINARY` variable naming the
   launch's own binary. The effort is the one the agent runs with:
   the step's own effort if given, otherwise the inherited effort when the
   binary takes it. The model is the one the launch was given or inherited, and is left
   unset when amplihack chose the model itself; see the paragraph below the
   [variables table](#variables). A nested `amplihack recipe run` inside a step
   therefore inherits from that step, not from the top-level session.

With neither variable set and no other source, argv, environment and stderr are
byte-identical to a run without this feature. In particular, a launch with its
own `--effort` or `--model` and nothing inherited gains no
`CLAUDE_CODE_EFFORT_LEVEL` or `ANTHROPIC_MODEL`; it gains only the
`AMPLIHACK_PARENT_*` exports from step 5.

## Where the run's values come from

`amplihack recipe run` takes the first source in each list that has a valid
value.

**Effort**

| Order | Source | Label in log lines |
|---|---|---|
| 1 | `-c effort=<level>`, or `effort` in `--context-file` | `the recipe context` |
| 2 | `AMPLIHACK_PARENT_EFFORT` | `the parent amplihack launch` |
| 3 | `--effort` on the nearest `claude` ancestor process (Linux only) | `the parent claude process` |
| 4 | `CLAUDE_CODE_EFFORT_LEVEL` | `CLAUDE_CODE_EFFORT_LEVEL` |

**Model**

| Order | Source | Label in log lines |
|---|---|---|
| 1 | `-c model=<id>`, or `model` in `--context-file` | `the recipe context` |
| 2 | `AMPLIHACK_PARENT_MODEL` | `the parent amplihack launch` |
| 3 | `--model` on the nearest `claude` ancestor process (Linux only, see [Limitations](#limitations)) | `the parent claude process` |
| 4 | `ANTHROPIC_MODEL` | `ANTHROPIC_MODEL` |

There is no `amplihack config set` key for either setting.

The ancestor lookup in row 3 reads the argv of the closest process named
`claude` above `amplihack recipe run`, following only processes owned by the
same user. It is skipped silently when `/proc` is missing or unreadable, and on
platforms other than Linux. It accepts both `--effort high` and
`--effort=high`; when a flag appears more than once, the last one wins, and
nothing after `--` is read.

`AMPLIHACK_PARENT_EFFORT`, `AMPLIHACK_PARENT_EFFORT_BINARY`,
`AMPLIHACK_PARENT_MODEL` and `AMPLIHACK_PARENT_MODEL_BINARY` cannot be set
through `-c`, a context file or the `context:` of the recipe the run starts, in
any letter case: a context key starting with `AMPLIHACK_` there makes the run
refuse to start. A sub-recipe's
`context:` and a `type: recipe` step's `context:` are not checked; see
[Limitations](#limitations).

## Precedence inside each step

When a step's launch builds its argv, the model is chosen in this order,
highest first:

1. The step's own `--model` in its arguments. This includes the runner's
   `--model auto` retry, for that attempt only. `auto` is the runner's
   final-attempt sentinel, not a model choice: it is never put in
   `ANTHROPIC_MODEL`, and the step's children are passed the inherited model
   (or nothing, when none is inherited) as `AMPLIHACK_PARENT_MODEL`. A
   `recipe run` nested under a retried step therefore keeps the run's model
   and is not pinned to `auto`.
2. *Reserved for the role model from #1872. Not used yet.*
3. The LiteLLM gateway model (`AMPLIHACK_LITELLM_MODEL`), chosen as described
   under
   [AMPLIHACK_DEFAULT_MODEL](./environment-variables.md#amplihack_default_model).
4. The inherited model, from `AMPLIHACK_PARENT_MODEL`.
5. `AMPLIHACK_DEFAULT_MODEL`.
6. The built-in default, `claude-opus-5[1m]`.

Effort has two tiers: the step's own `--effort` wins; otherwise the inherited
effort is added. The inherited flags go before the step's own arguments and
before any `resume` subcommand.

Example: the run inherits `effort=high model=claude-opus-5-5`, and one step
passes `--model claude-haiku-4-5`. That step starts with:

```text
claude ... --effort high ... --model claude-haiku-4-5
```

and its environment holds `ANTHROPIC_MODEL=claude-haiku-4-5` and
`CLAUDE_CODE_EFFORT_LEVEL=high`. The child never holds both the step's model and
the parent's.

## Support for each agent binary

| Binary | Effort | Model |
|---|---|---|
| `claude` | `--effort L`, and `CLAUDE_CODE_EFFORT_LEVEL=L` when an effort is inherited | `--model M`, and `ANTHROPIC_MODEL=M` when a model is inherited |
| `rusty`, `rustyclawd`, `amplifier` | None; an inherited effort is dropped with a log line | `--model M`, and `ANTHROPIC_MODEL=M` when a model is inherited |
| `codex` | `-c model_reasoning_effort=L` when L is `low`, `medium` or `high`; any other value is dropped with a log line | A model selected in the recipe context or exported by a parent Codex launch, and never while a LiteLLM gateway variable is set. Models selected for another binary are dropped with a log line. |
| `copilot` | `--reasoning-effort L`, only for an effort chosen for copilot: an `effort` in the context of a copilot run, or one exported by a parent Copilot launch. An effort chosen for another binary, which includes every effort from a Claude session (its launch, the `claude` ancestor, `CLAUDE_CODE_EFFORT_LEVEL`), is dropped with a log line. L must also be `low`, `medium`, `high`, `xhigh` or `max`; any other value, `none` and `minimal` included, is dropped with a log line. See the notes | A model selected in the recipe context or exported by a parent Copilot launch, and never while a LiteLLM gateway variable is set. Models selected for another binary are dropped with a log line. |

Notes:

- `rusty`, `rustyclawd` and `amplifier` receive no inherited effort. None of
  them lists an `--effort` option, and an unknown flag would make every step
  fail. No reader of `CLAUDE_CODE_EFFORT_LEVEL` was found in their sources
  either, so amplihack does not set it for them: the run prints the
  "cannot be passed" line and its summary shows `effort=unset`, as for any
  binary that takes no effort. A step's own `--effort` is still passed to them
  unchanged, and passed on to their children. They still take the inherited
  model. `amplifier` sets its reasoning effort in its own provider
  configuration (`reasoning_effort`), not from the environment.
- For `codex`, the `-c` value is built from the matched literal `low`, `medium`
  or `high`, so a value such as `high,foo=bar` is rejected rather than passed.
- For `copilot`, the `--reasoning-effort` value is built from a matched
  literal in the same way. Copilot refuses to start, with exit status 1, when
  the level is not one its model supports (`copilot --help`: "supported levels
  depend on the model"). So an effort chosen for another binary is never
  passed to copilot: a Claude session's `max` says nothing about the model
  copilot runs, in the same way a Claude model id says nothing about copilot's
  models. Copilot takes an inherited effort only when
  `AMPLIHACK_PARENT_EFFORT_BINARY` names `copilot`: the effort came from the
  context of a copilot run, or from a copilot launch, which ran with it. Every
  other effort is dropped with the "cannot be passed" line and copilot uses
  its own default. On top of that, with copilot 1.0.109 and its default model,
  `claude-sonnet-5.5`, `none` and `minimal` are refused
  (`Error: Reasoning effort "none" is not supported for model "claude-sonnet-5.5".`),
  and `low`, `medium`, `high`, `xhigh` and `max` are accepted, so amplihack
  passes only those five. To give a copilot run an effort, pass
  `-c effort=<level>` to it, or give a step its own `--reasoning-effort`.
- Quoted Codex TOML overrides are decoded before export: both
  `-c 'model_reasoning_effort="low"'` and
  `-c "model_reasoning_effort='low'"` export `low`, so subsequent Codex
  launches inherit the effort without TOML quotes or escapes, and Claude
  launches receive `--effort low`. Whitespace
  around `=` is accepted too: `-c 'model_reasoning_effort = "high"'`
  exports `high` to nested recipe runs. Attached short options such as
  `-cmodel_reasoning_effort=low` and `-c='model_reasoning_effort="low"'`
  also export `low` and suppress an inherited effort override.
- A Codex step can select its own model with `-m <id>`, `-m=<id>`,
  `-m<id>`, `--model <id>` or `--model=<id>`. Each suppresses inherited
  model arguments and exports the selected model for nested runs.
- Values consumed by other options are not inheritance flags. For example,
  `--system-prompt '--model=haiku'` leaves an inherited `sonnet` selected
  and exports `sonnet`; the prompt is forwarded unchanged. `--` stops
  inheritance option parsing.
- A step's own effort wins: `--effort` for Claude-compatible binaries,
  `-c model_reasoning_effort=L` for `codex` and `--reasoning-effort` for
  `copilot`.
- A binary amplihack does not know receives no effort, and a model only on the
  same terms as `codex` and `copilot`.
- The model is checked against the binary each launch starts, not only the
  run's binary. A run's `AMPLIHACK_PARENT_MODEL` reaches every process under
  the run, including a bash step that starts another binary, such as the
  `amplihack copilot -p …` steps in `quality-loop.yaml`. Such a launch drops
  the model with one "cannot be passed" line when its provenance names a
  different binary (`codex` and `copilot` need the same binary;
  Claude-compatible binaries need a Claude-compatible origin, or none).
  `AMPLIHACK_PARENT_MODEL_BINARY` records the model's origin; older recipe
  runs use `AMPLIHACK_AGENT_BINARY` as a compatibility fallback. The effort
  is checked the same way for `copilot`, against
  `AMPLIHACK_PARENT_EFFORT_BINARY`, which has no fallback: under a Claude
  run, the `amplihack copilot -p …` steps in `quality-loop.yaml` start
  without `--reasoning-effort` and print one "cannot be passed" line for the
  effort. `claude` and `codex` launches take the effort through the checks
  above, whatever its origin.
- Only `claude` receives an added `CLAUDE_CODE_EFFORT_LEVEL`. `codex` and
  `copilot` receive no added `CLAUDE_CODE_EFFORT_LEVEL` or
  `ANTHROPIC_MODEL`. They still see `AMPLIHACK_PARENT_EFFORT` and
  `AMPLIHACK_PARENT_MODEL`, because the runner passes its environment to every
  step.

## Variables

| Variable | Set by | Read by |
|---|---|---|
| `AMPLIHACK_PARENT_EFFORT` | Each `amplihack <binary>` launch (the effort its agent runs with) and `amplihack recipe run` (for the runner) | `amplihack recipe run`, and each step's launch |
| `AMPLIHACK_PARENT_MODEL` | Each `amplihack <binary>` launch (see below) and `amplihack recipe run` (for the runner) | `amplihack recipe run`, and each step's launch |
| `AMPLIHACK_PARENT_EFFORT_BINARY` | Each launch or recipe run, alongside its exported effort (the launch's binary, or the run's agent binary); removed when no effort is exported | Copilot's effort check in nested launches and recipe runs |
| `AMPLIHACK_PARENT_MODEL_BINARY` | Each launch or recipe run, alongside its exported model; removed when no model is exported | Model compatibility checks in nested launches and recipe runs |
| `CLAUDE_CODE_EFFORT_LEVEL` | Each `claude` step, only when an effort is inherited. It holds the step's own `--effort` if given, otherwise the inherited effort. | The agent; and `amplihack recipe run` as the last effort fallback |
| `ANTHROPIC_MODEL` | Each Claude-compatible step, only when a model is inherited. It holds the step's own `--model` if given, otherwise the inherited model. | The agent; and `amplihack recipe run` as the last model fallback |

A launch with nothing inherited sets neither `CLAUDE_CODE_EFFORT_LEVEL` nor
`ANTHROPIC_MODEL`, even when it has its own `--effort` or `--model`.

Codex and Copilot export explicit and inherited models with their binary name
in `AMPLIHACK_PARENT_MODEL_BINARY`. A subsequent same-binary launch or recipe
run inherits that choice, including a model first selected from recipe context.
A step's explicit `--model` replaces the parent choice and provenance for its
children. Models are never matched by guessing their vendor from their spelling.

A launch sets `AMPLIHACK_PARENT_MODEL` only when the model came from the
command line or was inherited. It is not set when amplihack chose the model
itself (`AMPLIHACK_DEFAULT_MODEL`, the built-in default or the gateway model),
so amplihack's own default never takes the place of a real choice further down.

None of these variables is ever set to an empty string. A launch sets them after
the launch environment is built and are not managed by `EnvBuilder`. The
gateway's `ANTHROPIC_MODEL` is applied last, so it still wins under the
gateway.

## Log lines

All lines go to stderr. Values in angle brackets are filled in.

**Summary, once per run**, printed by `amplihack recipe run` when the steps
receive at least one of effort and model from any source in the
[source tables](#where-the-runs-values-come-from), including `-c effort=` or
`-c model=` alone. It names only values the run's agent binary can take: a
value refused by a "cannot be passed" line, or replaced by the LiteLLM gateway
model, shows as `unset`, and when every value is refused the summary is not
printed:

```text
amplihack: steps inherit effort=<level|unset> model=<id|unset> (from <source>)
```

`<source>` is one label from the [source tables](#where-the-runs-values-come-from)
when both values share it; otherwise it reads
`<effort source> and <model source>`, with only the labels of values that are
set. Example:

```text
amplihack: steps inherit effort=high model=unset (from the parent claude process)
```

With only `-c effort=low` given and no other source:

```text
amplihack: steps inherit effort=low model=unset (from the recipe context)
```

With `-c model=claude-sonnet-4-5` and an effort of `high` from
`AMPLIHACK_PARENT_EFFORT`:

```text
amplihack: steps inherit effort=high model=claude-sonnet-4-5 (from the parent amplihack launch and the recipe context)
```

**Setting that cannot be passed, at most once per setting per run:**

```text
amplihack: <effort|model> '<value>' cannot be passed to <binary>; steps use <binary>'s default
```

Example, two or more `codex` steps under one run with effort `xhigh` print
this once in total:

```text
amplihack: effort 'xhigh' cannot be passed to codex; steps use codex's default
```

**Model replaced by the LiteLLM gateway, once per run.** While a LiteLLM
gateway variable is set, Claude-compatible steps run the gateway model, so an
inherited model reaches no step. It is dropped with this line, is not exported
to the steps, and the summary shows `model=unset`:

```text
amplihack: model '<id>' is not passed to <binary>; steps use the LiteLLM gateway model
```

`amplihack recipe run` prints these lines next to the agent-binary notice. Each
step's launch prints nothing for them while `AMPLIHACK_RECIPE_RUN_ID` is set.
Outside a recipe run, a launch prints the line once per process, for an
inherited effort and for an inherited model that `codex` or `copilot` cannot
take. Inside a recipe run, a launch of a binary other than the run's prints
the effort and model lines itself, because the run's lines named only the
run's binary. Under a Claude run with effort `max`, an `amplihack codex`
launch from a bash step prints
`amplihack: effort 'max' cannot be passed to codex; steps use codex's default`.
For example, under a Claude run with effort `max` and model
`claude-opus-4-7`, each `amplihack copilot` launch from a bash step prints:

```text
amplihack: effort 'max' cannot be passed to copilot; steps use copilot's default
amplihack: model 'claude-opus-4-7' cannot be passed to copilot; steps use copilot's default
```

**Rejected value, once per source:**

```text
amplihack: ignoring <effort|model> from <source>: not a valid option value
```

The rejected value is not printed.

**Model notice inside a step.** The #1421 notice
(`amplihack: passing --model … (from …)`) is unchanged, word for word, when
nothing is inherited. When a step's model is inherited, the step prints no
model notice under a recipe run; the run's summary line covers it. Outside a
recipe run, an inherited model gives:

```text
amplihack: passing `--model <id>` to `<binary>` (from the parent session). Pass `--model` to override it.
```

## Value checks

Every value, from any source, is trimmed and then checked before use:

- 1 to 256 bytes long
- does not start with `-`
- contains no control characters, zero-width characters (U+200B to U+200F,
  U+FEFF) or bidirectional controls (U+202A to U+202E, U+2066 to U+2069)

A value that fails is treated as unset and the next source is tried, with one
"ignoring" line. Values that pass are used unchanged, with two exceptions:
the `codex` and `copilot` effort allowlists in the table above. `codex` takes
only `low`, `medium` or `high`, and `copilot` takes only `low`, `medium`,
`high`, `xhigh` or `max`; each value is rebuilt from a matched literal, and any
other effort is dropped with a log line. There is no allowlist of models.
Values are always passed as separate argv elements, never through a shell.
The one place a value is written into shell text is the tmux hand-off; see
[Limitations](#limitations).

No value amplihack infers can stop a launch or a run. A value that fails a
check, or that the binary cannot take, is dropped with a log line; the worst
case is "nothing inherited", and the operator's own flags still apply. This
includes copilot: an effort it was not chosen for is dropped rather than
passed (see [Support for each agent binary](#support-for-each-agent-binary)).

Two things that someone named explicitly can still stop one:

- A context key in the `AMPLIHACK_` namespace makes `recipe run` refuse to
  start (see [Where the run's values come from](#where-the-runs-values-come-from)).
- An effort chosen for copilot is passed as it stands. Copilot refuses to
  start if the model it runs does not support that level: `-c effort=` on a
  copilot run whose model does not take it, or an effort inherited from a
  copilot launch by a step that names a different `--model` of its own.
  Copilot would refuse the same `--reasoning-effort` typed by hand.

## Changes in behaviour

- **Inside recipe runs**, an inherited model now wins over
  `AMPLIHACK_DEFAULT_MODEL` and the built-in default. The LiteLLM gateway model
  still wins over the inherited one.
- **A top-level `amplihack claude`** with only `ANTHROPIC_MODEL` set behaves as
  before. `ANTHROPIC_MODEL` and `CLAUDE_CODE_EFFORT_LEVEL` are fallbacks only
  while `amplihack recipe run` works out the run's values.
- **A nested `amplihack <binary>` launch** inside an amplihack session (for
  example an agent running `amplihack claude -p …` from its shell) sees
  `AMPLIHACK_PARENT_MODEL` and `AMPLIHACK_PARENT_EFFORT` and inherits them.
  Pass `--model` or `--effort` to that launch to choose differently.

## Limitations

- **One agent binary per run.** `amplihack recipe run` checks its values for
  the run's `AMPLIHACK_AGENT_BINARY`, and its log lines name that binary only.
  A step that starts a different binary checks the values again for itself:
  its effort through the checks in
  [Support for each agent binary](#support-for-each-agent-binary) (for
  `copilot`, by refusing one whose provenance is not copilot), and its model
  by refusing one whose provenance names an incompatible binary. That launch
  prints its own "cannot be passed" lines for the effort and the model.
- **Sessions started by an older amplihack.** Such a session does not export
  `AMPLIHACK_PARENT_*`. Its effort is still found through the ancestor lookup.
  Its model is not: when `AMPLIHACK_SESSION_ID` is set, the ancestor's
  `--model` may be amplihack's own default rather than the user's choice, so
  the ancestor model is ignored. Restart the session with a current amplihack,
  or pass `-c model=<id>` to the run.
- **Plain `claude` sessions** (not started by amplihack) pass both effort and
  model through the ancestor lookup on Linux. On other platforms, set
  `CLAUDE_CODE_EFFORT_LEVEL` and `ANTHROPIC_MODEL`, or pass `-c effort=` and
  `-c model=`.
- **No per-step effort field** in the recipe schema. A step sets its own effort
  by passing `--effort` in its agent arguments.
- **Reserved context keys: the top-level recipe only.** `amplihack recipe run`
  refuses to start when a key in the `AMPLIHACK_` namespace, in any letter
  case, is given with `-c`, in a context file, or in the `context:` of the
  recipe it runs. This includes `amplihack_parent_effort`,
  `AMPLIHACK_PARENT_EFFORT_BINARY`, `AMPLIHACK_PARENT_MODEL` and
  `AMPLIHACK_PARENT_MODEL_BINARY`. `recipe run` reads the recipe once, with
  the parser `amplihack recipe validate` uses (YAML, or JSON with
  surrogate-pair escapes; see
  [Recipe text encoding](./recipe-cli-reference.md#recipe-text-encoding)),
  merges its `context:` with `-c` and the context file, and checks that one
  map before any work: before the runner is looked up or started. When the
  map is handed to the runner it is checked again, as before #1883, on every
  run; a `--dry-run` also starts the runner, so it was refused before #1883
  too, only later. It does not reach sub-recipes.
  The runner loads those itself, in the same process, and exports their
  `context:` keys, and a `type: recipe` step's own `context:`, in the same
  way. With recipe-runner 0.6.0, a sub-recipe holding
  `amplihack_parent_model: <id>` gives its bash steps
  `AMPLIHACK_PARENT_MODEL=<id>`. Refusing that namespace there is the
  runner's job (rysweet/amplihack-recipe-runner#146). Until then, a sub-recipe
  can choose the model and effort of the launches its own bash steps start.
  Each such launch still applies the [value checks](#value-checks), and the
  run's summary line does not report the sub-recipe's values. A recipe can
  already do the same by putting `--model` or `--effort` on its own step
  commands. Use `effort` and `model` to select the documented recipe context
  values.
- **tmux sessions.** `tmux new-session` gives its command the tmux server's
  global environment, which tmux copied from whatever process started the
  server, not the caller's environment (tmux(1), GLOBAL AND SESSION
  ENVIRONMENT). A server first started inside an amplihack session keeps that
  session's `AMPLIHACK_PARENT_EFFORT`, `AMPLIHACK_PARENT_EFFORT_BINARY`,
  `AMPLIHACK_PARENT_MODEL`, `AMPLIHACK_PARENT_MODEL_BINARY`,
  `CLAUDE_CODE_EFFORT_LEVEL` and `ANTHROPIC_MODEL`. A `recipe run` started in a later session of that server
  would rank them above everything but its own context, so every step would
  run the server starter's model at its effort, and the summary line would
  name "the parent amplihack launch". Start such a run with
  `tmux new-session -d "$(amplihack agent-binary --shell) amplihack recipe run …"`
  ([the hand-off](./active-agent-binary.md#handing-the-binary-to-a-detached-launch)).
  The hand-off removes all six variables and then sets the caller's own value
  of each one the caller holds, so the run inherits what it would inherit if
  started from the caller with `setsid`, or nothing. It sets a value only when
  every character is a letter, a digit or one of `- _ . : / @ % + ,`, because
  the line is read by a shell. A model such as `claude-opus-5[1m]` holds
  characters a shell would expand, so it is removed and not set, and
  `agent-binary` prints on the caller's terminal:

  ```text
  amplihack: not handed on: AMPLIHACK_PARENT_MODEL, ANTHROPIC_MODEL. Only a valid option value made of letters, digits and - _ . : / @ % + , can cross the hand-off, so a recipe run in the new session does not inherit this effort or model. Pass -c effort=<level> or -c model=<id> to that run, in single quotes inside the double-quoted tmux command.
  ```

  `amplihack agent`'s resumed pane removes the same six variables before it
  sets the agent's recorded ones, and `amplihack recipe run --detach` starts a
  tmux server without them; the detached run itself still gets the caller's
  values. A hand-written `tmux new-session` without the hand-off still gets
  the server's values.
- **Values from a recipe's own context.** A recipe can set `effort` or
  `model` in its context, and its steps and nested runs then use them. This
  mainly affects cost. A recipe could already do the same by putting
  `--model` or `--effort` on its own step commands, and the summary line names
  `the recipe context` as the source.

## Relation to per-step roles (#1872)

#1872 adds a model for each step's role (planner, executor). Its role model
will take slot 2 in [Precedence inside each step](#precedence-inside-each-step):
below the step's own `--model`, above the gateway model and the inherited
model. Until then, slot 2 is empty and has no effect.

## Design labels

Code comments and tests for #1883 cite these labels.

| Label | Meaning |
|---|---|
| D1 | A step's launch reads `AMPLIHACK_PARENT_EFFORT`, `AMPLIHACK_PARENT_MODEL`, their binary provenance and its own arguments; never `/proc` or the fallback variables. |
| D2 | The `claude` ancestor lookup is a fallback only, follows same-user processes, and is skipped when the values are already known. |
| D3 | The source order for each setting, and the source labels in log lines. |
| D4 | What a launch adds for an inherited effort. |
| D5 | The step's own `--effort` or `--model` wins; the child holds only that value. |
| D6 | What each agent binary can receive ([Support for each agent binary](#support-for-each-agent-binary)), checked again by a launch whose binary is not the run's. Copilot takes only an effort chosen for copilot. |
| D7 | The "cannot be passed" line: once per run, or once per process outside a run. |
| D8 | The once-per-run summary line; no per-step model notice under a run. |
| D9 | An invalid or empty value is treated as unset, with one "ignoring" line. |
| D10 | A top-level launch with only `ANTHROPIC_MODEL` set behaves as before. |
| S3 | Control, zero-width and bidirectional characters are refused. |
| S4 | A value is 1 to 256 bytes and does not start with `-`. |
| S5 | The codex `-c` value is built from a fixed word, never from the input. |
| S6 | `AMPLIHACK_PARENT_*` cannot be set from `-c`, a context file or the top-level recipe's `context:`; sub-recipe context is the runner's (#146). |
| S7 | Nothing after `--` is read as a flag. |
| S8 | An ancestor's command line is never logged. |
| S9 | A rejected value is never printed. |
| C1 | Each launch exports the effort and model its agent runs with, for its children. |
| C2 | An ancestor's `--model` is not trusted under an older amplihack session. |
| C3 | The recipe runner removes and sets `AMPLIHACK_PARENT_*` for every step. |
| C4 | With nothing inherited, a launch adds no `CLAUDE_CODE_EFFORT_LEVEL` or `ANTHROPIC_MODEL`. |
| I1 | None of the variables is ever set to an empty or invalid value. |
| I2 | Both parent variables are removed before being set, so a stale value is never passed on. |
| F3 | With nothing inherited, argv, environment and stderr match a run without this feature. |

## See also

- [AMPLIHACK_DEFAULT_MODEL and the model notice](./environment-variables.md#amplihack_default_model)
- [Active agent binary](./active-agent-binary.md), which decides the run's binary
- [Launch Target Resolution](../LAUNCH_TARGET_RESOLUTION.md)
