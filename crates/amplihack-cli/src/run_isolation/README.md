# Recipe execution isolation internals

These Rust modules implement the candidate for issue #2096. They are included
at the recipe and internal-command boundaries; they do not provide a public
API or an operating-system sandbox. Acceptance and delivery remain incomplete.

`input_snapshot.rs` captures a committed tree, staged object identities, and
working contents independently. Preparation compares observations before and
after capture. Normal untracked files and an explicitly selected ignored recipe
are supported. Byte paths and safe relative symlinks are preserved. Unsupported
index flags, conflicts, submodules, special files, and escaping symlinks fail
preparation. Complete declarations for required ignored inputs and external
dependencies are not implemented.

Installation checks staged object identities in one Git batch, reuses blobs
already transferred by the private clone, and copies only missing staged blobs.
Query failures or unexpected responses abort preparation. This avoids two Git
processes per already cloned blob without sharing object storage or caching
caller state across runs. Missing-blob transfers still verify the resulting ID.

`commands/recipe/run/run_checkout.rs` owns the execution lease. Every independent
launch, including nested launches, provisions a private repository and detached
worktree without caller object alternates or hardlinks. Preparation disables
global/system configuration, hooks, executable filters, and templates. Explicit
caller-local absolute paths map into the execution tree. Embedded opaque caller
paths are rejected. Release validates the manifest and filesystem identities;
durable child worktrees retain their repository metadata. Descriptor-relative
cleanup and later reclamation of retained repositories remain acceptance gaps.

`observer/linux.rs` observes successful executable launches in the runner's
descendant tree using Linux ptrace. It follows fork, vfork, clone, and exec, and
records executables named `git` or `git-*` except known read-only commands.
Journals carry run and attempt identities, executable, cwd, launch-selected Git
directory/index, routing environment, redacted argv, and timestamp. Required
storage and attribution failures fail supervision; journals and receipts are
synchronized before final publication. A verified ancestor observer can relay
an independent nested scope into that run's own journal. An environment marker
alone cannot authorize reuse. This observes neither unrelated host processes,
arbitrary non-Git index writes, nor preparation/release commands. It does not
establish same-user tamper resistance. Non-Linux production execution is
rejected rather than falling back to advisory Trace2.

`review_generation.rs` provisions exact recorded PR commits in private
repositories and preserves PR instruction files as review data. Separate launch
directories contain explicitly supplied trusted instructions. A review
generation is trusted only after three checks: it has an ownership manifest the
run wrote itself; the filesystem, source-repository and private-repository
identities recorded there still match; and HEAD is detached at the recorded
PR-source commit. Release deletes only paths that manifest proves the run owns
and never follows symlinks. Native publication
adds the generation and reviewed SHA to verdict records. Independently launched
loops own separate generations even when they share an evidence directory.
Advancing a PR creates a new generation, and cleanup releases only its owner's
generation chain. Actual agent automatic instruction/configuration discovery
still requires production validation.

Regression contracts live in `tests/issue_2096_*`. The focused suite uses
temporary repositories and explicit temporary profiles. Socket handshakes and
observer acknowledgements establish overlap and completion. The opt-in real-hook
replay is separate from the controlled Rust runner/agent/hook fixtures. See
`docs/operations/evidence/issue-2096-step8-implementation.md` for completed checks,
remaining review findings, and delivery blockers.
