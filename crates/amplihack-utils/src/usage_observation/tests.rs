//! Public contract for #2029, exercised against the production store.
//!
//! ObservationStore::open(root) uses the existing profile-state lock.
//! lock() returns a guard with authorised_login, binding, retire, capture,
//! publish and read operations. Only authorised lifecycle callers establish
//! bindings. capture binds an identifier and capture time to one monotonic
//! ticket. Reusing a Capture never allocates another ticket. record_path is
//! the non-secret managed-record location, exposed for diagnostics too.

#![cfg(unix)]

use amplihack_utils::usage_observation::{
    Binding, Capture, Evidence, MAX_RECORD_BYTES, ObservationStore, PublishOutcome, ReadOutcome,
    read_bounded_record,
};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

const NOW: i64 = 1_790_000_000;

#[path = "tests_complete.rs"]
mod complete;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: ObservationStore,
    binding: Binding,
}

impl Fixture {
    fn new() -> Self {
        // Explicit scratch location: TMPDIR may itself be a recipe runtime
        // beneath the real ~/.amplihack. Nothing consults HOME or credentials.
        let tmp = tempfile::tempdir_in(fs::canonicalize("/tmp").unwrap()).unwrap();
        let root = tmp.path().join("profiles");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("work")).unwrap();
        fs::set_permissions(root.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
        let store = ObservationStore::open(&root).unwrap();
        let binding = store
            .lock()
            .unwrap()
            .authorised_login("work", "account-work")
            .unwrap();
        Self {
            _tmp: tmp,
            root,
            store,
            binding,
        }
    }

    fn capture(&self, id: &str) -> Capture {
        self.store
            .lock()
            .unwrap()
            .capture(&self.binding, id, NOW)
            .unwrap()
    }

    fn publish(&self, capture: &Capture, evidence: &Evidence) -> PublishOutcome {
        self.store
            .lock()
            .unwrap()
            .publish(capture, evidence)
            .unwrap()
    }

    fn read(&self) -> ReadOutcome {
        self.store.lock().unwrap().read(&self.binding).unwrap()
    }

    fn path(&self) -> PathBuf {
        self.store.record_path(&self.binding).unwrap()
    }
}

fn usage(session: f64, week: f64) -> Evidence {
    Evidence {
        session_window: amplihack_utils::usage_observation::SessionWindow::Present,
        session_used: Some(session),
        week_used: Some(week),
        ..Evidence::default()
    }
}

#[test]
fn an_observation_round_trips_its_captured_identity_time_and_complete_evidence() {
    let f = Fixture::new();
    assert!(matches!(f.read(), ReadOutcome::Missing));
    let capture = f.capture("status-1");
    let evidence = usage(20.0, 90.0);
    assert!(matches!(
        f.publish(&capture, &evidence),
        PublishOutcome::Accepted { .. }
    ));
    let ReadOutcome::Usable(record) = f.read() else {
        panic!("missing accepted observation")
    };
    assert_eq!(record.capture, capture);
    assert_eq!(record.capture.captured_at, NOW);
    assert_eq!(record.evidence, evidence);
}

#[test]
fn recognised_limit_evidence_does_not_invent_percentages() {
    let f = Fixture::new();
    let evidence = Evidence {
        limit_reached: true,
        ..Evidence::default()
    };
    assert!(matches!(
        f.publish(&f.capture("limit-1"), &evidence),
        PublishOutcome::Accepted { .. }
    ));
    let ReadOutcome::Usable(record) = f.read() else {
        panic!("limit evidence must be usable")
    };
    assert!(record.evidence.limit_reached);
    assert_eq!(
        (record.evidence.session_used, record.evidence.week_used),
        (None, None)
    );
}

#[test]
fn duplicate_publication_does_not_replace_the_file_or_advance_revision() {
    let f = Fixture::new();
    let capture = f.capture("status-1");
    let evidence = usage(85.0, 40.0);
    f.publish(&capture, &evidence);
    let before = fs::read(f.path()).unwrap();
    let inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(f.path()).unwrap());
    assert_eq!(f.publish(&capture, &evidence), PublishOutcome::Duplicate);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(f.path()).unwrap()),
        inode,
        "duplicates must not generate atomic-replacement events"
    );
}

