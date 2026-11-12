//! #2240: behaviour of the live quick-change commands, including real step 04.
//!
//! Synthetic checkouts, a local bare origin and fake gh isolate publication.
//! RecipeContext renders child overrides and exports the real setup JSON;
//! no test fabricates worktree_setup or replaces guards, hooks or repair.
//! Only the builder's edits and structured result are supplied by the tests.
//! Command completion and hook records provide synchronisation, without timers.
#![cfg(unix)]

#[path = "issue_2240_quick_change_continuation/contracts.rs"]
mod contracts;
#[path = "issue_2240_quick_change_continuation/fixture.rs"]
mod fixture;
#[path = "issue_2240_quick_change_continuation/publication.rs"]
mod publication;
#[path = "issue_2240_quick_change_continuation/selection.rs"]
mod selection;

#[path = "issue_2240_quick_change_continuation/environment.rs"]
mod environment;

#[path = "issue_2240_quick_change_continuation/safeguards.rs"]
mod safeguards;
