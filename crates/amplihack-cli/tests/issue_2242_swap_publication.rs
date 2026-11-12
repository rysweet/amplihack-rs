//! Crash boundaries carry the original automatic completion into recovery.
#![cfg(target_os = "linux")]
use crate::support::profile_2242::{Fixture, inode, value};

#[test]
fn recovery_between_placement_and_publication_publishes_original_event_once() {
    for point in ["s1", "s2", "s3", "s4", "swap-event-published"] {
        let f = Fixture::new();
        let spare = inode(&f.slot("spare"));
        let o = f
            .command(&["profile", "rotate"])
            .env("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER", point)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(86), "required boundary {point}");
        let _ = f.rotate();
        assert_eq!(
            inode(&f.live()),
            spare,
            "no second swap during completion recovery"
        );
        let event = value(&f.state().join("swap.json"));
        assert_eq!(event["seq"], 1, "publication is idempotent at {point}");
        assert_eq!(event["from_login"], "work");
        assert_eq!(event["live_login"], "spare");
        assert_eq!(event["reading"]["session"], 5);
        assert_eq!(
            value(&f.state().join("ledger.json"))["last_cycle"]["outcome"],
            "swapped"
        );
        assert!(!f.state().join("journal.json").exists());
    }
}
