---
title: "Why a Recipe Run Does Not Trace Git Commands"
description: "Why recipe runs no longer attach a ptrace observer or journal the Git commands they make, what the private clone does instead, and what a tamper-resistant record would require."
last_updated: 2026-11-17
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
`ptrace` privilege on any platform. What keeps a run out of the caller's
checkout is the private clone, not observation of what the run does afterwards.

Between issues
[#2290](https://github.com/rysweet/amplihack-rs/issues/2290) and
[#2738](https://github.com/rysweet/amplihack-rs/issues/2738) this was not the
case. This page explains what was added, why it was removed rather than
repaired, and what would have to be true before anything like it returned. For
the behaviour as it stands, see the
[checkout isolation reference](../reference/recipe-checkout-isolation.md).

## What was asked for

Issue [#2096](https://github.com/rysweet/amplihack-rs/issues/2096) asked for
one thing: that recipe runs stop sharing the caller's checkout. A step that
ran `git add` or `git commit` was acting on the index and working tree of
whoever had started the run.

That request is satisfied by giving each run a private repository and a
detached worktree under its own run folder, preparing them from a snapshot of
the caller's committed, staged, unstaged and untracked state, and starting
every step inside them. A Git command a step makes then acts on the private
repository because that is the only checkout it has been given.

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
removed no control: the containment was the private clone throughout.

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
`Authorization:` headers — so `RunCheckout::apply_to` now clears the whole
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

## What stays

The private clone, which was the actual subject of #2096, is unchanged. So is
everything that keeps a caller's state out of it:

- `amplihack_git::scrub` on every command the run prepares, so that an
  inherited repository selection cannot redirect the run and an inherited
  environment-based setting cannot inject configuration into it. It removes
  every name in `amplihack_git::REPOSITORY_ENV_VARS`, among them `GIT_DIR`,
  `GIT_INDEX_FILE`, `GIT_WORK_TREE`, `GIT_COMMON_DIR` and the
  object-directory overrides, together with `GIT_CONFIG`, `GIT_CONFIG_COUNT`,
  `GIT_CONFIG_PARAMETERS`, every `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`
  pair, `GIT_EXEC_PATH` and `GIT_TEMPLATE_DIR`. The runner command sets none
  of `GIT_CONFIG_NOSYSTEM`, `GIT_CONFIG_SYSTEM` and `GIT_CONFIG_GLOBAL`:
  steps read the user's own global and system configuration, including a
  credential helper, as they did before #2290. See
  [paths and Git configuration](../reference/recipe-checkout-isolation.md#paths-and-git-configuration).
- Parent-directory checking on both mapping paths, so a context value cannot
  point outside the tree it is mapped into.
- The error raised when a context value embeds the caller's checkout path
  inside other text rather than being a path in its own right. This reports
  `context <key> embeds a caller-checkout path in opaque text` and asks for
  the path as a separate context value.

That last one is easy to mistake for part of the removed mechanism, because it
is an error about paths and it was added in the same change. It is not. It
sits in the branch of context mapping that handles values which are not
absolute caller-rooted paths, and without it a value such as
`"see /home/user/checkout/src for the failing case"` would reach the run with
the caller's path still inside it, pointing a step back at the checkout the
run is supposed to stay out of. That is the defect #2096 was filed about, so
the error stays.

Step recovery
([#2570](https://github.com/rysweet/amplihack-rs/issues/2570)) and agentic
merge readiness
([#2637](https://github.com/rysweet/amplihack-rs/issues/2637)) never referred
to the observer or the journal and are unaffected.

## How a run's status is decided

The run's status follows the runner's own exit, through the decisions the CLI
already made and still makes: a runner stopped by a signal
([#1606](https://github.com/rysweet/amplihack-rs/issues/1606)) or one that
passes its timeout is a failure whatever it would otherwise have exited with,
and an auto-drive merge run is a success only when it merged
([#2663](https://github.com/rysweet/amplihack-rs/issues/2663)). None of those
is tracer residue; all three predate the removal and are untouched by it. What
changed is the absence: nothing observes the runner while it runs, and no
outcome is derived from what its subprocesses did. The CLI prepares the
checkout, launches the runner, waits for it, then removes the checkout and
finishes the recovery admission and the workspace. Teardown still walks the
process tree, but it reads the process table as any other program would.

## Evidence on each platform

On Linux, `crates/amplihack-cli/tests/issue_2738_no_ptrace.rs` runs the real
CLI and checks that the runner and every process in its tree report
`TracerPid: 0` in `/proc/<pid>/status` while the run is in flight, and that a
run whose release succeeds reports the runner's own exit status.

macOS has no equivalent of that file, and no comparable check was added for
it. The evidence there is that the crate contains no tracing code to run:
a search of `crates/amplihack-cli/src/` for `ptrace` matches only the sentence
in `run_isolation/README.md` saying that none is needed. The test reads
`Name:`, `PPid:` and `TracerPid:` and nothing else, so neither the arguments
nor the environment of a run's subprocesses reach a log.

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

- [Recipe Run and Auto-Drive Checkout Isolation](../reference/recipe-checkout-isolation.md) - What a run copies, where steps run, platform support and the tests
- [Recipe Runner Logging Reference](../reference/recipe-runner-logging.md#git-commands-a-run-makes) - What a run folder holds, `GIT_TRACE2*` clearing, and older run folders
- [Recipe Runner Architecture](./recipe-runner-architecture.md) - Which of the CLI and the runner owns each part of a run
- [Run a Recipe](../howto/run-a-recipe.md) - Launching a run and knowing where it works
