//! Portable auto-drive helpers. None of them merges a PR: the only merge path
//! is the finalize `merge` step (`autodrive_merge.sh`), gated by the captured
//! invocation that `helper autodrive authorization` reports.
pub mod authorization;
pub mod helper;
pub mod identity;
pub(crate) mod report;
pub mod state;
pub(crate) mod trusted;
