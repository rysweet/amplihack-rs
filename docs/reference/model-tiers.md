# Model Tiers — Reference

A recipe agent step says how much model it needs, not which model to use. It
writes one of three words — `fast`, `standard` or `strong` — and amplihack
turns that word into a model, and where the harness has one, a reasoning
effort, for the CLI it is about to launch. Claude Code, Codex and Copilot CLI
each have their own column in the mapping, so the same recipe runs on all of
them without naming a model that a harness or an account does not have.

## Contents

- [The three words](#the-three-words)
- [How a step declares a tier](#how-a-step-declares-a-tier)
  - [Where the tier word arrives on the command line](#where-the-tier-word-arrives-on-the-command-line)
- [The shipped mapping](#the-shipped-mapping)
  - [Harness columns](#harness-columns)
- [Precedence](#precedence)
- [Falling back to the harness default](#falling-back-to-the-harness-default)
- [Reasoning effort, per harness](#reasoning-effort-per-harness)
  - [Settings that are not launch controls](#settings-that-are-not-launch-controls)
- [Overriding the mapping](#overriding-the-mapping)
  - [The override file](#the-override-file)
  - [Overriding one step's tier](#overriding-one-steps-tier)
  - [Forcing one model everywhere](#forcing-one-model-everywhere)
- [Measuring what each step ran on](#measuring-what-each-step-ran-on)
  - [The run summary section](#the-run-summary-section)
  - [When a line names a step](#when-a-line-names-a-step)
  - [The record file](#the-record-file)
- [Tiers in the bundled recipes](#tiers-in-the-bundled-recipes)
- [Combined review for small diffs](#combined-review-for-small-diffs)
- [What amplihack does not check](#what-amplihack-does-not-check)
- [Diagnostics](#diagnostics)
- [Related](#related)

## The three words

| Tier | Meant for |
|------|-----------|
| `fast` | Mechanical steps: workspace preparation, prerequisite lookups, loop-result verification, summaries, cleanup, status and marker writing |
| `standard` | Code, security and philosophy reviews, documentation, PR body writing, test authoring |
| `strong` | Design, implementation, debugging, recovery decisions, orchestration, classification and planning |

These are the only three. There is no `light`, `medium`, `heavy` or `xhigh`
tier, and no alias for any of the three words. A fourth word in a recipe or in
an override file is an error, reported by name, and the shipped mapping
continues to apply.

The concept is called a **model tier** throughout, in prose, config keys and
code. `amplihack-workflows` already uses the bare word "tier" for its
execution cascade; the two are unrelated.

Matching is case-insensitive on a trimmed value, so `model: "Strong"` and
`model: "strong"` mean the same thing. Any other value is treated as a
concrete model id and forwarded exactly as typed.

## How a step declares a tier

An agent step writes a tier word in its existing `model:` field:

```yaml
steps:
  - id: "autodrive-prerequisites"
    agent: "amplihack:core:builder"
    model: "fast"
    prompt: |
      Read back the task and check its prerequisites ...

  - id: "step-05-architecture"
    agent: "amplihack:architect"
    model: "strong"
    prompt: |
      Design the solution architecture ...
```

Only an agent step takes a tier. A `bash` step runs no model, so it declares
none — `step-01-prepare-workspace`, for instance, is a bash step.

The tier reuses `model:` rather than adding a `tier:` field because `model:`
is the one step field the pinned recipe runner turns into a launch argument. A
new field name would be reported as unknown by the runner's parser and would
never reach the step, so the tier would have no effect. The runner is
unchanged by this feature, and the pin does not move.

A concrete model id in `model:` still works and is still passed through
untouched:

```yaml
    model: "claude-sonnet-5-5"   # a concrete id, forwarded as typed
```

No tiered recipe names a concrete model. A test over the tiered set in
`amplifier-bundle/recipes/` asserts that every `model:` value there is one of
the three words.

### Where the tier word arrives on the command line

The `--model` carrying the tier word comes from the **recipe runner**, not
from amplihack. The runner builds the child command as
`amplihack <cli> -- …`, and puts the step's `model:` value on the passthrough
side itself:

```text
amplihack codex -- exec --output-last-message <path> --model strong -
amplihack claude -- --system-prompt-file <path> -p --add-dir . --model strong
```

That is why resolving a tier is **delete, then decide**: amplihack finds the
`--model <tier-word>` pair the runner wrote, removes it, and puts its own
answer in its place. Without the delete, the tier word would reach the CLI as
a literal model name — and for Codex and Copilot CLI especially, because
amplihack's own model selection returns early for a tool that is not
Claude-compatible and would never have looked at the argument at all.

## The shipped mapping

The defaults ship with amplihack as a TOML table embedded in the binary. They
are configuration, not a code path, and they name no host, no file-system path
and no account.

| Tier | Claude-compatible | Codex | Copilot CLI |
|------|-------------------|-------|-------------|
| `strong` | `claude-opus-5[1m]` | no model named, effort `xhigh` | no model named |
| `standard` | `claude-sonnet-5-5` | no model named, effort `high` | no model named |
| `fast` | `claude-haiku-5-5` | no model named, effort `low` | no model named |

Three things to read out of that table:

- **Claude ids are concrete, never aliases.** `claude-opus-5[1m]` is the same
  value as amplihack's built-in `DEFAULT_MODEL`, asserted by a test, for the
  reason in issue #1421: an alias is resolved by the CLI, so its meaning
  depends on the CLI version, and a retired id then appears in a 404 that
  nobody chose.
- **Codex names no model.** Opus and Sonnet do not exist there, and naming any
  Codex model in a shipped default would guess at the account. Codex tiers
  differ by reasoning effort alone, and the model stays whatever the Codex
  install is configured to use.
- **Copilot CLI has a section with no entries.** amplihack passes no `--model`
  to Copilot today, and a model name there would depend on the account. The
  section exists so an instance can fill it in; until it does, all three
  Copilot tiers fall back.

### Harness columns

| Column | Launch commands it covers |
|--------|---------------------------|
| `claude` | `amplihack claude`, `amplihack rusty`, `amplihack rustyclawd`, `amplihack amplifier` — every Claude-compatible tool |
| `codex` | `amplihack codex` |
| `copilot` | `amplihack copilot` |

A harness with no column at all — a tool amplihack learns to launch later, or
one an override file does not mention — takes the fall-back below. Unknown
column names in an override file are kept rather than reported, so a file
written for a newer amplihack still loads on an older one.

**`step_tiers` is a reserved table name, not a harness column.** The parser
matches it before the keep-unknown-columns rule, because
[`[step_tiers]`](#overriding-one-steps-tier) is structurally a column whose
keys are step ids rather than tier words. No harness column may be called
`step_tiers`.

## Precedence

Highest first:

| Rank | Source | Effect |
|------|--------|--------|
| 0 | The recipe runner's `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` final attempt | Rewrites the step's model to the literal `auto` **before amplihack runs**. |
| 1 | An explicit concrete `--model <id>` on the launched command | Forwarded verbatim. amplihack does not read it, rewrite it or resolve it. |
| 2 | `model.force` / `AMPLIHACK_FORCE_MODEL` | Replaces a tier word, and supplies a model where the command has none. |
| 3 | A per-step tier override (`[step_tiers]`) | Changes which tier the named step declares. |
| 4 | Override-file entries | Replace the matching cells of the shipped mapping. |
| 5 | The shipped mapping | The table above. |
| 6 | The harness default | What a launch carrying no tier gets; see [the fall-back](#falling-back-to-the-harness-default). |

Rank 0 is not amplihack's. When
`AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` is set to a non-empty value, the
pinned recipe runner substitutes `auto` for the step's `model:` on the last of
its `1 + AMPLIHACK_RATELIMIT_MAX_RETRIES` attempts, as the rate-limit message
suggests. amplihack is then launched with `--model auto` and has no way to
know a tier was ever declared: `auto` is not one of the three words, so it is
forwarded as a concrete id, and **the tier's Codex reasoning effort is lost
with it**. That final attempt therefore runs at whatever effort the Codex
install defaults to. The variable is off unless you set it.

Two more exceptions are worth stating next to the promises they qualify:

- **A LiteLLM gateway launch ignores tiers.** A launch goes through the
  gateway when the tool is one the gateway routes — Claude Code, RustyClawd
  or GitHub Copilot CLI — **and** `AMPLIHACK_LITELLM_ENDPOINT`,
  `AMPLIHACK_LITELLM_API_KEY` or `AMPLIHACK_LITELLM_MODEL` is set. There
  amplihack removes the tier's `--model` pair. The gateway routes on the
  model name, has no default of its own, and was never configured to route
  the ids in the shipped mapping; it refuses outright a launch whose explicit
  `--model` is not `AMPLIHACK_LITELLM_MODEL`, so leaving a tier's model in
  place would stop the step rather than route it badly. A forced model does
  not route past it either.

  On Claude Code and RustyClawd, amplihack's existing model chain then writes
  `--model $AMPLIHACK_LITELLM_MODEL` in the tier's place — the same argument
  that launch carried before this feature existed. On Copilot CLI nothing is
  written in its place, because amplihack passes no `--model` to Copilot at
  all; the gateway routes it by environment. This holds whether or not you
  have filled in the `[copilot]` column.

  Codex never takes this path: the gateway does not route it, so gateway
  variables do not change a `codex` launch and its tiers apply as usual.
- **`model.force` outranks an explicit `--model`.** This is the one narrow
  exception to issue #1527, which asks that a value an operator typed be left
  alone. The forced value comes only from your own user config or your own
  environment, and the thing it is aimed at is steps, which carry a tier word
  rather than a concrete id. A forced launch keeps the tier's reasoning
  effort, so forcing one model across a mixed-tier recipe does not quietly
  flatten Codex effort to one value.

## Falling back to the harness default

A tier falls back when any of these is true:

- the harness has no column in the mapping;
- the tier has no entry in that column;
- the entry names an effort but no model, as every shipped Codex entry does;
- the launch goes through the LiteLLM gateway.

Falling back **removes** the tier word from the command line. It never
substitutes a model name of its own. What happens next depends on the harness,
and the two answers are different:

| Harness | After the tier word is removed |
|---------|-------------------------------|
| Claude-compatible | amplihack's existing model chain applies, exactly as for a launch that never mentioned a model: `AMPLIHACK_LITELLM_MODEL` on a gateway launch, else `AMPLIHACK_DEFAULT_MODEL`, else the built-in `claude-opus-5[1m]`. Setting `AMPLIHACK_DEFAULT_MODEL` to an empty value is how you get no `--model` at all. |
| Codex, Copilot CLI | Nothing is passed. amplihack does not give a non-Claude tool a model, so the CLI uses its own default. |

So "harness default" means *amplihack's configured default* on a
Claude-compatible launch and *the CLI's own default* on Codex and Copilot. The
stderr lines say which happened, and the existing issue #1421 notice still
names the model a Claude-compatible launch ends up with.

The step never fails because of a fall-back, and the fall-back costs nothing:

- **No probe.** Availability is a statement in the mapping, not a question
  asked over the network. A tier with no entry for a harness is what "that
  model is not available there" looks like in configuration.
- **No second launch.** A harness that still rejects a configured model says
  so in its own output. amplihack does not launch again with a different
  model, because that is a retry.
- **No timers and no waiting.**

Every launch that carried a tier — falling back or not — appends one line to
[the record file](#the-record-file) and prints one line to stderr: the line
names what the tier gave the launch, or, when it gave it nothing, that the
harness's own default runs. The record's `fallback` field is `null` when the
launch did not fall back, so the per-step summary can render both kinds from
the same file.

## Reasoning effort, per harness

| Harness | Effort launch control | What a tier changes |
|---------|-----------------------|---------------------|
| Codex | `-c model_reasoning_effort=<effort>`, injected by amplihack | The effort. The model is Codex's own default unless a column entry names one. |
| Claude Code (and every Claude-compatible tool) | None exists | The model |
| Copilot CLI | None exists | Nothing yet; the column has no entries |

Accepted effort values are `minimal`, `low`, `medium`, `high` and `xhigh`.

**That list is amplihack's allowlist, not a list amplihack read from Codex.**
Its purpose is argv safety: the value becomes the right-hand side of
`-c model_reasoning_effort=…` on a command line, so amplihack accepts only
these five words and rejects anything else before writing it. Whether the
Codex you have installed accepts the word it is handed is Codex's decision,
and a rejection appears in Codex's own output; amplihack does not translate
it, and does not keep a table of which Codex version takes which values.
Codex also accepts `--strict-config` in both its root and its `exec` scope,
which turns an unknown or unusable config key from ignored into fatal;
amplihack does not pass that flag, and a command line that does carries the
consequence.

An effort value an override file supplies that is not one of the five is
reported and dropped; the tier's model still applies and the launch proceeds.

For Codex, amplihack adds the setting alongside its existing Codex
injections. If your own command already carries
`-c model_reasoning_effort=…`, yours is left alone and amplihack adds nothing.

Claude Code and Copilot CLI have no launch argument for reasoning effort, so
an effort written in their column of an override file is recorded in the run
summary and the record file but does not reach the CLI. The summary shows it
as requested rather than effective, which is what makes the difference
visible rather than silent.

### Settings that are not launch controls

These look like they should route a model or an effort, and do not:

| Setting | What it actually is |
|---------|---------------------|
| A recipe's `-c model=…` | A recipe context variable, readable in prompt templates. It is not passed to the launched CLI. |
| A recipe's `-c reasoning_effort=…` | The same: a context variable only. |
| An outer Codex session's own `-c model_reasoning_effort=…` | A setting of that session. It is not inherited by a child launch. |
| `"model"` in `~/.claude/settings.json` | Read by Claude Code, but amplihack's `--model` outranks it. |

Issue #1619's 3 November comment reports the consequence of the first three:
a step assigned `high` ran as `xhigh`, because nothing carried the assignment
to the child. The step's model and effort now reach the child as real
arguments, and `amplihack recipe run --verbose` and the record file both show
what was sent.

## Overriding the mapping

### The override file

amplihack reads **one** override file, the first of these that exists:

1. the path in `model.tier_map`;
2. `$AMPLIHACK_HOME/model-tiers.toml`;
3. `~/.amplihack/model-tiers.toml`.

Its entries are merged cell by cell over the shipped mapping, so a file that
names one cell changes that cell and leaves the rest alone.

```toml
# ~/.amplihack/model-tiers.toml
version = 1

# Put reviews on the strongest model, leave fast and strong alone.
[claude.standard]
model = "claude-opus-5[1m]"

# Give this install's Codex a model name, and lower fast-tier effort.
[codex.fast]
model = "gpt-6.1-sol"
effort = "minimal"

# Fill in the Copilot column for this account.
[copilot.strong]
model = "claude-opus-5"
```

| Field | Meaning |
|-------|---------|
| `version` | `1`. A file with a version amplihack does not know is reported and the shipped mapping applies. |
| `<column>.<tier>.model` | The model id for that cell. Omit it to leave the cell with no model, which means "fall back". |
| `<column>.<tier>.effort` | The reasoning effort for that cell, from the accepted list. Omit it for none. |

Rules the file is held to:

- **A tier key must be `fast`, `standard` or `strong`.** Any other tier key is
  reported by name.
- **An unknown field inside a cell is reported**, so a typo such as
  `modle = …` is visible rather than ignored.
- **A model id is validated the same way `agent.binary` is**: at most 128
  bytes, no control characters or whitespace, none of `/`, `\`, `;`, `$`, a backtick, `'` or `"`,
  not starting with `-`, and drawn from letters, digits and
  `. _ [ ] @ : -`. That
  admits `claude-opus-5[1m]` and `claude-sonnet-5.5` and excludes anything
  that could act as another argument.
- **The file is held to the same trust rules as `~/.amplihack/config.toml`.**
  A file owned by another user, or writable by group or others, is ignored
  with one stderr line, and the shipped mapping applies. The settings in this
  file choose the model every agent step runs on and can retarget one named
  step, which is exactly what `model.force` and `model.tier_map` are gated
  for in the config file; the gate follows the settings rather than the file
  they happen to live in. Fix it with `chmod 600 ~/.amplihack/model-tiers.toml`.
- **The file is read once per process, through the same reader every other
  amplihack config file uses** (`user_config::safe::read_bounded`): at most
  64 KiB, a regular file only, opened non-blocking so a FIFO cannot stall the
  launch. A directory, a FIFO or a device file there is reported rather than
  read, and a dangling symlink is reported rather than mistaken for "no file".
  A symlink that resolves to a regular file **is** followed, exactly as
  `~/.amplihack/config.toml` is — this file is in your own amplihack home and
  is held to that home's trust rules, which the bullet above applies in full,
  not to a stricter one invented here.
- **A file amplihack cannot use changes nothing else.** A missing, malformed,
  over-size, unreadable or wrongly versioned file produces one stderr line
  naming the file and the reason, and then the shipped mapping applies and the
  run proceeds. The path is printed home-relative — `~/.amplihack/model-tiers.toml`,
  not `/home/alice/.amplihack/...` — because these lines get pasted into
  issues. It and any quoted key go through the same `display_safe` escaping
  and 200-character truncation as every other config diagnostic.

`model.tier_map` is a user-only key, like every path-valued key in amplihack.
A project file cannot name it: a cloned repository must not be able to choose
the file that drives amplihack's rewriting of recipe YAML, whose steps run
shell commands.

### Overriding one step's tier

A `[step_tiers]` table maps a step id to a tier word:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[step_tiers]
step-08-implement = "standard"
step-20-final-cleanup = "fast"
```

It lives in the override file above — the same single file as the mapping, so
`amplihack config get model.tier_map` always answers "which file is in force".

How it is applied: the step id is not visible at launch time — the recipe
runner does not tell a step's child process which step it is — so the
override is applied to the recipe instead. When `[step_tiers]` is non-empty,
`amplihack recipe run` writes patched copies of **only** the recipe files that
contain a named step id into a private directory under the run directory, and
points the runner at that directory first through
`AMPLIHACK_PACKAGE_RECIPE_DIR`, which is the runner's first search directory
and which amplihack sets nowhere else. The patch is one line per named step:
the step's `model:` line is rewritten, or added if it had none, to one of the
three words.

- With no `[step_tiers]` configured, nothing is staged, no extra environment
  variable is set, and the run behaves byte for byte as it did before.
- A step id must look like an identifier: it starts with a letter or digit,
  continues with letters, digits, `.`, `_` or `-`, and is at most 128
  characters. `.` and `..` are not step ids. Staged file names come only from
  the names of the recipe files amplihack found, never from the override file,
  and the staging directory is created with mode 0700 and its files with 0600.
- A step id that no recipe contains changes nothing: no recipe is copied
  for it, and the other named steps are still staged.
- **A value that is not one of the three words is reported by name and that
  one entry is dropped**; the rest of the table still applies and the step
  keeps the tier its recipe declares.
- Only one of the three tier words is ever written into a recipe file, so no
  text from an override file reaches a prompt or a shell body.

### Forcing one model everywhere

```sh
amplihack config set model.force claude-sonnet-5-5
# or, for one run:
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every launch then asks for that model, whatever tier the step declared, and
whatever the mapping says — including launches whose command line carries a
concrete `--model` of its own. The reasoning effort of the step's tier still
applies, so on Codex a `strong` step keeps `xhigh` and a `fast` step keeps
`low` while both run the forced model.

The forced value is validated exactly as an override-file model id is. The
run summary and `amplihack config list --show-origin` both name the origin,
so a model id in a later error traces back to this setting:

```text
model.force = "claude-sonnet-5-5"  # user (/home/alice/.amplihack/config.toml)
```

Where the pair lands: when the command line already carries a `--model`, the
forced value replaces that value in place. When it carries none, the pair is
inserted at the end of amplihack's own options — before any `--` on the
command line, never after it. Everything after `--` is forwarded to the
harness verbatim, so a pair written there would not be an option amplihack
set at all: with `-p` it would read as a second prompt and the launch would
be refused.

A gateway launch is the one place a forced model does not take effect; see
[Precedence](#precedence).

## Measuring what each step ran on

### The run summary section

`amplihack recipe run` prints a **Model tiers:** section after `Steps:`:

```text
Steps:
  ✓ autodrive-prerequisites (Prerequisites): completed [elapsed: 44s]
  ✓ step-05-architecture (Architecture): completed [elapsed: 6.2s]
  ✓ step-10-pre-commit-review (Pre-commit review): completed [elapsed: 2m 1s]

Model tiers:
  autodrive-prerequisites: fast → claude-haiku-5-5
  step-05-architecture: strong → claude-opus-5[1m]
  step-10-pre-commit-review: standard → claude-opus-5[1m] (requested: claude-sonnet-5-5)
```

On Codex, where the tier changes the effort rather than the model:

```text
Model tiers:
  autodrive-prerequisites: fast → codex default model, effort low (the `codex` entry names no model)
  step-05-architecture: strong → codex default model, effort xhigh (the `codex` entry names no model)
```

After a fall-back, the line names both what was asked for and what ran:

```text
Model tiers:
  step-06-documentation: standard → copilot default model (no entry in the `copilot` column)
```

The existing step status line and its `    Output: ` line are untouched by
this section, and the section is printed after the `Steps:` block.
`amplifier-bundle/tools/autodrive_loop.sh` reads those two lines with awk, and
a test runs that awk over the formatter's output.

The section is printed only by `--format table`. `--format json` and
`--format yaml` are the runner's own envelope and are unchanged; read
`model-tiers.jsonl` for the same facts in a machine-readable form.

### When a line names a step

**Step ids appear only when the counts agree**: the number of launches
recorded equals the number of agent steps the runner reports, and the two are
then paired in order. No step id ever reaches a child process, so this is a
correlation and amplihack will not pretend otherwise.

Any of these makes the counts disagree, and then every line in the section
prints by sequence number with one line saying why:

- a **`parallel_group`**, whose launches have no determinate order;
- a **rate-limit retry**, which relaunches one step — the runner runs a step up
  to `1 + AMPLIHACK_RATELIMIT_MAX_RETRIES` times, so one step can produce
  several launches with no parallel group anywhere;
- a **recovery or resumed run**, which adds launches for steps the summary
  already reports.

```text
Model tiers:
  launch 1: strong → claude-opus-5[1m]
  launch 2: standard → claude-sonnet-5-5
  launch 3: standard → claude-sonnet-5-5
  3 tiered launches against 2 recipe entries that ran: amplihack cannot tell which launch belongs to which, so none is named here. A retry, a parallel group or a resumed run all do this.
```

Giving a launch its step id directly needs a change in the recipe runner,
which is tracked separately. Until then the stderr line always names the
launch, never the step.

### The record file

Each launch appends one JSON line to
`$AMPLIHACK_RUN_DIR/model-tiers.jsonl`:

```json
{"v":1,"seq":3,"run_id":"20261116-052900-ab12","harness":"codex","column":"codex","tier":"strong","requested_model":null,"effective_model":null,"requested_effort":"xhigh","effective_effort":"xhigh","fallback":"no_model"}
```

| Field | Meaning |
|-------|---------|
| `v` | Record schema version, `1` |
| `seq` | Launch sequence within the run, from 1 |
| `run_id` | The run id, from `AMPLIHACK_RUN_ID` |
| `harness` | The launch command: `claude`, `codex`, `copilot`, `rusty`, `rustyclawd`, `amplifier` |
| `column` | The mapping column used: `claude`, `codex` or `copilot` |
| `tier` | `fast`, `standard`, `strong`, or `null` for a launch that declared none |
| `requested_model` | The model the mapping, or `model.force`, asked for |
| `effective_model` | The model on the launched command line after every rule above, or `null` when it carries none |
| `requested_effort` | The effort the mapping asked for |
| `effective_effort` | The effort actually passed, or `null` |
| `fallback` | `no_column`, `no_mapping`, `no_model`, `gateway`, or `null` when the launch did not fall back |

Those eleven fields are the whole schema, pinned by a test. The file carries
no command line, no environment, no prompt text, no working directory and no
path, because run artefacts are pasted into issues and PRs.

Operational details: the file is written only when `AMPLIHACK_RUN_DIR` is set
to a non-empty value naming a directory that already exists — amplihack never
creates one for this — with a fixed name, mode 0600, without following a
symlink, and under an advisory lock (`flock`). **`seq` is read from the file
and incremented inside that same lock, in the same critical section as the
append**, so two concurrent launches can neither interleave lines nor claim
the same number. A lock amplihack cannot take means the record is skipped,
never that a launch waits. With `AMPLIHACK_RUN_DIR` unset **or empty** —
`amplihack` sets it to the empty string for steps outside a run scope, and
empty means unset to every consumer — nothing is written and nothing is
created.

Counting a run's spend from the file:

```sh
jq -r '[.tier // "none", .effective_model // "harness default"] | @tsv' \
  "$AMPLIHACK_RUN_DIR/model-tiers.jsonl" | sort | uniq -c
```

```text
      9	fast	claude-haiku-5-5
      6	standard	claude-sonnet-5-5
      8	strong	claude-opus-5[1m]
```

## Tiers in the bundled recipes

Tiers are assigned to the **29 bundled recipe files reachable from
`default-workflow`, `investigation-workflow`, `smart-orchestrator` and the
auto-drive family** — 100 agent steps. The rule used:

| Tier | Steps |
|------|-------|
| `fast` | Prerequisite lookups including the QA-prerequisite lookup, task read-back, loop-result verification, summaries, cleanup, status and marker writing — `autodrive-prerequisites`, `step-20-final-cleanup`, `summarize` and their kind |
| `standard` | Code, security and philosophy reviews, documentation, PR body writing, test authoring — `step-10-pre-commit-review`, `step-10b-security-review`, `step-10c-philosophy-check`, `step-06-documentation`, `step-07-write-tests` |
| `strong` | Design, implementation, debugging, recovery decision steps, orchestration, classification and planning — `step-05-architecture`, `step-08-implement`, `step-11b-implement-feedback` |

**The other 25 bundled recipes with agent steps are unchanged**, and their 168
agent steps keep today's behaviour exactly: no `model:` field, no tier, no
`--model` from this feature, and amplihack's existing default chain decides as
it always did. That set includes the largest recipes in the bundle —
`oxidizer-workflow` (24 agent steps), `n-version-workflow` (20),
`debate-workflow` (15), `code-atlas` (13), `cascade-workflow` (10),
`auto-workflow` (7), `quality-audit-cycle` (7) and the `consensus-*` family.
They are left alone deliberately: tiering a recipe means reading every step
and deciding what it is for, and a wrong tier is worse than no tier. Tiering
any of them later is an edit to that recipe, not a change to this contract.

Design, implementation and debugging stay on `strong`. Nothing an instance
pinned at the top Codex effort is lowered: `strong` is `xhigh` there, which is
what every step was getting by default anyway; the saving comes from the
`standard` and `fast` steps.

**The three-word vocabulary has a deliberate limit.** It cannot express
"orchestration at `xhigh`, builders at `high`" as a fourth Codex tier. The way
to get that split is `[step_tiers]`: put the builder steps on `standard`,
which is `high` on Codex, and leave the orchestration steps on `strong`. If a
fourth word turns out to be needed, that is a change to this vocabulary and to
this document, not a value an instance can invent.

## Combined review for small diffs

On a small change, the separate code, security and philosophy review passes of
the refactor-review phase can run as one combined pass:

```sh
amplihack config set review.combined_small_diff true
amplihack config set review.combined_diff_threshold 120
```

| Key | Default | Meaning |
|-----|---------|---------|
| `review.combined_small_diff` | `false` | Whether a small diff may use one combined pass |
| `review.combined_diff_threshold` | `200` | Changed lines at or below which a diff counts as small |

With `review.combined_small_diff` left at `false`, the three passes run
separately, exactly as they did before this feature. Turned on, a phase whose
changed-line count is at or below the threshold runs one combined review step
on the `standard` tier instead of `step-10-pre-commit-review`,
`step-10b-security-review` and `step-10c-philosophy-check`, and a larger diff
still runs all three.

This applies to `workflow-refactor-review.yaml` only.
`workflow-pr-review.yaml` is deliberately left alone: its passes are
PR-level review rather than the review phase of a build.

## What amplihack does not check

amplihack has **not** verified that a model in the mapping is available to
your account, your plan or your harness install. The mapping is a statement
of what you want asked for. A harness that does not have the model says so in
its own output, and that output is the evidence — amplihack does not launch
again with a different model, and does not probe a model list.

If a tier's model is one your account does not carry, remove its entry in the
override file. That is what makes amplihack stop asking for it.

## Diagnostics

Every line goes to stderr, so `amplihack <cli> -p` keeps stdout for the
agent's own output. Like the existing issue #1421 model line, each line that
names a model ends with the advice clause telling you which setting controls
it, so the two cannot drift apart.

| Situation | Line |
|-----------|------|
| A tier gave the launch a model | ``amplihack: model tier `fast` on `claude`: passing `--model claude-haiku-5-5`. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.`` |
| A tier gave the launch an effort and no model | ``amplihack: model tier `strong` on `codex`: passing `-c model_reasoning_effort=xhigh`; the model stays `codex`'s own. …`` |
| A tier gave the launch both | ``amplihack: model tier `strong` on `codex`: passing `--model gpt-6.1-sol` and `-c model_reasoning_effort=xhigh`. …`` |
| The command line already set the effort | ``amplihack: model tier `strong` on `codex`: leaving the `-c model_reasoning_effort` already on the command line alone.`` |
| A forced model | ``amplihack: model.force is set, so `claude` is launched with `--model claude-sonnet-5-5` whatever the step asked for. Unset model.force or AMPLIHACK_FORCE_MODEL to restore per-step tiers.`` |
| A forced value amplihack will not write | ``amplihack: model.force is ignored: begins with `-`, so it could act as another argument. The step's own tier applies.`` |
| A tier with no entry in its column | ``amplihack: model tier `strong` has no entry in the `copilot` column, so `copilot` runs on its own default model. …`` |
| A harness with no column | ``amplihack: model tier `strong` has no `gemini` column, so `gemini` runs on its own default model. …`` |
| A cell that names no model | ``amplihack: model tier `strong` names no model in the `claude` column, so `claude` runs on its own default model. …`` — followed by the existing issue #1421 line naming what was passed |
| A gateway launch | ``amplihack: model tier `strong` is not applied to `claude`: the LiteLLM gateway routes on the model name. Set AMPLIHACK_LITELLM_MODEL to change it.`` |
| An effort value amplihack does not accept | ``amplihack: model tiers: `codex.fast`: `turbo` is not an accepted reasoning effort; it is ignored and the model still applies. The accepted values are minimal, low, medium, high, xhigh.`` |
| An override file amplihack cannot use | ``amplihack: ~/.amplihack/model-tiers.toml: is not valid TOML; ignored; the model tiers amplihack ships are used instead.`` |
| An override file another user can write | ``amplihack: ~/.amplihack/model-tiers.toml: ignored because the file is writable by group or others; the model tiers amplihack ships are used instead.`` |
| A tier word amplihack does not know | ``amplihack: model tiers: `claude.heavy`: `heavy` is not a tier; the entry is ignored. The tiers are fast, standard, strong.`` |
| An unknown field in a cell | ``amplihack: model tiers: `claude.fast`: unknown field `modle`; it is ignored. A cell may name `model` and `effort`.`` |
| A model id amplihack will not write | ``amplihack: model tiers: `claude.fast`: the model begins with `-`, so it could act as another argument; the entry is ignored and the shipped value applies.`` |
| A `[step_tiers]` value that is not a tier | ``amplihack: model tiers: step_tiers.step-08-implement: `heavy` is not a tier; the step keeps the tier its recipe declares.`` |
| A per-step override that cannot be staged | ``amplihack: model tiers: the per-step tier override could not be staged (<reason>); the recipes run with the tiers they declare.`` |

A step id no recipe contains stages nothing for that step; the other named
steps are still staged, and the run proceeds.

One launch gets one line about its tier. A launch that fell back and still
got something from the tier — every shipped Codex cell, which names an effort
and no model — gets the line naming what it got, and its record carries
`"fallback": "no_model"`; a launch the tier gave nothing gets the fall-back
line instead. Every launch with a tier writes exactly one record, and the
stderr line and the record's `fallback` value are derived from one value, so
they cannot disagree.

The run summary and the record file may name the step, under the rule in
[When a line names a step](#when-a-line-names-a-step); the stderr line always
names the launch, because the recipe runner does not tell a child process
which step it is.

## Related

- [User Configuration](./user-config.md) — `model.force`, `model.tier_map`,
  the `review.*` keys and the `amplihack config` command
- [AMPLIHACK_* Environment Variable Audit](./amplihack-env-vars.md) —
  `AMPLIHACK_FORCE_MODEL`, `AMPLIHACK_DEFAULT_MODEL`,
  `AMPLIHACK_LITELLM_MODEL` and `AMPLIHACK_RUN_DIR`.
  `AMPLIHACK_RATELIMIT_*` is the recipe runner's own, not amplihack's, so it
  is not in that audit
- [Put Recipe Steps on Model Tiers](../howto/set-model-tiers.md) — the
  task-by-task version of this reference
- [Active Agent Binary](./active-agent-binary.md) — which CLI a launch uses,
  which decides the mapping column
- [External LiteLLM Gateway](../concepts/external-litellm-boundary.md) — the
  route that ignores tiers
- [Recipe Command](./recipe-command.md) — `amplihack recipe run`, its output
  formats and detached runs
