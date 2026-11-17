# How to Put Recipe Steps on Model Tiers

A recipe step declares how much model it needs, `fast`, `standard` or
`strong`, and amplihack picks the model and, on Codex, the reasoning effort
for the CLI it launches. This guide covers the things you do yourself. For the
full contract, including precedence, the shipped mapping, the fall-back rules
and the record schema, see the
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

The file is merged cell by cell over the shipped mapping, so this changes
`standard` on Claude-compatible tools and leaves every other cell alone. The
`version = 1` line is required: without it the whole file is ignored with one
stderr line, which is why every block below repeats it. A file owned by another
user, or writable by group or others, is also ignored, which is what the
`chmod` above avoids.

## 2. Fill in a harness column

The shipped Codex and Copilot columns name no model, because a model name
there would guess at your account. Name one for your install:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
model = "gpt-6.1-sol"
effort = "xhigh"

[copilot.fast]
model = "claude-haiku-5-5"
```

Accepted effort values are `minimal`, `low`, `medium`, `high` and `xhigh`.
Only Codex has a launch control for effort; a value in another column is
recorded as requested rather than effective.

## 3. Move one step to another tier

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

## 4. Force one model for every step

```sh
amplihack config set model.force claude-sonnet-5-5
# or, for one run only:
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every launch then asks for that model whatever tier its step declared. The
tier's Codex effort still applies, so a mixed-tier recipe keeps its effort
split. Unset the key to restore per-step tiers.

## 5. Run small-change reviews as one pass

```sh
amplihack config set review.combined_small_diff true
amplihack config set review.combined_diff_threshold 120
```

Off by default, so the code, security and philosophy passes keep running
separately until you turn this on. Above the threshold they run separately
anyway. This applies to the review phase of a build, not to PR review.

## 6. Read back what the run spent

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

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `heavy is not a tier` | The vocabulary is `fast`, `standard`, `strong` only | Use one of the three words |
| The mapping file seems ignored | amplihack reads one file: `model.tier_map`, else `$AMPLIHACK_HOME/model-tiers.toml`, else `~/.amplihack/model-tiers.toml` | `amplihack config get model.tier_map`, then check which file you edited |
| The whole file is ignored, and a line says so | It does not declare `version = 1` | Add `version = 1` as the first line |
| `model.tier_map` will not be set | The key takes an absolute path with no `..` component | Give the full path, not `~/...` or a relative one |
| A line about permissions, and the file is ignored | It is writable by group or others, or owned by another user | `chmod 600 ~/.amplihack/model-tiers.toml` |
| Steps all run one model you did not ask for | `model.force` or `AMPLIHACK_FORCE_MODEL` is set | `amplihack config get model.force --show-origin`, and check your environment |
| Tiers have no effect at all | The launch goes through the LiteLLM gateway | Unset `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` and `AMPLIHACK_LITELLM_MODEL`, or set the gateway's model |
| One step ran on `auto`, at the wrong Codex effort | `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` made the runner replace that step's model before amplihack saw a tier | Unset it, or accept that the last retry of a rate-limited step is untiered |
| An effort value is dropped | It is not one of `minimal`, `low`, `medium`, `high`, `xhigh` | Use an accepted value; the model applied anyway |
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
