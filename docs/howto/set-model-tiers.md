---
title: Set Recipe Model Tiers
description: Configure model mappings and step overrides, and remove obsolete auto-drive entry-gate overrides after deployment.
doc_type: howto
last_updated: 2026-11-21
---

# How to Put Recipe Steps on Model Tiers

A recipe step declares how much model it needs, `fast`, `standard` or `strong`,
and amplihack picks the model and, on Claude Code and Codex, the reasoning
effort for the CLI it launches. This guide covers the things you do yourself. For the full
contract, including precedence, the shipped mapping, the fall-back rules and
the record schema, see the
[Model Tiers Reference](../reference/model-tiers.md).

## 1. Change one cell of the mapping

Put reviews on a stronger model than the shipped `standard` tier gives them:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[claude.standard]
model = "claude-opus-5[1m]"
```

```sh
chmod 600 ~/.amplihack/model-tiers.toml
```

The file merges cell by cell over the shipped mapping, so this changes
`standard` on Claude-compatible tools and leaves every other cell alone.
Nothing merges inside a cell: the cell you write replaces the shipped one
whole, so write every value you want in it. That matters on Codex, whose
shipped cells each carry an effort (section 2), and on Claude `strong`, whose
shipped cell carries `xhigh` (section 3).

The `version = 1` line is required: without it the whole file is ignored with
one stderr line, which is why every block below repeats it. A file owned by
another user, or writable by group or others, is also ignored, which is what
the `chmod` above avoids.

## 2. Fill in a harness column

The shipped Codex and Copilot columns name no model, because a model name there
would guess at your account. Name one for your install:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
model = "gpt-6.1-sol"
effort = "xhigh"

[copilot.fast]
model = "claude-haiku-5-5"
```

The Codex cell names its `effort` as well as its `model`. Your cell replaces
the shipped `[codex.strong]`, which asks for `high`, so a cell with only a
`model` line launches with no effort argument and Codex uses its own default.

Accepted effort values depend on the column. Claude takes `low`, `medium`,
`high`, `xhigh` and `max`; Codex takes `minimal`, `low`, `medium`, `high` and
`xhigh`. Copilot CLI has no launch control for effort, so a value in its column
is recorded as requested rather than effective.

## 3. Set the effort a Claude tier is launched at

The shipped `strong` tier on Claude-compatible tools asks for `xhigh`, which
amplihack passes as `--effort xhigh`; `standard` and `fast` name no effort and
Claude Code uses its own default. The `xhigh` default raises token consumption
on every `strong` step. To lower it on this host, restate the cell:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[claude.strong]
model = "claude-opus-5[1m]"
effort = "high"
```

To choose another model for `strong` on this host, such as a Fable model, name
it in the same cell and say which effort you want:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[claude.strong]
model = "claude-fable-5-2"
effort = "max"
```

Your cell replaces the shipped one whole, so a cell that names only a `model`
launches with no `--effort` at all. That is also the fix on a host whose Claude
Code is too old to accept `--effort`: amplihack does not detect that, so
clear the effort in the cell and the flag is no longer written.

```toml
[claude.strong]
model = "claude-opus-5[1m]"
```

To bound every Claude tier at once, declare a ceiling:

```toml
[claude]
max_effort = "high"
```

A ceiling lowers an effort amplihack chooses; it does not lower an `--effort`
you typed on the command yourself, which is always left alone.

## 4. Raise or lower the effort your Codex account is asked for

The shipped Codex `strong` and `standard` tiers both ask for `high`, and `fast`
asks for `low`. Where the model your Codex account uses accepts `xhigh`, raise
`strong`:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
effort = "xhigh"
```

A `strong` step then launches with `-c model_reasoning_effort=xhigh`, and
`standard` and `fast` are untouched. Where your model does not accept an effort
a cell asks for, lower that cell the same way.

To bound all three tiers at once, declare the column's ceiling instead:

```toml
[codex]
max_effort = "medium"
```

Each tier is then written at its own effort or at the ceiling, whichever is
lower. amplihack does not test what your account accepts, because that needs a
live call, so the ceiling is your declaration, and a step whose effort is
lowered runs rather than failing.

## 5. Move one step to another tier

Find the step id in the recipe, then name it in `[step_tiers]`:

```sh
grep -rn 'id: "step-08-implement"' ~/.amplihack/amplifier-bundle/recipes/
```

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[step_tiers]
step-08-implement = "standard"
step-20-final-cleanup = "fast"
```

