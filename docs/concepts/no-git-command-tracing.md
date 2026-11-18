---
title: "Why a Recipe Run Does Not Trace Git Commands"
description: "Why recipe runs no longer attach a ptrace observer or journal the Git commands they make, how named checkouts are selected, and what a tamper-resistant record would require."
last_updated: 2026-11-18
review_schedule: quarterly
owner: rysweet
doc_type: explanation
related:
  - ../reference/recipe-checkout-isolation.md
  - ../reference/recipe-runner-logging.md
  - ./recipe-runner-architecture.md
  - ../howto/run-a-recipe.md
---

# Why a Recipe Run Does Not Trace Git Commands

A recipe run starts no tracer. Nothing traces, journals or attributes the Git
commands a run's runner, steps, agents and hooks launch, and a run needs no
`ptrace` privilege on any platform. Steps execute in the named checkout and
its legitimate task or PR worktree. Git-command observation does not select
or protect that checkout.

Between issues
[#2290](https://github.com/rysweet/amplihack-rs/issues/2290) and
[#2738](https://github.com/rysweet/amplihack-rs/issues/2738) this was not the
case. This page explains what was added, why it was removed rather than
repaired, and what would have to be true before anything like it returned. For
the behaviour as it stands, see the
[named-checkout execution reference](../reference/recipe-checkout-isolation.md).

## What was asked for

Issue [#2096](https://github.com/rysweet/amplihack-rs/issues/2096) asked for
one thing: that recipe runs stop sharing the caller's checkout. A step that
ran `git add` or `git commit` was acting on the index and working tree of
whoever had started the run.

PR #2290 initially answered that request with a private repository and a
detached execution copy. The owner decision in
[#2808](https://github.com/rysweet/amplihack-rs/issues/2808) removes that
mechanism. Builds use the default workflow's own task worktree; drives use
the PR's worktree in the named repository. A recipe command can change its
selected checkout. Agents resolve their location and requirements through
the existing workflow rather than a runner-created copy.

## What was added alongside it

The change that delivered the private clone also added a second mechanism that
had not been asked for: a Linux `ptrace` observer attached to the runner's
process tree, and a journal that recorded each observed Git invocation and
attributed it to a repository and index.

- `crates/amplihack-cli/src/run_isolation/observer/linux.rs` attached to the
  runner and followed its descendants, inspecting each successful `exec` while
  the process was stopped.
- `crates/amplihack-cli/src/commands/recipe/run/git_record.rs` wrote the
  journal, and ended a run when a command could not be attributed.

On Linux both were required rather than optional, and both ended the run on
their own failures. macOS had no observer: `begin_on` took its
`RecordingPlatform::Other` branch, printed that command recording was
unavailable on this platform and that the run continued in its private
checkout without it, and carried on unrecorded. Two things could still stop a
macOS run. A run with no writable run folder was rejected before the runner
started, with `cannot initialize required Git recording; provide a writable
absolute AMPLIHACK_WORK_ROOT and retry`; and a run whose `git-recording.json`
could not be written failed at the end. Issue #2738 lists stopped macOS runs
among the costs without saying which of the two it saw.

## Why it was removed rather than repaired

### It was not a containment boundary

Each module said so in its own header. The observer described itself as
"process observation, not protection against same-user tampering", and the
journal recorded that its evidence

> covers named Git executables in the observed runner tree, not unrelated host
> processes, arbitrary non-Git file writes or tamper resistance against the
> same user.

A run's steps execute as the same user as the CLI that observes them. Anything
that mechanism recorded, that user could also have altered, and anything it
did not name as a Git executable it did not see at all. Removing it therefore
removed no OS security boundary. The separate private execution-copy
mechanism was subsequently removed too; worktree selection is now the
responsibility of the caller and workflow.

### Its failures were availability failures

Because both parts were required on Linux and ended the run when they failed,
every defect in them became a stopped run rather than a missing record. Three
defects reached releases in succession: regressions in 1.2.163
([#2677](https://github.com/rysweet/amplihack-rs/issues/2677)), runs ending on
`ESRCH` between 1.2.165 and 1.2.167
([#2716](https://github.com/rysweet/amplihack-rs/issues/2716)), and runs
ending because an observed Git command could not be attributed to a repository
or index ([#2737](https://github.com/rysweet/amplihack-rs/issues/2737)). The
last of these ended auto-drives in ordinary use.

Each had been treated as a defect to patch. Since the mechanism protected
nothing, the repairs bought availability back at the cost of more code in the
path of every run, and the underlying question stayed open. Ryan put it
directly in #2738:

> did anyone ask you for a change that makes you map a git command to a repo?

Nobody had.

### It wrote command lines and paths to disk

The required journal was the observer's own. Each successful `exec` it saw
produced a line tagged `"source": "linux-ptrace-exec"` holding the
executable, the working directory, the resolved `GIT_DIR` and index path, and
the argument vector. The CLI checked those fields on every attempt and
concatenated the lines into `git-commands.jsonl` in the run folder, which
outlives the run. The same module also held a `GIT_TRACE2_EVENT` parser, but
its own header called that "a diagnostic compatibility API" which "production
never uses ... as coverage"; it was the fallback path, not the required one.
No `GIT_TRACE2_*` setting of amplihack's own went away with the removal,
because the required path set none: it removed both variables before spawning
the runner. The scrubbing of the caller's own trace variables is not part of
what went away either. The tracer happened to own it, but it is a credential
boundary in its own right — `GIT_TRACE2_REDACT=0` disables Git's redaction of
`Authorization:` headers — so runner command construction clears the whole
`GIT_TRACE2*` namespace for its own reason, with its own test. See
[what a run folder holds](../reference/recipe-runner-logging.md#git-commands-a-run-makes).

Argv was redacted on the way in, but only in two shapes: user information in a
URL became `scheme://<redacted>@host/path`, and a `-c` option whose value was
the following argument became `key=<redacted>`. A secret given as a positional
argument passed through, so did a `-ckey=value` joined into one argument, and
the working directory, `GIT_DIR` and index path were never redacted at all.
Removing the capture removes what redaction did not reach, and nothing that
replaces it may reintroduce it.

### It required a standing privilege

`PTRACE_ATTACH` over the runner and every one of its descendants was a
precondition of starting a run on Linux. That ruled out hosts and containers
which are configured not to allow it. With it gone, a run works under a Yama
`ptrace_scope` of `2` or `3`, in a container with `CAP_SYS_PTRACE` dropped,
under a `seccomp` profile that blocks the call, under a debugger, and inside a
step of another recipe run.

## Checkout selection and environment handling

The runner executes in the named checkout; the default workflow creates or
recovers its task worktree, and auto-drive uses the actual PR worktree.
There is no private execution repository, path translation, or refusal of
prose merely because it includes a checkout path.

Useful command environment handling remains. `amplihack_git::scrub` runs
last so inherited repository/index routing and environment-injected Git
configuration cannot redirect operations. Credentials, ordinary user/system
Git configuration, profiles, guidance, context transport, and session metadata
retain their existing handling. Existing Git trace-environment clearing also
remains; see [Git trace variables](../reference/recipe-runner-logging.md#caller-git-trace-variables-are-cleared).

Reviewers run directly in the PR worktree and are told to leave it unchanged.
An agent after review compares status, the index, and file contents against
ordinary pre-review evidence. It preserves existing and uncertain work and
handles reviewer edits without failing merely because a difference exists.
See [review assessment](../reference/recipe-checkout-isolation.md#assessment-after-review).

Recovery, runtime finalization, authorization, and completion evidence retain
their roles. Interrupted runs keep uncommitted and unpushed work. Removing
retired execution/review machinery does not authorize deleting legacy
repository data.

## How a run's status is decided

The run's status follows the runner's own exit, through the decisions the CLI
already made and still makes: a runner stopped by a signal
([#1606](https://github.com/rysweet/amplihack-rs/issues/1606)) or one that
passes its timeout is a failure whatever it would otherwise have exited with,
and an auto-drive merge run is a success only when it merged
([#2663](https://github.com/rysweet/amplihack-rs/issues/2663)). None of those
is tracer residue; all three predate the removal and are untouched by it. What
changed is the absence: nothing observes the runner while it runs, and no
outcome is derived from a Git-command observer. The CLI launches the runner
in the selected directory, waits for it, and finishes recovery admission and
runtime finalization. It does not release a private execution checkout.
Teardown still walks the process tree, but it reads the process table as any
other program would.

## Platform boundaries

Runs require no tracing privilege on Linux or macOS. Git discovery and
worktree selection are separate from process supervision. Behavioral launch
checks establish the effective cwd, repository identity, and destination of
writes; a search of implementation text is not execution evidence.

## If a record were wanted again

Nothing here argues that a record of what a run does is worthless, only that
this one cost availability and exposure without providing containment. A
record that would be worth having needs two properties the removed mechanism
could not have:

- It is written somewhere the run cannot reach, so that the record is not
  produced by the same user whose actions it describes.
- Its failure is not the run's failure, so that a defect in it stops nothing.

Both point at the runner writing the record itself rather than at the CLI
observing the runner from outside. That is a separate piece of work, and no
part of it is implemented.

## Related

- [Recipe Execution in Named Checkouts](../reference/recipe-checkout-isolation.md) - Where steps run and how review preserves existing work
- [Recipe Runner Logging Reference](../reference/recipe-runner-logging.md#git-commands-a-run-makes) - What a run folder holds, `GIT_TRACE2*` clearing, and older run folders
- [Recipe Runner Architecture](./recipe-runner-architecture.md) - Which of the CLI and the runner owns each part of a run
- [Run a Recipe](../howto/run-a-recipe.md) - Launching a run and knowing where it works
