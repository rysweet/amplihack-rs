---
title: "Keep Shell Test Stubs from Blocking on stdin"
description: "Why a multi-mode shell stub must dispatch on its mode before reading stdin, the orch helper modes that take a blob and the ones that do not, the null stdin the bundled-tool helper gives every call, and how to prove a stub answers without opening stdin."
last_updated: 2026-11-16
review_schedule: quarterly
owner: rysweet
doc_type: howto
related:
  - ./keep-tests-from-prompting-for-git.md
  - ./keep-tests-passing-under-host-load.md
  - ./stop-background-jobs-by-pid.md
  - ./parse-agent-verdicts-with-orch-helper.md
  - ../reference/structured-verdict-parsing.md
  - ../reference/recipe-checkout-isolation.md
  - ../reference/recipe-context-transport.md
---

# Keep Shell Test Stubs from Blocking on stdin

A shell stub that stands in for a command with several modes reads stdin for
exactly the modes that are given something on it. A stub that reads stdin
first and dispatches afterwards waits forever whenever one of its modes is
called with no pipe and the caller's stdin is a terminal, a still-open pipe or
an inherited descriptor that nobody closes.

This guide is for anyone writing or running the shell tests under `tests/`,
and for anyone whose test run stopped with no output and no failing check.

Issue: [#2582](https://github.com/rysweet/amplihack-rs/issues/2582).

## Contents

- [The rule](#the-rule)
- [Which orch helper modes take a blob](#which-orch-helper-modes-take-a-blob)
- [Half one: dispatch before reading](#half-one-dispatch-before-reading)
- [Half two: a null stdin at the call site](#half-two-a-null-stdin-at-the-call-site)
- [Why /dev/null and not a closed descriptor](#why-devnull-and-not-a-closed-descriptor)
- [Why the piped modes still drain stdin](#why-the-piped-modes-still-drain-stdin)
- [Why the completion state stays set](#why-the-completion-state-stays-set)
- [Prove a stub answers without stdin](#prove-a-stub-answers-without-stdin)
- [Adding a mode to a stub](#adding-a-mode-to-a-stub)
- [Where the rule lives](#where-the-rule-lives)
- [Symptoms and what they mean](#symptoms-and-what-they-mean)
- [Related](#related)

## The rule

Two halves, each sufficient on its own, kept together because each covers a
case the other does not:

| Half | Where | What it does |
| --- | --- | --- |
| Dispatch before reading | The stub | A mode that is never given a blob exits without opening stdin. Adding a call site cannot reintroduce the stop. |
| A null stdin | The call site | Every bundled tool the test starts gets stdin from `/dev/null`, so any program under it that reads stdin sees end of file at once. Covers a tool, or a stub mode, the test has not thought about yet. |

Neither half uses a timer, a `timeout` wrapper or a retry. A stub that needs
one of those to finish is a stub with this defect still in it.

## Which orch helper modes take a blob

`amplihack orch helper` has three kinds of mode, and only the first kind
belongs on a stub's blob list. What each blob mode does with its stdin — the
object it selects, the field it prints, the token it collapses to — is defined
in [Structured Verdict & Intent Parsing Reference](../reference/structured-verdict-parsing.md);
that reference is the contract, and this table records only which modes read
stdin at all and how.

`tests/issue_1437_amplihack_stub.sh` stands in for modes from the first and
third kinds.

| Mode | Stdin | Piped into by (not exhaustive) |
| --- | --- | --- |
| `extract-json` | A blob on a pipe | `amplifier-bundle/tools/autodrive_crusty_record.sh`, `workflow_enforce_verdict.sh` and six other bundled tools; also eight recipes and four recipe tests |
| `extract-field` | A blob on a pipe | The same tools and recipes, usually downstream of `extract-json` |
| `normalise-verdict` | A blob on a pipe | `amplifier-bundle/tools/workflow_enforce_verdict.sh` and `amplifier-bundle/recipes/workflow-pr-review.yaml`, as the third stage of a three-stage pipeline in both |
| `normalise-loop-verdict` | A blob on a pipe | `amplifier-bundle/recipes/loop-health-evaluator.yaml`, three sites |
| `normalise-type` | A blob on a pipe | `amplifier-bundle/recipes/smart-classify-route.yaml` |
| `select-workflow`, `build-workstreams-config`, `count-workstreams`, `reclassify-task-type` | Arguments, plus a regular-file redirect | Nothing pipes into them: each reads a tempfile (`< "$_DECOMP_TMPFILE"`, `< "$_TASK_TMPFILE"`) in `smart-classify-route.yaml` or `smart-execute-routing.yaml` |
| `autodrive-completion-seal` | Arguments only | `autodrive_crusty_record.sh`, as `--private-state <dir>`, with no pipe |
| `autodrive-completion-prepare`, `autodrive-completion-dispatch`, `step-recovery` | Arguments only | Called with arguments only |

The five pipe modes are the allowlist. Everything else answers and exits
without opening stdin.

The "piped into by" column names enough call sites to make the point, not all
of them. A mode moves between kinds only when someone changes how it is
called, so re-derive the set rather than trusting a count here. This lists
every mode in the tree with how often it is invoked:

```bash
grep -rhoE 'orch helper [a-z-]+' amplifier-bundle/ crates/ tests/ \
  | sort | uniq -c | sort -rn
```

Then read the call sites of the one you care about, which is what settles
which kind it is:

```bash
grep -rn 'orch helper normalise-verdict' amplifier-bundle/ crates/ tests/
```

The list includes `normalise-verdict` and `normalise-type` even though the
stub has no stand-in for either: being piped into is what puts a mode on the
list, not whether the stub does anything with the blob. See
[Why the piped modes still drain stdin](#why-the-piped-modes-still-drain-stdin).

The four middle modes are the reason the second column says *how* and not just
*whether*. Each one does read stdin in production, so a grep for `< ` finds
them alongside the pipe modes — but each reads it from a regular file the
caller wrote and redirected, never from a pipe. A stub that leaves one of them
off the allowlist can neither stop (a regular file returns end of file, and in
any case the stub does not open stdin at all) nor send `SIGPIPE` to anything
(a file has no writer to signal). They stay off the list until a stub needs the
blob, at which point each goes on the list like the rest.

## Half one: dispatch before reading

In `tests/issue_1437_amplihack_stub.sh`, the `orch helper` branch decides
whether this mode has a blob at all before it parses a flag or opens stdin.
With `BLOB="$(cat)"` ahead of that dispatch, `autodrive-completion-seal`
inherited the caller's stdin and the test stopped there (#2582); a mode that is
piped into belongs on the allowlist even when the stub has no stand-in for it:

```bash
if [ "${1:-} ${2:-}" = "orch helper" ]; then
  if [ -n "${REAL_AMPLIHACK:-}" ]; then exec "$REAL_AMPLIHACK" "$@"; fi
  MODE="${3:-}"
  # Answer and leave, before stdin or the arguments are touched, for every mode
  # this stub has no stand-in for: see the stdin rule in this file's header.
  # Only the five modes below carry a blob, and only they use the arguments.
  case "$MODE" in
    normalise-loop-verdict|extract-json|extract-field|normalise-verdict|normalise-type) : ;;
    *) exit 0 ;;
  esac
  FIELD=""; DEFAULT=""; REQUIRE_BOOLEAN=false
  shift 3 || true
  while [ $# -gt 0 ]; do
    case "$1" in
      --require-field|--field) FIELD="${2:-}"; shift 2 ;;
      --default) DEFAULT="${2:-}"; shift 2 ;;
      --require-boolean) REQUIRE_BOOLEAN=true; shift ;;
      *) shift ;;
    esac
  done
  BLOB="$(cat)"
  case "$MODE" in
    normalise-loop-verdict)
      ...
```

Three details carry weight:

- The gate sits above the flag parse, so the five modes past it are exactly the
  modes that reach `BLOB="$(cat)"`. That keeps the read unconditional: there is
  no path to the stand-ins on which `$BLOB` is unset, so `set -u` needs no
  `BLOB=""` ahead of the dispatch and the allowlist is stated once rather than
  split across a read gate and a dispatch that have to be kept in step.
- `*) exit 0` keeps the answer the stub already gave an unhandled mode. The
  second `case` ends the same way, so the behaviour of a mode the stub does
  not implement is unchanged: exit 0, no output. Skipping the flag parse
  changes nothing either: it only ever set `$FIELD`, `$DEFAULT` and
  `$REQUIRE_BOOLEAN`, which none of the modes past `*) exit 0` read.
- The `REAL_AMPLIHACK` passthrough stays above the dispatch. When a test sets
  that variable the stub is not in the path at all, and the real binary reads
  stdin for whichever modes it reads stdin for.

The same shape applies to any new stub: the mode is an argument, so it is
known before a single byte of stdin is needed.

## Half two: a null stdin at the call site

Before the redirect, the hop it protects. A bundled tool does not run the stub
directly. It runs `$AMPLIHACK_BIN`, which `tests/issue_1437_autodrive_loop.sh`
points at `amplihack-alt`, a second script the test writes inline just above
`ab()` to prove every call goes through `$AMPLIHACK_BIN` and not a bare
`amplihack` on `PATH`. `amplihack-alt` logs the call and forwards one
subcommand family to the stub:

```bash
  '  "orch helper") STUB_STATE="$AB_STATE" exec "$AB_STUB" "$@" ;;' \
  '  "internal review-bind-record") cat -- "$4"; exit 0 ;;' \
```

So the chain is: bundled tool → `amplihack-alt` → `exec` the stub, with stdin
inherited at every step and never redirected along the way. `amplihack-alt`
itself never reads stdin — it `printf`s to a log, and its
`review-bind-record` arm reads a file named in `$4`, not a pipe — which is
what makes [half one](#half-one-dispatch-before-reading) sufficient on its
own: the only process in that chain that can block on stdin is the stub.

`tests/issue_1437_autodrive_loop.sh` runs bundled tools through one helper, so
the redirect belongs in the helper and not at the five call sites:

```bash
ab() { # ab <script> <args...>: run a bundle script with the alternate binary
  # </dev/null: no tool under here may block on the caller's stdin (#2582).
  AB_LOG="${AB}/log" AB_STATE="${AB}/state" AB_STUB="${WORK}/bin/amplihack" REAL_AMPLIHACK="" \
    HOME="${WORK}/home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
    AMPLIHACK_BIN="${AB}/amplihack-alt" PATH="${AB}/path:${PATH}" bash "${TOOLS}/$1" "${@:2}" </dev/null
}
```

The rest of the helper's isolation is unchanged: a tempfile `HOME`, no global
or system git configuration, and the alternate binary under
`AMPLIHACK_BIN` so that every call has to go through it rather than a bare
`amplihack` on `PATH`.

Run a shell test the same way when you run it by hand:

```bash
bash tests/issue_1437_autodrive_loop.sh </dev/null
```

CI already gets the equivalent for free. The `Auto-drive — an agent/API
failure is not a policy refusal (issue #1437)` step in
`.github/workflows/ci.yml` runs `bash tests/issue_1437_autodrive_loop.sh` with
no terminal on stdin, so a stub that reads it sees end of file at once. That is
why this defect stayed invisible in CI and appeared the first time someone ran
the test from a terminal or from an agent session, where stdin stays open.

## Why /dev/null and not a closed descriptor

`<&-` closes descriptor 0 instead of pointing it somewhere harmless. The
descriptor is then free, so the next `open()` in that process or any child
receives 0, and a file the program opened for its own purposes becomes what
anything reading stdin reads — or writes. `</dev/null` keeps descriptor 0
occupied and returns end of file on the first read.

Use `</dev/null`. It is also what the git credential guidance uses, for the
same reason; see
[Keep Tests from Prompting for Git Credentials](./keep-tests-from-prompting-for-git.md).

A null stdin does not stop git, which opens `/dev/tty` directly. That is a
separate problem with a separate answer, in the same guide.

## Why the piped modes still drain stdin

`amplifier-bundle/tools/workflow_enforce_verdict.sh` builds a three-stage
pipeline, and `amplifier-bundle/recipes/workflow-pr-review.yaml` builds the
same three stages a second time:

```bash
VERDICT_LINE=$(printf '%s' "$VERDICT_RAW" | amplihack orch helper extract-json)
VERDICT=$(printf '%s' "$VERDICT_LINE" \
  | amplihack orch helper extract-field --field verdict --default INSUFFICIENT_EVIDENCE \
  | amplihack orch helper normalise-verdict)
```

If a stage exits without reading, the writer upstream gets `SIGPIPE` and the
pipeline's status becomes 141. Under `set -o pipefail`, which the loop test
sets, that turns a clean pass into a failure whose cause is nowhere near the
test that reports it. So a mode that anything in the tree pipes into stays on
the blob list, whether or not the stub does anything with what it reads.

Both effects come from the same decision, in opposite directions:

| Mode is | Reads stdin | Result if wrong |
| --- | --- | --- |
| Piped into | Must read | Writer takes `SIGPIPE`, pipeline exits 141, `pipefail` fails an unrelated check |
| Not piped into | Must not read | The stub waits on the caller's stdin and the test produces no further output |

## Why the completion state stays set

The hang surfaced at one check, and the quickest way to make it go away is to
leave `AUTODRIVE_COMPLETION_STATE` unset so `autodrive_crusty_record.sh` never
reaches its seal call. Do not do that. The check exists because the seal has
to go through `$AMPLIHACK_BIN`:

```bash
ok AMPLIHACK-BIN-RECORD-SEAL "the completion seal calls \$AMPLIHACK_BIN" \
  grep -qxF "BIN orch helper autodrive-completion-seal --private-state ${AB}/completion" "${AB}/log"
```

Unsetting the variable deletes that coverage and leaves the stub defect in
place for the next mode that takes no blob. The variable stays set; the stub
and the helper change instead.

## Prove a stub answers without stdin

Running the test with `</dev/null` proves the pair works, but it does not
distinguish the two halves: the redirect alone makes a stub that reads first
finish too. Two checks show the stub itself.

### Check one: the blob is still there afterwards

Give the stub a pipe, and read what is left on it after the stub exits. A mode
that does not open stdin leaves the bytes for the next reader:

```bash
d="$(mktemp -d)"
printf 'LEFTOVER' | {
  STUB_STATE="$d" bash tests/issue_1437_amplihack_stub.sh \
    orch helper autodrive-completion-seal --private-state "$d/state" >/dev/null
  cat
}
echo; rm -rf "$d"
```

Expected output:

```text
LEFTOVER
```

Against a stub that reads stdin before it dispatches, the same command prints
nothing, because the `cat` in the stub consumed the blob. This check never
blocks, whatever the stub does, so it is the one to use in a test or a hook.

The same check on a blob mode prints nothing, and should: `extract-json` is on
the list precisely because it has to consume what it is given.

### Check two: a stdin that never ends

To show the stronger property — that the stub does not merely consume stdin
quickly but never opens it — give it a stdin that never reaches end of file.

A FIFO opened read-write has no writer to close and no reader to hang up, so a
read on it blocks forever:

```bash
d="$(mktemp -d)"; mkfifo "$d/stdin"
exec 9<>"$d/stdin"
STUB_STATE="$d" bash tests/issue_1437_amplihack_stub.sh \
  orch helper autodrive-completion-seal --private-state "$d/state" <&9
echo "exit $?"
exec 9>&-; rm -rf "$d"
```

Expected output:

```text
exit 0
```

It returns at once. Completion is the whole signal: there is no timer and no
`timeout` wrapper, because a wrapper here would both hide the defect and
invite the process-group stop that
[#2674](https://github.com/rysweet/amplihack-rs/issues/2674) describes.

Against a stub that reads stdin first, the same command does not return. If
you try it that way, start it in the background, record the PID and stop that
PID:

```bash
STUB_STATE="$d" bash tests/issue_1437_amplihack_stub.sh \
  orch helper autodrive-completion-seal --private-state "$d/state" <&9 &
pid=$!
kill "$pid"      # the PID this shell started, and only that PID
wait "$pid" 2>/dev/null
```

Never reach for `pkill -f`, `killall` or `kill $(pgrep ...)` here or anywhere
else. Matching on process text is exactly the capability #2582 removes, and
three mass stops of live runs came from it. See
[Stop Background Jobs by PID](./stop-background-jobs-by-pid.md).

## Adding a mode to a stub

| Question | If yes | If no |
| --- | --- | --- |
| Does anything pipe into the mode? | Add it to the blob list in the first `case`, even if the stub writes nothing for it | Leave it off the list; it falls to `*) exit 0` |
| Does the stub need to parse the blob? | Add a stand-in to the second `case` | The blob is read and discarded, which is enough to keep the writer from taking `SIGPIPE` |
| Does the mode take a field name? | It already comes through `--field` or `--require-field` | Do not extend the `$FIELD` interpolation to a mode that does not use it |
| Is a new call site a bundled tool? | Run it through the helper that supplies `</dev/null` | Supply `</dev/null` at the call site |

Then bring the header with you. The stub's `orch helper` header comment is the
first thing a reader meets, and it is the only prose statement of the
invariant inside the test tree, so it names the invariant and lists every mode
on the allowlist — including the ones the stub reads and discards. A header
that describes two modes while the `case` allows five invites the next editor
to delete an allowlist entry that looks undocumented.

Keep fixture writes inside the test's own `$WORK` or `$STUB_STATE`, and leave
`autodrive_crusty_record.sh`'s `umask 077` alone: the round record is trusted
as evidence only because nothing else could have written it.

## Where the rule lives

| File | What it holds |
| --- | --- |
| `tests/issue_1437_amplihack_stub.sh` | The `orch helper` dispatch, the blob list, and a header comment naming the invariant and listing all five allowlisted modes |
| `tests/issue_1437_autodrive_loop.sh` | The `ab()` helper with its `</dev/null`, and the inline `amplihack-alt` that `exec`s the stub for `orch helper` |
| `tests/issue_1437_autodrive_refusal.sh` | The other test that copies the same stub onto `PATH` |
| `amplifier-bundle/tools/autodrive_crusty_record.sh` | The caller whose seal step takes no blob |
| `.github/workflows/ci.yml` | The two steps that run these tests |

## Symptoms and what they mean

| Symptom | Cause | Fix |
| --- | --- | --- |
| A shell test prints its earlier checks and then nothing, and does not exit | A stub mode is reading a stdin that never closes | Put the mode on the right side of the dispatch; run the test with `</dev/null` |
| The same test passes in CI and stops in a terminal | CI supplies `/dev/null`; a terminal does not | The stub, not the terminal |
| A check fails with status 141 and names a tool that was working | A stub mode stopped draining a pipe it is piped into | Add the mode to the blob list |
| A stub mode answers but a later `open()` in a tool reads the wrong thing | The call site used `<&-` and descriptor 0 was reused | Use `</dev/null` |

## Related

- [Keep Tests from Prompting for Git Credentials](./keep-tests-from-prompting-for-git.md) - The null stdin the Rust test helpers set, and why git needs more than one
- [Keep Tests Passing Under Host Load](./keep-tests-passing-under-host-load.md) - Waiting on events and conditions instead of timers
- [Stop Background Jobs by PID](./stop-background-jobs-by-pid.md) - Stop a job you started by `$!`, never by pattern
- [Structured Verdict & Intent Parsing Reference](../reference/structured-verdict-parsing.md) - The stdin contract of every blob mode in the table above, and the source of truth a stub stands in for
- [Parse Agent Verdicts with orch helper](./parse-agent-verdicts-with-orch-helper.md) - How to build the pipelines the blob modes sit in
- [Recipe Checkout Isolation](../reference/recipe-checkout-isolation.md) - The private review checkout the same stub stands in for
- [Recipe Context Transport Reference](../reference/recipe-context-transport.md) - The #2582 change these tests cover