#[test]
fn older_capture_cannot_overwrite_newer_evidence_even_after_reopening() {
    let f = Fixture::new();
    let older = f.capture("older");
    let newer = f.capture("newer");
    assert!(older.ticket < newer.ticket);
    f.publish(&newer, &usage(95.0, 70.0));
    let store = ObservationStore::open(&f.root).unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .publish(&older, &usage(10.0, 20.0))
            .unwrap(),
        PublishOutcome::Superseded
    );
    let ReadOutcome::Usable(record) = f.read() else {
        panic!("newer evidence was lost")
    };
    assert_eq!(record.capture, newer);
}

#[test]
fn one_identifier_cannot_be_relabelled_with_a_new_ticket_or_changed_payload() {
    let f = Fixture::new();
    let capture = f.capture("same-event");
    f.publish(&capture, &usage(40.0, 50.0));
    assert!(
        f.store
            .lock()
            .unwrap()
            .capture(&f.binding, "same-event", NOW + 1)
            .is_err()
    );
    assert_eq!(
        f.publish(&capture, &usage(99.0, 99.0)),
        PublishOutcome::InvalidEvidence
    );
}

#[test]
fn relogin_rejects_stale_reads_and_late_completions_for_the_same_profile() {
    let f = Fixture::new();
    let old = f.capture("before-login");
    f.publish(&old, &usage(20.0, 30.0));
    let next = f
        .store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    assert_ne!(next.generation, f.binding.generation);
    assert_eq!(
        f.publish(&old, &usage(90.0, 90.0)),
        PublishOutcome::RetiredGeneration
    );
    assert!(matches!(
        f.store.lock().unwrap().read(&next).unwrap(),
        ReadOutcome::StaleGeneration | ReadOutcome::Missing
    ));
}

#[test]
fn removing_and_recreating_a_login_cannot_restore_its_old_binding() {
    let f = Fixture::new();
    let old = f.capture("old");
    f.store.lock().unwrap().retire(&f.binding).unwrap();
    let store = ObservationStore::open(&f.root).unwrap();
    let next = store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    assert_ne!(next.generation, old.binding.generation);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .publish(&old, &usage(1.0, 1.0))
            .unwrap(),
        PublishOutcome::RetiredGeneration
    );
}

#[test]
fn forged_identity_cannot_publish_or_create_a_missing_binding() {
    let f = Fixture::new();
    let capture = f.capture("forged");
    let mut forged = capture.clone();
    forged.binding.account_id = "account-someone-else".into();
    assert_eq!(
        f.publish(&forged, &usage(90.0, 90.0)),
        PublishOutcome::InvalidAttribution
    );
    forged.binding.profile = "unregistered".into();
    assert_eq!(
        f.publish(&forged, &usage(90.0, 90.0)),
        PublishOutcome::InvalidAttribution
    );
    assert!(
        f.store
            .lock()
            .unwrap()
            .binding("unregistered")
            .unwrap()
            .is_none()
    );
}

#[test]
fn invalid_numbers_empty_evidence_and_out_of_range_times_are_refused() {
    let f = Fixture::new();
    for (index, evidence) in [
        Evidence::default(),
        usage(-0.1, 20.0),
        usage(100.1, 20.0),
        usage(f64::NAN, 20.0),
        usage(f64::INFINITY, 20.0),
        Evidence {
            session_reset_at: Some(i64::MAX),
            ..usage(10.0, 20.0)
        },
    ]
    .into_iter()
    .enumerate()
    {
        let capture = f.capture(&format!("invalid-{index}"));
        assert_eq!(
            f.publish(&capture, &evidence),
            PublishOutcome::InvalidEvidence
        );
    }
    assert!(matches!(f.read(), ReadOutcome::Missing));
    assert!(
        f.store
            .lock()
            .unwrap()
            .capture(&f.binding, "negative-time", -1)
            .is_err()
    );
    assert!(
        f.store
            .lock()
            .unwrap()
            .capture(&f.binding, &"x".repeat(MAX_RECORD_BYTES + 1), NOW)
            .is_err()
    );
}

