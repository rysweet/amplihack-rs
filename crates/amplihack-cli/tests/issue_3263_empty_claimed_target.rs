//! Issue #3263: a run that never builds with Cargo leaves the folder its
//! launcher claimed empty, and that folder is removed when the run ends.
//!
//! Drives the real `amplihack recipe run` with the stub runner of
//! `issue_2738_support`, launched in the fixture's caller checkout, which
//! holds no Cargo workspace. Each run gets a claimed folder of its own inside
//! the fixture's tempfile root and has `AMPLIHACK_RUN_ID` removed, so it is
//! the outermost run and never inherits a claim from the process running it.
//!
//! Contract:
//! - a claimed folder that is still empty at the end of the run is removed;
//! - a claimed folder holding one unrelated file is kept with that file;
//! - the run's exit status is the same in both cases.
//!
//! The first test passes on the base of #3263's fix as well: an empty claimed
//! folder has been removed since v1.2.194 (#2906), and the host report in
//! #3263 did not reproduce. These tests pin that behaviour for a whole run;
//! they do not reproduce the reported fault, whose cause is not known.

#![cfg(unix)]

use crate::issue_2738_support::{Fixture, succeeds};
use std::fs;
use std::path::Path;
use std::process::Output;

const FLAG: &str = "AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR";

fn run_with_claim(f: &Fixture, label: &str, claimed: &Path) -> Output {
    Fixture::finish(
        label,
        f.command(label)
            .env("CARGO_TARGET_DIR", claimed)
            .env(FLAG, "1")
            .env_remove("AMPLIHACK_RUN_ID"),
    )
}

#[test]
fn a_claimed_folder_that_stayed_empty_is_removed_when_the_run_ends() {
    let f = Fixture::new();
    let claimed = f.root.join("claimed-empty");
    fs::create_dir(&claimed).unwrap();

    let out = run_with_claim(&f, "empty", &claimed);

    succeeds(&out);
    assert!(
        !claimed.exists(),
        "the empty claimed folder was left:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_claimed_folder_holding_an_unrelated_file_is_kept_with_the_file() {
    let f = Fixture::new();
    let claimed = f.root.join("claimed-not-cargo");
    fs::create_dir(&claimed).unwrap();
    fs::write(claimed.join("notes.txt"), "not build output\n").unwrap();

    let out = run_with_claim(&f, "unrelated", &claimed);

    succeeds(&out);
    assert_eq!(
        fs::read_to_string(claimed.join("notes.txt")).unwrap(),
        "not build output\n"
    );
}
