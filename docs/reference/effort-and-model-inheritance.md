---
title: Effort and Model Inheritance for Recipe Steps
description: How agent steps started by `amplihack recipe run` inherit the parent session's effort and model, and how a step overrides them.
issue: 1883
related: [1421, 1527, 1872]
last_updated: 2026-11-03
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
   runner's environment. A variable is set only when it has a value. Both are
   removed first, so a stale value from the caller's environment never reaches
   the runner.
4. The runner passes its environment to every step unchanged. Each step's
   `amplihack <binary> -- …` launch reads only these two variables and the
   step's own arguments. Steps do not read `/proc` or the fallback variables.
5. Each launch exports `AMPLIHACK_PARENT_EFFORT` and `AMPLIHACK_PARENT_MODEL`
   again for the agent it starts. The effort is taken from the agent's final
   argv. The model is the one the launch was given or inherited, and is left
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

`AMPLIHACK_PARENT_EFFORT` and `AMPLIHACK_PARENT_MODEL` cannot be set through
`-c` or a context file, in any letter case. Context keys starting with
`AMPLIHACK_` are reserved and are not exported to the runner.

## Precedence inside each step

When a step's launch builds its argv, the model is chosen in this order,
highest first:

1. The step's own `--model` in its arguments. This includes the runner's
   `--model auto` retry.
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
| `rusty`, `rustyclawd`, `amplifier` | `CLAUDE_CODE_EFFORT_LEVEL=L` when an effort is inherited; no `--effort` on argv | `--model M`, and `ANTHROPIC_MODEL=M` when a model is inherited |
| `codex` | `-c model_reasoning_effort=L` when L is `low`, `medium` or `high`; any other value is dropped with a log line | Only a model from the recipe context (`-c model=` or a context file), and never while a LiteLLM gateway variable is set. An inherited Claude model id is dropped with a log line. |
| `copilot` | Not supported; dropped with a log line | Only a model from the recipe context, and never while a LiteLLM gateway variable is set. An inherited Claude model id is dropped with a log line. |

Notes:

- `rusty`, `rustyclawd` and `amplifier` do not receive `--effort`, because an
  unknown flag would make every step fail. They read
  `CLAUDE_CODE_EFFORT_LEVEL`, so no log line is printed for them.
- For `codex`, the `-c` value is built from the matched literal `low`, `medium`
  or `high`, so a value such as `high,foo=bar` is rejected rather than passed.
- An unknown binary is handled as `copilot`.
- `codex` and `copilot` receive no added `CLAUDE_CODE_EFFORT_LEVEL` or
  `ANTHROPIC_MODEL`. They still see `AMPLIHACK_PARENT_EFFORT` and
  `AMPLIHACK_PARENT_MODEL`, because the runner passes its environment to every
  step.

## Variables

| Variable | Set by | Read by |
|---|---|---|
| `AMPLIHACK_PARENT_EFFORT` | Each `amplihack <binary>` launch (from the agent's final argv) and `amplihack recipe run` (for the runner) | `amplihack recipe run`, and each step's launch |
| `AMPLIHACK_PARENT_MODEL` | Each `amplihack <binary>` launch (see below) and `amplihack recipe run` (for the runner) | `amplihack recipe run`, and each step's launch |
| `CLAUDE_CODE_EFFORT_LEVEL` | Each Claude-compatible step, only when an effort is inherited. It holds the step's own `--effort` if given, otherwise the inherited effort. | The agent; and `amplihack recipe run` as the last effort fallback |
| `ANTHROPIC_MODEL` | Each Claude-compatible step, only when a model is inherited. It holds the step's own `--model` if given, otherwise the inherited model. | The agent; and `amplihack recipe run` as the last model fallback |

A launch with nothing inherited sets neither `CLAUDE_CODE_EFFORT_LEVEL` nor
`ANTHROPIC_MODEL`, even when it has its own `--effort` or `--model`.

A launch sets `AMPLIHACK_PARENT_MODEL` only when the model came from the
command line or was inherited. It is not set when amplihack chose the model
itself (`AMPLIHACK_DEFAULT_MODEL`, the built-in default or the gateway model),
so amplihack's own default never takes the place of a real choice further down.

None of the four variables is ever set to an empty string. A launch sets them after
the launch environment is built and are not managed by `EnvBuilder`. The
gateway's `ANTHROPIC_MODEL` is applied last, so it still wins under the
gateway.

## Log lines

All lines go to stderr. Values in angle brackets are filled in.

**Summary, once per run**, printed by `amplihack recipe run` when at least one
of effort and model has a value from any source in the
[source tables](#where-the-runs-values-come-from), including `-c effort=` or
`-c model=` alone:

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

Example, two or more `copilot` steps under one run print this once in total:

```text
amplihack: effort 'high' cannot be passed to copilot; steps use copilot's default
```

`amplihack recipe run` prints these lines next to the agent-binary notice. Each
step's launch prints nothing for them while `AMPLIHACK_RECIPE_RUN_ID` is set.
Outside a recipe run, a launch prints the line once per process.

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
"ignoring" line. Values that pass are used unchanged; there is no allowlist of
efforts or models, except the `codex` effort mapping above. Values are always
passed as separate argv elements, never through a shell.

No failure in this path stops a launch or a run. The worst case is "nothing
inherited", and the operator's own flags still apply.

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

- **One agent binary per run.** The checks in
  [Support for each agent binary](#support-for-each-agent-binary) use the run's
  `AMPLIHACK_AGENT_BINARY`. A step that starts a different binary receives
  values checked for the run's binary.
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
- **LiteLLM gateway.** When `AMPLIHACK_LITELLM_MODEL` is set, Claude-compatible
  steps use the gateway model. The run's `steps inherit … model=` line still
  shows the inherited model, which the gateway overrides.
- **Nested runs under codex or copilot steps.** A codex or copilot step
  receives the inherited model as its own option, but does not export it as
  `AMPLIHACK_PARENT_MODEL`. A `recipe run` started from inside such a step
  therefore does not inherit the model. Pass `-c model=<id>` to that run.

## Relation to per-step roles (#1872)

#1872 adds a model for each step's role (planner, executor). Its role model
will take slot 2 in [Precedence inside each step](#precedence-inside-each-step):
below the step's own `--model`, above the gateway model and the inherited
model. Until then, slot 2 is empty and has no effect.

## See also

- [AMPLIHACK_DEFAULT_MODEL and the model notice](./environment-variables.md#amplihack_default_model)
- [Active agent binary](./active-agent-binary.md), which decides the run's binary
- [Launch Target Resolution](../LAUNCH_TARGET_RESOLUTION.md)
