//! Trusted HTTP identities and interruption recovery using fake credentials.
#![cfg(target_os = "linux")]
use crate::support::profile_2242::{Fixture, inode, private, value};
use std::fs;

#[test]
fn verified_duplicate_does_not_prevent_a_same_cycle_real_exchange() {
    let f = Fixture::new();
    let live = inode(&f.live());
    let spare = inode(&f.slot("spare"));
    private(&f.slot("work"), &fs::read(f.live()).unwrap());
    let before = f.requests("work").len();
    let o = f.rotate();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(inode(&f.live()), spare, "{}", f.log());
    assert_eq!(inode(&f.slot("work")), live);
    assert!(
        f.requests("work").len() >= before + 2,
        "verify both credential identities"
    );
    assert_eq!(value(&f.state().join("swap.json"))["seq"], 1);
}

#[test]
fn every_parking_interruption_keeps_custody_explained_and_recovers_to_one_swap() {
    // Required named boundaries. No timer races or process observation.
    for point in ["duplicate-intent", "duplicate-moved", "duplicate-receipt"] {
        let f = Fixture::new();
        private(&f.slot("work"), &fs::read(f.live()).unwrap());
        let spare = inode(&f.slot("spare"));
        let o = f
            .command(&["profile", "rotate"])
            .env("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER", point)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(86),
            "interruption point {point} was not reached: {}",
            f.log()
        );
        assert!(
            f.state().join("journal.json").exists(),
            "durable custody intent at {point}"
        );
        let journal = fs::read(f.state().join("journal.json")).unwrap();
        assert!(
            !crate::support::rotate_2242_legacy::accepts(&journal),
            "a pre-#2242 reader must refuse active parking before movement at {point}"
        );
        assert!(
            !f.state().join("swap.json").exists(),
            "incomplete swaps publish nothing"
        );
        let _ = f.rotate();
        assert_eq!(
            inode(&f.live()),
            spare,
            "recover {point} in the next driven cycle"
        );
        assert_eq!(value(&f.state().join("swap.json"))["seq"], 1);
        assert!(!f.state().join("journal.json").exists());
    }
}