`amplihack recipe run` rewrites that one `model:` line in a private copy of the
recipe for the run. A step id no recipe in the run declares is reported by name
at the top of the output and changes nothing, which is the usual sign of a
typo.

## 6. Point a whole column at models your account has

The shipped `claude` column names Opus, Sonnet and Haiku. On an account or a
backend without them, name three models it does have and keep the tiers apart:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[claude.strong]
model = "gpt-6.1-sol"

[claude.standard]
model = "gpt-6.1"

[claude.fast]
model = "gpt-6.1-mini"
```

For one model across every tier on Claude-compatible tools, without editing a
file, set `AMPLIHACK_DEFAULT_MODEL` instead:

```sh
AMPLIHACK_DEFAULT_MODEL='claude-opus-5[1m]' amplihack recipe run default-workflow ...
```

That replaces whatever each tier maps to on the `claude` column, which is the
quickest fix on an account without Sonnet or Haiku. Codex and Copilot CLI do
not read it, so a Claude id set here never reaches them; give them a model with
a `[codex.<tier>]` or `[copilot.<tier>]` cell as in section 2, where a Codex
cell names its `effort` alongside the `model`. It leaves a concrete `--model` a
recipe pinned alone; to displace those too, use `model.force` below.

## 7. Force one model for every step

```sh
amplihack config set model.force claude-sonnet-5-5
# or, for one run only:
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every launch then asks for that model whatever tier its step declared, on every
harness, and unlike `AMPLIHACK_DEFAULT_MODEL` it also replaces a concrete model
a step pinned. The tier's effort still applies, on Claude and on Codex, so a
mixed-tier recipe keeps its effort split. Unset the key to restore per-step tiers.

## 8. Run small-change reviews as one pass

```sh
amplihack config set review.combined_small_diff true
amplihack config set review.combined_diff_threshold 120
```

Off by default, so the code, security and philosophy passes keep running
separately until you turn this on. Above the threshold they run separately
anyway. They also run separately whenever the step cannot measure the diff or
read the setting, and the reason is on stderr. This applies to the review phase
of a build, not to PR review.

## 9. Read back what the run spent

The run summary prints a **Model tiers:** section naming the model and effort
each step ran on. For the same facts per launch:

```sh
jq -r '[.tier // "none", .effective_model // "harness default"] | @tsv' \
  "$AMPLIHACK_RUN_DIR/model-tiers.jsonl" | sort | uniq -c
```

```text
      9	fast	claude-haiku-5-5
      6	standard	claude-sonnet-5-5
      8	strong	claude-opus-5[1m]
```