#[test]
fn valid_percentage_boundaries_are_inclusive() {
    let f = Fixture::new();
    for (i, evidence) in [usage(0.0, 100.0), usage(100.0, 0.0)]
        .into_iter()
        .enumerate()
    {
        assert!(matches!(
            f.publish(&f.capture(&format!("boundary-{i}")), &evidence),
            PublishOutcome::Accepted { .. }
        ));
    }
}

#[test]
fn bounded_reads_distinguish_unsupported_schema_and_invalid_records() {
    let f = Fixture::new();
    f.publish(&f.capture("initial"), &usage(10.0, 20.0));
    let valid = fs::read(f.path()).unwrap();
    let mut future: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    let version = future["schema_version"].as_u64().unwrap();
    future["schema_version"] = serde_json::json!(version.checked_add(1).unwrap());
    fs::write(f.path(), serde_json::to_vec(&future).unwrap()).unwrap();
    assert!(matches!(f.read(), ReadOutcome::UnsupportedSchema));
    for bytes in [b"{truncated".to_vec(), vec![b' '; MAX_RECORD_BYTES + 1]] {
        fs::write(f.path(), bytes).unwrap();
        assert!(matches!(f.read(), ReadOutcome::Invalid));
    }
}

#[test]
fn publication_and_reads_refuse_symlink_records_without_changing_the_target() {
    let f = Fixture::new();
    f.publish(&f.capture("initial"), &usage(10.0, 20.0));
    let outside = f._tmp.path().join("outside");
    fs::write(&outside, b"unchanged").unwrap();
    fs::remove_file(f.path()).unwrap();
    symlink(&outside, f.path()).unwrap();
    assert!(matches!(f.read(), ReadOutcome::Unsafe));
    assert_eq!(
        f.publish(&f.capture("symlink"), &usage(30.0, 40.0)),
        PublishOutcome::Unsafe
    );
    assert_eq!(fs::read(outside).unwrap(), b"unchanged");
}

#[test]
fn non_regular_or_public_records_are_unsafe_not_missing() {
    let f = Fixture::new();
    f.publish(&f.capture("initial"), &usage(10.0, 20.0));
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(f.read(), ReadOutcome::Unsafe));
    fs::remove_file(f.path()).unwrap();
    fs::create_dir(f.path()).unwrap();
    assert!(matches!(f.read(), ReadOutcome::Unsafe));
}

#[test]
fn profile_traversal_and_unsafe_parent_directories_are_refused() {
    let f = Fixture::new();
    for name in [
        "",
        "../escape",
        "/absolute",
        "work/child",
        "..",
        "bad\0name",
    ] {
        assert!(
            f.store
                .lock()
                .unwrap()
                .authorised_login(name, "account")
                .is_err(),
            "accepted {name:?}"
        );
    }
    let alias = f._tmp.path().join("alias");
    symlink(&f.root, &alias).unwrap();
    assert!(ObservationStore::open(&alias).is_err());
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(ObservationStore::open(&f.root).is_err());
}

