//! Issue #1845 (D1, AC7): each profile's recorded account, the account it is
//! meant to hold, at `<root>/.auto-switch.d/expected/<name>.json`.
//!
//! Contract:
//! - `expected::record(&State, &ProfileName, &Account) -> Result<(),
//!   ProfileError>` writes `{"account": Account::key()}` through
//!   `State::write_sub_json("expected", name, ..)`: atomic, mode 0600, never
//!   through a symlink. A later call overwrites the record.
//! - `expected::recorded(&State, &ProfileName) -> Option<String>` reads it
//!   through `State::read_sub_json`. A missing, unreadable, non-0600,
//!   symlinked, oversized or malformed file, or an `account` that is not a
//!   string accepted by `ledger::safe_account`, is `None`, silently.
//! - `expected::mismatched(&State, &ProfileName, &Account) -> Option<bool>`:
//!   `None` without a record, else whether `Account::matches_key(record)`
//!   fails.
//! - `expected.rs` holds no identity field literal and does no file I/O of
//!   its own.

use super::auto_switch::kit::State;
use super::expected::{mismatched, record, recorded, save};
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::store::ProfileStore;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    state: State,
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn account(uuid: Option<&str>, email: Option<&str>) -> Account {
    let mut block = serde_json::json!({});
    if let Some(u) = uuid {
        block["accountUuid"] = u.into();
    }
    if let Some(e) = email {
        block["emailAddress"] = e.into();
    }
    Account::from_block(&block).expect("a uuid or an e-mail makes an account")
}

fn work_account() -> Account {
    account(Some("u-SENTINEL-work"), Some("Work@Example.com"))
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("root");
    let store = ProfileStore::new(root.clone());
    store.add(&n("work")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let state = State::new(&root).unwrap();
    Env {
        _tmp: tmp,
        root,
        state,
    }
}

impl Env {
    fn file(&self, name: &str) -> PathBuf {
        self.root
            .join(".auto-switch.d")
            .join("expected")
            .join(format!("{name}.json"))
    }

    /// Records `work`, then replaces the file's bytes, keeping mode 0600.
    fn raw(&self, body: &[u8]) {
        record(&self.state, &n("work"), &work_account()).unwrap();
        fs::write(self.file("work"), body).unwrap();
        fs::set_permissions(self.file("work"), fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn raw_json(&self, v: serde_json::Value) {
        self.raw(v.to_string().as_bytes());
    }
}

// --- Writing ----------------------------------------------------------------

#[test]
fn a_record_holds_only_the_account_key_in_a_private_file() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    let path = e.file("work");
    let meta = fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_file(), "a regular file");
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"account": "u-SENTINEL-work"}));
}

#[test]
fn a_record_reads_back_as_the_key() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("u-SENTINEL-work")
    );
}

#[test]
fn an_account_without_a_uuid_is_recorded_by_its_lower_cased_email() {
    let e = env();
    record(
        &e.state,
        &n("work"),
        &account(None, Some("Mail@Example.COM")),
    )
    .unwrap();
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("mail@example.com")
    );
}

#[test]
fn a_second_record_overwrites_the_first() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    record(
        &e.state,
        &n("work"),
        &account(Some("u-SENTINEL-next"), None),
    )
    .unwrap();
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("u-SENTINEL-next")
    );
}

/// The swap's backfill saves with `keep`: a record written after the guard
/// read none (another rotator, or a `profile login`) is left in place.
#[test]
fn save_with_keep_leaves_an_existing_record() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    save(
        &e.root,
        &n("work"),
        &account(Some("u-SENTINEL-next"), None),
        true,
    );
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("u-SENTINEL-work")
    );
}

#[test]
fn save_with_keep_records_when_there_is_none_and_without_keep_replaces() {
    let e = env();
    save(&e.root, &n("work"), &work_account(), true);
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("u-SENTINEL-work")
    );
    save(
        &e.root,
        &n("work"),
        &account(Some("u-SENTINEL-next"), None),
        false,
    );
    assert_eq!(
        recorded(&e.state, &n("work")).as_deref(),
        Some("u-SENTINEL-next")
    );
}

#[test]
fn records_are_per_profile() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    assert_eq!(recorded(&e.state, &n("other")), None);
}

#[test]
fn a_record_is_never_written_through_a_symlink() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    let target = e.root.join("elsewhere.json");
    fs::write(&target, "untouched").unwrap();
    fs::remove_file(e.file("work")).unwrap();
    std::os::unix::fs::symlink(&target, e.file("work")).unwrap();
    // The write replaces the link atomically or fails; it never follows it.
    let _ = record(&e.state, &n("work"), &work_account());
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
}

// --- Reading: anything doubtful is no record ---------------------------------

#[test]
fn no_file_is_no_record_and_reading_creates_nothing() {
    let e = env();
    assert_eq!(recorded(&e.state, &n("work")), None);
    assert!(
        !e.root.join(".auto-switch.d").exists(),
        "a read creates no state directory"
    );
}

#[test]
fn malformed_json_is_no_record() {
    let e = env();
    e.raw(b"{\"account\": ");
    assert_eq!(recorded(&e.state, &n("work")), None);
}

#[test]
fn an_empty_file_is_no_record() {
    let e = env();
    e.raw(b"");
    assert_eq!(recorded(&e.state, &n("work")), None);
}

