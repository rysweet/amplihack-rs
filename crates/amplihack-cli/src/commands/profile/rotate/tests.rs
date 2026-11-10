//! Test modules for `rotate` that use no other test module and are used by
//! none, gathered here so that `rotate/mod.rs` stays within its line budget
//! (`rotate_files_stay_within_the_line_budget`). Each file still sits beside
//! `mod.rs`; the glob import below keeps their `super::` paths pointing at
//! the `rotate` items they test.

#[allow(unused_imports)]
use super::*;

#[path = "tests_adopt_unreadable.rs"]
mod tests_adopt_unreadable;
#[path = "tests_autostart_owner.rs"]
mod tests_autostart_owner;
#[path = "tests_damp.rs"]
mod tests_damp;
#[path = "tests_decide.rs"]
mod tests_decide;
#[path = "tests_fixture_identity.rs"]
pub(super) mod tests_fixture_identity;
#[path = "tests_gate_record.rs"]
mod tests_gate_record;
#[path = "tests_identity_limits.rs"]
mod tests_identity_limits;
#[path = "tests_issue_1886_collide.rs"]
mod tests_issue_1886_collide;
#[path = "tests_issue_1947.rs"]
mod tests_issue_1947;
#[path = "tests_ledger.rs"]
mod tests_ledger;
#[path = "tests_live_readers.rs"]
mod tests_live_readers;
#[path = "tests_login_identity.rs"]
mod tests_login_identity;
#[path = "tests_overflow.rs"]
mod tests_overflow;
#[path = "tests_owner.rs"]
mod tests_owner;
#[path = "tests_pin_platform.rs"]
mod tests_pin_platform;
#[path = "tests_platform.rs"]
mod tests_platform;
#[path = "tests_platform_block.rs"]
mod tests_platform_block;
#[path = "tests_platform_identify.rs"]
mod tests_platform_identify;
#[path = "tests_settings.rs"]
mod tests_settings;
#[path = "tests_swap_record.rs"]
mod tests_swap_record;
