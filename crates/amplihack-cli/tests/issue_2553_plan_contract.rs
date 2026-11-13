//! Decomposition/configuration contracts through the production Rust builder.

use crate::issue_2553_fixture::{FIRST, Fixture, SECOND, describe, leaf, plan};
use serde_json::{Value, json};

#[test]
fn absent_issue_forms_are_omitted_from_canonical_configuration() {
    let f = Fixture::new();
    let mut forms = vec![Value::Null, json!(""), json!(" \t\n ")];
    for placeholder in [
        "TBD",
        "TBC",
        "TODO",
        "N/A",
        "none",
        "null",
        "unknown",
        "unassigned",
        "pending",
    ] {
        forms.extend([
            json!(placeholder),
            json!(format!(" \t{}\n", placeholder.to_ascii_lowercase())),
            json!(placeholder.to_ascii_uppercase()),
        ]);
    }
    let mut leaves = vec![leaf("omitted", "Operations", "Report status in chat.")];
    for (i, value) in forms.into_iter().enumerate() {
        let mut item = leaf(
            &format!("absent-{i}"),
            "Operations",
            "Report status in chat.",
        );
        item["issue"] = value;
        leaves.push(item);
    }
    let config = f.normalise(&plan(leaves));
    for item in config.as_array().unwrap() {
        assert!(
            item.get("issue").is_none(),
            "absence must remain absent: {item:#}"
        );
    }
}

#[test]
fn positive_issue_boundaries_and_trimmed_decimal_strings_are_preserved() {
    let f = Fixture::new();
    let forms = [
        (json!(1), 1),
        (json!(2553), 2553),
        (json!(u64::MAX), u64::MAX),
        (json!(" 002553 "), 2553),
        (json!(u64::MAX.to_string()), u64::MAX),
    ];
    for (value, expected) in forms {
        let mut item = leaf("tracked", "Operations", "Report status in chat.");
        item["issue"] = value.clone();
        let config = f.normalise(&plan(vec![item]));
        assert_eq!(config[0]["issue"].as_u64(), Some(expected), "input={value}");
    }
}

#[test]
fn malformed_issues_are_errors_with_leaf_and_field_diagnostics() {
    let f = Fixture::new();
    for value in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(true),
        json!([]),
        json!({}),
        json!("+1"),
        json!("-1"),
        json!("1.0"),
        json!("1e3"),
        json!("#2553"),
        json!("https://github.com/example/operations/issues/2553"),
        json!("18446744073709551616"),
        json!("１２"),
        json!("25 53"),
        json!("not assigned yet"),
    ] {
        let mut item = leaf("bad-tracking", "Operations", "Report status in chat.");
        item["issue"] = value.clone();
        let out = f.normalise_output(&plan(vec![item]), "");
        assert!(
            !out.status.success(),
            "invalid {value} accepted: {}",
            describe(&out)
        );
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains("bad-tracking") && error.contains("issue"),
            "{error}"
        );
    }
}

#[test]
fn independent_ids_are_uuid_strings_preserved_when_tracking_is_added() {
    let f = Fixture::new();
    let mut a = leaf("a", "Operations", "Report status in chat.");
    let mut b = leaf("b", "Operations", "Report status in chat.");
    a["workstream_id"] = json!(FIRST);
    b["workstream_id"] = json!(SECOND);
    let before = f.normalise(&plan(vec![a.clone(), b.clone()]));
    a["issue"] = json!(2553);
    b["issue"] = json!(2553);
    let after = f.normalise(&plan(vec![a, b]));
    for (i, id) in [FIRST, SECOND].into_iter().enumerate() {
        assert_eq!(before[i]["workstream_id"], id);
        assert_eq!(after[i]["workstream_id"], id);
        uuid::Uuid::parse_str(after[i]["workstream_id"].as_str().unwrap()).unwrap();
    }
}

#[test]
fn new_issue_less_leaves_receive_distinct_non_nil_uuids() {
    let f = Fixture::new();
    let config = f.normalise(&plan(vec![
        leaf("a", "Operations", "Report status in chat."),
        leaf("b", "Operations", "Answer the questions in chat."),
    ]));
    let ids: Vec<_> = config
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let id = uuid::Uuid::parse_str(entry["workstream_id"].as_str().expect("UUID field"))
                .unwrap();
            assert!(!id.is_nil());
            id
        })
        .collect();
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn dependencies_resolve_keys_to_persisted_execution_identities() {
    let f = Fixture::new();
    let mut producer = leaf("research", "Operations", "Answer questions in chat.");
    let mut consumer = leaf(
        "report",
        "Operations",
        "Report the research result in chat.",
    );
    producer["workstream_id"] = json!(FIRST);
    consumer["workstream_id"] = json!(SECOND);
    consumer["depends_on"] = json!(["research"]);
    let config = f.normalise(&plan(vec![consumer, producer]));
    assert_eq!(config[0]["depends_on"], json!([FIRST]), "{config:#}");
    assert_eq!(config[1]["depends_on"], json!([]));
}