#[test]
fn a_record_others_can_read_is_no_record() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    fs::set_permissions(e.file("work"), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(recorded(&e.state, &n("work")), None);
}

#[test]
fn a_symlinked_record_is_no_record() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    let target = e.root.join("planted.json");
    fs::write(&target, r#"{"account":"u-SENTINEL-planted"}"#).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(e.file("work")).unwrap();
    std::os::unix::fs::symlink(&target, e.file("work")).unwrap();
    assert_eq!(recorded(&e.state, &n("work")), None);
}

#[test]
fn an_oversized_record_is_no_record() {
    let e = env();
    let pad = "x".repeat(70 * 1024);
    e.raw_json(serde_json::json!({"account": "u-SENTINEL-work", "pad": pad}));
    assert_eq!(recorded(&e.state, &n("work")), None);
}

#[test]
fn an_account_that_is_not_a_usable_string_is_no_record() {
    let long = "u".repeat(321);
    for v in [
        serde_json::json!({"account": 42}),
        serde_json::json!({"account": null}),
        serde_json::json!({"account": ["u-SENTINEL-work"]}),
        serde_json::json!({"account": {"accountUuid": "u-SENTINEL-work"}}),
        serde_json::json!({"account": ""}),
        serde_json::json!({"account": "u-SENTINEL\nwork"}),
        serde_json::json!({"account": "u-SENTINEL\u{1b}[2Jwork"}),
        serde_json::json!({"account": long}),
        serde_json::json!({}),
        serde_json::json!(["u-SENTINEL-work"]),
        serde_json::json!("u-SENTINEL-work"),
    ] {
        let e = env();
        e.raw_json(v.clone());
        assert_eq!(recorded(&e.state, &n("work")), None, "{v}");
    }
}

#[test]
fn an_unreadable_state_directory_is_no_record() {
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads through any mode
    }
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    let dir = e.file("work").parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let got = recorded(&e.state, &n("work"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(got, None);
}

// --- Comparing --------------------------------------------------------------

#[test]
fn without_a_record_nothing_is_mismatched() {
    let e = env();
    assert_eq!(mismatched(&e.state, &n("work"), &work_account()), None);
}

#[test]
fn the_recorded_account_matches() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    assert_eq!(
        mismatched(&e.state, &n("work"), &work_account()),
        Some(false)
    );
}

#[test]
fn another_account_is_mismatched() {
    let e = env();
    record(&e.state, &n("work"), &work_account()).unwrap();
    let other = account(Some("u-SENTINEL-other"), Some("other@example.com"));
    assert_eq!(mismatched(&e.state, &n("work"), &other), Some(true));
}

#[test]
fn an_email_record_matches_ignoring_case() {
    // `Account::matches_key` is the only comparison.
    let e = env();
    record(
        &e.state,
        &n("work"),
        &account(None, Some("work@example.com")),
    )
    .unwrap();
    let same = account(None, Some("WORK@example.com"));
    assert_eq!(mismatched(&e.state, &n("work"), &same), Some(false));
    let with_uuid = account(Some("u-SENTINEL-new"), Some("work@example.com"));
    assert_eq!(
        mismatched(&e.state, &n("work"), &with_uuid),
        Some(false),
        "an e-mail record still names an account that gained a uuid"
    );
}

#[test]
fn a_damaged_record_is_treated_as_none_when_comparing() {
    let e = env();
    e.raw(b"not json");
    let other = account(Some("u-SENTINEL-other"), None);
    assert_eq!(mismatched(&e.state, &n("work"), &other), None);
}

// --- Source guards ----------------------------------------------------------

/// The identity field names stay in `rotate/identify.rs` (see
/// `identity_keys_are_read_only_in_identify`), and the file is read and
/// written only through the state helpers.
#[test]
fn expected_rs_names_no_identity_field_and_does_no_io_of_its_own() {
    let src = include_str!("expected.rs");
    for bad in ["accountUuid", "emailAddress", "organizationUuid"] {
        assert!(!src.contains(bad), "expected.rs names {bad}");
    }
    for bad in [
        "std::fs",
        "fs::read",
        "fs::write",
        "File::",
        "OpenOptions",
        ".credentials.json",
        "CREDS",
    ] {
        assert!(!src.contains(bad), "expected.rs uses {bad}");
    }
    assert!(
        src.contains("read_sub_json"),
        "reads via State::read_sub_json"
    );
    assert!(
        src.contains("write_sub_json"),
        "writes via State::write_sub_json"
    );
    assert!(src.contains("safe_account"), "keys are checked");
    assert!(src.contains("matches_key"), "the one comparison");
    for bad in [".unwrap()", ".expect("] {
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains(bad), "expected.rs uses {bad}");
    }
}

/// Nothing is derived from a path: the record is keyed by a parsed
/// [`ProfileName`] only.
#[test]
fn expected_rs_takes_profile_names_not_strings() {
    let src = include_str!("expected.rs");
    for f in ["fn record(", "fn recorded(", "fn mismatched("] {
        let at = src.find(f).unwrap_or_else(|| panic!("{f} missing"));
        let sig: String = src[at..].chars().take_while(|c| *c != '{').collect();
        assert!(sig.contains("&ProfileName"), "{sig}");
        assert!(!sig.contains("&str"), "{sig}");
        assert!(!sig.contains("&Path"), "{sig}");
    }
}
