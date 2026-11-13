//! Cargo entry point for the shared contract's TDD tests. Keeping the tests
//! outside lib.rs lets unrelated library tests compile during the red phase.
#[path = "../src/usage_observation/tests.rs"]
mod contract;