#[test]
fn invalid_complete_plans_are_rejected_before_any_external_action() {
    let f = Fixture::new();
    let a = leaf("a", "Operations", "Report status in chat.");
    let b = leaf("b", "Operations", "Answer questions in chat.");
    let mut cases = vec![];
    let mut bad = b.clone();
    bad["name"] = json!("a");
    cases.push(plan(vec![a.clone(), bad]));
    for dependencies in [json!(["missing"]), json!(["b"]), json!(["a", "a"])] {
        let mut bad = b.clone();
        bad["depends_on"] = dependencies;
        cases.push(plan(vec![a.clone(), bad]));
    }
    let mut cyclic_a = a.clone();
    cyclic_a["depends_on"] = json!(["b"]);
    let mut cyclic_b = b.clone();
    cyclic_b["depends_on"] = json!(["a"]);
    cases.push(plan(vec![cyclic_a, cyclic_b]));
    for (field, value) in [
        ("description", json!(" \n")),
        ("name", json!("")),
        ("workstream_id", json!("../escape")),
        ("repository", json!("example/unauthorised")),
        ("classification", json!("invented-route")),
        ("classification", json!(null)),
        ("required_outputs", json!({"answer": "unknown-kind"})),
    ] {
        let mut bad = b.clone();
        bad[field] = value;
        cases.push(plan(vec![a.clone(), bad]));
    }
    let mut duplicate_a = a.clone();
    duplicate_a["workstream_id"] = json!(FIRST);
    let mut duplicate_b = b;
    duplicate_b["workstream_id"] = json!(FIRST);
    cases.push(plan(vec![duplicate_a, duplicate_b]));
    cases.extend([
        json!({}),
        json!({"workstreams": []}),
        json!({"workstreams": "invalid"}),
    ]);
    for input in cases {
        let out = f.normalise_output(&input, "");
        assert!(
            !out.status.success(),
            "accepted {input:#}: {}",
            describe(&out)
        );
        assert!(
            out.stdout.is_empty(),
            "no usable prefix or config path on failure"
        );
        assert!(
            f.events().is_empty(),
            "normalisation performs no external actions"
        );
    }
}

#[test]
fn explicit_invalid_classification_cannot_fall_through_to_valid_lower_priority_field() {
    let f = Fixture::new();
    let mut item = leaf("bad", "unrecognised", "Report status in chat.");
    item["task_type"] = json!("Operations");
    item["type"] = json!("Operations");
    let out = f.normalise_output(&plan(vec![item]), "");
    assert!(!out.status.success(), "{}", describe(&out));
}

#[test]
fn classification_precedence_and_legacy_homogeneous_fallback_are_retained() {
    let f = Fixture::new();
    let mut item = leaf("precedence", "Operations", "Report status in chat.");
    item["task_type"] = json!("Development");
    item["type"] = json!("Investigation");
    assert_eq!(
        f.normalise(&plan(vec![item.clone()]))[0]["classification"],
        "Operations"
    );
    item.as_object_mut().unwrap().remove("classification");
    item["task_type"] = json!("Operations");
    assert_eq!(
        f.normalise(&plan(vec![item.clone()]))[0]["classification"],
        "Operations"
    );
    item.as_object_mut().unwrap().remove("task_type");
    item["type"] = json!("Operations");
    assert_eq!(
        f.normalise(&plan(vec![item.clone()]))[0]["classification"],
        "Operations"
    );
    item.as_object_mut().unwrap().remove("type");
    assert_eq!(
        f.normalise(&plan(vec![item]))[0]["classification"],
        "Operations"
    );
}

#[test]
fn mixed_plans_require_explicit_leaf_classifications() {
    let f = Fixture::new();
    let mut item = leaf("unspecified", "Operations", "Report status in chat.");
    item.as_object_mut().unwrap().remove("classification");
    let input = plan(vec![
        item,
        leaf("docs", "Development", "Update README.md in the repository."),
    ]);
    let out = f.normalise_output(&input, "");
    assert!(!out.status.success(), "{}", describe(&out));
}
