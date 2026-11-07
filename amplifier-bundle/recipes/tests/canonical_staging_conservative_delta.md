# Checkpoint scope

Each bounded checkpoint uses its existing phase and a complete extracted
helper. Capture records the parent task, linked worktree, branch, Git and index
identities, literal paths and outgoing history. A read-only reviewer selects
only captured paths required by that task. This grants path scope, not code
approval.

Immutable authority remains outside the staged worktree. Its hash and file
identity are held separately by the calling shell. Operation observations
cannot renew permission. Refusal before authorised mutation preserves the
entry state. A later failed operation preserves its actual index, worktree,
commits and error.

Strict staging validates the complete literal list before writing. Empty
permission stages nothing. No-argument callers retain their existing behaviour.
Scoped repair may change content within reviewed paths; unknown paths stop
restaging, retry and nested commits. Terminal status 79 remains terminal.

Configuration inspection and publication must use the same owner-declared
destination. Shallow history is unsupported because it cannot establish every
required parent and outgoing change. Actual commits are checked against every
parent, including intermediate and unreachable commits.

For a supported rebase, validation replays each known original change onto the
fetched parent and compares complete trees. Git writes replay objects into an
assigned private object directory, reading existing objects through an
explicit alternate. Those private writes are recorded. The validator does
not write the user's index, worktree, references or repository object store.
Unsupported or conflicting replay refuses before publication.

The fixture requirements are in [canonical-staging.md](canonical-staging.md).
The original 40 controls, four additional opt-out controls and seven checkpoint
sites remain distinct inventories. A source inspection or mocked decision
does not replace their execution.
