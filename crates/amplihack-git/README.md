# Git boundaries

This crate owns sanitised Git subprocesses and the workflow's repository,
index and publication boundaries. It does not change recipe-runner defaults.

`Workspace::open` validates an existing checkout without changing it.
`require_workflow` additionally requires an origin, committed base history and
complete history without local grafts. Ordinary launch never initialises Git.

`runtime_preparation::prepare` registers the owned runtime catalogue in common
Git `info/exclude`. Cooperating writers serialise on a separate advisory lock.
Replacement is atomic, keeps existing bytes and permissions, and is idempotent.
Configuration remains visible; exclusions are not publication authority.

`launcher_state::resolve` computes an external path from canonical clone and
checkout identities without creating directories. `prepare` creates private
directories. Checkout-contained overrides, including symlink aliases, fail.
Launcher context includes and verifies its original identity metadata.

`TaskScope` binds exact paths agreed by established requirements and reviewed
design to a version, task, project and checkout. Candidate lists and Git refs
never add authority. Missing scope fails closed.

Checkpoints lock the real index, reject prohibited or overlapping staged work,
and commit only authorised candidates through a private index. All normal
commit hooks run. Wrappers validate after each index-mutating commit hook,
including commit-msg. Reconciliation changes only committed candidates and
preserves the user's staged blobs, modes and deletions. Reconciliation errors
identify a commit that has already been created.

Publication freezes exact branch updates and obtains one advertisement from
each actual push destination. Disposable repositories receive the complete
advertised and proposed histories. The outgoing set is proposed reachability
minus the reachability of every advertised commit tip. Every outgoing commit
is checked separately, including changes later reverted. A two-parent merge
is compared with its reconstructed automatic merge tree, so advertised imports
remain valid but unauthorised resolution changes are rejected. Unsupported or
unreconstructable merges fail closed. Failed or partial fetches stop publication.
Tracking refs, FETCH_HEAD, replace refs and grafts cannot shrink the range.

Publication uses exact object IDs and destination refs, ordinary non-forced
atomic pushes and normal hooks. Implicit tag following is disabled. A later
transport failure reports any earlier completed destinations. The CLI adapter
returns status 78 for authority refusals and status 2 for operational failures.
`complete-publication` emits one versioned JSON result across preparation,
checkpointing and pushing, including partial outcomes. Rewrite chains that
would redirect an expanded URL are refused. Implicit submodule publication is
disabled. Checkpoints support index versions 2 and 3 and rehearse preservation
of all unaffected entry metadata before committing.

The original 25 native template controls, the qualifying released and authorised
runner for #201, and Linux-only cases remain separate acceptance obligations.
