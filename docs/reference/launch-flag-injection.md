---
title: Launch Flag Injection — Reference
description: Automatically injected agent CLI flags, ownership requirements, and caller overrides.
doc_type: reference
last_updated: 2026-11-18
---

# Launch Flag Injection — Reference

When `amplihack` starts `claude`, `copilot`, `codex`, or `amplifier`, it builds
the subprocess command line by combining flags it injects automatically with any
extra arguments the user supplied. This document describes every flag that
`amplihack` injects, the conditions under which each is injected, and how users
can override the defaults.

## Contents

- [Injected flags overview](#injected-flags-overview)
- [--dangerously-skip-permissions](#-dangerously-skip-permissions)
- [--model](#-model)
- [--resume and --continue](#-resume-and-continue)
- [--deny-tool (Copilot CLI)](#-deny-tool)
- [Codex agent-step flags](#codex-agent-step-flags)
  - [Owned review launch directories](#owned-review-launch-directories)
- [Extra args passthrough](#extra-args-passthrough)
- [Complete command-line assembly](#complete-command-line-assembly)
- [Python launcher parity](#python-launcher-parity)
- [Related](#related)

---

## Injected flags overview

| Flag | Injected when? | Applicable tools | Override mechanism |
|------|----------------|-------------------|-------------------|
| `--dangerously-skip-permissions` | `--skip-permissions` passed AND tool is Claude-compatible | `claude`, `rusty`, `rustyclawd`, `amplifier` | omit `--skip-permissions` |
| `--model <value>` | tool is Claude-compatible AND user did not pass `--model`, unless `AMPLIHACK_DEFAULT_MODEL` is set to an empty value outside the LiteLLM gateway path. Unset, the value is `claude-opus-5[1m]`; see [--model](#-model) | `claude`, `rusty`, `rustyclawd`, `amplifier` | pass `--model`, or set `AMPLIHACK_DEFAULT_MODEL=` (empty) to pass none |
| `--resume` | only when `amplihack launch --resume` | `launch` only (not `claude`) | pass `--resume` to the `launch` subcommand |
| `--continue` | only when `amplihack launch --continue` | `launch` only (not `claude`) | pass `--continue` to the `launch` subcommand |
| `--deny-tool shell(<prefix>:*)` | always, once per entry in the [dangerous-command policy](./dangerous-command-policy.md); nothing in your own arguments suppresses one | `copilot` | none for a shipped prefix. `policy.deny_commands` only adds, and `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` leaves these flags in place. See [--deny-tool](#-deny-tool) |
| `--sandbox danger-full-access` | argv after `--` contains an `exec`/`e` subcommand AND the caller chose no sandbox | `codex` | pass your own sandbox (`--sandbox`, `--sandbox=`, `-s`, `-s<mode>`, `-s=<mode>`, `-c sandbox_mode=…`); see [Codex agent-step flags](#codex-agent-step-flags) |
| `--dangerously-bypass-hook-trust` | argv after `--` contains an `exec`/`e` subcommand AND the flag is not already present | `codex` | none; it is added once on every Codex `exec` launch |
| `--skip-git-repo-check` | `exec`/`e` runs in a validated, amplihack-owned review launch directory AND the flag is not already present | `codex` | automatic from existing ownership evidence; caller-supplied flags are preserved |

---

## --dangerously-skip-permissions

`amplihack` passes `--dangerously-skip-permissions` to the tool subprocess only
when **both** conditions are met:

1. The user passed `--skip-permissions` on the amplihack command line.
2. The target tool is **Claude-compatible** (`claude`, `rusty`, `rustyclawd`, or `amplifier`).

Tools that are not Claude-compatible (`copilot`, `codex`) never receive this
flag because they do not support it.

```sh
# User explicitly opts in:
amplihack launch --skip-permissions

# amplihack spawns (the --model is amplihack's default; see --model below):
claude --dangerously-skip-permissions --model claude-opus-5[1m]
```

```sh
# Without --skip-permissions, the flag is NOT injected:
amplihack claude

# amplihack spawns:
claude --model claude-opus-5[1m]
```

```sh
# Non-Claude tools never receive it, even with --skip-permissions:
amplihack copilot --skip-permissions

# amplihack spawns:
copilot <extra_args...>
```

**Rationale:** The `--dangerously-skip-permissions` flag bypasses Claude's
interactive confirmation prompts. It is gated behind an explicit opt-in
(`--skip-permissions`) so that users in trusted automated environments can
suppress the prompt, while interactive sessions retain the safety check.

**Python launcher note:** The Python launcher in `amplihack/launcher/core.py`
may behave differently. Verify Python launcher behavior independently.

---

## --model

`amplihack` passes a `--model` to **Claude-compatible** tools (`claude`, `rusty`,
`rustyclawd`, `amplifier`) unless you pass one yourself or turn it off.
Non-Claude tools (`copilot`, `codex`) never receive an injected `--model` and use
their own default model selection. (Codex `exec` launches receive sandbox and
hook-trust flags instead; see [Codex agent-step flags](#codex-agent-step-flags).)

This section summarises the rules. The full reference for the value, the
dotted-id rewrite and every stderr line amplihack prints about the model is
[`AMPLIHACK_DEFAULT_MODEL`](./environment-variables.md#amplihack_default_model).

### Which value is passed

Highest precedence first:

| Situation | What the tool receives |
|---|---|
| `--model <id>` or `--model=<id>` on the amplihack command line | that argument, exactly as typed; amplihack adds no `--model` of its own |
| a [LiteLLM gateway variable](./environment-variables.md#external-litellm-gateway-variables) is set | `--model` with `AMPLIHACK_LITELLM_MODEL`, unchanged |
| `AMPLIHACK_DEFAULT_MODEL` is empty or whitespace-only | no `--model`; the tool picks, and the `"model"` in `~/.claude/settings.json` takes effect |
| `AMPLIHACK_DEFAULT_MODEL` is set to an id | `--model` with that id, trimmed; a dotted Claude id is rewritten to hyphens |
| `AMPLIHACK_DEFAULT_MODEL` is unset | `--model claude-opus-5[1m]`, amplihack's built-in default |

### Default: a concrete model id

With neither `--model` on the command line nor `AMPLIHACK_DEFAULT_MODEL` in the
environment, amplihack passes its built-in default:

```sh
# User runs:
amplihack claude

# amplihack spawns:
claude --model claude-opus-5[1m]
```

**Why a concrete id and not an alias (issue #1421):** amplihack used to force
`--model opus[1m]`. That string is an alias resolved by the tool, not by
amplihack, and what it resolves to depends on the tool's version. On one
install it resolved to the retired `claude-opus-4-1-20250805`, so every agent
step failed with `API Error: 404 ... model: claude-opus-4-1-20250805`, an id
the user had never chosen and could not find in any config file or binary. A
concrete id cannot drift that way: a given Claude Code version either accepts
`claude-opus-5[1m]` or fails naming that exact string.

Because amplihack passes `--model` by default, its choice outranks the
`"model"` in `~/.claude/settings.json`. To let the tool and that file decide,
set the variable to an empty value:

```sh
AMPLIHACK_DEFAULT_MODEL= amplihack claude

# amplihack spawns:
claude
```

### Pinning a model via environment variable

Set `AMPLIHACK_DEFAULT_MODEL` to pin a model for every session without changing
the command line:

```sh
export AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4-5
amplihack claude

# amplihack spawns:
claude --model claude-sonnet-4-5
```

A dotted Claude id, the spelling GitHub Copilot CLI uses, is rewritten to the
hyphenated id Claude Code accepts (issue #1527):

```sh
export AMPLIHACK_DEFAULT_MODEL='claude-opus-5.5[1m]'
amplihack claude

# amplihack spawns:
claude --model claude-opus-5-5[1m]
```

The exact form that is rewritten, and the values left alone, are listed under
[`AMPLIHACK_DEFAULT_MODEL`](./environment-variables.md#amplihack_default_model).

For teams or CI environments that standardise on one model:

```yaml
# .github/workflows/ai-tasks.yml
env:
  AMPLIHACK_DEFAULT_MODEL: "claude-sonnet-4-5"
  AMPLIHACK_NONINTERACTIVE: "1"

steps:
  - run: amplihack claude --print 'Run the lint checks'
    # spawns: claude --model claude-sonnet-4-5 --print 'Run the lint checks'
```

### The stderr line

Whenever amplihack adds `--model`, it prints one line to stderr naming the model
and where it came from, so a later "model not found" error can be traced back to
it:

```text
amplihack: passing `--model claude-sonnet-4-5` to `claude` (from AMPLIHACK_DEFAULT_MODEL). Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty value to let claude choose its own default model.
```

The source reads `amplihack's built-in default` when the variable is unset, and
the line also names the original spelling when a dotted id was rewritten. On
the LiteLLM gateway path it names `AMPLIHACK_LITELLM_MODEL` as both the source
and the variable to change. The environment variable reference quotes each
form.

### Override via command-line flag

Pass `--model` directly and amplihack adds none of its own;
`AMPLIHACK_DEFAULT_MODEL` is ignored. The value is forwarded exactly as typed
and is never rewritten:

```sh
amplihack claude --model claude-haiku-4-5

# amplihack spawns:
claude --model claude-haiku-4-5
```

A dotted Claude id given to `--model` is also forwarded unchanged. Claude model
ids use hyphens, and the launched tool may not report the problem itself
(issue #1527), so amplihack prints a warning to stderr naming the hyphenated
spelling:

```sh
amplihack claude --model claude-opus-5.5

# amplihack spawns:
claude --model claude-opus-5.5
# and warns: ... Use `--model claude-opus-5-5`.
```

There is no warning for `amplihack copilot` or `amplihack codex`, since the
dotted spelling is GitHub Copilot CLI's own, and none on the LiteLLM gateway
path, where the model is a gateway route name and a dot in it may be correct.

Detection is exact: an argument equal to `--model`, or one starting with
`--model=`, counts as an explicit model. Arguments that only begin with
`--model`, such as `--model-config`, do not.

### Supported model identifiers

Apart from the dotted-id rewrite of `AMPLIHACK_DEFAULT_MODEL`, amplihack does
not validate the model string; it forwards it to the tool. Aliases such as
`opus[1m]`, `sonnet`, and `haiku` are resolved by the tool, and which concrete
model each maps to changes with the tool's version. This document deliberately
does not tabulate those mappings: a table here would go stale exactly the way
the old hardcoded alias did. Ask the tool (`claude --help`, or its release
notes) for the current list.

---

## --resume and --continue

These flags are passed through only when the user explicitly requests them on
the `launch` subcommand. They are never injected automatically.

**Important:** The `claude` subcommand does **not** support `--resume` or
`--continue`. Only `launch` exposes these flags.

```sh
amplihack launch --resume --skip-permissions
# spawns: claude --dangerously-skip-permissions --model claude-opus-5[1m] --resume

amplihack launch --continue --skip-permissions
# spawns: claude --dangerously-skip-permissions --model claude-opus-5[1m] --continue
```

The `claude`, `copilot`, `codex`, and `amplifier` subcommands do not support
`--resume` or `--continue`.

---

## Codex agent-step flags

Recipe agent steps on Codex run `amplihack codex -- exec …`.
When the arguments after `--` contain an `exec` subcommand (or its alias `e`),
amplihack inserts the following baseline flags immediately after that token.
An [owned review launch directory](#owned-review-launch-directories) also
receives `--skip-git-repo-check`:

The command examples below assume execution in a Git checkout.

```sh
amplihack codex -- exec --output-last-message /d0/run/final-message -

# amplihack spawns:
codex exec --sandbox danger-full-access --dangerously-bypass-hook-trust \
  --output-last-message /d0/run/final-message -
```

For `exec resume`, the flags go before `resume`, because `--sandbox` is not an
`exec resume` option; codex-cli 0.166.0 rejects it after `resume` with
`unexpected argument '--sandbox' found`:

```sh
amplihack codex -- exec resume --last -
# → codex exec --sandbox danger-full-access --dangerously-bypass-hook-trust resume --last -
```

The `exec` token is found by scanning root-level arguments from the start. The
values of root options that take one (`-c/--config`, `-m/--model`,
`-p/--profile`, `-C/--cd`, `-s/--sandbox`, `-a/--ask-for-approval`,
`-i/--image`, `--add-dir`, `--enable`, `--disable`, `--local-provider`) are
skipped, `--x=y` and attached short forms are single tokens, and a bare `--`
ends the scan. As in clap, one `=` after a short option is dropped, so
`-s=read-only` is `-s read-only`. A `-c` key is trimmed before it is compared,
so `-c 'sandbox_mode = "read-only"'` is a sandbox choice. A `-` followed by a
non-ASCII character is passed through for Codex to reject. Whether an
option takes a value comes from the per-scope Codex option tables in
`crates/amplihack-launcher/src/prompt_delivery/codex_options.rs`
(`codex_options::takes_value`): an option takes a value if the root, `exec`,
`exec resume` or `exec fork` table says so. The launcher's mode parser
(`codex_args.rs`) and the prompt rewrite (`codex_exec_args.rs`) read the same
tables, which are checked against the saved codex-cli help. If no `exec` is
found, the argv is passed through unchanged.

The flags that choose a sandbox on their own are `--approve-for-me`,
`--dangerously-bypass-approvals-and-sandbox` and its hidden alias `--yolo`.
The list is taken from `codex exec --help` for codex-cli 0.166.0, a copy of
which is kept in
`crates/amplihack-cli/src/commands/launch/fixtures/codex-exec-help-0.166.0.txt`;
a unit test fails if the list and that copy disagree. `--full-auto` is not
on the list because codex-cli 0.140.0 to 0.166.0 reject it.

The following table describes sandbox and hook-trust injection. The separate
ownership rule below determines whether `--skip-git-repo-check` is also added.

| Caller argv (after `--`) | What amplihack adds |
|---|---|
| `exec --output-last-message F -` | both flags after `exec` |
| `e -` | both flags after `e` |
| `-m gpt-5.5 exec -` | both flags after `exec`; `-m gpt-5.5` stays first |
| `exec --sandbox read-only -` | `--dangerously-bypass-hook-trust` only |
| `-s danger-full-access exec -` | `--dangerously-bypass-hook-trust` only |
| `exec -c sandbox_mode=read-only -` | `--dangerously-bypass-hook-trust` only |
| `exec --sandbox=read-only -`, `exec -sread-only -`, `exec -s=read-only -`, `exec --config=sandbox_mode=read-only -`, `exec -c=sandbox_mode=read-only -` | `--dangerously-bypass-hook-trust` only |
| `exec -c 'sandbox_mode = "read-only"' -` | `--dangerously-bypass-hook-trust` only; the key is trimmed |
| `exec --approve-for-me -` | `--dangerously-bypass-hook-trust` only; `--approve-for-me` selects the workspace-write sandbox and codex-cli 0.166.0 refuses it alongside `--sandbox` |
| `exec --dangerously-bypass-approvals-and-sandbox -` | `--dangerously-bypass-hook-trust` only; the caller's flag is forwarded unchanged |
| `exec -c sandbox_workspace_write.network_access=false -` | `--sandbox danger-full-access` and `--dangerously-bypass-hook-trust`; the caller's `-c` is forwarded unchanged, is not a sandbox choice and restricts nothing under full access |
| `exec --dangerously-bypass-hook-trust -` | `--sandbox danger-full-access`; hook trust is not repeated |
| `-p ci exec -` | both flags; a profile is not a sandbox choice, and the command-line `--sandbox` overrides the profile's `sandbox_mode` |
| (none), `resume --last`, `"Explain this repository"` | nothing; interactive argv is unchanged |
| `-c exec`, `-m exec`, `-- exec` | nothing; `exec` here is a value or a positional, not the subcommand |

**Notice (issue #2010).** When amplihack adds `--sandbox danger-full-access`,
it writes one line to stderr before Codex starts. The line lists the sandbox
and hook-trust flags added, in the order they appear in the argv. The two possible lines are:

| Caller already set | Line written to stderr |
|---|---|
| nothing | `amplihack: codex exec: added --sandbox danger-full-access, --dangerously-bypass-hook-trust (#1985)` |
| `--dangerously-bypass-hook-trust` | `amplihack: codex exec: added --sandbox danger-full-access (#1985)` |

The line is written once per launch, and only to stderr, so a step's stdout
and its `--output-last-message` file are unchanged. The line is built from
fixed text only: it never contains the caller's arguments, paths, profile
names, prompt text or anything read from `config.toml`, so it is safe to keep
in CI logs.

**Where the line is seen.** The line is reliably seen only when a person runs
`amplihack codex -- exec` directly, or when a caller captures that process's
stderr itself. In a recipe run it usually is not seen. Under the recipe runner
pinned in `claude-plugin/recipe-runner.rev` (recipe-runner-rs `4d5187e`,
`src/adapters/cli_subprocess.rs`), an agent step's stderr is not streamed to
the recipe log:

- the runner writes it to a file under `/tmp/amplihack-agent-stderr/`;
- when the step succeeds, the runner deletes that file, so the line is gone;
- when the step fails, the error quotes only the last 4000 bytes of the file.
  The notice is the first line written to stderr, so after a long Codex run it
  is usually not in that excerpt. The file is kept, and its path is printed
  after `--- stderr-log:`, so the line can still be found there;
- when a failed attempt is retried because of a rate limit, its file is deleted
  before the retry.

Issue #2066 tracks keeping or surfacing `amplihack:` lines from agent steps.
Until it is fixed, finding no line in a recipe log does not mean no flags were
added; the table above is the record of what an agent step receives.

No line is written when:

- the caller chose a sandbox, in any of the forms listed above. The hook-trust
  flag is still added in that case, without a line. Issue #2048 tracks
  reporting it; issue #2009 is limited to whether the flag is added at all;
- the argv has no `exec`/`e` subcommand, including an empty argv,
  `resume --last`, `-c exec` and `-- exec`.

There is no option or environment variable to turn the line off. It is
informational: the spawned argv remains the authoritative record of what Codex
received, and nothing in amplihack reads or acts on the line.

amplihack never adds `--dangerously-bypass-approvals-and-sandbox` or `--yolo`,
and never writes any of these settings to `config.toml`. There is no
environment variable to turn the injection off; pass your own sandbox to change
it. Because the caller's sandbox wins, untrusted text (prompts, issue bodies)
must reach Codex on stdin, never in argv.

**Rationale (issues #1985 and #2317):** Codex agent steps run with the same
trust as Claude agent steps, which are launched with
`--dangerously-skip-permissions`. Under the earlier `workspace-write` sandbox a
step in a linked git worktree could not commit, because the worktree's index
and refs live under the main checkout's `.git/`, outside the writable root. An
explicit `--sandbox` also overrides any `sandbox_mode` in the host
`config.toml`, so every agent step gets the same mode. The amplihack
PreToolUse hooks apply the same deny rules to Claude and Codex steps; they
check commands and are not a filesystem or network sandbox. Codex skips
untrusted hooks without saying so, which would turn off the command-check hook
(software-factory #149), so hook trust is bypassed. Full access means a step
can write `.codex/hooks.json`, `~/.codex/`, `.git/hooks` and credential
stores, and a hook planted by one step runs in later steps without a trust
prompt. The execution-directory preconditions are in
[Sandbox and hook trust for agent steps](../howto/install-codex-plugin.md#sandbox-and-hook-trust-for-agent-steps).

### Owned review launch directories

Codex normally requires a Git checkout or a trusted project for unattended
execution. Auto-drive's Crusty context and review steps, and merge-ready
review steps, run in a separate, amplihack-created launch directory that
contains no Git checkout. For these invocations amplihack automatically adds
`--skip-git-repo-check` immediately after `exec` or its alias `e`.

Eligibility comes from the existing review ownership manifest and live
ownership validation. The normalized child execution directory must equal
the validated generation's recorded launch path exactly. The launcher's
own working directory does not determine eligibility. Any agent step using
that same owned launch directory receives the same treatment, independently
of its recipe name.

| Child execution directory | Automatically add `--skip-git-repo-check`? |
| --- | --- |
| Recorded launch directory of a live, validated review generation | Yes, for `exec`/`e` |
| Ordinary Git checkout | No |
| Linked Git worktree or the review generation's checkout | No |
| Unowned directory outside Git | No |
| Directory with missing, unreadable, malformed, stale, retired or unrelated ownership evidence | No |
| A parent, child or sibling of the recorded launch directory | No |

A directory name such as `launch`, an instruction file, or the absence of
Git metadata does not establish ownership. Missing or invalid evidence
withholds automatic injection; the lookup introduces no additional launch
error. Codex then applies its normal directory check unless the caller
explicitly supplied the flag.

Flag detection uses parsed options. A caller-supplied
`--skip-git-repo-check` is forwarded without adding a duplicate. The same
text used as another option's value or as prompt text does not count as an
explicit flag. Invocations without an `exec`/`e` subcommand receive no
automatic directory-check bypass.

This behavior requires no new configuration key or environment variable.
It writes no project trust entries or other settings to
`$CODEX_HOME/config.toml`. Existing sandbox selection, hook handling,
stdin prompt delivery and final-message capture retain their existing
behavior. The review launch directory remains separate from the checkout;
PR instruction files remain review data. See
[private review generations](recipe-checkout-isolation.md#private-review-generations).

#### Internal ownership interface

The crate-private `runner_validation::is_owned_review_launch_dir(&Path) -> bool`
provides the shared launcher with a read-only ownership result. It examines
candidate manifests only in the immediate review allocation directory,
derives their checkout paths internally, and reuses
`review_generation::read_live(checkout)` validation. Exact normalized launch
path equality is required; invalid candidates yield no eligibility.

`build_command_for_dir` resolves ownership for the explicit child execution
directory in its Codex branch. The Codex argument transformer receives that
Boolean and remains pure. This interface adds no public API, ownership
registry or manifest schema.

---

## --deny-tool

Every Copilot CLI launch receives one `--deny-tool shell(<prefix>:*)` argument
pair per prefix in amplihack's
[dangerous-command policy](./dangerous-command-policy.md) — ten for the
shipped list, which renders ten prefixes from six policy entries, since the
five forced and deleting push forms share one entry. amplihack spawns
`copilot` directly, with no shell in between, so each value is one argv
element:

```text
copilot
--deny-tool
shell(pkill:*)
--deny-tool
shell(killall:*)
--deny-tool
shell(pgrep:*)
--deny-tool
shell(tmux kill-server:*)
--deny-tool
shell(tmux kill-session -a:*)
--deny-tool
shell(git push --force:*)
--deny-tool
shell(git push -f:*)
--deny-tool
shell(git push --force-with-lease:*)
--deny-tool
shell(git push --delete:*)
--deny-tool
shell(git push -d:*)
```

The `:*` suffix is the prefix form GitHub documents for shell rules:
`shell(git:*)` matches `git push` but not `gitea`, while a bare value is
documented as matching literally. See
[Verified behaviour and open checks](./dangerous-command-policy.md#verified-behaviour-and-open-checks).

The quotes in `--deny-tool 'shell(pkill:*)'` are shell syntax, needed only when
you type the flag at a prompt yourself. The value amplihack passes holds no
quotes; do not copy them into a config file.

Copilot CLI is the only tool that receives these flags. Claude Code and Codex
read the same policy from files amplihack writes at install time, so their
command lines are unchanged; see
[Dangerous Command Policy](./dangerous-command-policy.md).

They are not the only flags a `copilot` launch receives. amplihack also
injects `--allow-all` (issue #303), unless the caller passed an allow-all flag
of its own or set `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1`. The opt-out suppresses
`--allow-all` and leaves the deny flags in place. Which of the two wins for a
command they both cover is Copilot's resolution order, not amplihack's:
amplihack passes both and does not enforce a precedence between them.

- Copilot CLI has no user-level file for command deny rules, only per-session
  flags, so the policy is applied here rather than at install.
- The flags are a function of the policy list alone. Nothing you pass is read
  when they are built, so no argument of yours can suppress one. If you deny
  the same prefix yourself, Copilot receives `--deny-tool` twice for it, in
  whichever spellings were used; both deny the same command. Whether Copilot
  accepts the repetition is recorded as an open check in
  [Verified behaviour and open checks](./dangerous-command-policy.md#verified-behaviour-and-open-checks).
  An earlier version did skip a prefix that already followed a `--deny-tool`
  in your arguments; it was removed because the scan matched any adjacent
  pair, including one where Copilot reads `--deny-tool` as another flag's
  value, and so could drop a policy rule silently.
- Entries from `policy.deny_commands` in `~/.amplihack/config.toml` follow the
  shipped defaults, in the order written. A change applies to the next launch;
  no install or update is needed.
- A `copilot` started by hand, outside `amplihack`, receives none of this.

Whether Copilot CLI resolves a `--deny-tool` over a session approval or an
allow flag, and whether it accepts a multi-token prefix such as
`shell(git push --force:*)` at all, are third-party behaviours recorded with the
version they were checked against in
[Verified behaviour and open checks](./dangerous-command-policy.md#verified-behaviour-and-open-checks).
Until those checks are settled, the Copilot rendering is the weakest of the
three.

---

## Extra args passthrough

All positional arguments and flags after the subcommand name are forwarded
verbatim to the tool subprocess after the injected flags. Order is:

```
<binary> [--dangerously-skip-permissions] [--model <value>] [--resume|--continue] <extra_args...>
```

```sh
amplihack claude --print 'Fix the failing tests' --output-format json

# amplihack spawns:
claude --model claude-opus-5[1m] --print 'Fix the failing tests' --output-format json
```

There is no processing or escaping of `extra_args`. What the user types is what
the subprocess receives, except that a Codex argv containing an `exec`
subcommand receives the [Codex agent-step flags](#codex-agent-step-flags)
after `exec`.

---

## Complete command-line assembly

`build_command_for_dir()` in `crates/amplihack-cli/src/commands/launch/command.rs`
assembles the final command line. The assembly order is:

1. Binary path (resolved by `bootstrap::ensure_tool_available()`)
2. `--dangerously-skip-permissions` — only if `skip_permissions == true` **and**
   the tool is Claude-compatible (`claude`, `rusty`, `rustyclawd`, `amplifier`)
3. `--model <value>`: only if the tool is Claude-compatible **and** `--model`
   is not already present in `extra_args`. The value is `AMPLIHACK_LITELLM_MODEL`
   on the LiteLLM gateway path, otherwise `AMPLIHACK_DEFAULT_MODEL` (a dotted
   Claude id rewritten to hyphens), otherwise `claude-opus-5[1m]`. An empty
   `AMPLIHACK_DEFAULT_MODEL` outside the gateway means no `--model`
4. `--resume` (if requested — `launch` subcommand only)
5. `--continue` (if requested — `launch` subcommand only)
6. `--allow-all`, then `--deny-tool shell(<prefix>:*)` once per
   dangerous-command policy entry, unconditionally. `copilot` only; see
   [--deny-tool](#-deny-tool)
7. All `extra_args` in the order they were passed on the command line. For
   `codex`, `codex_agent_step_args()` first inserts the
   [Codex agent-step flags](#codex-agent-step-flags) after an `exec`/`e`
   subcommand, if there is one. When the sandbox is among them,
   `codex_agent_step_notice()` supplies the one stderr line described under
   [Codex agent-step flags](#codex-agent-step-flags)

The following examples show the full assembled command for each launch
subcommand with no extra args:

```sh
amplihack claude
# → claude --model claude-opus-5[1m]

amplihack claude --skip-permissions
# → claude --dangerously-skip-permissions --model claude-opus-5[1m]

amplihack copilot
# → copilot --allow-all --deny-tool shell(pkill:*) … (one --deny-tool argument
#   pair per policy entry; see the --deny-tool section above)

amplihack codex
# → codex

amplihack codex -- exec -
# → codex exec --sandbox danger-full-access --dangerously-bypass-hook-trust -

amplihack amplifier
# → amplifier --model claude-opus-5[1m]

amplihack launch --skip-permissions
# → claude --dangerously-skip-permissions --model claude-opus-5[1m]

AMPLIHACK_DEFAULT_MODEL=claude-sonnet-4-5 amplihack claude
# → claude --model claude-sonnet-4-5

AMPLIHACK_DEFAULT_MODEL= amplihack claude
# → claude
```

---

## Python launcher parity

The Rust launcher's injection behaviour is designed to match the Python launcher
in `amplihack/launcher/core.py`. The following table documents the parity
contract:

| Behaviour | Python launcher | Rust launcher |
|-----------|----------------|---------------|
| `--dangerously-skip-permissions` | always injected | conditional: Claude-compatible tool AND `--skip-permissions` |
| `--model <default>` | `opus[1m]` unless `AMPLIHACK_DEFAULT_MODEL` set | **intentional divergence (#1421):** the concrete id `claude-opus-5[1m]` unless `AMPLIHACK_DEFAULT_MODEL` is set; an empty value passes none; a dotted Claude id is rewritten to hyphens (#1527); Claude-compatible tools only |
| `--model` suppressed when user provides it | yes | yes |
| `--resume` passthrough | yes | `launch` subcommand only |
| `--continue` passthrough | yes | `launch` subcommand only |
| `extra_args` forwarded verbatim | yes | yes |

Intentional divergences (not bugs) are documented in
[Parity Test Scenarios](./parity-test-scenarios.md).

---

## Related

- [Environment Variables](./environment-variables.md) — `AMPLIHACK_DEFAULT_MODEL` and other variables that influence launch behaviour
- [Parity Test Scenarios](./parity-test-scenarios.md) — tier5 and tier7 test cases that verify flag injection
- [Run amplihack in Non-interactive Mode](../howto/run-in-noninteractive-mode.md) — CI configuration guide
- [Manage Tool Update Notifications](../howto/manage-tool-update-checks.md) — How the pre-launch update check interacts with the launch sequence
- [Dangerous Command Policy](./dangerous-command-policy.md): the policy list behind the injected `--deny-tool` flags, and the files written for Claude Code and Codex
