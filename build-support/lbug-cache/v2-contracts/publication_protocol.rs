//! Pure release-protocol tests use synthetic metadata; no GitHub credentials,
//! network requests, release creation, uploads, or repository mutations occur.
use crate::support::KEY;
use lbug_cache::publication::{
    Lookup, classify_lookup, create_draft_body, validate_public_release,
};
use serde_json::{Value, json};

fn release() -> Value {
    json!({
        "id": 1917, "tag_name": KEY, "draft": false,
        "prerelease": true, "immutable": true,
        "html_url": format!("https://github.com/rysweet/amplihack-rs/releases/tag/{KEY}"),
        "assets": [
            {"id": 1, "name": format!("{KEY}.tar.gz"), "digest": format!("sha256:{}", "a".repeat(64))},
            {"id": 2, "name": format!("{KEY}.tar.gz.sha256"), "digest": format!("sha256:{}", "b".repeat(64))}
        ]
    })
}

#[test]
fn producer_draft_is_a_prerelease_excluded_from_latest_with_string_false() {
    let commit = "c89e50c3887563c8b0eac4283c25ac255631c3f3";
    let body: Value =
        serde_json::from_str(&create_draft_body(KEY, commit, "owned-attempt-1").unwrap()).unwrap();
    assert_eq!(body["tag_name"], KEY);
    assert_eq!(body["target_commitish"], commit);
    assert_eq!(body["draft"], true);
    assert_eq!(body["prerelease"], true);
    assert_eq!(
        body["make_latest"], "false",
        "GitHub requires a string here"
    );
}

#[test]
fn mutable_draft_wrong_repository_and_unanchored_assets_cannot_supply_release_trust() {
    let valid = release();
    validate_public_release(&valid.to_string(), KEY).unwrap();
    for (field, value) in [
        ("draft", json!(true)),
        ("immutable", json!(false)),
        ("prerelease", json!(false)),
        (
            "html_url",
            json!("https://github.com/attacker/amplihack-rs/releases/tag/forged"),
        ),
        ("tag_name", json!("different-key")),
    ] {
        let mut forged = valid.clone();
        forged[field] = value;
        assert!(
            validate_public_release(&forged.to_string(), KEY).is_err(),
            "{field}"
        );
    }
    let mut forged = valid.clone();
    forged["assets"][0]
        .as_object_mut()
        .unwrap()
        .remove("digest");
    forged["body"] = json!(format!("sha256:{}", "a".repeat(64)));
    assert!(validate_public_release(&forged.to_string(), KEY).is_err());
    let mut missing_sidecar = valid;
    missing_sidecar["assets"].as_array_mut().unwrap().pop();
    assert!(validate_public_release(&missing_sidecar.to_string(), KEY).is_err());
}

#[test]
fn only_a_confirmed_miss_allows_production() {
    assert_eq!(
        classify_lookup(404, r#"{"message":"Not Found"}"#, true),
        Lookup::Missing
    );
    for (status, body, authenticated) in [
        (401, "{}", true),
        (403, "{}", true),
        (429, "{}", true),
        (500, "{}", true),
        (200, "truncated", true),
        (404, "{}", false),
    ] {
        assert_eq!(
            classify_lookup(status, body, authenticated),
            Lookup::Unavailable
        );
    }
}
