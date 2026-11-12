//! The real Linux atomic exchange; portable cycle contracts live beside rotate.
#![cfg(target_os = "linux")]
use crate::support::profile_2242::{Fixture, Reply, inode, private, value};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn refresh_locked_best_candidate_falls_back_and_publishes_the_actual_target() {
    let f = Fixture::new();
    let extra = inode(&f.slot("extra"));
    fs::create_dir(f.root.join("spare/.oauth_refresh.lock")).unwrap();
    let o = f.rotate();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(inode(&f.live()), extra);
    assert_eq!(value(&f.state().join("swap.json"))["live_login"], "extra");
    assert!(
        f.log()
            .lines()
            .any(|l| l.contains("spare") && l.contains("refus"))
    );
}

#[test]
fn different_account_conflict_uses_the_later_candidate_and_retains_every_original_inode() {
    let f = Fixture::new();
    private(&f.slot("work"), &fs::read(f.slot("extra")).unwrap());
    let (live, conflict, spare, extra) = (
        inode(&f.live()),
        inode(&f.slot("work")),
        inode(&f.slot("spare")),
        inode(&f.slot("extra")),
    );
    let expected = f.root.join(".auto-switch.d/expected/work.json");
    let owner = fs::read(&expected).unwrap();
    let o = f.rotate();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        inode(&f.live()),
        extra,
        "a later candidate must actually become live"
    );
    assert_eq!(inode(&f.slot("work")), conflict);
    assert_eq!(inode(&f.slot("spare")), spare);
    assert_eq!(fs::read(expected).unwrap(), owner);
    // Ask the CLI to discover the outgoing account rather than assuming a
    // recovery directory name or treating hidden, unreachable storage as success.
    let usage = f.run(&["profile", "usage", "--json"]);
    let rows: serde_json::Value = serde_json::from_slice(&usage.stdout).unwrap();
    assert!(
        rows.as_array().unwrap().iter().any(|row| {
            row["account"].as_str() == Some("work@example.com")
                && row["profile"]
                    .as_str()
                    .is_some_and(|name| !["work", "spare", "extra"].contains(&name))
        }),
        "outgoing account is discoverable under its recovery profile through profile usage"
    );
    fn find(root: &std::path::Path, wanted: u64, found: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(root).unwrap() {
            let p = entry.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                find(&p, wanted, found);
            } else if m.is_file() && m.ino() == wanted {
                found.push(p);
            }
        }
    }
    let mut custody = Vec::new();
    find(&f.root, live, &mut custody);
    assert_eq!(
        custody.len(),
        1,
        "outgoing credential is retained once beneath the synthetic root"
    );
    assert_eq!(
        fs::metadata(custody[0].parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::metadata(&custody[0]).unwrap().nlink(), 1);
    let record = value(&f.state().join("swap.json"));
    assert_eq!(record["from_login"], "work");
    assert_eq!(record["live_login"], "extra");
    assert_eq!(record["seq"], 1);
    assert_eq!(record["reading"]["session"], 25);
    assert!(!f.state().join("journal.json").exists());
    assert_eq!(
        f.log()
            .lines()
            .filter(|l| l.contains("ALERT")
                && l.contains("work@example.com")
                && l.contains("extra@example.com"))
            .count(),
        1
    );
}

#[test]
fn unverified_occupied_slot_never_authorises_parking_or_alternate_custody() {
    let f = Fixture::new();
    private(&f.slot("work"), &fs::read(f.slot("extra")).unwrap());
    let before = [
        inode(&f.live()),
        inode(&f.slot("work")),
        inode(&f.slot("spare")),
        inode(&f.slot("extra")),
    ];
    f.reply("extra", Reply::Unavailable);
    let _ = f.rotate();
    assert_eq!(
        [
            inode(&f.live()),
            inode(&f.slot("work")),
            inode(&f.slot("spare")),
            inode(&f.slot("extra"))
        ],
        before
    );
    assert!(!f.state().join("swap.json").exists());
    assert!(
        f.requests("extra")
            .iter()
            .any(|p| p == "/api/oauth/profile"),
        "the fake server must be consulted before classifying a duplicate"
    );
    assert!(!f.log().contains("synthetic-2242"));
}
