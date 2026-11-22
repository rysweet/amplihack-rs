# Model Tiers (Reference)

A recipe agent step says how much model it needs, not which model to use. It
writes one of three words, `fast`, `standard` or `strong`, and amplihack turns
that word into a model and, where the harness has one, a reasoning effort for
the CLI it is about to launch. Claude Code, Codex and Copilot CLI each have
their own column in the mapping, so the same recipe runs on all of them without
naming a model that a harness or an account does not have.

## Contents

- [The three tier words](#the-three-tier-words)
- [Declaring a tier on a step](#declaring-a-tier-on-a-step)
- [The shipped mapping](#the-shipped-mapping)
- [Precedence](#precedence)
- [Falling back to the harness default](#falling-back-to-the-harness-default)
- [Reasoning effort, per harness](#reasoning-effort-per-harness)
- [Overriding the mapping](#overriding-the-mapping)
- [Measuring what each step ran on](#measuring-what-each-step-ran-on)
- [Tiers in the bundled recipes](#tiers-in-the-bundled-recipes)
- [Combined review for small diffs](#combined-review-for-small-diffs)
- [Diagnostics](#diagnostics)

## The three tier words

| Tier | Meant for |
|------|-----------|
| `fast` | Mechanical steps: workspace preparation, prerequisite lookups, task read-back, loop-result verification, summaries, cleanup |
| `standard` | Code, security and philosophy reviews, documentation, PR body writing, test authoring, and the evidence steps that verify a PR's head or adjudicate a review verdict |
| `strong` | Design, implementation, debugging, recovery decisions, orchestration, classification, planning |

These are the only three. There is no `light`, `medium` or `heavy` tier and no
alias for any of the words. Matching is case-insensitive on a trimmed value;
any other value is treated as a concrete model id and forwarded as typed.

## Declaring a tier on a step

An agent step writes the tier word in its existing `model:` field. A `bash`
step runs no model and declares none.

```yaml
steps:
  - id: "autodrive-prerequisites"
    agent: "amplihack:core:builder"
    model: "fast"

  - id: "step-05-architecture"
    agent: "amplihack:architect"
    model: "strong"
```

The tier reuses `model:` because that is the one step field the pinned runner
turns into a launch argument: it writes `--model <tier-word>`, and amplihack
finds that pair, removes it, and puts its own answer in its place.

## The shipped mapping

The defaults ship as a TOML table compiled into the binary. They are
configuration, not a code path, and they name no host, no file-system path and
no account.

| Tier | Claude-compatible | Codex | Copilot CLI |
|------|-------------------|-------|-------------|
| `strong` | `claude-opus-5[1m]`, effort `xhigh` | no model named, effort `high` | no model named |
| `standard` | `claude-sonnet-5-5` | no model named, effort `high` | no model named |
| `fast` | `claude-haiku-5-5` | no model named, effort `low` | no model named |

Claude ids are concrete, never aliases: an alias is resolved by the CLI, so its
meaning depends on the CLI version (issue #1421). Claude `strong` carries an
effort because the work it is meant for (design, planning, orchestration and
complex coding) is where a deeper reasoning budget pays for itself; `standard`
and `fast` name no effort, so Claude Code applies its own default there. The
shipped `xhigh` raises token consumption on every `strong` step. An instance
that wants a lower cost lowers the cell, or the column's ceiling, in its own
file (see [setting a Claude effort](#setting-a-claude-effort)).

Codex names no model, because Opus and Sonnet do not exist there and any id
would guess at the account, so its tiers differ by effort alone. Codex `strong`
ships at `high`, not `xhigh`, because not every Codex model accepts `xhigh` and
a shipped default must not make a step fail; an instance whose model accepts it
raises the cell in its own file (see
[raising a Codex effort](#raising-a-codex-effort)). Copilot ships empty and
falls back until an instance fills it in.

The `claude` column covers every Claude-compatible tool: `amplihack claude`,
`rusty`, `rustyclawd` and `amplifier`. A harness with no column takes the
fall-back below. An unknown column name in an override file is kept rather than
reported, so a file written for a newer amplihack still loads on an older one;
`step_tiers` is a reserved table name, matched first.

## Precedence

Highest first:

| Rank | Source | Effect |
|------|--------|--------|
| 0 | The runner's `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` final attempt | Rewrites the step's model to the literal `auto` before amplihack runs |
| 1 | `model.force` / `AMPLIHACK_FORCE_MODEL` | Replaces a tier word or a concrete id, and supplies a model where the command has none |
| 2 | An explicit concrete `--model <id>` on the launched command | Forwarded verbatim, unless a model is forced |
| 3 | A set, non-blank `AMPLIHACK_DEFAULT_MODEL` | Replaces a tier word, on the `claude` column only |
| 4 | A per-step tier override (`[step_tiers]`) | Rewrites the tier the named step declares, before the run starts |
| 5 | Override-file cells | Replace the matching cells of the shipped mapping |
| 6 | The shipped mapping | The table above |
| 7 | The harness default model | What a launch still carrying no model gets |

Rank 0 is not amplihack's. With that variable set, which it is not unless you
set it, the runner substitutes `auto` for the step's model on its last
rate-limit attempt. `auto` is not a tier word, so amplihack forwards it as
typed and the launch has no tier at all: no model from the mapping, and no
reasoning effort either, since an effort belongs to a tier amplihack resolved
and `auto` is a model it did not choose. Nor does that launch append a record
line: a line is written for a launch that carried a tier word or a forced
model, and `auto` is neither.

Rank 3 keeps the lever an operator had before tiers existed, on the launches it
reached then: Claude-compatible tools, the `claude` column. On an account
without Sonnet or Haiku it is the simplest way to keep every step on a model the
account has. It replaces a tier word and only a tier word, so a concrete
`--model` you typed is still forwarded as typed, and no `--model` pair is ever
inserted where the command line had none. Codex and Copilot CLI launches do
not read it. The value is held to the same model-id check as `model.force`. See
[where `AMPLIHACK_DEFAULT_MODEL` applies](#where-amplihack_default_model-applies).

Rank 2 sits above rank 3, so a recipe that pins a concrete `--model` on a step
keeps it whatever you set. If that is not what you want, use `model.force`.

Rank 4 acts at a different moment, rather than competing over one command line:
a `[step_tiers]` entry is applied to a staged copy of the recipe before the run
starts, so the `--model <tier-word>` pair the runner later writes already
carries the overridden word. It sits below ranks 1 to 3 because each of those
still replaces whatever the overridden tier resolves to.

A LiteLLM gateway launch ignores tiers. One goes through the gateway when the
tool is one the gateway routes (Claude Code, RustyClawd or Copilot CLI) and
`AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` or
`AMPLIHACK_LITELLM_MODEL` is set. The gateway routes on the model name and was
never configured for the ids in the shipped mapping, so amplihack removes the
tier's `--model` pair and its existing chain writes `AMPLIHACK_LITELLM_MODEL`
in its place. Ranks 1 and 3 are dropped on that path for the same reason. The
tier's reasoning effort is not dropped: an effort is a session setting, not a
routing key, so a gateway-routed Claude launch still receives `--effort` from
its tier. Codex is not routed by the gateway, so its tiers apply as usual.

`model.force` outranks an explicit `--model`, because the forced value comes
from your own user config or environment. A forced launch keeps the tier's
reasoning effort, so forcing one model across a mixed-tier recipe does not
flatten the effort to one value on Claude or on Codex.

## Falling back to the harness default

A tier falls back when the harness has no column, the tier has no entry in that
column, the entry names an effort but no model (as every shipped Codex entry
does), or the launch goes through the gateway. Falling back removes the tier
word from the command line and never substitutes a model name of its own. On a
Claude-compatible launch amplihack's existing chain then applies, exactly as
for a launch that never mentioned a model: `AMPLIHACK_LITELLM_MODEL` on a
gateway launch, else `AMPLIHACK_DEFAULT_MODEL`, else the built-in
`claude-opus-5[1m]`, with an empty `AMPLIHACK_DEFAULT_MODEL` giving no
`--model` at all. On Codex and Copilot CLI nothing is passed, so the CLI uses
its own default; `AMPLIHACK_DEFAULT_MODEL` is not consulted there.

The step never fails because of a fall-back, and there is no probe:
availability is a statement in the mapping, not a question asked over the
network. There is no second launch and there are no timers. Every launch that
carried a tier, falling back or not, prints to stderr and appends one line to
[the record file](#the-record-file); the line and the record's `fallback` field
come from one value.

## Reasoning effort, per harness

Claude Code and Codex each have a launch control for effort, and amplihack
passes the tier's value to whichever one it is launching:

| Harness | Argument written | Prepended so that |
|---------|------------------|-------------------|
| Claude-compatible (`claude`, `rusty`, `rustyclawd`, `amplifier`) | `--effort <effort>` | the pair sits inside amplihack's own options, ahead of any positional prompt |
| Codex | `-c model_reasoning_effort=<effort>` | `exec` remains the first positional argument |

If your own command already carries the argument, yours is left alone, and one
stderr line says so. On Claude that means an `--effort <value>` or an
`--effort=<value>` before any bare `--`; an `--effort` written after a bare `--`
is prompt text for Claude and does not count. On Codex it means the
`model_reasoning_effort` key. Copilot CLI has no such argument, so an effort
written in its column is recorded as requested rather than effective.

When amplihack adds `--effort` to a Claude-compatible launch, it reads the
installed binary's `--help` once and keeps the flag only if the text lists it.
See
[a Claude Code that does not know `--effort`](#a-claude-code-that-does-not-know---effort).

Accepted effort values differ by harness, weakest first:

| Column | Accepted values |
|--------|-----------------|
| `claude` | `low`, `medium`, `high`, `xhigh`, `max` |
| `codex` | `minimal`, `low`, `medium`, `high`, `xhigh` |
| any other column, including `copilot` | all six words: `minimal`, `low`, `medium`, `high`, `xhigh`, `max` |

Those lists are amplihack's allowlists, not ones read from the CLIs: the value
becomes part of a command-line argument, so only listed words are ever written.
Matching is exact apart from ASCII case, so `XHIGH` is the same word as
`xhigh`: there is no trimming and no prefix matching, and a value carrying a
stray space or newline is reported rather than guessed at. Before issue #2916
an effort value was trimmed, so a Codex cell with `effort = " high"` was
accepted; it is now reported, and the cell's model still applies.
`minimal` is a Codex word, so it is refused for Claude, and `max` is a Claude
word, so it is refused for Codex. A column amplihack writes nothing onto accepts
the union, because no word there can be wrong on a command line. A value from an
override file that is outside its column's list is reported, naming the column
and tier and listing that column's accepted words, and is dropped; the tier's
model still applies and the launch succeeds. Model and effort resolve
independently, so an unavailable or unmapped model drops only the model
argument.

A recipe's `-c model=...` and `-c reasoning_effort=...` are context variables,
readable in prompt templates and not passed to the launched CLI, and an outer
Codex session's own `-c model_reasoning_effort=...` is not inherited by a child
launch. Issue #1619's 3 November comment reports the consequence: a step
assigned `high` ran as `xhigh`. The model and the effort of a tier now both
reach the launched CLI as real arguments.

### Setting a Claude effort

The shipped `[claude.strong]` cell names both a model and an effort:

```toml
[claude.strong]
model = "claude-opus-5[1m]"
effort = "xhigh"
```

A `strong` step on a Claude-compatible tool then launches with
`--model claude-opus-5[1m] --effort xhigh`. `standard` and `fast` name no
effort, so they launch with a `--model` and no `--effort`.

To change it for one host, write the cell in your own
[override file](#the-override-file). The cell replaces the shipped one whole, so
it restates the model it wants and the effort it wants:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

# Plan and design on a Fable model, at the strongest effort.
[claude.strong]
model = "claude-fable-5-2"
effort = "max"
```

A cell that names a model and no `effort` launches that model with no
`--effort`, and Claude Code applies its own default. That is the way to clear
the shipped `xhigh` on one host:

```toml
[claude.strong]
model = "claude-opus-5[1m]"
```

To keep the shipped model and only lower the effort, restate the model:

```toml
[claude.strong]
model = "claude-opus-5[1m]"
effort = "high"
```

A concrete model id written by hand on a step (`--model claude-opus-5[1m]`
rather than a tier word) gets no tier, no effort and no record line, as before:
`--effort` appears only where a tier word was present.

### A Claude Code that does not know `--effort`

A Claude Code old enough not to accept `--effort` would refuse the whole launch
with its own message. To avoid that, a launch to which amplihack has added
`--effort` first runs the resolved binary's `--help` once, with the same
handling as the existing `--version` probe. When the help text does not list
`--effort`, amplihack takes the pair back off the command line, prints one
stderr line saying the step runs without it, and records no effective effort.
The step then runs on the tier's model. This is a fall-back, not a gate: a help
text that cannot be read leaves the flag in place, and nothing here ends the
launch. No probe runs when the tier carries no effort or when your command
already sets `--effort`.

To stop amplihack writing the flag at all on such a host, and save the probe,
write the cell with no `effort` key, which clears the shipped effort:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[claude.strong]
model = "claude-opus-5[1m]"
```

Or lower every Claude tier at once with a
[ceiling](#when-an-account-refuses-an-effort), which bounds an effort without
removing it.

`rusty`, `rustyclawd` and `amplifier` share the `claude` column and so receive
the same `--effort`. They already receive Claude's `--model` with Claude-only
ids, so the column is already treated as command-line compatible with Claude.
Their `--help` is read in the same way, and if one of them refuses the flag
anyway, clear the cell as above.

### Raising a Codex effort

The shipped `strong` and `standard` tiers both ask Codex for `high`. Where your
Codex model accepts `xhigh`, raise `strong` in your own
[override file](#the-override-file):

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
effort = "xhigh"
```

A `strong` step then launches with `-c model_reasoning_effort=xhigh`, and the
other two tiers are unchanged.

### When an account refuses an effort

amplihack does not ask a model whether it will take an effort: that needs a
live call, and a tier must not cost a request, a retry or a timer. So the limit
is something you declare about your own account, as a `max_effort` inside a
column:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex]
max_effort = "medium"
```

Every tier of that column is then written at its own effort or at the ceiling,
whichever is lower. A ceiling also bounds a cell you raised: with `strong` at
`xhigh` and `max_effort = "high"`, a `strong` step runs at `high`. A ceiling
never raises an effort, and never reaches a harness that has no launch control
for one. The same holds on the `claude` column: with `[claude] max_effort =
"high"`, a `strong` step that asks for `xhigh` launches with `--effort high`.
The step runs: stepping an effort down is not a fall-back and does not fail the
launch. The record keeps both values, `requested_effort` beside
`effective_effort`, and the one line the launch already prints says the tier's
effort was lowered.

A `max_effort` that is not one of its column's accepted words (see the table
above) is reported where you wrote it and leaves the column with no ceiling at
all. Applying some default cap on a typo would silently change what every step
of that column runs at. A ceiling is validated against its own column, so
`[claude] max_effort = "minimal"` is reported and `[codex] max_effort = "max"`
is too.

A ceiling is a cost and policy setting for the efforts amplihack chooses. It is
advisory against an effort you type yourself: a command that already carries
`--effort` keeps it, as described above, and the ceiling does not lower it.

The other way is to lower the effort in the cell itself, which the
[override file](#the-override-file) does cell by cell:

```toml
[codex.strong]
effort = "medium"
```

The difference is reach: a cell changes one tier, a ceiling bounds all three.

## Overriding the mapping

### The override file

amplihack reads one override file, the first of these that exists: the path in
`model.tier_map`, then `$AMPLIHACK_HOME/model-tiers.toml`, then
`~/.amplihack/model-tiers.toml`. Entries merge cell by cell over the shipped
mapping, so a file naming one cell changes that cell alone. This file is the
only source of a table beyond the shipped one: a repository you have cloned
cannot set a model or an effort. Nothing merges
inside a cell: a cell you write replaces the shipped cell whole, and a value it
leaves out is not taken from the shipped one. Every shipped Codex cell and the
shipped `[claude.strong]` cell carry an effort, so a `[codex.<tier>]` or
`[claude.strong]` cell that names a `model` restates the `effort` it wants;
without one, that tier launches with no effort argument.

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
```

`version` must be `1`; a version amplihack does not know is reported and the
shipped mapping applies. `<column>.<tier>.model` and `<column>.<tier>.effort`
set the cell's two values. Omitting `model` leaves the cell with no model,
which means "fall back", and omitting `effort` leaves it with no effort.
`<column>.max_effort` is the column's
[effort ceiling](#when-an-account-refuses-an-effort); it is reserved, so it is
never read as a tier word, and inside `[step_tiers]` the same spelling is an
ordinary step id. A tier key that is not one of the three words, and an unknown
field inside a cell such as `modle`, are each reported by name and ignored. A
model id is held to a check of its own: at most 128 bytes, not starting with
`-`, and drawn from ASCII letters and digits plus `.`, `_`, `-`, `[`, `]`, `@`
and `:`.
That admits `claude-opus-5[1m]` and excludes whitespace, control characters and
anything a shell or a command line would read as structure.

The file is held to the trust rules of `~/.amplihack/config.toml`: one owned by
another user, or writable by group or others, is ignored with one stderr line,
correctable with `chmod 600 ~/.amplihack/model-tiers.toml`. The check is made
on the open file rather than on its path, so it cannot be made to describe a
different object from the one read. It is read once per process by the reader
every other amplihack config file uses: at most 64 KiB, regular files only,
opened non-blocking so a FIFO cannot stall the launch, and a symlink followed
to the file actually judged. A missing, malformed, over-size, unreadable or
wrongly versioned file gives one stderr line naming the file, home-relative,
and the reason, after which the shipped mapping applies and the run
proceeds.

`model.tier_map` takes an absolute path with no `..` component; a relative path,
or one written with `~`, is refused by `amplihack config set`. It is user-only,
like every path-valued key: a cloned repository must not choose the file that
drives amplihack's rewriting of recipe YAML. `$AMPLIHACK_HOME` is still read
from the environment, so whatever sets it chooses the tier file; the same
variable already selects the `config.toml` holding `model.force`, and the
chosen file is still held to the rules above.

### Overriding one step's tier

A `[step_tiers]` table maps a step id to a tier word, in the same file:

```toml
[step_tiers]
step-08-implement = "standard"
step-20-final-cleanup = "fast"
```

The step id is not visible at launch time, because the runner does not tell a
child process which step it is, so the override is applied to the recipe
instead. When `[step_tiers]` is non-empty, `amplihack recipe run` writes
patched copies of only the recipe files containing a named step id into a
private directory under the run directory, and gives the runner that directory
as its **first `-R`**. The patch is one line per named step: that step's own
`model:` line is rewritten, or added if it had none, to one of the three words.
With no `[step_tiers]` configured nothing is staged, no extra argument is
added, and the run behaves as before.

`-R` is the delivery route because it is the only one that wins. The runner
resolves a recipe name from its `-R` directories first, then an anchored search
from the recipe it is running, then `AMPLIHACK_PACKAGE_RECIPE_DIR`; amplihack
forwards the bundle as a `-R` of its own, so a staged copy offered through that
variable was always outranked by the recipe it had been patched from.

A `-R` directory is searched for a *name*, while the recipe named on the
command line is passed as a path, so `-R` alone never reaches that one. When it
is among the files staged, the positional argument becomes its patched copy and
the directory the original sits in is appended as the **last** `-R`, so a
recipe resolving a sibling sub-recipe by name keeps finding it. Where a name
appears both in a search directory and as the recipe named on the command line,
the search directory's copy is the one patched.

Only agent steps are patched, `model:` being a field the runner ignores on
every other kind of step; an entry naming one of those is reported by name and
changes nothing. The step kind is read the way the runner reads it: the
declared `type:` if there is one, else a `recipe:` makes a sub-recipe, an
`agent:` makes an agent step, and a `prompt:` with no `command:` makes an agent
step. A patched copy is re-read as YAML before it is written, and one that no
longer parses is dropped with a line.

A step id must look like an identifier: a letter or digit, then letters,
digits, `.`, `_` or `-`, at most 128 characters; `.` and `..` are not step ids.
Staged file names come only from the recipe files amplihack found, never from
the override file, and the staging directory is mode 0700 with its files 0600.
A step id no staged recipe declares is reported by name and changes nothing;
the line names the causes it cannot tell apart. A key that is not an
identifier, and a value that is not a tier word, are reported and dropped. Only
one of the three tier words is ever written into a recipe file, so no text from
an override file reaches a prompt or a shell body.

### Where `AMPLIHACK_DEFAULT_MODEL` applies

Before tiers existed, `AMPLIHACK_DEFAULT_MODEL` chose the model for every
Claude-compatible launch and was never read by Codex or Copilot CLI. It keeps
that scope. Set to a non-blank value, it replaces the model a tier maps to on
the `claude` column:

```sh
AMPLIHACK_DEFAULT_MODEL='claude-opus-5[1m]' amplihack recipe run default-workflow ...
```

- **It applies to the `claude` column only**: `amplihack claude`, `rusty`,
  `rustyclawd` and `amplifier`. A Codex or Copilot CLI launch does not read it
  and prints no line about it, so a Claude id set for Claude Code never reaches
  a harness that has no such model. The step resolves its tier from the
  mapping as if the variable were unset.
- **It replaces a tier word, and only a tier word.** A concrete `--model` a
  recipe or an operator typed is forwarded as typed (issue #1527), and nothing
  is inserted where the command line carries no `--model` at all.
- **A blank or whitespace-only value decides nothing** and leaves the mapping
  in force. It still means "pass no `--model`" on a launch that has none.
- **It is held to the model-id check** described under
  [the override file](#the-override-file), on both the tiered and the untiered
  path. On a Claude-compatible launch a value outside it is reported, without
  being quoted, and the step's own tier applies; the launch proceeds. A dotted
  Claude id is rewritten to the hyphenated spelling (issue #1527).

It is not read at all while a LiteLLM gateway variable is set. To displace a
concrete `--model` as well, use `model.force` below.

With `AMPLIHACK_DEFAULT_MODEL=claude-sonnet-5-5` set, a `strong` step receives:

| Harness | `--model` | Effort argument |
|---------|-----------|-----------------|
| Claude-compatible | `claude-sonnet-5-5` | `--effort xhigh` |
| Codex, shipped mapping | none; the cell names no model, so Codex's own runs | `-c model_reasoning_effort=high` |
| Codex, with the instance cell below | that cell's model | `-c model_reasoning_effort=high` |
| Copilot CLI, shipped mapping | none | none; no launch control, so an effort there is recorded as requested only |

To give a Codex tier a model, write the cell in your own `model-tiers.toml`:

```toml
# ~/.amplihack/model-tiers.toml
version = 1

[codex.strong]
model = "gpt-6.1-sol"
effort = "high"
```

The cell replaces the shipped `[codex.strong]` whole, so it restates
`effort = "high"`. A cell naming only `model` launches that model with no
effort argument, and Codex then applies its own default effort.

### Forcing one model everywhere

```sh
amplihack config set model.force claude-sonnet-5-5
# or, for one run:
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every launch then asks for that model, on every harness, whatever tier the step
declared, including a launch whose command line carries a concrete `--model` of
its own. That is the difference from `AMPLIHACK_DEFAULT_MODEL`, which leaves a
concrete id alone and applies to the `claude` column only. The tier's reasoning
effort still applies, so on Codex a `fast` step keeps `low`, and on Claude a
`strong` step keeps `xhigh`, while running the forced model. The value is validated exactly as an override-file model id is,
and `amplihack config list --show-origin` names its origin. Where the command
line carries a `--model` the forced value replaces it in place; where it
carries none the pair is inserted at the end of amplihack's own options, before
any `--`. A gateway launch is the one place a forced model does not take
effect.

## Measuring what each step ran on

`amplihack recipe run` prints a **Model tiers:** section after `Steps:`:

```text
Model tiers:
  autodrive-prerequisites: fast → claude-haiku-5-5
  step-04-design: strong → claude-opus-5[1m], effort xhigh
  step-05-architecture: strong → codex default model, effort high (the `codex` entry names no model)
  step-06-documentation: standard → copilot default model (no entry in the `copilot` column)
```

The step status line and its `    Output: ` line are untouched, and the section
follows the `Steps:` block, which is what
`amplifier-bundle/tools/autodrive_loop.sh` reads with awk. It names at most 50
launches one by one and then says how many more there were, and is printed only
by `--format table`; `--format json` and `--format yaml` are the runner's own
envelope and are unchanged.

Step ids appear only when the counts agree: the launches recorded match the
agent steps the runner reports, and the two are paired in order. No step id
reaches a child process, so this is a correlation, and it is the whole of what
the pinned runner allows. A `parallel_group`, a rate-limit retry, or a
recovered or resumed run makes the counts disagree, and then every line prints
by sequence number, with one line saying why.

### The record file

Each launch appends one JSON line to `$AMPLIHACK_RUN_DIR/model-tiers.jsonl`:

```json
{"v":1,"seq":3,"run_id":"20261116-052900-ab12","harness":"codex","column":"codex","tier":"strong","requested_model":null,"effective_model":null,"requested_effort":"high","effective_effort":"high","fallback":"no_model"}
```

The eleven fields are the whole schema, pinned by a test: `v`, the schema
version; `seq`, the launch sequence within the run, from 1; `run_id`, from
`AMPLIHACK_RUN_ID`; `harness`, the launch command; `column`, the mapping column
used; `tier`, one of the three words or `null`; `requested_model` and
`requested_effort`, what the mapping or an override asked for;
`effective_model` and `effective_effort`, what reached the command line, each
`null` when it carries none; and `fallback`, one of `no_column`, `no_mapping`,
`no_model`, `gateway`, or `null`. The line above is a `strong` Codex step on
the shipped mapping: the cell names an effort and no model, so the model falls
back with `no_model` while the effort is passed. The file carries no command
line, no environment, no prompt text, no working directory and no path.

It is written only when `AMPLIHACK_RUN_DIR` names a directory that already
exists, with a fixed name, mode 0600, without following a symlink, and under an
advisory lock. `seq` is read and incremented inside that lock, in the same
critical section as the append, so two concurrent launches neither interleave
lines nor claim the same number. A lock amplihack cannot take skips the record
rather than making a launch wait.

```sh
jq -r '[.tier // "none", .effective_model // "harness default"] | @tsv' \
  "$AMPLIHACK_RUN_DIR/model-tiers.jsonl" | sort | uniq -c
```

## Tiers in the bundled recipes

Tiers are assigned to the bundled recipe files reachable from
`default-workflow`, `investigation-workflow`, `smart-orchestrator` and the
auto-drive family, by the rule in [the table above](#the-three-tier-words). The
other bundled recipes with agent steps keep today's behaviour: no `model:`
field, no tier, and amplihack's existing default chain deciding as it always
did.

On the shipped Codex column `strong` and `standard` both run at `high`, so the
saving there comes from the `fast` steps at `low`. Issue #1619's 3 November
comment asks for orchestration at `xhigh` and builders at `high`. The
three-word vocabulary has no fourth Codex tier for that: an instance
[raises `strong` to `xhigh`](#raising-a-codex-effort) and moves the builder
steps it wants at `high` to `standard` with `[step_tiers]`.

## Combined review for small diffs

On a small change, the separate code, security and philosophy passes of the
refactor-review phase can run as one combined pass.

| Key | Default | Meaning |
|-----|---------|---------|
| `review.combined_small_diff` | `false` | Whether a small diff may use one combined pass |
| `review.combined_diff_threshold` | `200` | Changed lines at or below which a diff counts as small |

Left at its default the three passes run separately. Turned on, a phase whose
changed-line count is at or below the threshold runs one combined review step
on the `standard` tier instead of `step-10-pre-commit-review`,
`step-10b-security-review` and `step-10c-philosophy-check`; a larger diff still
runs all three. Both keys are user-only, for the reason `model.force` is: the
step reads them from inside the worktree under review, and a repository must
not choose whether its own security pass runs. It applies to
`workflow-refactor-review.yaml` only, PR-level review being a different thing.

The `review-mode` step decides by running
`amplifier-bundle/tools/workflow_review_mode_step.sh` from the installed
bundle. The step looks for the script under `$AMPLIHACK_HOME`, then
`~/.copilot`, then `~/.amplihack`, and nowhere else; the script finds the
worktree helper it calls, `worktree_setup_path.sh`, in the same three places.
A copy of either in the repository under review or in the step's working
directory is never run, so a repository cannot ship a script that prints
`true`. When none of the three locations has the script, the step prints
`false`.

The script measures the worktree the run set up for the build. It counts the
committed and uncommitted changed lines there against the merge base with the
run's base branch, or `origin/main` when none is set; a binary file counts as 0
lines and untracked files are not counted. A threshold that is not a whole
number is read as `200`. Only an exact `true` from the script selects the
combined pass. A missing script, a run with no worktree, an error or any other
output keeps the three separate passes, with the reason on stderr, and the step
still succeeds.

## Diagnostics

Every line goes to stderr, so `amplihack <cli> -p` keeps stdout for the agent's
own output, and each line ends by naming either the setting that controls it or
the values it would have accepted.

```text
amplihack: model tier `fast` on `claude`: passing `--model claude-haiku-5-5`. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tier `strong` on `claude`: passing `--model claude-opus-5[1m]` and `--effort xhigh`. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tier `strong` on `codex`: passing `-c model_reasoning_effort=high`; the model stays `codex`'s own. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tier `strong` on `claude`: leaving the `--effort` already on the command line alone.
amplihack: model tier `strong` has no entry in the `copilot` column, so `copilot` runs on its own default model. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tiers: step_tiers.step-99-polish: amplihack staged no recipe declaring a step with that id, so nothing is overridden and every step keeps the tier its recipe declares. Usually the id is mistyped; a recipe amplihack could not read, one larger than the size bound, and one past the candidate cap all look the same from here. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: AMPLIHACK_DEFAULT_MODEL is set, so `claude` is launched with `--model claude-sonnet-5-5` in place of the model its tier maps to. Unset it to restore the mapping. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tier `strong` on `codex`: passing `-c model_reasoning_effort=medium`; the model stays `codex`'s own. The tier asks for `high`; the column's `max_effort` lowered it. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
```

One launch gets at most two lines about its tier, each naming a distinct fact,
and names the launch rather than the step. The `[step_tiers]` lines are printed
once per run by `amplihack recipe run`, before anything is staged. Every
diagnostic here leaves the run going. amplihack does not verify that a mapped
model is available to your account, your plan or your harness install; a
harness that does not have it says so in its own output.

Under `amplihack recipe run` the per-launch lines come from each step's child
launch, and the pinned runner keeps an agent step's stderr only when that step
fails. For a run whose steps succeed, the `Model tiers:` section and the record
file are where the tier, the model, the effort and any fall-back appear; the
record also keeps a lowered effort's `requested_effort`.

## Related

- [Put Recipe Steps on Model Tiers](../howto/set-model-tiers.md), the
  task-by-task version of this reference
- [User Configuration](./user-config.md), for `model.force`, `model.tier_map`,
  the `review.*` keys and the `amplihack config` command
- [AMPLIHACK_* Environment Variable Audit](./amplihack-env-vars.md), for
  `AMPLIHACK_FORCE_MODEL`, `AMPLIHACK_DEFAULT_MODEL`, `AMPLIHACK_LITELLM_MODEL`
  and `AMPLIHACK_RUN_DIR`
- [Active Agent Binary](./active-agent-binary.md), for which CLI a launch uses
  and so which mapping column applies
- [External LiteLLM Gateway](../concepts/external-litellm-boundary.md), the
  route that ignores tiers