#[test]
fn atomic_publication_keeps_open_readers_on_a_complete_private_old_record() {
    let f = Fixture::new();
    f.publish(&f.capture("old"), &usage(10.0, 20.0));
    let mut old = fs::File::open(f.path()).unwrap();
    f.publish(&f.capture("new"), &usage(95.0, 80.0));
    let old_json: serde_json::Value = serde_json::from_reader(&mut old).unwrap();
    let new_json: serde_json::Value =
        serde_json::from_reader(fs::File::open(f.path()).unwrap()).unwrap();
    assert_ne!(old_json, new_json);
    assert_eq!(
        fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(f.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn concurrent_older_writer_cannot_replace_the_accepted_high_water_mark() {
    let f = Fixture::new();
    let old = f.capture("old");
    let new = f.capture("new");
    let barrier = Arc::new(Barrier::new(2));
    let root = f.root.clone();
    let release = barrier.clone();
    let writer = std::thread::spawn(move || {
        let store = ObservationStore::open(&root).unwrap();
        release.wait();
        store
            .lock()
            .unwrap()
            .publish(&old, &usage(10.0, 20.0))
            .unwrap()
    });
    f.publish(&new, &usage(95.0, 80.0));
    barrier.wait();
    assert_eq!(writer.join().unwrap(), PublishOutcome::Superseded);
    let ReadOutcome::Usable(record) = f.read() else {
        panic!("lost accepted record")
    };
    assert_eq!(record.capture, new);
}

#[test]
fn an_abandoned_temporary_record_cannot_lower_ordering_after_recovery() {
    let f = Fixture::new();
    let old = f.capture("old");
    f.publish(&f.capture("new"), &usage(95.0, 80.0));
    let mut partial = tempfile::NamedTempFile::new_in(f.path().parent().unwrap()).unwrap();
    partial.write_all(b"{\"schema_version\":1,").unwrap();
    let store = ObservationStore::open(&f.root).unwrap();
    assert_eq!(
        store
            .lock()
            .unwrap()
            .publish(&old, &usage(1.0, 1.0))
            .unwrap(),
        PublishOutcome::Superseded
    );
    assert!(matches!(
        store.lock().unwrap().read(&f.binding).unwrap(),
        ReadOutcome::Usable(_)
    ));
}

#[test]
fn bounded_decoder_never_reads_past_the_size_limit_and_one_sentinel_byte() {
    struct GuardedRead {
        consumed: usize,
    }
    impl std::io::Read for GuardedRead {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            assert!(
                self.consumed < MAX_RECORD_BYTES + 1,
                "unbounded read attempted after the sentinel byte"
            );
            let n = out.len().min(MAX_RECORD_BYTES + 1 - self.consumed);
            out[..n].fill(b' ');
            self.consumed += n;
            Ok(n)
        }
    }
    let f = Fixture::new();
    let mut input = GuardedRead { consumed: 0 };
    assert!(matches!(
        read_bounded_record(&mut input, &f.binding).unwrap(),
        ReadOutcome::Invalid
    ));
    assert!(input.consumed <= MAX_RECORD_BYTES + 1);
}

#[test]
fn high_water_mark_survives_record_removal_and_cannot_be_rebuilt_from_old_evidence() {
    let f = Fixture::new();
    let old = f.capture("old");
    f.publish(&old, &usage(10.0, 20.0));
    let old_bytes = fs::read(f.path()).unwrap();
    f.publish(&f.capture("new"), &usage(95.0, 80.0));
    fs::remove_file(f.path()).unwrap();
    let reopened = ObservationStore::open(&f.root).unwrap();
    assert_eq!(
        reopened
            .lock()
            .unwrap()
            .publish(&old, &usage(10.0, 20.0))
            .unwrap(),
        PublishOutcome::Superseded
    );
    // Inject the old-record/new-ordering boundary that recovery must never
    // use to lower its persistent accepted ticket.
    fs::write(f.path(), old_bytes).unwrap();
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o600)).unwrap();
    let recovered = ObservationStore::open(&f.root).unwrap();
    assert!(
        !matches!(
            recovered.lock().unwrap().read(&f.binding).unwrap(),
            ReadOutcome::Usable(_)
        ),
        "recovery exposed superseded evidence"
    );
    assert_eq!(
        recovered
            .lock()
            .unwrap()
            .publish(&old, &usage(10.0, 20.0))
            .unwrap(),
        PublishOutcome::Superseded
    );
}

