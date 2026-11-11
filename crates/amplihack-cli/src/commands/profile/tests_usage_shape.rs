//! Issue #1856: `ExtraUsageShape`, the redacted shape of the usage
//! endpoint's `extra_usage` that `--raw` prints for a credits-on row.
//!
//! Contract:
//! - `ExtraUsageShape::of(&Value) -> ExtraUsageShape` cannot fail or panic
//!   and does not recurse. `line() -> &str` is the text after
//!   `note: profile "<name>": `. The field is private; the type derives
//!   `Debug`, `Clone` and `PartialEq` and is never serialised.
//! - An object gives `extra_usage: object; keys: <keys>; is_enabled: <form>`
//!   followed by `; <key>: bool <v>` or `; <key>: null` for every other
//!   listed member holding a bool or `null`, in key order.
//! - Any other value gives only its type (`string`, `number`, `array`),
//!   except a bool, which also gives its value (`bool true`).
//! - Keys are sorted by byte order. At most 32 are listed, then
//!   ` (+N more)`. An object with no keys gives `keys: none`.
//! - In a key every character outside `[A-Za-z0-9_.-]` becomes `?`. A key
//!   longer than 64 characters is cut to 63 and followed by `…`; an empty
//!   key is `""`.
//! - `is_enabled` is `missing`, `null`, `bool true`, `bool false`,
//!   `string "<word>"` for one of `true false on off yes no enabled
//!   disabled 0 1` matched ignoring ASCII case and printed in lower case,
//!   `string (<n> chars)` for any other string, `number 0`, `number 1`
//!   (integer or float exactly equal to 0 or 1),
//!   `number` for any other number, `object` or `array`.
//! - No string or number from the value, other than the allow-listed
//!   words and `0` and `1`, ever appears in the line.

use super::usage_request::ExtraUsageShape;
use serde_json::{Value, json};

fn line(v: Value) -> String {
    ExtraUsageShape::of(&v).line().to_string()
}

