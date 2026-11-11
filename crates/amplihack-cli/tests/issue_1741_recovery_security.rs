//! Historical data is untrusted; current storage failures are execution errors.
#![cfg(unix)]

use crate::issue_1741_fixture::{Fixture, describe, files};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn durable_records_are_private_regular_files_outside_disposable_runtime() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let recovery = first.dir().join("recovery");
    let metadata = fs::symlink_metadata(&recovery).expect("durable recovery directory");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    let records = files(&recovery);
    assert!(
        !records.is_empty(),
        "success must be recorded before the usage stop"
    );
    for path in records {
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "{}",
            path.display()
        );
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "{}",
            path.display()
        );
        assert!(!path.starts_with(first.dir().join("runtime")));
    }
}

#[test]
fn truncated_historical_records_produce_a_logged_fresh_execution() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let records = files(&first.dir().join("recovery"));
    assert!(!records.is_empty());
    for path in records {
        fs::write(path, b"{\"schema_version\":").unwrap();
    }
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert!(String::from_utf8_lossy(&second.output.stderr).contains("state_unavailable"));
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}

#[test]
fn missing_completion_tree_never_counts_as_a_completed_prefix() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let recovery = first.dir().join("recovery");
    assert!(!files(&recovery).is_empty());
    fs::remove_dir_all(&recovery).unwrap();
    // An interrupted temporary publication is not authoritative state.
    fs::create_dir(&recovery).unwrap();
    fs::set_permissions(&recovery, fs::Permissions::from_mode(0o700)).unwrap();
    let partial = recovery.join("interrupted.tmp");
    fs::write(&partial, b"{\"completed\":true").unwrap();
    fs::set_permissions(partial, fs::Permissions::from_mode(0o600)).unwrap();
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}

#[test]
fn duplicate_record_fields_are_rejected_instead_of_using_last_value() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let mut changed = 0;
    for path in files(&first.dir().join("recovery")) {
        let bytes = fs::read(&path).unwrap();
        let Ok(serde_json::Value::Object(object)) = serde_json::from_slice(&bytes) else {
            continue;
        };
        let Some((key, value)) = object.iter().next() else {
            continue;
        };
        let original = serde_json::to_string(&object).unwrap();
        // Duplicate an actual emitted property: the JSON remains well formed.
        let duplicate = format!(
            "{{{}:{},{}",
            serde_json::to_string(key).unwrap(),
            value,
            &original[1..]
        );
        fs::write(path, duplicate).unwrap();
        changed += 1;
    }
    assert!(changed > 0, "must damage actual JSON recovery state");
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}

#[test]
fn symlinked_historical_tree_is_not_followed_or_modified() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let recovery = first.dir().join("recovery");
    let outside = f.root.join("unowned-history");
    fs::rename(&recovery, &outside).unwrap();
    let before: Vec<_> = files(&outside)
        .into_iter()
        .map(|p| {
            let bytes = fs::read(&p).unwrap();
            (p, bytes)
        })
        .collect();
    assert!(!before.is_empty());
    symlink(&outside, &recovery).unwrap();
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn insecure_historical_record_permissions_prevent_replay() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let records = files(&first.dir().join("recovery"));
    assert!(!records.is_empty());
    for path in records {
        fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
    }
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}

#[test]
fn unsafe_current_work_root_stops_before_any_step_executes() {
    let f = Fixture::new();
    fs::set_permissions(&f.work_root, fs::Permissions::from_mode(0o777)).unwrap();
    let result = f.run(&[]);
    assert!(
        !result.output.status.success(),
        "{}",
        describe(&result.output)
    );
    assert!(
        f.events().is_empty(),
        "unsafe admission cannot fall back to temporary execution"
    );
}

#[test]
fn unsupported_version_in_emitted_history_never_restores_outputs() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    let mut changed = 0;
    for path in files(&first.dir().join("recovery")) {
        let Ok(mut record) = serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap())
        else {
            continue;
        };
        // The exact private filenames remain runner-owned. Version-1 durable
        // envelopes must carry an explicit schema version.
        if record.get("schema_version").is_some() {
            record["schema_version"] = serde_json::json!(u32::MAX);
            fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
            changed += 1;
        }
    }
    assert!(changed > 0, "versioned recovery envelope required");
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_fresh("state_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}
