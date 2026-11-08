# Branch and filename identity

Step 04's `branch_name` identifies the publication branch. Its optional
`local_branch` identifies the linked worktree's local alias. Admission checks
the declared local branch against HEAD and checks both names against protected
branches. An alias must have the existing `<branch_name>--wt-<positive integer>`
form and track `origin/<branch_name>`.

The immutable binding retains both names. Ordinary branches also require
tracking and publication to agree with the owner-declared branch. Outgoing
history is calculated for that destination. A configuration change cannot
silently substitute another publication range.

Strict staging compares each existing path component with literal directory
entry bytes before writing the index. A case-insensitive filesystem must not
resolve reviewed spelling to a different filename. Symlink parents and
malformed or stale inputs refuse without a broad staging fallback.

The synthesis-alias tests in
`crates/amplihack-cli/tests/workflow_checkpoint_ports.rs` exercise alias capture, normal and rejecting hooks, failed publication,
stale ownership, unrelated staging and literal spelling. The additional
publication and history suites cover routing, complete ancestry and
independent merge-parent differences. Their results remain separate from
native runner acceptance and the requirements in
[canonical-staging.md](canonical-staging.md).
