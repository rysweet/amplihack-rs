//! Issue #1619 (R3): reading one tier file.
//!
//! The file chooses the model every tiered agent step in the run launches
//! on, and `[step_tiers]` can retarget a single named step, so it is read
//! under the same trust rules as the config file holding `model.force`. The
//! check and the read must address one object: a name checked and then
//! opened again is two lookups, and only the second one decides what is
//! parsed.
//!
//! A file that cannot be trusted or cannot be parsed costs the run its
//! override and nothing else. The shipped mapping applies, one line says so,
//! and the launch proceeds: an unreadable preference file must not stop a
//! run of a hundred agent steps.
//!
//! Paths here are tempfile paths only; nothing reads the real home.

use super::notice::Notice;
use super::read_tier_file;
use super::table::{Table, Tier};
use std::fs;

const OVERRIDE: &str = "version = 1\n[claude.fast]\nmodel = \"claude-haiku-5-5\"\n";

fn load_failed(notices: &[Notice]) -> Option<&str> {
    notices.iter().find_map(|n| match n {
        Notice::TableLoadFailed { reason, .. } => Some(reason.as_str()),
        _ => None,
    })
}

#[test]
fn a_private_tier_file_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-tiers.toml");
    fs::write(&path, OVERRIDE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let (table, notices) = read_tier_file(&path);
    assert_eq!(load_failed(&notices), None, "{notices:?}");
    assert_eq!(
        table
            .merged_over(Table::shipped())
            .entry("claude", Tier::Fast)
            .and_then(|e| e.model.as_deref()),
        Some("claude-haiku-5-5")
    );
}

#[cfg(unix)]
#[test]
fn a_group_writable_tier_file_is_refused_and_the_shipped_mapping_applies() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-tiers.toml");
    fs::write(
        &path,
        "version = 1\n[claude.fast]\nmodel = \"planted-model\"\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();

    let (table, notices) = read_tier_file(&path);
    let reason = load_failed(&notices).expect("the refusal is said once");
    assert!(reason.contains("writable by group or others"), "{reason}");
    assert_eq!(
        table
            .merged_over(Table::shipped())
            .entry("claude", Tier::Fast)
            .and_then(|e| e.model.as_deref()),
        Some("claude-haiku-5-5"),
        "nothing from a file amplihack would not trust reaches the mapping"
    );
}

#[cfg(unix)]
#[test]
fn a_tier_file_linked_in_from_elsewhere_is_judged_by_what_it_points_at() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();

    let private = dir.path().join("dotfiles.toml");
    fs::write(&private, OVERRIDE).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
    let good = dir.path().join("good-model-tiers.toml");
    std::os::unix::fs::symlink(&private, &good).unwrap();
    let (_good_table, good_notices) = read_tier_file(&good);
    assert_eq!(
        load_failed(&good_notices),
        None,
        "a link to a private file is an ordinary arrangement and is read"
    );

    let shared = dir.path().join("shared.toml");
    fs::write(&shared, OVERRIDE).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o666)).unwrap();
    let bad = dir.path().join("bad-model-tiers.toml");
    std::os::unix::fs::symlink(&shared, &bad).unwrap();
    let (_bad_table, bad_notices) = read_tier_file(&bad);
    let reason = load_failed(&bad_notices).expect("the target decides");
    assert!(reason.contains("writable by group or others"), "{reason}");
}

#[test]
fn a_tier_file_that_is_not_a_regular_file_is_refused_rather_than_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let (_table, notices) = read_tier_file(dir.path());
    assert!(
        load_failed(&notices).is_some(),
        "a directory where a file was expected is said once: {notices:?}"
    );
}

#[test]
fn the_reason_a_tier_file_was_refused_quotes_none_of_its_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-tiers.toml");
    fs::write(
        &path,
        "version = 1\n[claude.fast\nmodel = \"token-shaped-secret\"\n",
    )
    .unwrap();
    let (_table, notices) = read_tier_file(&path);
    let reason = load_failed(&notices).expect("malformed TOML is said once");
    assert!(
        !reason.contains("token-shaped-secret"),
        "these lines get pasted into issues: {reason}"
    );
}
