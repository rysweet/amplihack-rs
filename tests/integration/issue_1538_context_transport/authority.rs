use super::fixtures::*;
use serde_json::json;

// Each pair exercises the helper and actual YAML body independently.
macro_rules! authority_case {
    ($helper:ident,$gate:ident,$fixture:expr,$state:literal,$diagnostic:literal) => {
        #[test]
        fn $helper() {
            ($fixture).helper().helper_state($state);
        }
        #[test]
        fn $gate() {
            ($fixture).gate().gate($diagnostic);
        }
    };
}

authority_case!(
    helper_canonical_object,
    enforcer_canonical_object,
    Fixture::new().env("RECIPE_VAR_verdict_json", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_COMPLETED",
    "APPROVED"
);
authority_case!(
    helper_legacy_only,
    enforcer_legacy_only,
    Fixture::new().env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_COMPLETED",
    "APPROVED"
);
authority_case!(helper_file_positive_over_stale_negative,enforcer_file_positive_over_stale_negative,
    Fixture::new().file(json!({"verdict_json":serde_json::from_str::<serde_json::Value>(&verdict("WORK_VERIFIED")).unwrap()}))
        .env("RECIPE_VAR_verdict_json",verdict("HOLLOW_SUCCESS")).env("VERDICT_JSON",verdict("HOLLOW_SUCCESS")),
    "IMPLEMENTATION_COMPLETED","APPROVED");
authority_case!(
    helper_canonical_hollow_over_stale_positive,
    enforcer_canonical_hollow_over_stale_positive,
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", verdict("HOLLOW_SUCCESS"))
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "HOLLOW_SUCCESS"
);
authority_case!(
    helper_file_hollow_over_stale_positive,
    enforcer_file_hollow_over_stale_positive,
    Fixture::new()
        .file(json!({"verdict_json":{"verdict":"HOLLOW_SUCCESS"}}))
        .env("RECIPE_VAR_verdict_json", verdict("WORK_VERIFIED"))
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "HOLLOW_SUCCESS"
);
authority_case!(
    helper_empty_canonical_authoritative,
    enforcer_empty_canonical_authoritative,
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", "")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_malformed_canonical_authoritative,
    enforcer_malformed_canonical_authoritative,
    Fixture::new()
        .env(
            "RECIPE_VAR_verdict_json",
            "broken {\"verdict\":\"WORK_VERIFIED\"}"
        )
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_nested_only_verdict_is_insufficient,
    enforcer_nested_only_verdict_is_insufficient,
    Fixture::new()
        .env("RECIPE_VAR_verdict_json__verdict", "WORK_VERIFIED")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_missing_file_field_suppresses_fallback,
    enforcer_missing_file_field_suppresses_fallback,
    Fixture::new()
        .file(json!({}))
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_empty_file_selector_suppresses_fallback,
    enforcer_empty_file_selector_suppresses_fallback,
    Fixture::new()
        .env("AMPLIHACK_CONTEXT_FILE", "")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_missing_file_suppresses_fallback,
    enforcer_missing_file_suppresses_fallback,
    Fixture::new()
        .env("AMPLIHACK_CONTEXT_FILE", "/missing/issue1538/context.json")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_malformed_file_suppresses_fallback,
    enforcer_malformed_file_suppresses_fallback,
    Fixture::new()
        .file_raw("{broken")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_UNPROVEN",
    "INSUFFICIENT_EVIDENCE"
);
authority_case!(
    helper_canonical_cohort_suppresses_stale_noop,
    enforcer_canonical_cohort_suppresses_stale_noop,
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", verdict("HOLLOW_SUCCESS"))
        .env("ALLOW_NO_OP", "true")
        .env("IMPLEMENTATION", SENTINEL),
    "IMPLEMENTATION_UNPROVEN",
    "HOLLOW_SUCCESS"
);
authority_case!(
    helper_file_cohort_suppresses_stale_sentinel,
    enforcer_file_cohort_suppresses_stale_sentinel,
    Fixture::new()
        .file(json!({"verdict_json":{"verdict":"HOLLOW_SUCCESS"},"allow_no_op":false}))
        .env("ALLOW_NO_OP", "true")
        .env("IMPLEMENTATION", SENTINEL),
    "IMPLEMENTATION_UNPROVEN",
    "HOLLOW_SUCCESS"
);
authority_case!(
    helper_root_negative_beats_nested_positive,
    enforcer_root_negative_beats_nested_positive,
    Fixture::new()
        .env("RECIPE_VAR_verdict_json", verdict("HOLLOW_SUCCESS"))
        .env("RECIPE_VAR_verdict_json__verdict", "WORK_VERIFIED"),
    "IMPLEMENTATION_UNPROVEN",
    "HOLLOW_SUCCESS"
);
authority_case!(
    helper_unrelated_namespace_preserves_legacy,
    enforcer_unrelated_namespace_preserves_legacy,
    Fixture::new()
        .env("RECIPE_VAR_task_description", "unrelated")
        .env("VERDICT_JSON", verdict("WORK_VERIFIED")),
    "IMPLEMENTATION_COMPLETED",
    "APPROVED"
);

#[test]
fn invalid_file_roots_never_revive_legacy_positive() {
    for raw in ["", "null", "[]", "true", "\"text\"", "{} {}"] {
        Fixture::new()
            .file_raw(raw)
            .env("VERDICT_JSON", verdict("WORK_VERIFIED"))
            .helper()
            .helper_state("IMPLEMENTATION_UNPROVEN");
    }
}

#[cfg(unix)]
#[test]
fn symlink_context_file_is_rejected_without_touching_target() {
    let f = Fixture::new().file(json!({"verdict_json":{"verdict":"WORK_VERIFIED"}}));
    let target = f.temp.path().join("context.json");
    let before = std::fs::read(&target).unwrap();
    let link = f.temp.path().join("link.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let f = f
        .env("AMPLIHACK_CONTEXT_FILE", link.display().to_string())
        .env("VERDICT_JSON", verdict("WORK_VERIFIED"));
    f.helper().helper_state("IMPLEMENTATION_UNPROVEN");
    assert_eq!(std::fs::read(&target).unwrap(), before);
}

#[test]
fn directory_context_is_rejected_conservatively() {
    let f = Fixture::new();
    let path = f.temp.path().display().to_string();
    f.env("AMPLIHACK_CONTEXT_FILE", path)
        .env("VERDICT_JSON", verdict("WORK_VERIFIED"))
        .helper()
        .helper_state("IMPLEMENTATION_UNPROVEN");
}