/// The `is_enabled: …` form for `{"is_enabled": v}`.
fn enabled(v: Value) -> String {
    let l = line(json!({ "is_enabled": v }));
    let prefix = "extra_usage: object; keys: is_enabled; is_enabled: ";
    l.strip_prefix(prefix)
        .unwrap_or_else(|| panic!("unexpected line for {v}: {l}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The documented examples
// ---------------------------------------------------------------------------

#[test]
fn the_reference_example_is_exact() {
    assert_eq!(
        line(json!({
            "is_enabled": true,
            "monthly_limit": 5000,
            "used_credits": 12.5,
            "currency": "USD",
            "org_id": "0f2c-uuid"
        })),
        "extra_usage: object; keys: currency, is_enabled, monthly_limit, org_id, used_credits; is_enabled: bool true"
    );
}

#[test]
fn a_string_false_is_told_apart_from_the_bool() {
    assert_eq!(
        line(json!({"is_enabled": "false"})),
        "extra_usage: object; keys: is_enabled; is_enabled: string \"false\""
    );
    assert_eq!(
        line(json!({"is_enabled": false})),
        "extra_usage: object; keys: is_enabled; is_enabled: bool false"
    );
}

#[test]
fn other_bool_and_null_members_follow_in_key_order() {
    assert_eq!(
        line(json!({"overage": null, "limit_reached": false, "amount": 3})),
        "extra_usage: object; keys: amount, limit_reached, overage; is_enabled: missing; limit_reached: bool false; overage: null"
    );
}

#[test]
fn an_empty_object_has_no_keys() {
    assert_eq!(
        line(json!({})),
        "extra_usage: object; keys: none; is_enabled: missing"
    );
}

// ---------------------------------------------------------------------------
// Non-object values
// ---------------------------------------------------------------------------

#[test]
fn a_value_that_is_not_an_object_shows_only_its_type() {
    assert_eq!(line(json!("on")), "extra_usage: string");
    assert_eq!(line(json!("SENTINEL-x")), "extra_usage: string");
    assert_eq!(line(json!(0)), "extra_usage: number");
    assert_eq!(line(json!(987_654)), "extra_usage: number");
    assert_eq!(line(json!([])), "extra_usage: array");
    assert_eq!(line(json!(["SENTINEL"])), "extra_usage: array");
    assert_eq!(line(json!(true)), "extra_usage: bool true");
    assert_eq!(line(json!(false)), "extra_usage: bool false");
}

// ---------------------------------------------------------------------------
// `is_enabled`
// ---------------------------------------------------------------------------

#[test]
fn is_enabled_missing_null_and_bools() {
    assert_eq!(
        line(json!({"currency": "USD"})),
        "extra_usage: object; keys: currency; is_enabled: missing"
    );
    assert_eq!(enabled(json!(null)), "null");
    assert_eq!(enabled(json!(true)), "bool true");
    assert_eq!(enabled(json!(false)), "bool false");
}

#[test]
fn allow_listed_strings_print_the_constant_in_lower_case() {
    for (input, want) in [
        ("true", "true"),
        ("false", "false"),
        ("FALSE", "false"),
        ("On", "on"),
        ("off", "off"),
        ("YES", "yes"),
        ("no", "no"),
        ("Enabled", "enabled"),
        ("DISABLED", "disabled"),
        ("0", "0"),
        ("1", "1"),
    ] {
        assert_eq!(
            enabled(json!(input)),
            format!("string \"{want}\""),
            "{input}"
        );
    }
}

#[test]
fn other_strings_show_only_their_length_in_characters() {
    assert_eq!(enabled(json!("SENTINEL-x")), "string (10 chars)");
    assert_eq!(enabled(json!("")), "string (0 chars)");
    assert_eq!(enabled(json!("héllo")), "string (5 chars)");
    // Near misses of the allow-list are not matched.
    assert_eq!(enabled(json!(" true")), "string (5 chars)");
    assert_eq!(enabled(json!("truee")), "string (5 chars)");
    // A non-ASCII case fold is not an ASCII match.
    assert_eq!(enabled(json!("TRUE\u{0130}")), "string (5 chars)");
}

#[test]
fn only_the_numbers_zero_and_one_are_printed() {
    assert_eq!(enabled(json!(0)), "number 0");
    assert_eq!(enabled(json!(1)), "number 1");
    for n in [
        json!(2),
        json!(-1),
        json!(0.5),
        json!(987_654),
        json!(4321.5),
    ] {
        assert_eq!(enabled(n.clone()), "number", "{n}");
    }
}

#[test]
fn floats_exactly_zero_or_one_are_the_flag() {
    assert_eq!(enabled(json!(0.0)), "number 0");
    assert_eq!(enabled(json!(1.0)), "number 1");
    for n in [json!(0.5), json!(2.0), json!(0.9999), json!(1.0001)] {
        assert_eq!(enabled(n.clone()), "number", "{n}");
    }
}

#[test]
fn nested_values_of_is_enabled_show_only_their_type() {
    assert_eq!(enabled(json!({})), "object");
    assert_eq!(enabled(json!({"secret": "SENTINEL"})), "object");
    assert_eq!(enabled(json!([])), "array");
    assert_eq!(enabled(json!([true, "SENTINEL"])), "array");
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

#[test]
fn keys_are_sorted_by_byte_order() {
    assert_eq!(
        line(json!({"b": 1, "a": 1, "B": 1, "_": 1, "0": 1})),
        "extra_usage: object; keys: 0, B, _, a, b; is_enabled: missing"
    );
}

#[test]
fn at_most_32_keys_are_listed_and_the_rest_counted() {
    let mut o = serde_json::Map::new();
    for i in 0..40 {
        o.insert(format!("k{i:02}"), json!(1));
    }
    let l = line(Value::Object(o));
    let listed: Vec<String> = (0..32).map(|i| format!("k{i:02}")).collect();
    assert_eq!(
        l,
        format!(
            "extra_usage: object; keys: {} (+8 more); is_enabled: missing",
            listed.join(", ")
        )
    );
    assert!(!l.contains("k32"), "{l}");
}

#[test]
fn exactly_32_keys_have_no_count() {
    let mut o = serde_json::Map::new();
    for i in 0..32 {
        o.insert(format!("k{i:02}"), json!(1));
    }
    let l = line(Value::Object(o));
    assert!(!l.contains("more"), "{l}");
    assert!(l.contains("k31; is_enabled: missing"), "{l}");
}

#[test]
fn flags_are_shown_only_for_listed_keys() {
    let mut o = serde_json::Map::new();
    for i in 0..40 {
        o.insert(format!("k{i:02}"), json!(1));
    }
    o.insert("k01".into(), json!(null));
    o.insert("k35".into(), json!(true));
    let l = line(Value::Object(o));
    assert!(l.ends_with("; is_enabled: missing; k01: null"), "{l}");
    assert!(!l.contains("k35"), "{l}");
}

#[test]
fn is_enabled_is_shown_even_when_past_the_key_cap() {
    let mut o = serde_json::Map::new();
    for i in 0..40 {
        o.insert(format!("a{i:02}"), json!(1));
    }
    o.insert("is_enabled".into(), json!(true));
    let l = line(Value::Object(o));
    assert!(l.contains(" (+9 more); is_enabled: bool true"), "{l}");
}

#[test]
fn a_long_key_is_cut_to_63_characters_and_an_ellipsis() {
    let long = "a".repeat(100);
    let l = line(json!({ long: 1 }));
    assert_eq!(
        l,
        format!(
            "extra_usage: object; keys: {}…; is_enabled: missing",
            "a".repeat(63)
        )
    );
    let exact = "b".repeat(64);
    let l = line(json!({ exact.clone(): 1 }));
    assert!(l.contains(&format!("keys: {exact};")), "64 is kept: {l}");
}

#[test]
fn an_empty_key_is_shown_as_two_quotes() {
    assert_eq!(
        line(json!({"": true, "is_enabled": true})),
        "extra_usage: object; keys: \"\", is_enabled; is_enabled: bool true; \"\": bool true"
    );
}

#[test]
fn key_characters_outside_the_allow_list_become_question_marks() {
    for (key, want) in [
        ("a b", "a?b"),
        ("a,b", "a?b"),
        ("a;b", "a?b"),
        ("a:b", "a?b"),
        ("a\"b", "a?b"),
        ("a\nb", "a?b"),
        ("a\u{1b}[31mb", "a??31mb"),
        ("a\u{202e}b", "a?b"),
        ("a\u{2066}b", "a?b"),
        ("a\u{200b}b", "a?b"),
        ("a\u{200e}b", "a?b"),
        ("a\u{2028}b", "a?b"),
        ("a\u{0}b", "a?b"),
        // Cyrillic `а` in place of the Latin `a`.
        ("is_en\u{0430}bled", "is_en?bled"),
        ("ok.key-1_2", "ok.key-1_2"),
    ] {
        let l = line(json!({ key: 1 }));
        assert_eq!(
            l,
            format!("extra_usage: object; keys: {want}; is_enabled: missing"),
            "{key:?}"
        );
    }
}

#[test]
fn a_key_cannot_forge_a_field_of_the_line() {
    let forged = "a; is_enabled: bool false";
    let l = line(json!({ forged: true, "is_enabled": true }));
    assert!(l.contains("a??is_enabled??bool?false"), "{l}");
    assert_eq!(l.matches("is_enabled:").count(), 1, "{l}");
    assert!(l.contains("; is_enabled: bool true"), "{l}");
    assert!(!l.contains("bool false"), "{l}");
}

#[test]
fn a_homoglyph_key_does_not_pass_for_is_enabled() {
    let l = line(json!({"is_en\u{0430}bled": false}));
    assert!(l.contains("is_enabled: missing"), "{l}");
    assert!(l.contains("is_en?bled: bool false"), "{l}");
}

#[test]
fn every_character_of_the_line_is_safe_to_print() {
    let l = line(json!({
        "\u{202e}x": null,
        "y\u{7}": true,
        "z\r": false,
        "is_enabled": "\u{202e}\u{1b}"
    }));
    assert!(
        l.chars().all(|c| c == '…' || (' '..='~').contains(&c)),
        "{l:?}"
    );
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// The fake endpoint's credits-on `extra_usage` (D11).
fn sentinel() -> Value {
    json!({
        "is_enabled": true,
        "email": "SENTINEL-EMAIL@example.invalid",
        "organization_uuid": "00000000-SENTINEL-0000-0000-000000000001",
        "account_uuid": "00000000-SENTINEL-0000-0000-000000000002",
        "monthly_limit": 987_654,
        "used_credits": 4321.5,
        "currency": "SENTINELCUR",
        "nested": {"token": "SENTINEL-TOKEN", "is_enabled": false}
    })
}

#[test]
fn no_value_from_the_reply_appears_in_the_line_or_debug() {
    let shape = ExtraUsageShape::of(&sentinel());
    let l = shape.line().to_string();
    let debug = format!("{shape:?}");
    for leak in [
        "SENTINEL", "987654", "987", "4321", "@", "example", "TOKEN", "CUR",
    ] {
        assert!(!l.contains(leak), "{leak} in {l}");
        assert!(!debug.contains(leak), "{leak} in {debug}");
    }
    assert_eq!(
        l,
        "extra_usage: object; keys: account_uuid, currency, email, is_enabled, monthly_limit, nested, organization_uuid, used_credits; is_enabled: bool true"
    );
}

#[test]
fn nested_objects_are_never_looked_into() {
    let l = line(json!({"is_enabled": true, "limits": {"inner": null, "flag": true}}));
    assert_eq!(
        l,
        "extra_usage: object; keys: is_enabled, limits; is_enabled: bool true"
    );
}

#[test]
fn no_json_type_panics_at_either_level() {
    let all = [
        json!(null),
        json!(true),
        json!(1.5),
        json!("s"),
        json!([1, [2, {"a": null}]]),
        json!({"a": {"b": {"c": []}}}),
    ];
    for v in &all {
        let _ = line(v.clone());
        for w in &all {
            let _ = line(json!({ "is_enabled": w, "other": w }));
        }
    }
    // Deep nesting is not walked.
    let mut deep = json!(true);
    for _ in 0..100 {
        deep = json!({ "x": deep });
    }
    assert_eq!(
        line(json!({"is_enabled": deep})),
        "extra_usage: object; keys: is_enabled; is_enabled: object"
    );
}

#[test]
fn equal_values_give_equal_shapes() {
    assert_eq!(
        ExtraUsageShape::of(&sentinel()),
        ExtraUsageShape::of(&sentinel()).clone()
    );
    assert_ne!(
        ExtraUsageShape::of(&json!({"is_enabled": true})),
        ExtraUsageShape::of(&json!({"is_enabled": "true"}))
    );
}
