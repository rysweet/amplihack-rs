# Model Tiers (Reference)

A recipe agent step says how much model it needs, not which model to use. It
writes one of three words, `fast`, `standard` or `strong`, and amplihack turns
that word into a model and, where the harness has one, a reasoning effort for
the CLI it is about to launch. Claude Code, Codex and Copilot CLI each have
their own column in the mapping, so the same recipe runs on all of them
without naming a model that a harness or an account does not have.

## The three tier words

| Tier | Meant for |
|------|-----------|
| `fast` | Mechanical steps: workspace preparation, prerequisite lookups, task read-back, loop-result verification, summaries, cleanup |
| `standard` | Code, security and philosophy reviews, documentation, PR body writing, test authoring |
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

The tier reuses `model:` because that is the one step field the pinned recipe
runner turns into a launch argument. The runner writes the value on the
passthrough side itself, so resolving a tier is delete, then decide: amplihack
finds the `--model <tier-word>` pair the runner wrote, removes it, and puts its
own answer in its place. Without the delete the tier word would reach the CLI
as a literal model name.

## The shipped mapping

The defaults ship as a TOML table compiled into the binary. They are
configuration, not a code path, and they name no host, no file-system path and
no account.

| Tier | Claude-compatible | Codex | Copilot CLI |
|------|-------------------|-------|-------------|
| `strong` | `claude-opus-5[1m]` | no model named, effort `xhigh` | no model named |
| `standard` | `claude-sonnet-5-5` | no model named, effort `high` | no model named |
| `fast` | `claude-haiku-5-5` | no model named, effort `low` | no model named |