A line whose `effective_model` is `null` fell back, and its `fallback` field
says why. A line whose `requested_effort` is set and whose `effective_effort`
is `null` ran on a harness with no launch control for effort; one where the two
differ was lowered by the column's `max_effort`.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `heavy is not a tier` | The vocabulary is `fast`, `standard`, `strong` only | Use one of the three words |
| The mapping file seems ignored | amplihack reads one file: `model.tier_map`, else `$AMPLIHACK_HOME/model-tiers.toml`, else `~/.amplihack/model-tiers.toml` | `amplihack config get model.tier_map`, then check which file you edited |
| The whole file is ignored, and a line says so | It does not declare `version = 1` | Add `version = 1` as the first line |
| `model.tier_map` will not be set | The key takes an absolute path with no `..` component | Give the full path, not `~/...` or a relative one |
| A line about permissions, and the file is ignored | It is writable by group or others, or owned by another user | `chmod 600 ~/.amplihack/model-tiers.toml` |
| Steps all run one model you did not ask for | `model.force`, `AMPLIHACK_FORCE_MODEL` or `AMPLIHACK_DEFAULT_MODEL` is set | `amplihack config get model.force --show-origin`, and check your environment |
| Tiers have no effect at all | The launch goes through the LiteLLM gateway | Unset `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` and `AMPLIHACK_LITELLM_MODEL`, or set the gateway's model |
| One step ran on `auto`, with no tier effort | `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` made the runner replace that step's model before amplihack saw a tier | Unset it, or accept that the last retry of a rate-limited step is untiered |
| An effort value is dropped | It is outside its column's list: `low`, `medium`, `high`, `xhigh`, `max` for Claude, or `minimal`, `low`, `medium`, `high`, `xhigh` for Codex | Use an accepted value; the model applied anyway |
| Claude refuses `--effort` | The installed Claude Code is too old to know the option, or a Claude-compatible tool other than `claude` does not accept it | Name the `[claude.<tier>]` cell with its `model` and no `effort` |
| A Claude `strong` step ran at a different effort than the cell says | The command already carried its own `--effort`, which amplihack leaves alone, or `[claude] max_effort` lowered it | Check the command line and the column's ceiling |
| Codex refuses the effort amplihack passed | Your account's model does not accept the value that cell names | Lower `effort` in that `[codex.<tier>]` cell, or set `[codex] max_effort` to bound all three |
| A Codex or Copilot CLI step ignores `AMPLIHACK_DEFAULT_MODEL`, with no line about it | The variable applies to Claude-compatible tools only | Write a `[codex.<tier>]` cell with `model` and `effort`, or a `[copilot.<tier>]` cell with `model`, in your `model-tiers.toml` |
| A Codex step lost its tier effort after you gave it a model | Your cell replaces the shipped cell whole and names no `effort` | Add `effort = "high"`, or the value you want, to that `[codex.<tier>]` cell |
| A Claude-compatible step ignores `AMPLIHACK_DEFAULT_MODEL`, and a line says so | The value could act as another argument: it starts with `-`, is over 128 bytes, or uses a character outside the model-id set | Use a plain model id |
| A removed `[claude.*]` model still launches with `--model` | The tier falls back to amplihack's own default chain on Claude-compatible tools | Set `AMPLIHACK_DEFAULT_MODEL` empty, or to the id you want |
| The summary says `launch 1` instead of a step id | The launch count does not match the agent-step count: a parallel group, a rate-limit retry or a resumed run | Read `model-tiers.jsonl`, which carries the same data per launch |
| `model.tier_map` in a project file is ignored | Path-valued keys are user-only, so a cloned repository cannot name the file that rewrites recipe YAML | Set it in your user config |
| A line says a patched recipe copy was dropped | The rewritten file no longer parsed as YAML, so the original ran instead | Report it: a `model:` line is the only edit made, and it should always parse |
| `review.combined_small_diff` in a project file is ignored | Both `review.*` keys are user-only, so a repository cannot decide whether its own security pass runs | Set it in your user config |

## Related

- [Model Tiers Reference](../reference/model-tiers.md), the full contract
- [User Configuration](../reference/user-config.md), for the `model.*` and
  `review.*` keys and the `amplihack config` command
- [Recipe Command](../reference/recipe-command.md), for `amplihack recipe run`
  and its output formats

## Remove deployed auto-drive entry-gate overrides

After deploying named-checkout execution, remove the obsolete host overrides
for these three step IDs from `[step_tiers]` in
`~/.amplihack/model-tiers.toml`:

- `step-01-build-preflight`
- `step-01-crusty-loop-preflight`
- `step-01-merge-loop-preflight`

Remove only those entries, retaining `version = 1`, all model mappings, and
unrelated step overrides. Keep the file private. This is a post-shipment
operator action; installation and recipe execution do not edit host
configuration automatically. Do not delete the whole file to remove three
entries.

Run subsequent drives with the same named checkout and PR-worktree selection.
Inspect the run summary's model-tier section to confirm each gate uses its
recipe-declared `fast` tier rather than a `strong` override. Also account for
other explicit model settings when inspecting the effective model. The repair
is repository routing, not a stronger model or a raised timeout. See
[named-checkout configuration](../reference/recipe-checkout-isolation.md#configuration-and-migration).
