# How to Put Recipe Steps on Model Tiers

A recipe step declares how much model it needs — `fast`, `standard` or
`strong` — and amplihack picks the model, and on Codex the reasoning effort,
for the CLI it launches. The 29 bundled recipe files that `default-workflow`,
`investigation-workflow`, `smart-orchestrator` and the auto-drive family
reach carry tiers; the other 25 recipes with agent steps are unchanged. This
guide covers the things you do yourself: changing the mapping for your
install, moving one step, forcing one model everywhere, and reading back what
a run actually spent.

For the full contract — precedence, the shipped mapping, the fall-back rules,
the record schema — see the
[Model Tiers Reference](../reference/model-tiers.md).

## When to Use This

| Scenario | What to do |
|----------|------------|
| Your Codex install has a model name worth pinning | [Fill in a harness column](#2-fill-in-a-harness-column) |
| Reviews are costing more than they need to | [Change one cell of the mapping](#1-change-one-cell-of-the-mapping) |
| One step keeps needing more model than its tier gives it | [Move one step](#3-move-one-step-to-another-tier) |
| You want a whole run on one model, for a comparison or a budget | [Force one model](#4-force-one-model-for-every-step) |
| You want to show the saving | [Read the run summary](#5-read-back-what-the-run-spent) |
| A tier's model is not on your account | [Remove its entry](#6-handle-a-model-your-account-does-not-have) |

## Steps

### 1. Change one cell of the mapping

Write a `model-tiers.toml` in your amplihack home. Entries merge over the
shipped mapping cell by cell, so you only write the cells you are changing.

```sh
cat > ~/.amplihack/model-tiers.toml <<'EOF'
version = 1

[claude.standard]
model = "claude-haiku-5-5"
EOF
```

That moves the `standard` tier — reviews, documentation, PR bodies, test
authoring — onto Haiku for Claude-compatible launches. `fast` and `strong`
keep their shipped values, and Codex and Copilot CLI are untouched.

Check that exact cell by launching with the tier you changed. A recipe step's
tier arrives as `--model <tier word>` on the passthrough side of
`amplihack <cli> --`, so you can type the same thing:

```sh
amplihack claude -- --model standard -p 'print the word ok' 2>&1 >/dev/null |
  grep '^amplihack:'
```

```text
amplihack: model tier `standard` on `claude`: passing `--model claude-haiku-5-5`. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
```

A bare `amplihack claude -p …` declares no tier, so it will not show this —
it prints the ordinary default-model line instead, as it did before.

### 2. Fill in a harness column

The shipped mapping names no Codex or Copilot model, because those depend on
the install and the account. Name yours:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
model = "gpt-6.1-sol"
effort = "xhigh"

[codex.standard]
model = "gpt-6.1-sol"
effort = "high"

[codex.fast]
model = "gpt-6.1-sol-mini"
effort = "low"
```

amplihack accepts `minimal`, `low`, `medium`, `high` and `xhigh` as effort
values. That is amplihack's own allowlist, kept short so the value is safe to
put on a command line; whether your installed Codex accepts the word is
Codex's call, and it says so in its own output if not. Codex is the only
harness with a reasoning-effort launch control — on Claude Code and Copilot
CLI a tier changes the model only, and an effort written in their column is
reported in the run summary but is not sent.

### 3. Move one step to another tier

Name the step id and the tier you want it on:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[step_tiers]
step-10-pre-commit-review = "strong"
step-20-final-cleanup = "fast"
```

Step ids come from the recipe. List them without running anything:

```sh
grep -n '^  - id:' ~/.amplihack/amplifier-bundle/recipes/workflow-refactor-review.yaml
```

```text
33:  - id: "step-09-refactor"
64:  - id: "step-09b-optimize"
86:  - id: "step-10-pre-commit-review"
119:  - id: "step-10b-security-review"
140:  - id: "step-10c-philosophy-check"
163:  - id: "step-11-incorporate-feedback"
182:  - id: "step-11b-implement-feedback"
205:  - id: "checkpoint-after-review-feedback"
```

This is also how to get a split the three words cannot express on their own.
On Codex, `strong` is `xhigh` and `standard` is `high`, so putting the builder
steps on `standard` and leaving the orchestration steps on `strong` gives you
orchestration at `xhigh` and builders at `high`:

```toml
[step_tiers]
step-08-implement = "standard"
step-11b-implement-feedback = "standard"
```

A step id that no recipe contains changes nothing: no recipe is copied for it, and the rest of the table still applies.

### 4. Force one model for every step

```sh
# For every run, until you unset it
amplihack config set model.force claude-sonnet-5-5

# For one run only
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every step then asks for that model, whatever tier it declared — and, unlike
everything else here, this also replaces a concrete `--model` that a command
line carries. The tier's reasoning effort still applies, so on Codex a
`strong` step keeps `xhigh` while running the forced model.

Confirm which setting is in force, and from where. `config get` prints the
bare value followed by its origin:

```sh
amplihack config get model.force --show-origin
```

```text
claude-sonnet-5-5  # user (/home/alice/.amplihack/config.toml)
```

For the `key = value  # origin` row form, use `amplihack config list
--show-origin` instead. Undo it with `amplihack config unset model.force`.

!!! note

    A launch through the LiteLLM gateway ignores both tiers and
    `model.force`; `AMPLIHACK_LITELLM_MODEL` keeps deciding the model there.

### 5. Read back what the run spent

The run summary names the model and effort each step ran on:

```sh
amplihack recipe run default-workflow -c task_description='...'
```

```text
Model tiers:
  autodrive-prerequisites: fast → claude-haiku-5-5
  step-05-architecture: strong → claude-opus-5[1m]
  step-10-pre-commit-review: standard → claude-sonnet-5-5
```

Step ids appear only when the launch count matches the agent-step count. A
parallel group, a rate-limit retry or a resumed run breaks that match, and the
section then lists launches by number with a line saying so — see
[When a line names a step](../reference/model-tiers.md#when-a-line-names-a-step).

For a count across the whole run, read the record file the run wrote. Every
launch that carried a tier appends one line, falling back or not:

```sh
jq -r '[.tier // "none", .effective_model // "harness default"] | @tsv' \
  "$AMPLIHACK_RUN_DIR/model-tiers.jsonl" | sort | uniq -c
```

```text
      9	fast	claude-haiku-5-5
      6	standard	claude-sonnet-5-5
      8	strong	claude-opus-5[1m]
```

Before this feature that run would have been 23 launches on the strongest
model. The file's fields are listed in
[the reference](../reference/model-tiers.md#the-record-file).

### 6. Handle a model your account does not have

Leave the cell with no model. That is how the mapping says "do not ask for
anything here":

```toml
# ~/.amplihack/model-tiers.toml
version = 1

# This account has no Opus. Ask for nothing from the tier.
[claude.strong]
# no model key
```

What happens next depends on the harness, and the difference matters:

- On **Codex and Copilot CLI** the step launches with no `--model` at all and
  the CLI uses its own default.
- On a **Claude-compatible** tool amplihack's ordinary model chain takes over,
  so the launch still gets `--model claude-opus-5[1m]` — amplihack's built-in
  default — unless you also redirect that chain. To hand the choice to Claude
  Code itself, set `AMPLIHACK_DEFAULT_MODEL` to an empty value; to pin a
  different id, set it to that id.

```sh
# Ask for nothing at all on Claude-compatible launches
AMPLIHACK_DEFAULT_MODEL= amplihack recipe run default-workflow
```

The step does not fail, nothing is retried, and the run log says once what
happened:

```text
amplihack: model tier `strong` names no model in the `claude` column, so `claude` runs on its own default model. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: passing `--model claude-opus-5[1m]` to `claude` (from amplihack's built-in default). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty value to let claude choose its own default model.
```

amplihack does not check in advance whether a model is on your account. If
you leave a model in the mapping that your account does not carry, the harness
says so in its own output and the step's own error is the evidence.

### 7. Run small-change reviews as one pass

On a small diff, the separate code, security and philosophy passes of the
refactor-review phase — `step-10-pre-commit-review`,
`step-10b-security-review` and `step-10c-philosophy-check` — can run as one:

```sh
amplihack config set review.combined_small_diff true
amplihack config set review.combined_diff_threshold 120
```

Off by default, so the three passes keep running separately until you turn
this on. Above the threshold they run separately anyway. This applies to the
refactor-review phase of a build, not to PR review.

## Verify

```sh
# The mapping file loads cleanly: no amplihack: line about it
amplihack claude -p 'print ok' 2>&1 >/dev/null | grep 'model-tiers.toml' ||
  echo 'mapping file is fine'

# Every tiered step declares a tier word, not a model id
awk -F'"' '/^[[:space:]]*model:/ {print $2}' \
  ~/.amplihack/amplifier-bundle/recipes/*.yaml | sort -u
```

```text
fast
standard
strong
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `heavy is not a model tier` | The vocabulary is `fast`, `standard`, `strong` only | Use one of the three words |
| The mapping file seems ignored | amplihack reads one file: `model.tier_map`, else `$AMPLIHACK_HOME/model-tiers.toml`, else `~/.amplihack/model-tiers.toml` | `amplihack config get model.tier_map` and check which file you edited |
| Steps all run one model, and you did not ask for that | `model.force` or `AMPLIHACK_FORCE_MODEL` is set | `amplihack config get model.force --show-origin`; check your environment |
| Tiers have no effect at all | The launch goes through the LiteLLM gateway | Unset `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` and `AMPLIHACK_LITELLM_MODEL`, or set the gateway's model |
| One step ran on `auto`, at the wrong Codex effort | `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` is set, so the runner replaced that step's model with `auto` on its final rate-limit attempt, before amplihack saw a tier | Unset it, or accept that the last retry of a rate-limited step is untiered |
| An effort value is dropped | It is not one of `minimal`, `low`, `medium`, `high`, `xhigh` | Use an accepted value; the model applied anyway |
| A removed `[claude.*]` model still launches with `--model` | The tier falls back to amplihack's own default chain on Claude-compatible tools | Set `AMPLIHACK_DEFAULT_MODEL` to an empty value, or to the id you want |
| Lines in `Model tiers:` say `launch 1` instead of a step id | The launch count does not match the agent-step count — a parallel group, a rate-limit retry or a resumed run | Read the record file, which carries the same data per launch |
| `config set model.tier_map` in a project file is ignored | Path-valued keys are user-only, so a cloned repository cannot name the file that rewrites recipe YAML | Set it in your user config |
| A step id in `[step_tiers]` does nothing | No recipe in the run contains it | Check the id with the `grep` in step 3 |

## Related

- [Model Tiers Reference](../reference/model-tiers.md) — the full contract
- [User Configuration](../reference/user-config.md) — `model.*` and
  `review.*` keys, and `amplihack config`
- [Recipe Command](../reference/recipe-command.md) — `amplihack recipe run`
  and its output formats
