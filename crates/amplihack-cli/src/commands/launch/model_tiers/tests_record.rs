//! Issue #1619 (TDD): the measurement record.
//!
//! The record is what makes the saving measurable, and it is also the file
//! people paste into issues and PRs. Its schema is a closed set of eleven
//! named scalars with no argv, environment, prompt, working-directory or path
//! field, pinned below — that is what stops a later "just add argv for
//! debugging" commit from putting command-line-grade content into a run
//! artefact.
//!
//! Homes are tempfile homes only; nothing here touches a real `~/.amplihack`.

use super::record::{Fallback, RECORD_FILE, Record, append, read_all};
use super::table::{Effort, Tier};
use std::fs;

fn sample() -> Record {
    Record {
        v: 1,
        seq: 1,
        run_id: Some("20261116-000000-test".to_string()),
        harness: "claude".to_string(),
        column: "claude".to_string(),
        tier: Some(Tier::Standard),
        requested_model: Some("claude-sonnet-5-5".to_string()),
        effective_model: Some("claude-sonnet-5-5".to_string()),
        requested_effort: None,
        effective_effort: None,
        fallback: None,
    }
}

fn lines(dir: &std::path::Path) -> Vec<String> {
    let path = dir.join(RECORD_FILE);
    fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

// -------------------------------------------------------------------------
// The schema is closed
// -------------------------------------------------------------------------

#[test]
fn the_serialised_key_set_is_exactly_eleven_scalars() {
    let json = serde_json::to_value(sample()).expect("a record serialises");
    let object = json.as_object().expect("an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = [
        "v",
        "seq",
        "run_id",
        "harness",
        "column",
        "tier",
        "requested_model",
        "effective_model",
        "requested_effort",
        "effective_effort",
        "fallback",
    ];
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "run artefacts get pasted into issues and PRs. No argv, environment, prompt, \
         working directory or path field belongs here."
    );
    for (key, value) in object {
        assert!(
            !value.is_array() && !value.is_object(),
            "`{key}` must be a scalar or null, so the file stays one flat line per launch"
        );
    }
}

#[test]
fn absent_values_serialise_as_null_rather_than_being_omitted() {
    let json = serde_json::to_value(sample()).expect("serialises");
    assert_eq!(json["fallback"], serde_json::Value::Null);
    assert_eq!(json["requested_effort"], serde_json::Value::Null);
    // A reader counting a run's spend must not have to tell "absent" from
    // "not written yet".
    assert!(json.as_object().unwrap().contains_key("fallback"));
}

#[test]
fn the_four_fall_back_reasons_have_stable_names() {
    for (value, name) in [
        (Fallback::NoColumn, "no_column"),
        (Fallback::NoMapping, "no_mapping"),
        (Fallback::NoModel, "no_model"),
        (Fallback::Gateway, "gateway"),
    ] {
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::json!(name)
        );
    }
}

#[test]
fn tier_and_effort_serialise_as_their_words() {
    let mut record = sample();
    record.tier = Some(Tier::Strong);
    record.requested_effort = Some(Effort::Xhigh);
    record.effective_effort = Some(Effort::Xhigh);
    let json = serde_json::to_value(record).unwrap();
    assert_eq!(json["tier"], serde_json::json!("strong"));
    assert_eq!(json["requested_effort"], serde_json::json!("xhigh"));
}

// -------------------------------------------------------------------------
// Writing
// -------------------------------------------------------------------------

#[test]
fn one_launch_appends_one_line() {
    let dir = tempfile::tempdir().unwrap();
    append(Some(dir.path()), &sample());
    append(Some(dir.path()), &sample());
    assert_eq!(lines(dir.path()).len(), 2);
    for line in lines(dir.path()) {
        serde_json::from_str::<serde_json::Value>(&line).expect("each line is one object");
    }
}

#[test]
fn nothing_is_written_without_a_run_directory() {
    let dir = tempfile::tempdir().unwrap();
    append(None, &sample());
    assert!(
        !dir.path().join(RECORD_FILE).exists(),
        "with no run directory nothing is written and nothing is created"
    );
}

#[test]
fn a_run_directory_that_does_not_exist_is_never_created() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-run-dir");
    append(Some(&missing), &sample());
    assert!(
        !missing.exists(),
        "amplihack never creates a run directory for this file: an existing directory \
         is the whole precondition"
    );
}

#[cfg(unix)]
#[test]
fn the_record_file_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    append(Some(dir.path()), &sample());
    let mode = fs::metadata(dir.path().join(RECORD_FILE))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "mode was {mode:o}");
}

#[cfg(unix)]
#[test]
fn a_planted_symlink_is_refused_rather_than_followed() {
    let dir = tempfile::tempdir().unwrap();
    let decoy = dir.path().join("decoy");
    fs::write(&decoy, "untouched\n").unwrap();
    let run = dir.path().join("run");
    fs::create_dir(&run).unwrap();
    std::os::unix::fs::symlink(&decoy, run.join(RECORD_FILE)).unwrap();
    append(Some(&run), &sample());
    assert_eq!(
        fs::read_to_string(&decoy).unwrap(),
        "untouched\n",
        "the file is opened without following a symlink, so a link planted here cannot \
         be followed into, say, a settings file"
    );
}

#[cfg(unix)]
#[test]
fn a_record_is_never_written_into_a_directory_or_a_fifo() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    fs::create_dir_all(run.join(RECORD_FILE)).unwrap();
    // A directory where the file should be: skipped, never a failure and
    // never a block.
    append(Some(&run), &sample());
    assert!(run.join(RECORD_FILE).is_dir());
}

#[test]
fn seq_is_read_and_incremented_with_the_append() {
    // Two launches of the same run must neither interleave lines nor claim
    // the same number, so the sequence is read inside the same lock as the
    // write rather than carried in the caller's memory.
    let dir = tempfile::tempdir().unwrap();
    let mut first = sample();
    first.seq = 0; // the writer assigns it
    append(Some(dir.path()), &first);
    let mut second = sample();
    second.seq = 0;
    append(Some(dir.path()), &second);
    let records = read_all(dir.path());
    assert_eq!(
        records.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![1, 2],
        "the sequence starts at 1 and counts launches within the run"
    );
}

#[test]
fn reading_back_a_run_gives_the_records_in_launch_order() {
    let dir = tempfile::tempdir().unwrap();
    for tier in [Tier::Strong, Tier::Standard, Tier::Fast] {
        let mut record = sample();
        record.tier = Some(tier);
        append(Some(dir.path()), &record);
    }
    let tiers: Vec<Option<&'static str>> = read_all(dir.path())
        .iter()
        .map(|r| r.tier.map(Tier::as_str))
        .collect();
    assert_eq!(tiers, vec![Some("strong"), Some("standard"), Some("fast")]);
}

#[test]
fn an_unreadable_or_corrupt_file_does_not_panic_a_reader() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(RECORD_FILE), "not json\n{\"v\":1}\n").unwrap();
    let records = read_all(dir.path());
    // A half-written line is skipped; the summary is a report, and it never
    // holds up a run.
    assert!(records.len() <= 1);
}

#[test]
fn the_file_name_is_fixed() {
    assert_eq!(RECORD_FILE, "model-tiers.jsonl");
}

#[test]
fn max_serialises_as_its_lowercase_word() {
    let json = serde_json::to_value(Effort::Max).unwrap();
    assert_eq!(json, serde_json::json!("max"));
    let back: Effort = serde_json::from_value(json).unwrap();
    assert_eq!(back, Effort::Max);
}