Claude ids are concrete, never aliases, because an alias is resolved by the CLI
and so depends on the CLI version (issue #1421); `claude-opus-5[1m]` is the
same value as amplihack's built-in default, asserted by a test. Codex names no
model, because Opus and Sonnet do not exist there and any Codex model would
guess at the account, so its tiers differ by effort alone. The Copilot section
ships empty, so all three Copilot tiers fall back until an instance fills it
in.

The `claude` column covers every Claude-compatible tool: `amplihack claude`,
`rusty`, `rustyclawd` and `amplifier`. The `codex` and `copilot` columns cover
one command each, and a harness with no column takes the fall-back below.
Unknown column names in an override file are kept rather than reported, so a
file written for a newer amplihack still loads on an older one. `step_tiers` is
a reserved table name, matched before that rule.

## Precedence

Highest first:

| Rank | Source | Effect |
|------|--------|--------|
| 0 | The runner's `AMPLIHACK_RATELIMIT_FALLBACK_AUTO_MODEL` final attempt | Rewrites the step's model to the literal `auto` before amplihack runs |
| 1 | `model.force` / `AMPLIHACK_FORCE_MODEL` | Replaces a tier word or a concrete id, and supplies a model where the command has none |
| 2 | An explicit concrete `--model <id>` on the launched command | Forwarded verbatim, unless a model is forced |
| 3 | A per-step tier override (`[step_tiers]`) | Rewrites the tier the named step declares, before the run starts |
| 4 | Override-file entries | Replace the matching cells of the shipped mapping |
| 5 | The shipped mapping | The table above |
| 6 | The harness default | What a launch carrying no tier gets |

Rank 0 is not amplihack's. With that variable set, which it is not unless you
set it, the runner substitutes `auto` on its last rate-limit attempt; `auto` is
not a tier word, so it is forwarded as a concrete id and the tier's Codex
effort goes with it.

Rank 3 acts at a different moment from the ranks above it, rather than
competing with them over one command line. A `[step_tiers]` entry is applied to
a staged copy of the recipe before the run starts, so the `--model <tier-word>`
pair the runner later writes already carries the overridden word. It sits below
ranks 1 and 2 because a forced model, or an explicit concrete `--model`, still
replaces whatever the overridden tier resolves to.

A LiteLLM gateway launch ignores tiers. One goes through the gateway when the
tool is one the gateway routes (Claude Code, RustyClawd or Copilot CLI) and
`AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` or
`AMPLIHACK_LITELLM_MODEL` is set. The gateway routes on the model name and was
never configured for the ids in the shipped mapping, so amplihack removes the
tier's `--model` pair and its existing chain writes `AMPLIHACK_LITELLM_MODEL`
in its place. Codex is not routed by the gateway, so its tiers apply as usual.

`model.force` outranks an explicit `--model`, because the forced value comes
from your own user config or environment. A forced launch keeps the tier's
reasoning effort, so forcing one model across a mixed-tier recipe does not
flatten Codex effort to one value.

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
its own default.

The step never fails because of a fall-back. There is no probe: availability is
a statement in the mapping, not a question asked over the network, and a tier
with no entry for a harness is what "that model is not available there" looks
like in configuration. There is no second launch and there are no timers.

Every launch that carried a tier, falling back or not, prints one line to
stderr and appends one line to [the record file](#the-record-file). The line
names the launch, the tier, the model asked for, and the substitute: either a
concrete id, or the harness's own default model where the mapping supplies
none. The record's `fallback` field is `null` when the launch did not fall
back, and the line and the field come from one value.

## Reasoning effort, per harness

Codex is the only harness with a launch control for effort, and amplihack
passes the tier's value as Codex's own `-c model_reasoning_effort=<effort>`,
prepended so `exec` remains the first positional argument. If your own command
already carries that key, yours is left alone. Claude Code and Copilot CLI have
no such argument, so an effort written in their column is recorded as requested
rather than effective.

Accepted effort values are `minimal`, `low`, `medium`, `high` and `xhigh`. That
list is amplihack's allowlist, not one read from Codex: the value becomes the
right-hand side of a `-c` argument, so only these five words are written.
Whether the installed Codex accepts the word is Codex's decision, visible in
its own output. A value from an override file that is not one of the five is
reported and dropped, and the tier's model still applies. Model and effort
resolve independently, so an unavailable or unmapped model drops only the model
argument.

A recipe's `-c model=...` and `-c reasoning_effort=...` are context variables,
readable in prompt templates and not passed to the launched CLI, and an outer
Codex session's own `-c model_reasoning_effort=...` is a setting of that
session, not inherited by a child launch. Issue #1619's 3 November comment
reports the consequence: a step assigned `high` ran as `xhigh`. Model and
effort now reach the child as real arguments.

## Overriding the mapping

### The override file

amplihack reads one override file, the first of these that exists: the path in
`model.tier_map`, then `$AMPLIHACK_HOME/model-tiers.toml`, then
`~/.amplihack/model-tiers.toml`. Entries merge cell by cell over the shipped
mapping, so a file naming one cell changes that cell alone.

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
set the cell's two values, and omitting `model` leaves the cell with no model,
which means "fall back". A tier key that is not one of the three words, and an
unknown field inside a cell such as `modle`, are each reported by name and
ignored. A model id is held to a check of its own: at most 128 bytes, not
starting with `-`, and drawn from ASCII letters and digits plus `.`, `_`, `-`,
`[`, `]`, `@` and `:`. That admits `claude-opus-5[1m]` and excludes whitespace,
control characters and anything a shell or a command line would read as
structure.

The file is held to the trust rules of `~/.amplihack/config.toml`: one owned by
another user, or writable by group or others, is ignored with one stderr line
and the shipped mapping applies. The check is made on the open file rather than
on its path, so it cannot be made to describe a different object from the one
read. Correct a rejected file with `chmod 600 ~/.amplihack/model-tiers.toml`.
It is read once per process through the reader every other amplihack config
file uses: at most 64 KiB, regular files only, opened without following a
symlink and non-blocking, so a FIFO cannot stall the launch. A missing,
malformed, over-size, unreadable or wrongly versioned file gives one stderr
line naming the file and the reason, after which the shipped mapping applies
and the run proceeds; the path is printed home-relative, because these lines
get pasted into issues.

`model.tier_map` takes an absolute path with no `..` component; a relative
path, or one written with `~`, is refused by `amplihack config set` with the
requirement printed. It is user-only, like every path-valued key in amplihack.
A project file cannot name it: a cloned repository must not choose the file
that drives amplihack's rewriting of recipe YAML, whose steps run shell
commands.

The search order leaves the environment the selection route `model.tier_map`
was denied: `$AMPLIHACK_HOME` is read from the environment, so whatever sets it
chooses `$AMPLIHACK_HOME/model-tiers.toml` and therefore the `[step_tiers]`
table that patches recipe files. That is accepted rather than closed, because
`$AMPLIHACK_HOME` already selects the `config.toml` holding `model.force`, and
anything able to set it can set `AMPLIHACK_FORCE_MODEL` or edit the recipe
itself; the chosen file is still held to the ownership and permission rules
above.

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
private directory under the run directory, and points the runner at that
directory first through `AMPLIHACK_PACKAGE_RECIPE_DIR`, the runner's first
search directory, which amplihack sets nowhere else. The patch is one line per
named step: that step's own `model:` line is rewritten, or added if it had
none, to one of the three words. With no `[step_tiers]` configured nothing is
staged, no extra environment variable is set, and the run behaves as before.

Where a recipe file name appears both in a search directory and as the recipe
named on the command line, the search directory's copy is the one patched, so
staging cannot put a repository-local file of that name ahead of the bundled
recipe the runner would otherwise have loaded. A patched copy is re-read as
YAML before it is written, and one that no longer parses is dropped with a
line, leaving that recipe on the tier it declares.

A step id must look like an identifier: a letter or digit, then letters,
digits, `.`, `_` or `-`, at most 128 characters; `.` and `..` are not step ids.
Staged file names come only from the recipe files amplihack found, never from
the override file, and the staging directory is mode 0700 with its files 0600.
A step id no recipe in the run declares is reported by name and changes
nothing; a key that is not an identifier is reported and ignored; a value that
is not a tier word is reported and dropped, leaving that step on the tier its
recipe declares. Only one of the three tier words is ever written into a recipe
file, so no text from an override file reaches a prompt or a shell body.

### Forcing one model everywhere

```sh
amplihack config set model.force claude-sonnet-5-5
# or, for one run:
AMPLIHACK_FORCE_MODEL=claude-sonnet-5-5 amplihack recipe run default-workflow
```

Every launch then asks for that model, whatever tier the step declared,
including launches whose command line carries a concrete `--model` of its own.
The tier's reasoning effort still applies, so on Codex a `strong` step keeps
`xhigh` while running the forced model. The forced value is validated exactly
as an override-file model id is, and `amplihack config list --show-origin`
names its origin. Where the command line already carries a `--model`, the
forced value replaces that value in place; where it carries none, the pair is
inserted at the end of amplihack's own options, before any `--`. A gateway
launch is the one place a forced model does not take effect.

## Measuring what each step ran on

`amplihack recipe run` prints a **Model tiers:** section after `Steps:`:

```text
Model tiers:
  autodrive-prerequisites: fast → claude-haiku-5-5
  step-05-architecture: strong → codex default model, effort xhigh (the `codex` entry names no model)
  step-06-documentation: standard → copilot default model (no entry in the `copilot` column)
```

The existing step status line and its `    Output: ` line are untouched, and
the section follows the `Steps:` block, which is what
`amplifier-bundle/tools/autodrive_loop.sh` reads with awk. It names at most 50
launches one by one and then says how many more there were, and is printed only
by `--format table`; `--format json` and `--format yaml` are the runner's own
envelope and are unchanged.

Step ids appear only when the counts agree: the number of launches recorded
equals the number of agent steps the runner reports, and the two are then
paired in order. No step id reaches a child process, so this is a correlation.
A `parallel_group`, a rate-limit retry, or a recovered or resumed run makes the
counts disagree, and then every line prints by sequence number, with one line
saying why.

### The record file

Each launch appends one JSON line to `$AMPLIHACK_RUN_DIR/model-tiers.jsonl`:

```json
{"v":1,"seq":3,"run_id":"20261116-052900-ab12","harness":"codex","column":"codex","tier":"strong","requested_model":null,"effective_model":null,"requested_effort":"xhigh","effective_effort":"xhigh","fallback":"no_model"}
```

The eleven fields are the whole schema, pinned by a test: `v`, the schema
version; `seq`, the launch sequence within the run, from 1; `run_id`, from
`AMPLIHACK_RUN_ID`; `harness`, the launch command; `column`, the mapping column
used; `tier`, one of the three words or `null`; `requested_model` and
`requested_effort`, what the mapping or `model.force` asked for;
`effective_model` and `effective_effort`, what reached the command line, each
`null` when it carries none; and `fallback`, one of `no_column`, `no_mapping`,
`no_model`, `gateway`, or `null`. The file carries no command line, no
environment, no prompt text, no working directory and no path, because run
artefacts are pasted into issues and PRs.

It is written only when `AMPLIHACK_RUN_DIR` names a directory that already
exists, with a fixed name, mode 0600, without following a symlink, and under an
advisory lock. `seq` is read and incremented inside that lock, in the same
critical section as the append, so two concurrent launches can neither
interleave lines nor claim the same number. A lock amplihack cannot take means
the record is skipped, never that a launch waits.

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
did. Tiering a recipe means reading every step and deciding what it is for, and
a wrong tier is worse than no tier.

Nothing an instance pinned at the top Codex effort is lowered: `strong` is
`xhigh` there, which is what every step was getting by default, so the saving
comes from the `standard` and `fast` steps. The three-word vocabulary cannot
express "orchestration at `xhigh`, builders at `high`" as a fourth Codex tier;
`[step_tiers]` is the way to get that split.

## Combined review for small diffs

On a small change, the separate code, security and philosophy passes of the
refactor-review phase can run as one combined pass.

| Key | Default | Meaning |
|-----|---------|---------|
| `review.combined_small_diff` | `false` | Whether a small diff may use one combined pass |
| `review.combined_diff_threshold` | `200` | Changed lines at or below which a diff counts as small |

With `review.combined_small_diff` left at its default the three passes run
separately, exactly as they did before this feature. Turned on, a phase whose
changed-line count is at or below the threshold runs one combined review step
on the `standard` tier instead of `step-10-pre-commit-review`,
`step-10b-security-review` and `step-10c-philosophy-check`; a larger diff still
runs all three. Both keys are user-only, for the reason `model.force` is: the
step reads them from inside the worktree under review, and a repository must
not choose whether its own security pass runs. This applies to
`workflow-refactor-review.yaml` only; `workflow-pr-review.yaml` is left alone,
because its passes are PR-level review rather than the review phase of a build.

## Diagnostics

Every line goes to stderr, so `amplihack <cli> -p` keeps stdout for the agent's
own output, and each line ends by naming either the setting that controls it or
the values it would have accepted.

```text
amplihack: model tier `fast` on `claude`: passing `--model claude-haiku-5-5`. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tier `strong` has no entry in the `copilot` column, so `copilot` runs on its own default model. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
amplihack: model tiers: step_tiers.step-99-polish: no recipe in this run declares a step with that id, so nothing is overridden and every step keeps the tier its recipe declares. See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.
```

One launch gets one line about its tier, naming the launch rather than the
step. The `[step_tiers]` lines are printed once per run by `amplihack recipe
run`, before anything is staged. Every diagnostic here leaves the run going: a
tier is a preference about which model runs a step, and no mistake in it may
stop a run of a hundred agent steps. amplihack does not verify that a model in
the mapping is available to your account, your plan or your harness install; a
harness that does not have the model says so in its own output.

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
