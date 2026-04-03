//! Tests for TestLevel enum.

use amplihack_agent_eval::levels::TestLevel;

// ── All levels exist ─────────────────────────────────────────

#[test]
fn all_twelve_levels_exist() {
    assert_eq!(TestLevel::all().len(), 12);
}

#[test]
fn all_levels_have_unique_ids() {
    let ids: Vec<u8> = TestLevel::all().iter().map(|l| l.id()).collect();
    let mut deduped = ids.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(ids.len(), deduped.len());
}

#[test]
fn level_ids_are_1_through_12() {
    let ids: Vec<u8> = TestLevel::all().iter().map(|l| l.id()).collect();
    assert_eq!(ids, (1..=12).collect::<Vec<u8>>());
}

// ── Display names ────────────────────────────────────────────

#[test]
fn l1_display_name() {
    assert_eq!(TestLevel::L1Recall.display_name(), "Recall");
}

#[test]
fn l3_display_name() {
    assert_eq!(
        TestLevel::L3TemporalReasoning.display_name(),
        "Temporal Reasoning"
    );
}

#[test]
fn l5_display_name() {
    assert_eq!(
        TestLevel::L5ContradictionHandling.display_name(),
        "Contradiction Handling"
    );
}

#[test]
fn l12_display_name() {
    assert_eq!(TestLevel::L12FarTransfer.display_name(), "Far Transfer");
}

#[test]
fn display_format_includes_id_and_name() {
    let s = format!("{}", TestLevel::L7TeacherStudent);
    assert_eq!(s, "L7 Teacher-Student");
}

// ── Difficulty ordering ──────────────────────────────────────

#[test]
fn l1_easier_than_l12() {
    assert!(TestLevel::L1Recall.difficulty() < TestLevel::L12FarTransfer.difficulty());
}

#[test]
fn difficulty_monotonically_increasing() {
    let levels = TestLevel::all();
    for window in levels.windows(2) {
        assert!(
            window[0].difficulty() < window[1].difficulty(),
            "{} should be easier than {}",
            window[0],
            window[1]
        );
    }
}

// ── Passing thresholds ───────────────────────────────────────

#[test]
fn l1_has_highest_threshold() {
    let l1_thresh = TestLevel::L1Recall.passing_threshold();
    for level in &TestLevel::all()[1..] {
        assert!(
            l1_thresh >= level.passing_threshold(),
            "L1 threshold {} should be >= {} threshold {}",
            l1_thresh,
            level,
            level.passing_threshold()
        );
    }
}

#[test]
fn all_thresholds_are_valid() {
    for level in TestLevel::all() {
        let t = level.passing_threshold();
        assert!(
            (0.0..=1.0).contains(&t),
            "{} has invalid threshold {}",
            level,
            t
        );
    }
}

#[test]
fn thresholds_decrease_with_difficulty() {
    let levels = TestLevel::all();
    for window in levels.windows(2) {
        assert!(
            window[0].passing_threshold() >= window[1].passing_threshold(),
            "{} threshold should be >= {} threshold",
            window[0],
            window[1]
        );
    }
}

// ── Serde roundtrip ──────────────────────────────────────────

#[test]
fn serde_roundtrip_all_levels() {
    for level in TestLevel::all() {
        let json = serde_json::to_string(level).unwrap();
        let deser: TestLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(*level, deser, "roundtrip failed for {}", level);
    }
}

#[test]
fn serde_json_uses_snake_case() {
    let json = serde_json::to_string(&TestLevel::L3TemporalReasoning).unwrap();
    assert_eq!(json, "\"l3_temporal_reasoning\"");
}

// ── From string conversion ───────────────────────────────────

#[test]
fn from_str_l_prefix() {
    assert_eq!(TestLevel::from_str_loose("L1"), Some(TestLevel::L1Recall));
    assert_eq!(
        TestLevel::from_str_loose("L12"),
        Some(TestLevel::L12FarTransfer)
    );
}

#[test]
fn from_str_lowercase() {
    assert_eq!(TestLevel::from_str_loose("l3"), Some(TestLevel::L3TemporalReasoning));
}

#[test]
fn from_str_display_name() {
    assert_eq!(
        TestLevel::from_str_loose("Recall"),
        Some(TestLevel::L1Recall)
    );
    assert_eq!(
        TestLevel::from_str_loose("Far Transfer"),
        Some(TestLevel::L12FarTransfer)
    );
}

#[test]
fn from_str_invalid_returns_none() {
    assert_eq!(TestLevel::from_str_loose("L0"), None);
    assert_eq!(TestLevel::from_str_loose("L13"), None);
    assert_eq!(TestLevel::from_str_loose("nonsense"), None);
}

#[test]
fn from_id_valid() {
    assert_eq!(TestLevel::from_id(1), Some(TestLevel::L1Recall));
    assert_eq!(TestLevel::from_id(12), Some(TestLevel::L12FarTransfer));
}

#[test]
fn from_id_invalid() {
    assert_eq!(TestLevel::from_id(0), None);
    assert_eq!(TestLevel::from_id(13), None);
}

// ── Descriptions ─────────────────────────────────────────────

#[test]
fn all_levels_have_nonempty_descriptions() {
    for level in TestLevel::all() {
        assert!(
            !level.description().is_empty(),
            "{} has empty description",
            level
        );
    }
}

#[test]
fn all_levels_have_nonempty_display_names() {
    for level in TestLevel::all() {
        assert!(
            !level.display_name().is_empty(),
            "level id {} has empty display name",
            level.id()
        );
    }
}
