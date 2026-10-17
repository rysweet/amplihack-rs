//! Rollback barriers through production recovery, distinct from committed cleanup.
//! Fault injection proves ordering/error handling, never power-loss behavior.
use super::*;
mod contents;
mod fixtures;
mod publication;
mod safety;
