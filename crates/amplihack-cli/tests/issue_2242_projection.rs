//! CLI control: projection must not be necessary for an ordinary threshold swap.
//! Controlled sample histories and reset boundaries are portable unit tests.
#![cfg(target_os = "linux")]
use crate::support::profile_2242::{Fixture, inode, value};

#[test]
fn threshold_switch_with_no_history_preserves_original_measured_event_values() {
    let f = Fixture::new();
    let spare = inode(&f.slot("spare"));
    let o = f.rotate();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(inode(&f.live()), spare);
    let event = value(&f.state().join("swap.json"));
    assert_eq!(event["reading"]["session"], 5);
    assert_eq!(event["reading"]["week"], 20);
}
