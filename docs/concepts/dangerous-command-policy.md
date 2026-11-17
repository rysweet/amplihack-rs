---
title: "Why Dangerous Commands Are Denied in the Harness"
description: "Why amplihack writes each harness's own deny configuration from a single policy list, rather than adding a hook, and what a prefix matcher can and cannot do."
last_updated: 2026-11-17
review_schedule: quarterly
owner: rysweet
doc_type: explanation
related:
  - ../reference/dangerous-command-policy.md
  - ../reference/hook-specifications.md
  - ../howto/add-dangerous-command-deny-rules.md
  - ../howto/stop-background-jobs-by-pid.md
  - ./prompt-text-out-of-argv.md
  - ./hook-block-single-source.md
---

# Why Dangerous Commands Are Denied in the Harness

amplihack declares one list of dangerous commands and writes it into Claude
Code's `permissions.deny`, a Codex execpolicy rules file and the Copilot CLI
command line. It adds no new hook, and it does not displace the one hook that
already covers two of the entries. This page explains why that shape was
chosen and where it stops.

The list itself, and the exact files, are in
[Dangerous Command Policy](../reference/dangerous-command-policy.md).

## Contents

- [What went wrong](#what-went-wrong)
- [Why guidance was not enough](#why-guidance-was-not-enough)
- [Why the harness's own mechanism](#why-the-harnesss-own-mechanism)
- [Why the tmux guard stays](#why-the-tmux-guard-stays)
- [Why one list and three renderings](#why-one-list-and-three-renderings)
- [Why ordinary pushes still run](#why-ordinary-pushes-still-run)
- [Why pgrep is on the list](#why-pgrep-is-on-the-list)
- [Why a failed write is loud](#why-a-failed-write-is-loud)
- [What this is not](#what-this-is-not)
- [Related](#related)

## What went wrong

An agent ran a pattern kill meant for one process:

```bash
pkill -f 'vitest.mjs'
```

`pkill -f` matches the whole command line. The recipe runner supervising that
agent had the task text in its own argv, and the task text mentioned
`vitest.mjs`. The kill ended the runner that was running the agent. The same
failure happened repeatedly on a shared host, once ending about twenty runs at
one go.

A second shape of the same defect is a wait loop:

```bash
until ! pgrep -f 'scripts/check' >/dev/null 2>&1; do sleep 10; done
```

The `bash -c` running that loop has `scripts/check` in its own command line, so
`pgrep -f` always matches the loop's own shell and the loop never ends. Four
builds hung for between 26 and 81 minutes on that one line.

Taking prompt text out of argv removes one side of the first problem, and is
useful for its own reasons. It does not help the second at all, and it
does not stop a pattern from matching another user's processes on a shared
host. The common cause is selecting a process by matching text rather than by
the identity of something the run started.

## Why guidance was not enough

The instruction to stop only recorded PIDs was in the agent guidance before any
of these incidents, and in the brief of the runs that caused them. It did not
prevent one of them. Guidance competes with everything else in a prompt; a
deny rule does not.

The deny rules do not replace the guidance. The guidance still says to record
`$!` and to wait with `wait $pid`, because an agent that cannot run `pkill`
still needs to know what to run instead. That instruction is written out in
[Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md), which
covers the same incidents from the other side.

Each policy entry carries justification text saying what to run instead, and
the intent is that the agent reads it at the moment it is stopped, which is
when it is most likely to act on it. Only one of the three renderings can
deliver that. The Codex execpolicy `prefix_rule` has a `justification` field
and prints it on a refusal; Claude Code's `permissions.deny` entries are bare
strings and Copilot CLI's `--deny-tool` is a flag with one argument, so on
those two the agent gets the harness's own denial message naming the rule that
matched, and nothing else. The argument that a deny rule teaches better than a
line of guidance therefore holds for Codex and is an aspiration for the other
two; what they contribute is the stop itself.

## Why the harness's own mechanism

Each supported harness already has an allow/deny mechanism, and each of them
sits closer to the command than amplihack does.

Another hook would have been the easier thing to build, and amplihack already
has hooks, including one that denies the dangerous tmux forms. A hook was not
chosen as the mechanism for this list, for three reasons:

- **A hook fails open.** When the harness times out a pre-tool-use hook,
  Claude Code and Copilot CLI run the command as though it had been allowed.
  A line in a settings file has no equivalent failure mode.
- **A hook has to be installed and enabled to run at all.** A harness started
  with amplihack's hooks absent or switched off still reads its own deny
  configuration.
- **A hook is amplihack's own policy layer, not the user's.** The harness's
  deny list is somewhere a user already looks, already edits and already
  understands. Entries they add by hand survive, which is a property a
  generated hook does not have.

What the harness mechanism does **not** buy is reach past the head of the
command. On codex-cli 0.166.0, measured against the shipped file, Codex
applies a rule to `pkill -f x` but not to `/usr/bin/pkill -f x` or to
`bash -lc "pkill -f x"`. On the Claude side, a live session with
`defaultMode: bypassPermissions` set (the mode amplihack launches in) and
`Bash(pgrep:*)` denied refused both `pgrep --version` and
`true && pgrep --version`, which is what the whole Claude rendering rests on.
The other chain separators (`;`, `|`, `$(...)`) rest on the live testing
reported in issue #2674. These are third-party behaviours that can change
between releases, so each claim is recorded with what it was checked against in
[Verified behaviour and open checks](../reference/dangerous-command-policy.md#verified-behaviour-and-open-checks)
rather than assumed here. Were a later Claude Code to stop consulting
`permissions.deny` in that mode, the Claude rendering would cover nothing and
the argument above would survive on Codex alone.

Where a harness has no mechanism, amplihack says so rather than substituting
one. Copilot CLI has no user-level file for command deny rules, only
per-session flags, so amplihack passes the flags on every launch it controls
and states that limit in the install output. Writing a hook in place of a
missing mechanism would have hidden which harnesses are actually covered.

Copilot is also where the division between what amplihack does and what the
harness does is sharpest. amplihack launches `copilot` with `--allow-all` and
with a `--deny-tool` per policy entry. Whether a deny flag outranks that allow,
or a project approval already saved in `permissions-config.json`, is Copilot's
own resolution order, and the deny flags are useful only if it resolves them
that way. amplihack passes the flags; it does not enforce the precedence
between them, cannot observe how Copilot resolved them, and no test in this
repository asserts it. The precedence is recorded as an open check against the
version it was last looked at, not as a property of this feature.

`rusty`, `rustyclawd` and `amplifier` need nothing of their own. amplihack
launches all three the way it launches `claude`, with `CLAUDE_CONFIG_DIR`
pinned to the home or profile directory it manages, so they read the
`settings.json` the Claude entries are written into. One file covers four
tools.

## Why the tmux guard stays

Two of the ten default prefixes name tmux, and amplihack already denies the
dangerous tmux forms through the `tmux_guard` pre-tool-use hook, on Claude
Code, Copilot and Codex alike. The guard was not replaced and is not
redundant.

A prefix is text; the guard is structural. It tokenises the command, sees
through absolute paths and the wrappers `env`, `command`, `exec`, `sudo` and
`timeout`, follows `if-shell`, `command-alias` and `send-keys`, reads tmux's global
options getopt-style, and works out whether a socket argument really names a
private server or the user's own. No amount of prefix matching reaches that,
and the policy does not pretend to: the tmux entries are a second layer kept
for the fail-open and not-installed cases above.

Being a second layer constrains what they may deny. The guard allows
`tmux kill-session -t <name>`, which is how amplihack's own recipe runs and
the profile rotator are stopped, and allows socket-scoped kills, which tests
and sandboxes need. The policy entries match that line exactly: `kill-server`
and `kill-session -a`, nothing wider. A policy that denied the targeted form
would have contradicted a shipped hook and broken documented procedures,
which is the clearest way to have the whole list switched off.

## Why one list and three renderings

The three mechanisms have nothing in common syntactically. Claude takes
`Bash(<prefix>:*)` strings in a JSON array; Codex takes a Starlark-like
`prefix_rule` call with a token list; Copilot takes `--deny-tool shell(...)`
arguments, passed as argv elements with no shell and so no quoting.

The temptation is to let each writer declare what it denies. That is how the
same rules came to be applied by hand to twenty-two profiles and four hosts,
each slightly differently, with no single place to read what was in force. One
list with three renderers means a new entry is one line, and the question "what
is denied?" has one answer.

A single entry may render to more than one string in one harness and fewer in
another. The push entry becomes five Claude prefixes and five Copilot flags,
but one Codex rule with alternatives. That asymmetry belongs in the renderer,
not in the list.

## Why ordinary pushes still run

The forced and deleting pushes are denied; `git push`,
`git push -u origin <branch>` and `git push origin HEAD:<branch>` are not. Any
rule that stopped ordinary pushes would stop amplihack's own work, and would be
removed by the first person it inconvenienced, taking the useful part with it.

A deny rule that gets switched off protects nothing. Keeping the denied set
narrow enough to live with is part of making it survive.

The cost is that the rules are expressed as prefixes of a specific flag order,
so `git push origin --force` is not matched. The Codex rendering carries the
three ordinary forms as `not_match` examples, which Codex checks when it loads
the file, so a future edit that accidentally denies an ordinary push does not
load.

## Why pgrep is on the list

`pgrep` is read-only, and denying it costs something: listing processes by name
is a reasonable thing to want. It is on the list because the wait-loop failure
above is not a kill at all, and because `kill $(pgrep -f ...)` is the same
mistake wearing a different command.

The justification text therefore routes to `ps -ef` and tells the agent to
read the list before acting, rather than recommending the command that is
denied.

The inconvenience is small but it is not hypothetical, and amplihack's own
documentation was part of it. Seven places across five pages told the reader
to run `pgrep` or `pkill`: the argv verification procedure in
[Check Agent argv for Prompt Text](../howto/check-agent-argv-for-prompt-text.md),
two checks and a cleanup step in
[Keep Tests From Prompting for Git](../howto/keep-tests-from-prompting-for-git.md),
a cleanup step in
[Power-Steering Worktree Troubleshooting](../howto/power-steering-worktree-troubleshooting.md),
a force-kill step in
[Stop Hook Exit Hang](../troubleshooting/stop-hook-exit-hang.md),
and a profiling line in [Developing amplihack](../DEVELOPING_AMPLIHACK.md).
All seven were rewritten to read the process list first, as were the listings in
[Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md)
and [Claude Profile Rotation](../howto/claude-profile-rotation.md), because
shipping a deny rule against a command your own instructions tell people to
run is how a deny rule gets removed. Anything else that asks for one of these commands should be
rewritten the same way rather than exempted.

This is a deliberate trade: that inconvenience against a failure mode that
hung four builds and ended several dozen runs.

## Why a failed write is loud

A deny rule can fail to arrive. The Codex home can be read-only, or be a
symlink amplihack [declines to write
through](../reference/dangerous-command-policy.md#how-the-files-are-written),
as is the `rules/` directory inside it; a settings file can hold a
`permissions` value amplihack will not merge into without discarding what the
user put there, and when it does, that file gets no entries at all.

Neither case stops the install. The rest of an install still has value, and a
harness home that cannot be written is the user's to fix, not amplihack's to
break the run over. So a refused write is a warning on stderr and an exit
status of 0.

What it must not be is quiet, and it must not sit under a line saying the
rules were written. The reason is particular to this kind of feature: an
unprotected agent and a protected one look identical until the moment the
protection would have mattered, and in between somebody relies on it. A
missing deny rule that is known about costs one command; a missing deny rule
that is believed to be present costs whatever the rule existed to prevent.

That is why the install output reports what the run actually wrote, harness by
harness, names a harness it found nothing to write into, and drops the
positive line for a harness whose files were refused. A report is readable
only if the absence of a line means something.

## What this is not

Every mechanism here matches a command prefix. A prefix matcher is a
mistake-prevention measure. It is not a security boundary, and treating it as
one would be the more expensive error.

An agent that means to run `pkill` can run `sh -c 'pkill -f x'`, `env pkill`,
a shell alias, or the same binary named by a path: `/usr/bin/pkill`, `./pkill`
or `busybox pkill`. None of the three harnesses will match any of them. Path
invocation is named on its own because `/` and `.` are accepted
characters in a policy entry, so a reader may reasonably expect a prefix to
cover the paths that reach the same binary. It does not: a prefix is compared
against the text or tokens the agent actually typed, and `/usr/bin/pkill` does
not begin with `pkill`. Adding the path forms as entries would not close this
either, since the set of paths to a binary is open.

The same agent runs as the user and can edit `~/.amplihack/config.toml`,
`~/.claude/settings.json` and the Codex rules file directly. Nothing here constrains a determined
process running as you; see
[Why Prompt Text Stays Out of argv](./prompt-text-out-of-argv.md), which makes
the same distinction about file permissions.

What the policy does is make the dangerous form of a common operation fail
loudly, with a sentence explaining the safe form, at the moment somebody
reaches for it. The escapes are enumerated in the reference so that nobody
later mistakes the one for the other.

## Related

- [Dangerous Command Policy](../reference/dangerous-command-policy.md): the list, the rendered files and the enumerated limits
- [Add Dangerous-Command Deny Rules](../howto/add-dangerous-command-deny-rules.md): adding instance entries
- [Why Prompt Text Stays Out of argv](./prompt-text-out-of-argv.md): the related change that removed task text from the process table
- [Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md): the guidance the deny rules sit on top of
- [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md): what to run in place of a pattern kill
- [Hook Specifications](../reference/hook-specifications.md#tmux-server-guard): the `tmux_guard` hook the two tmux entries sit behind
- [Hook Block: One Source of Truth](./hook-block-single-source.md): the merge-and-replace machinery the Claude writer reuses
