---
title: Vanished Observed Tasks in the Linux Run Observer
description: How the ptrace run observer treats a traced task that exits between its stop and the next request, what it records, and what still fails the run.
doc_type: reference
last_updated: 2026-11-16
related: issue #2716, issue #2637, issue #2686, issue #1267
---

# Vanished observed tasks in the Linux run observer

On Linux every `amplihack recipe run` traces the runner's whole process tree
with ptrace, so that the Git commands a run issues become required evidence.
See [Required recording](recipe-checkout-isolation.md#required-recording).

A traced tree of `claude`, `node`, `cargo`, `git` and shell processes loses
tasks all the time: a thread group leader calls `exit_group`, a child is sent
`SIGKILL`, a thread id is retired by a sibling's `exec`. The kernel answers a
ptrace request aimed at such a task with `ESRCH`. The observer treats that
answer as the fact it is — **this task has gone** — records it, and keeps
observing. The run's reported status comes from the runner's own exit.

This page states exactly where that applies, what it records, and what still
ends the run.

## Contents

- [The rule](#the-rule)
  - [The newborn registration order](#the-newborn-registration-order)
- [Where it applies](#where-it-applies)
- [What still fails the run](#what-still-fails-the-run)
- [The root task is the runner](#the-root-task-is-the-runner)
- [What a run reports](#what-a-run-reports)
- [Evidence](#evidence)
  - [The receipt's vanished count](#the-receipts-vanished-count)
  - [The stderr line](#the-stderr-line)
- [How a vanished task leaves the observed set](#how-a-vanished-task-leaves-the-observed-set)
- [Configuration](#configuration)
- [Worked example](#worked-example)
- [Rust components](#rust-components)
- [Tests](#tests)
- [Limits](#limits)
- [Related](#related)

## The rule

> A ptrace request issued from the observation loop that fails with `ESRCH`
> means the task it names has exited. Every site that classifies a task as
> gone is a site that does not resume it, so a task mistakenly classified
> stays in its ptrace stop and executes nothing further. No exit status is
> ever made up: the runner's status comes only from a real `waitpid` status.

Those three sentences carry the whole design. The observer is otherwise
fail-closed — missing evidence ends the run — and this is the one deliberate
exception. It is safe only because a task that vanished before its resume ran
nothing after its last recorded `exec`. Two kinds of change invalidate that
argument and oblige a revisit of this handling: resuming a task on a
classifying path, and registering a task into the observed set before the site
that may classify it as gone (see
[The newborn registration order](#the-newborn-registration-order)).

Classification reads the error number only, never message text: a traced
process can influence strings that reach an error chain, so no string a tracee
can touch steers a control decision. This follows the error-number-only
discipline already used for `PTRACE_TRACEME`.

### The newborn registration order

One classifying site sits inside the registration of a task that is not yet in
the observed set, so the order of two statements there is load-bearing. The
newborn's `/proc/<child>/status` read — and the newborn identity check before
it — **run before the newborn is inserted into the observed process set**, not
after.

In that order, abandoning the newborn leaves no registration anywhere: no
entry in the observed process set, the thread-group map or the ownership
chains. A newborn that was in fact alive then has its initial `SIGSTOP` find no
registration, so it takes the pending-stop branch that already exists for a
stop that outran its parent's event, and it is never resumed. That is exactly
what the rule promises, with no further machinery.

In the other order the rule would be false at this one site. The newborn would
stay in the observed set with no ownership chain, its initial stop would skip
the pending-stop branch (the set already contains it), and the loop-tail
`PTRACE_CONT` would resume it un-journaled. Its first `exec` would then fail
the run on `missing exec observation ownership` or `missing observed task
ownership`. Integrity would survive, but through a fatal bookkeeping backstop
rather than through non-resumption — a different and weaker argument than the
one above. Keeping the read first is what keeps the invariant a single
sentence.

The cost of that order is bounded and accepted. Between the fork event and the
insert, the newborn is not yet in the shared set that `kill_all`, `reap_all`
and `Observer::terminate` signal, so for that window it is outside their
coverage. It is held at its initial `SIGSTOP` for the whole window, so it
executes nothing, and `PTRACE_O_EXITKILL` — unchanged in the option set —
kills it if the tracer goes away.

## Where it applies

Handling is per call site, decided by whose life the request depends on, not
by a single blanket "carry on". Two of the sites sit inside the arm that is
handling the **parent's** fork or clone event, where the parent has not been
resumed yet; a plain jump to the next loop iteration there would leave the
parent stopped forever.

| Request | Name in the warning | Subject | On `ESRCH` |
| --- | --- | --- | --- |
| `PTRACE_SETOPTIONS` on the root | `PTRACE_SETOPTIONS` | the runner | Count it, go to the next wait. The observer stays uninitialised and the journal finishes empty. |
| `PTRACE_GETEVENTMSG` at a fork, vfork or clone event | `PTRACE_GETEVENTMSG (fork)` | the stopped task | Count it, go to the next wait. |
| `/proc/<newborn>/status` (the `Tgid:` read) | `thread group identity` | the newborn | Count it against the parent, register nothing at all, **still resume the parent** by falling through to the loop tail. Runs before the newborn joins the observed set — see [The newborn registration order](#the-newborn-registration-order). |
| `PTRACE_CONT` on a newborn at its initial `SIGSTOP` | `PTRACE_CONT (newborn)` | the newborn | Count it against the parent, leave its registration in place and the newborn unresumed, **still resume the parent**. |
| `PTRACE_GETEVENTMSG` at an `exec` event | `PTRACE_GETEVENTMSG (exec)` | the stopped task | Retire the rest of its thread group first, then count it and go to the next wait. |
| `PTRACE_CONT` at the end of the loop body | `PTRACE_CONT` | the stopped task | Count it, go to the next wait. |

The two `PTRACE_CONT` sites carry different names so that one warning line
identifies one site. Without that, the commonest two sites would print the same
string and the line would not say which fired.

The newborn `PTRACE_CONT` row leaves the registration alone on purpose. By then
the newborn is registered, and taking it back out would be a classifying site
mutating the observed set — see
[How a vanished task leaves the observed set](#how-a-vanished-task-leaves-the-observed-set).
The newborn `/proc` row has nothing to leave alone: it runs first, so there is
no registration yet.

The thread-group retirement at the `exec` site is not optional. A non-leader
`exec` destroys the rest of its thread group before the event arrives, and the
kernel need not report those thread ids separately. Dropping them is what
keeps the loop's `while !pids.is_empty()` condition reachable.

The `/proc/<newborn>/status` read is not a ptrace request, so it is listed
separately. It accepts `ENOENT` as well as `ESRCH`, because a thread id that
has gone leaves no `/proc` entry. It is in scope for one reason: its subject is
a newborn still held at its initial `SIGSTOP`, which by definition has run no
code at all, so nothing observable can be lost. Two predicates exist rather
than one precisely so that `ENOENT` can never be accepted from a ptrace
request.

## What still fails the run

| Case | Result |
| --- | --- |
| Any ptrace error other than `ESRCH` | The observation ends and the run fails, with the message that site already used — `required process observation failed` for a request, `read observed process relationship` for an event message. |
| `/proc/<pid>/environ` read at an `exec` stop | Fatal, unchanged. |
| `/proc/<pid>/exe` link read while recording an `exec` | Fatal, unchanged. |
| A newborn identity of `0` or less from `PTRACE_GETEVENTMSG` | The run fails with `implausible observed newborn identity`. |
| `missing observed thread group` — a `/proc` status that was read but holds no `Tgid:` | Fatal. "Gone" is only ever an error number from the open or read, never a parse outcome. |
| `unexpected newborn observation stop` | Fatal. |
| `missing fork observation ownership`, `missing exec observation ownership`, `missing observed task ownership` | Fatal. A chain the observer cannot find is a fault in its own bookkeeping, not a task that has gone. All three are reached only where a registration should already exist, and the [newborn registration order](#the-newborn-registration-order) is what keeps a vanished newborn from ever reaching them. |
| A storage, attribution or publication error | Fatal, unchanged. |
| `ESRCH` from the runner's own launch | Unchanged: the runner could not start, so the run reports the launch error. |

The newborn identity check matters because of this change, not in spite of it.
That identity is cast to `i32` and interpolated into a `/proc` path; before,
an implausible value failed loudly on `ENOENT`, and now `ENOENT` is accepted
at that read, so a kernel or ABI surprise would otherwise become invisible.

## The root task is the runner

An `ESRCH` on the root — the `recipe-runner-rs` process itself — never ends
the observation and never returns early. The observer counts it, keeps
waiting, and takes the runner's status from the real `WIFEXITED` or
`WIFSIGNALED` notification that the kernel delivers for it. If no such status
ever arrives, the existing `observer lost runner exit status` backstop still
fails the run closed.

When the root goes before `PTRACE_SETOPTIONS` succeeds, the observer never
initialises and its journal is finished empty. Downstream accepts that
unchanged — one attempt, zero lines — and the run reports the runner's own
(killed) status.

## What a run reports

A run whose runner finished reports the runner's own exit. In particular:

- `failed to wait for recipe-runner-rs: required process observation failed:
  No such process (os error 3)` no longer arises from a task that exited
  during the run.
- `required observer did not finalize; partial evidence retained: ...` no
  longer arises from this cause either, because the observation now reaches
  the journal finalisation at the end of the loop instead of bailing out of
  it.
- No failure classification arises from this cause at all, so nothing is held
  back from retry by it. Be precise about where that is checkable: `run.json`
  has no `indeterminate` value to carry — its `outcome` is exactly `success`,
  `failure` or `interrupted`. `indeterminate` is a *failure class* over a
  failed step's evidence, surfaced in the `failure_classification` result key
  and in an `amplihack.recipe.failure_class ` line on stderr. The observable
  pair for a run that lost a task is therefore `outcome: success` in
  `run.json` **and no `amplihack.recipe.failure_class ` line on stderr**.
  Before issue #2716 the observer's wait error went through the run's final
  failure record, which writes `outcome: failure`, and never reached
  classification at all.
- Coverage stays `established`: the observer observed the tree, and it says so.

## Evidence

### The receipt's vanished count

Each journal's receipt — `<run dir>/git-trace2/observer-<run id>.receipt` —
carries a `vanished` count of the tasks that journal's chain found gone:

```json
{"schema_version":1,"coverage":"established","complete":true,"run_id":"36721e1d-cf58-403d-842f-a7e6fcb087f7","attempt_id":"36721e1d-cf58-403d-842f-a7e6fcb087f7","scope":"runner-descendants","backend":"linux-ptrace-exec","vanished":2}
```

Facts about that field:

- It is written by the same single receipt write as the other keys: created
  new, mode `0600`, `O_NOFOLLOW`, after the journal is verified and synced.
  There is no second write path, no rewrite in place.
- The seven existing keys and their values are unchanged. The receipt reader
  compares the keys it validates by name and ignores the rest, so nothing
  downstream has to change.
- It is a number, never a list of process ids. The receipt is read under a
  4096-byte limit that errors when exceeded, and a traced tree can create
  unbounded tasks, so a list would hand that tree a way to turn this fix into
  a new failure it could trigger at will.
- Every journal in the vanished task's ownership chain is incremented, so the
  run's own journal — index `0`, the chain every task belongs to — holds the
  exact run total, including tasks owned by a relayed nested scope.
- `vanished` is `0` on a run that lost nothing.
- It is not published into `git-recording.json`, `run.json` or
  `diagnostics/`. Process-table detail stays inside the trace folder.

On a run that succeeds the trace folder is removed after validation, so the
receipt is a field artefact: it is there to read on a run that kept its
evidence. Nothing reads the count automatically, and `coverage` does not
depend on it — see
[a degraded run leaves no durable, enforced evidence](#limits).

### The stderr line

Each vanished task also prints one line to the run's stderr:

```
amplihack: warning: observed task 481392 was gone at PTRACE_CONT
```

- Fixed text, the process id, and a request name. Nothing else: no error
  chain, no path, no `argv`, no `cwd`, no `git_dir`, no environment value. The
  observer reads `environ` and `cmdline`, which hold tokens and private paths,
  and run stderr reaches CI logs and retained evidence folders.
- The request name is one of exactly six `&'static str` values, one per call
  site, and the vocabulary is closed — no value is built from anything a
  tracee can influence:

  ```text
  PTRACE_SETOPTIONS
  PTRACE_GETEVENTMSG (fork)
  PTRACE_GETEVENTMSG (exec)
  thread group identity
  PTRACE_CONT (newborn)
  PTRACE_CONT
  ```

  The newborn `/proc` read is named `thread group identity`, for what it was
  asking rather than where it asked: no name carries a `/`, so the whole line
  can be quoted into an issue or a CI log without being read first. Mapping
  each name back to its site is the
  [Where it applies](#where-it-applies) table.
- At most 16 lines per run. A traced tree can create unbounded tasks, so an
  uncapped per-task warning would be log amplification and a way to fill a
  disk. The count in the receipt stays exact past the cap.
- The line is a warning. It does not change the run's status.

## How a vanished task leaves the observed set

The site that classifies a task as gone does not remove it. Nothing is taken
out of the observed process set, the thread-group map, the ownership chains or
the pending-stop map there. The task leaves through the one path that already
removes tasks: its own `WIFEXITED` or `WIFSIGNALED` notification, one wait
event later.

A tracer must reap its tracee, so that notification is guaranteed for both
required cases — a thread destroyed by `exit_group`, and a tracee sent
`SIGKILL` in its ptrace stop. Removing the task eagerly instead would leave it
unreaped as a zombie child of the CLI, because the full reap only ran on the
error path this change removes, and would push the later notification into the
"exited before registration" branch.

The third cause named in issue #2716, a thread id retired by a sibling's
`exec`, is covered from both ends: the `exec` arm already removes retired
thread ids, and whichever event arrives first does the cleanup.

One related correction comes with this: a pending stop is now dropped when its
task exits. Without that, process id reuse could resume a task whose stop was
never consumed, and inflate the count. That removal is also what clears the
parked stop of a newborn abandoned at its `/proc` read whose initial stop had
already arrived: the entry stays in the pending-stop map, unresumed, until the
task's own exit notification drops it.

## Configuration

Nothing to configure. There is no environment variable, no flag, no threshold
and no opt-out, and no timer, retry, back-off or poll anywhere in this
behaviour. The observer gains no new ptrace request, no new ptrace option, no
new system call and no new dependency; `PTRACE_O_EXITKILL` stays in the option
set.

A host where ptrace cannot attach at all is a different case, unchanged by
this: see
[The Linux observer could not attach](recipe-runner-logging.md#the-linux-observer-could-not-attach).

## Worked example

Run anything that starts a busy tree, on Linux:

```sh
amplihack recipe run default-workflow \
  -c task_description="Add cache headers" \
  -c repo_path=. \
  -w "$PWD"
```

A run that loses a task prints one warning line among its step output and
carries on to the end:

```
amplihack: warning: observed task 481392 was gone at PTRACE_CONT
```

The run's exit code is the runner's. To confirm from the published record, in
the run directory the run named (`AMPLIHACK_RUN_DIR`, normally
`<work root>/runs/<run id>`):

```sh
jq -r '.coverage' "$RUN_DIR/git-recording.json"
# established
jq -r '.outcome' "$RUN_DIR/run.json"
# success
```

To read the count on a run that kept its evidence:

```sh
jq -r '.vanished' "$RUN_DIR/git-trace2/observer-$RUN_ID.receipt"
# 2
```

## Rust components

All of these are internal to `amplihack-cli`; none is a public API. This table
and the [test table](#tests) name code, so the code is authoritative where
they disagree — read them as an entry point, not as a contract.

| Component | Purpose |
| --- | --- |
| `vanished_task(&anyhow::Error) -> bool` | True when a failed observer request's own error number is `ESRCH`. Error number only. The number read is the one `PtraceRequestFailed` carries, and only the two ptrace helpers construct that, so an `ESRCH` from anything else cannot be mistaken for the ptrace result however it is wrapped. |
| `PtraceRequestFailed` | The error the two ptrace helpers return, carrying the request's `errno` in the type so classification never walks an error chain. Displays as the request's context text with the `io::Error` as its source, so the run's message and `root_cause` errno are what the `.context(…)` chain gave. |
| `vanished_proc_entry(&std::io::Error) -> bool` | True for `ESRCH` or `ENOENT`. Used at exactly one site, the newborn `Tgid:` read. Kept separate so `ENOENT` can never be accepted from a ptrace request. |
| `Journal.vanished: u64` | Per-journal count, `0` at both construction sites, written by `Journal::finish` as the receipt's `vanished` number. |
| `Scopes::vanished(task, owner, request)` | Infallible. Increments the count on every journal in the owner's chain, then prints the capped stderr warning. Takes the request name as a `&'static str` from the [six-value closed vocabulary](#the-stderr-line); the two `PTRACE_CONT` sites pass different values. |
| `retire_thread_group(leader, ...)` | The thread-group retirement of the `exec` arm, extracted so both the normal `exec` path and the vanished `exec` path use it. |
| `observe()` | The wait and dispatch loop, with the per-site handling tabulated above, the newborn identity check and `/proc` status read ordered ahead of the newborn's insertion into the observed set, and the pending-stop removal in the exit arm. |

`git_record.rs` is not modified. The journal line shape is not modified, and
no new file matching `observer-*.jsonl` is created — every line of that file
is validated, and any such name is scanned and required to have a receipt
beside it, so either would turn a lost task into a run failure on every run.

## Tests

| Test | What it holds |
| --- | --- |
| `observer/linux.rs` module tests: `only_esrch_means_a_ptrace_request_found_the_task_gone` | `ESRCH` classifies as gone; six other error numbers stay fatal, as does an error carrying no number and one whose *message text* reads `No such process (os error 3)`. |
| `observer/linux.rs`: `only_a_ptrace_request_of_this_module_can_report_a_task_gone` | An `ESRCH` that is not a ptrace request's own stays fatal, including when wrapped in a request's context text; and the helpers' error still prints and still reports the `root_cause` errno the `.context(…)` chain did. |
| `observer/linux.rs`: `a_proc_entry_is_gone_only_for_esrch_or_enoent` | The `/proc` predicate accepts those two error numbers and nothing else. |
| `observer/linux.rs`: `the_receipt_counts_the_tasks_that_were_gone` | A finished journal's receipt holds the seven existing keys byte for byte plus the `vanished` number, for a non-zero count and for zero. Uses a temporary folder. |
| `tests/issue_2716_vanished_observed_task.rs`: `a_thread_destroyed_by_exit_group_does_not_fail_the_run` | A thread group whose leader calls `exit_group` while a freshly cloned thread is still at its ptrace stop. The run's status is the runner's own; `run.json`'s `outcome` is `success`; stderr carries no `amplihack.recipe.failure_class ` line; coverage is `established`; and the warning appeared. |
| `tests/issue_2716_vanished_observed_task.rs`: `a_child_killed_in_its_stop_does_not_fail_the_run` | A child sent `SIGKILL` between its stop and its continue, with the same five assertions. |

Those five assertions are what is actually checkable. "Not classed
indeterminate" is not: no `run.json` field holds that value. The pair that
holds the same meaning is `outcome == "success"` together with the absence of
the `amplihack.recipe.failure_class ` marker — a classification was never
produced, because nothing failed.

Both behaviour tests use the existing `issue_2096` fixture harness: a
temporary `HOME`, `AMPLIHACK_HOME` and `CLAUDE_CONFIG_DIR`, a work root under
the temporary folder, `GIT_CONFIG_GLOBAL=/dev/null` and
`GIT_CONFIG_NOSYSTEM=1`. The real `~/.claude` and `~/.amplihack` are never
read or written. The fixture stops only a process id it created and recorded;
there is no `kill(0, ..)`, no `kill(-1, ..)`, no negative process group, no
`pkill`, no `killall` and no pattern matching.

Races cannot be made exact without timers, which issue #2716 rules out, so the
fixtures use bounded repetition — a fixed iteration count, no sleeps, no
retries, no waiting on a clock — and each test asserts that the warning
appeared. That assertion is what makes the tests non-vacuous: a race that
never landed fails on the missing line instead of passing silently. Each test
also asserts `coverage == "established"`, so a host whose ptrace scope
prevents tracing fails loudly rather than passing without having observed
anything.

Repetition alone is not enough, and this is the part worth knowing before
changing either mode. Two structural facts decide whether these races can
land at all, and neither can be bought with a higher iteration count:

- **The thread that creates a task cannot be the one that destroys it.**
  `PTRACE_O_TRACEFORK` and `PTRACE_O_TRACECLONE` stop the *creating* thread at
  its fork or clone event, and the observer holds it there until it has
  registered and resumed the newborn. A fixture whose only thread both clones
  and then calls `exit_group` therefore never overlaps the two, and the window
  never opens. So each child hands the cloning to threads that run freely and
  the group is destroyed underneath them — `clone_underneath` in the fixture.
- **A signal sent in the same breath as a `fork` arrives too early.** The
  child is killed before it has cloned anything, so there is no held stop to
  interrupt. The `sigkill-child` mode therefore signals the *previous* child,
  which has been cloning throughout the fork of the current one. At most two
  children are alive at a time, and the signal always names one process id the
  fixture itself forked and recorded.

With both in place the races land on every run; the iteration count is margin
on top, not the mechanism. If a mode stops landing, check these two
properties before raising the count.

The fixture modes race the newborn resume and the loop-tail resume only. They
never race an `exec` stop, because the `/proc` reads there are still fatal by
design and a test must not be shaped to dodge that.

## Limits

- **`ESRCH` is ambiguous and the distinction is discarded.** ptrace also
  answers `ESRCH` for "not traced by us" and "not stopped", which would be
  genuine observer-state faults. They are now absorbed into "gone". Two facts
  bound that: every request in the loop names a process whose stop was just
  consumed from `waitpid`, so no live-but-`ESRCH` state is reachable; and
  because no status is ever made up, a misclassification cannot produce a run
  that falsely succeeded — the loop simply keeps waiting. Occurrences stay
  visible in the receipt count and the stderr line.
- **A stall, rather than a false failure, is the residual risk.** If that
  bound were ever violated, a stopped and unresumed task could keep the loop
  from finishing. This is accepted deliberately: timers are ruled out,
  `PTRACE_O_EXITKILL` and the observer's teardown bound the blast radius, and a
  visible stall is better than a falsely failed P0 run that nothing retries.
  One case rests on `PTRACE_O_EXITKILL` alone: the newborn abandoned at
  `thread group identity` is never entered in `pids`, so `kill_all` does not
  signal it and `reap_all` does not wait for it. It dies with the tracer
  because the option is inherited across `clone`, not because teardown finds
  it. Its subject is still at its initial `SIGSTOP` and has executed nothing,
  so this costs no evidence.
- **The `/proc` reads at an `exec` stop are still fatal.** The `environ` read
  in `Scopes::exec` and the `exe` link read while recording an `exec` have the
  same shape of exposure and are out of scope here: the field evidence does not
  implicate them, and the integrity argument is weaker at an `exec` stop. This
  is a known adjacent hazard, tracked as
  [#2718](https://github.com/rysweet/amplihack-rs/issues/2718). Widening the
  `/proc` predicate to them "for symmetry" would be a mistake: the `environ`
  read is where a relayed nested scope's journal path is validated, so a task
  reaching an `exec` stop could then skip registration and its checks while
  the run still finished as established.
- **A degraded run leaves no durable, enforced evidence that it degraded.**
  Three things combine. `coverage` is a literal in the receipt, so it reads
  `established` whether the run lost no task or two hundred. The `vanished`
  number beside it is written and read by nothing: the recording validator
  compares seven named keys and ignores the rest, by the design above, so the
  count's only reader in the tree is this module's own test. And both visible
  surfaces are bounded or transient — the stderr line stops at sixteen per run
  while the count does not, and publication removes the trace folder after
  validation, so the receipt survives only where evidence is retained. The
  consequence is that the eight runs in #2716 were diagnosable because they
  *failed*; the same degradation on a passing run is invisible after the fact.
  This is a detectability gap and not a bypass — the degraded case can only
  stall, never report coverage it did not establish — and closing it means
  touching the report path, which was out of scope for a fix one error number
  wide. Tracked as
  [#2726](https://github.com/rysweet/amplihack-rs/issues/2726), with the
  constraint that the count must not become an eighth compared key: every
  traced tree can influence it, so a gate that reads it would hand that tree a
  new way to fail any run.
- **The "exited before registration" set is keyed by process id and never
  pruned.** Unchanged by this work. That set, and the `pending_stops` entry
  the abandoned-newborn path leaves behind, are both bookkeeping invariants
  that hold today only because `pid_max` makes reuse within one run
  unreachable; making them uniform is tracked as
  [#2718](https://github.com/rysweet/amplihack-rs/issues/2718).
- Non-Linux targets have no observer and none of this applies. See
  [Platform support](recipe-checkout-isolation.md#platform-support).

## Related

- [Recipe run and auto-drive checkout isolation](recipe-checkout-isolation.md#required-recording)
- [Recipe runner logging](recipe-runner-logging.md#git-command-record)
- [Diagnose a vanished observed task](../howto/diagnose-a-vanished-observed-task.md)
- [Keep tests from prompting for git credentials](../howto/keep-tests-from-prompting-for-git.md)
