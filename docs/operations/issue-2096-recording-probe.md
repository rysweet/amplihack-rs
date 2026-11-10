# Issue #2096: recording capability probe

This is prerequisite investigation evidence, not an implemented isolation feature
or the complete candidate-1 reproduction. No fix or historical attribution is
claimed.

The Rust example at
[`issue_2096_trace2_probe.rs`](../../crates/amplihack-git/examples/issue_2096_trace2_probe.rs)
creates a disposable caller repository, detached linked checkout, and profile
directories. It invokes absolute Git, stages private files, exercises relative
`GIT_INDEX_FILE` from two subdirectories, and attempts staging with an invalid
Trace2 destination. It compares caller index bytes before and after. It does not
run the repository's real hook set; that reproduction remains outstanding.

Run it with a fresh destination under a trusted parent:

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