#[test]
fn generated_capture_compaction_keeps_storage_bounded_without_reusing_tickets() {
    let f = Fixture::new();
    let mut previous = 0;
    for n in 0..64 {
        let capture = f
            .store
            .lock()
            .unwrap()
            .capture_new(&f.binding, NOW + n)
            .unwrap();
        assert!(capture.ticket > previous);
        previous = capture.ticket;
        f.publish(&capture, &usage(n as f64, 20.0));
        assert!(
            f.store
                .lock()
                .unwrap()
                .capture(&f.binding, &capture.observation_id, NOW + n)
                .is_err()
        );
    }
    let metadata = fs::read(f.root.join(".auto-switch.d/observation-order.json")).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(
        value["logins"]["work"]["captures"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn external_credential_replacement_retires_evidence_without_reading_credentials() {
    let f = Fixture::new();
    let path = f.root.join("work/.credentials.json");
    fs::write(&path, b"opaque credential fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let binding = f
        .store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    let capture = f.store.lock().unwrap().capture_new(&binding, NOW).unwrap();
    f.publish(&capture, &usage(20.0, 30.0));
    amplihack_utils::secure_files::write_private_atomic(
        path.parent().unwrap(),
        ".credentials.json",
        b"unexplained replacement",
    )
    .unwrap();
    assert!(
        f.store
            .lock()
            .unwrap()
            .reconcile_bindings()
            .unwrap()
            .is_empty()
    );
    assert!(f.store.lock().unwrap().retire_if_changed(&binding).unwrap());
    assert!(f.store.lock().unwrap().binding("work").unwrap().is_none());
    assert!(
        f.store.lock().unwrap().registered("work").unwrap(),
        "restart must distinguish retirement from pre-upgrade state"
    );
    assert_eq!(
        f.publish(&capture, &usage(20.0, 30.0)),
        PublishOutcome::RetiredGeneration
    );
}

#[test]
fn authorised_refresh_receipt_preserves_the_generation_and_existing_evidence() {
    let f = Fixture::new();
    let path = f.root.join("work/.credentials.json");
    fs::write(&path, b"opaque old credential fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let binding = f
        .store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    let capture = f.store.lock().unwrap().capture_new(&binding, NOW).unwrap();
    f.publish(&capture, &usage(20.0, 30.0));
    let receipt = amplihack_utils::usage_observation::capture_refresh(&path).unwrap();
    amplihack_utils::secure_files::write_private_atomic(
        path.parent().unwrap(),
        ".credentials.json",
        b"opaque refreshed credential fixture",
    )
    .unwrap();
    amplihack_utils::usage_observation::publish_refresh(receipt).unwrap();
    assert!(!f.store.lock().unwrap().retire_if_changed(&binding).unwrap());
    assert_eq!(
        f.store.lock().unwrap().binding("work").unwrap(),
        Some(binding.clone())
    );
    assert!(matches!(
        f.store.lock().unwrap().read(&binding).unwrap(),
        ReadOutcome::Usable(_)
    ));
}

#[test]
fn a_refresh_in_progress_defers_until_its_lock_removal_event() {
    let f = Fixture::new();
    let path = f.root.join("work/.credentials.json");
    fs::write(&path, b"opaque fixture").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let binding = f
        .store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    let receipt = amplihack_utils::usage_observation::capture_refresh(&path).unwrap();
    fs::create_dir(f.root.join("work/.oauth_refresh.lock")).unwrap();
    amplihack_utils::secure_files::write_private_atomic(
        path.parent().unwrap(),
        ".credentials.json",
        b"new fixture",
    )
    .unwrap();
    assert!(f.store.lock().unwrap().retire_if_changed(&binding).unwrap());
    assert_eq!(
        f.store.lock().unwrap().binding("work").unwrap(),
        Some(binding.clone())
    );
    amplihack_utils::usage_observation::publish_refresh(receipt).unwrap();
    fs::remove_dir(f.root.join("work/.oauth_refresh.lock")).unwrap();
    assert!(!f.store.lock().unwrap().retire_if_changed(&binding).unwrap());
}
