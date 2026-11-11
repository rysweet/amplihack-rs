---
title: Trace2 Recording Investigation
description: Retained diagnostic evidence explaining why cooperative Trace2 cannot certify descendant coverage.
doc_type: reference
last_updated: 2026-11-10
---

# Trace2 recording investigation

This document preserves an earlier diagnostic and its limitations. Its
implementation-status statements and host-specific results below describe
that experiment, not the execution contract. For the feature contract, see
[checkout isolation](../reference/recipe-checkout-isolation.md#required-recording).
For current requirement dispositions, see the
[requirement/review ledger](../reference/recipe-checkout-isolation.md#requirement-and-review-disposition-ledger).
Statements below that production integration was unchanged refer to the
diagnostic revision; later linked-worktree/Trace2 code does not satisfy the
stronger target contract.
The [recording review](evidence/issue-2096-recording-review.txt) and
[review follow-up](evidence/issue-2096-step17e-review.md) remain historical
evidence; neither certifies production behavior or identifies a historical writer.

This is prerequisite investigation evidence, not an implemented isolation feature
or the complete candidate-1 reproduction. No fix or historical attribution is
claimed.

The Rust example at
[`issue_2096_trace2_probe.rs` at the diagnostic revision](https://github.com/rysweet/amplihack-rs/blob/60ecbe573b60ff30f47273510e48a90341ffd217/crates/amplihack-git/examples/issue_2096_trace2_probe.rs)
creates a disposable caller repository, detached linked checkout, and profile
directories. It invokes absolute Git, stages private files, exercises relative
`GIT_INDEX_FILE` from two subdirectories, and attempts staging with an invalid
Trace2 destination. It compares caller index bytes before and after. It does not
run the repository's real hook set; that reproduction remains outstanding.

The review follow-up adds a shell-launched commit in the detached linked
checkout. Its controlled `pre-commit` hook clears inherited Git routing,
changes directory, and launches absolute Git twice. The second descendant
unsets `GIT_TRACE2_EVENT`. Its marker confirms execution, but its argv is absent
from the trace. A successful parent and a nonempty trace therefore do not prove
complete collection. This hook is a diagnostic fixture, not the project's hook
set.

The probe also attempts the platform observer without privilege escalation:
`dtruss -f /usr/bin/true` on macOS, or a process/cwd `strace` probe on Linux.
`observer.status` always labels coverage unverified, even if the observer exits
successfully. Tool presence and successful initialization are insufficient to
establish recording coverage or failure supervision.

At that revision, the diagnostic was invoked with a fresh destination under a
trusted parent:

```sh
cargo run --locked -p amplihack-git --example issue_2096_trace2_probe -- /private/tmp/issue-2096-probe-new
```

The destination must not exist and is created with mode 0700. Evidence stays
outside disposable runtime directories. `commands.txt` contains probe-controlled
launches, statuses, and output; these parent records are not descendant command
attribution. `trace2*.jsonl` contains Git's own events. `expected-index*.txt`
contains independently queried index paths. Binary before/after index files and
`recording-failure.*` retain the storage-failure experiment. The example refuses
destination reuse and retains partial evidence on error. It is a diagnostic
example, not the protected production artifact store described in the design.

Observed on macOS with Git 2.54.0 (Apple Git-157):

- Absolute invocation produces Trace2 start argv and timestamp records.
- Repository events identify the worktree. Index-write regions contain an
  absolute lock path, including for the relative alternate index.
- Git normalizes the relative alternate index to the checkout root in this
  experiment. Guessing from the parent's initial cwd would give the wrong path.
- An invalid Trace2 destination does not make staging fail. Native Trace2 is
  insufficient to enforce the required recording-failure policy by itself.
  The private index changed, the Git exit status was zero, and the caller index
  remained byte-identical.

Local validation passed: example execution, targeted Clippy with `-D warnings`,
Rust formatting, and whitespace checks. Retained evidence for this run is at
`/private/tmp/issue-2096-trace2-verified`; the build emitted macOS xcrun cache
permission warnings but completed successfully. No workspace-test or applicable
hook completion is claimed.

Trace2 does not establish complete executable provenance or arbitrary descendant
launch cwd here. Its `exe` field in the version event is a Git version string.
The example's parent launch evidence must not be presented as satisfying those
requirements. Shell, agent, and hook boundaries, recorder supervision, supported
Git versions, and the pinned runner still need capability validation.

The design explicitly requires stopping dependent implementation if required
attribution cannot be established. Production runner and auto-drive integration
remain unchanged pending a verified supplemental observation and failure-delivery
mechanism. This is an incomplete implementation step. Required regression tests,
real-hook candidate-1 reproduction, Linux process-observation coverage, and PR
delivery remain outstanding. Nothing in this experiment establishes which
command changed the historical caller index.

## Review follow-up validation

The updated example executed successfully using the pinned Rust 1.97.0 toolchain
and disposable build output. Clearing `RUSTC_WRAPPER` avoids the previous ccache
failure; setting `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`
avoids SDK discovery blocking compilation on this host. Apple tool wrappers
still emitted xcrun cache permission warnings. Direct compilation with the
toolchain's `rustc` and the Command Line Tools clang linker also passed. The
modified example passed the pinned `rustfmt --check`.

Sanitized retained outcomes, selected events, controlled hook commands, and
caller-index comparisons are in
[`evidence/issue-2096-recording-review.txt`](evidence/issue-2096-recording-review.txt).
Original evidence is retained separately at
`/private/tmp/issue-2096-review-proof-197`; the report does not rewrite it.

| Required boundary / failure | Observed result | Acceptance |
| --- | --- | --- |
| Absolute Git with `-C` | Trace2 argv and repository events | Partial: executable identity and launch cwd unproven |
| Relative alternate index from changed cwd | Independently queried index paths retained | Partial: not arbitrary descendant attribution |
| Shell → commit → hook → absolute Git | Controlled commit succeeds and marker identifies private checkout | Partial: project hooks and runner not exercised |
| Hook disables Trace2 | Git succeeds; descendant command missing from trace | Failed collection guarantee |
| Trace2 destination is not a directory | Git succeeds and private index changes | Failed failure-supervision guarantee |
| macOS supplemental observer | DTrace initialization denied: `Operation not permitted` | Unsupported in this sandbox |
| Linux supplemental observer | Probe code added; not executed here | Unverified |
| Production runner / agent boundaries | Not exercised | Unverified |
| Collection, persistence, finalization supervision | No verified mandatory observer | Unimplemented |

The prerequisite remains blocked. No production isolation, trusted review
provisioning, cleanup ownership enforcement, or mandatory recorder has been
integrated. These guarantees must not be inferred from this diagnostic's zero
exit status. Per the required dependency order, dependent implementation stops
here until a supported observer can establish complete attribution and deliver
collection failures to the supervisor. No production-path regression test,
project hook completion, full candidate-1 reproduction, or #2096 completion is
claimed.
