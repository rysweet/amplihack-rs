//! Unit specification for the pure core of the checkpoint engine (#1554):
//! exit mapping, the action table, advertisement validation, NUL records,
//! canonical JSON and base64 path identities. Expected bytes were produced by
//! the Python helpers at 918c256cf (`serial`, `parse`, `unb64`,
//! `json.dumps(dict(...))`, `destination_tip`, `ORIGIN_REF_PY`) run with
//! CPython 3.14 `-I`. The row-by-row parser table is in
//! `legacy_parity_tests.rs`; this file keeps the status and lookup rules.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::num::NonZeroI32;

use serde_json::{Value, json};

use super::actions::nul_record;
use super::advert::{AdvertError, AdvertMap};
use super::json as cjson;
use super::{
    ACTIONS, AdvertStatus, Arity, Fail, REFUSAL_PREFIX, Selectors, StdoutPolicy, advert_fail,
    to_error,
};

fn status(f: Fail) -> i32 {
    crate::command_error::exit_code(&to_error(f, REFUSAL_PREFIX))
        .expect("every Fail carries a status")
}

fn oid(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}

fn row(name: &str) -> &'static super::ActionSpec {
    ACTIONS.iter().find(|s| s.name == name).unwrap()
}

// ------------------------------------------------------------ exit mapping

#[test]
fn fail_variants_map_to_one_status_each() {
    assert_eq!(status(Fail::Refusal("r".into())), 2);
    assert_eq!(status(Fail::Advert(AdvertError::Malformed)), 2);
    assert_eq!(status(Fail::Terminal79), 79);
    assert_eq!(status(Fail::Passthrough(NonZeroI32::new(5).unwrap())), 5);
    assert_eq!(status(Fail::Crash("IndexError".into())), 1);
}

#[test]
fn passthrough_zero_cannot_be_built() {
    // No failure path maps to 0: the status type has no zero.
    assert!(NonZeroI32::new(0).is_none());
}

#[test]
fn unknown_action_is_the_918c_refusal() {
    let e = super::run(OsStr::new("frobnicate"), vec![]).unwrap_err();
    assert_eq!(crate::command_error::exit_code(&e), Some(2));
}

#[test]
fn a_missing_action_is_the_918c_index_error() {
    assert_eq!(
        crate::command_error::exit_code(&super::missing_action()),
        Some(1)
    );
}

#[test]
fn a_missing_indexed_argument_is_a_crash_not_a_refusal() {
    // 918c `context(sys.argv[2])` with no phase raised IndexError: status 1.
    let e = super::run(OsStr::new("capture"), vec![]).unwrap_err();
    assert_eq!(crate::command_error::exit_code(&e), Some(1));
    let e = super::run(OsStr::new("cleanup-skip-result"), vec![]).unwrap_err();
    assert_eq!(crate::command_error::exit_code(&e), Some(1));
}

// ------------------------------------------------------------- action table

/// The whole table, row by row, as the 918c Python read its arguments,
/// selectors, advertisements and streams.
#[test]
fn the_action_table_is_the_918c_behaviour_row_by_row() {
    use AdvertStatus as A;
    use Arity::{Checked, Ignored as Any, Indexed};
    use Selectors::{Context, Ignored};
    let expected: [(&str, Arity, Selectors, AdvertStatus, StdoutPolicy); 16] = [
        ("selectors", Any, Context, A::None, StdoutPolicy::OnSuccess),
        (
            "capture",
            Indexed(1),
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        (
            "admit",
            Indexed(1),
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        ("check", Any, Context, A::Refusal, StdoutPolicy::OnSuccess),
        ("paths", Any, Context, A::Refusal, StdoutPolicy::OnSuccess),
        (
            "stage-strict",
            Indexed(1),
            Ignored,
            A::None,
            StdoutPolicy::OnSuccess,
        ),
        (
            "before",
            Checked,
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        (
            "sync-fetch-mode",
            Checked,
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        (
            "after",
            Indexed(3),
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        (
            "destination-is-head",
            Any,
            Context,
            A::Refusal,
            StdoutPolicy::OnSuccess,
        ),
        (
            "verify-pushed-ref",
            Checked,
            Context,
            A::Refusal,
            StdoutPolicy::Verifier,
        ),
        (
            "advertised-ref",
            Indexed(3),
            Ignored,
            A::Exit78,
            StdoutPolicy::OnSuccess,
        ),
        ("sha256", Any, Ignored, A::None, StdoutPolicy::OnSuccess),
        (
            "admission-refused-result",
            Any,
            Ignored,
            A::None,
            StdoutPolicy::OnSuccess,
        ),
        (
            "cleanup-skip-result",
            Indexed(1),
            Ignored,
            A::None,
            StdoutPolicy::OnSuccess,
        ),
        (
            "worktree-path",
            Any,
            Ignored,
            A::None,
            StdoutPolicy::OnSuccess,
        ),
    ];
    assert_eq!(ACTIONS.len(), expected.len());
    for (spec, (name, arity, selectors, advert, stdout)) in ACTIONS.iter().zip(expected) {
        assert_eq!(spec.name, name);
        assert_eq!(
            (spec.arity, spec.selectors, spec.advert, spec.stdout),
            (arity, selectors, advert, stdout),
            "{name}"
        );
        assert!(!spec.source.is_empty(), "{name} cites no 918c source");
    }
}

#[test]
fn advert_errors_take_their_status_from_the_table_row() {
    let code = |name: &str| match advert_fail(row(name), AdvertError::Duplicate) {
        Fail::Passthrough(code) => code.get(),
        Fail::Refusal(reason) => {
            assert_eq!(reason, "duplicate publication advertisement");
            2
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(code("advertised-ref"), 78);
    for name in [
        "capture",
        "admit",
        "check",
        "before",
        "after",
        "destination-is-head",
    ] {
        assert_eq!(code(name), 2, "{name}");
    }
}

#[test]
fn a_non_ascii_destination_oid_is_the_caught_unicode_decode_error() {
    // 918c destination_tip: parts[0].decode('ascii') raises
    // UnicodeDecodeError, a ValueError, which the except clause refuses.
    let mut body = b"\xe9".to_vec();
    body.extend_from_slice(oid('a', 39).as_bytes());
    body.extend_from_slice(b"\trefs/heads/x\n");
    let error = AdvertMap::parse(&body, 0).unwrap_err();
    assert_eq!(
        error.checkpoint_reason(),
        "'ascii' codec can't decode byte 0xe9 in position 0: ordinal not in range(128)"
    );
    assert_eq!(status(Fail::Advert(error)), 2);
}

// ------------------------------------------------------------------ advert

fn adv(body: &str, rc: i32) -> Result<AdvertMap, AdvertError> {
    AdvertMap::parse(body.as_bytes(), rc)
}

#[test]
fn advert_valid_empty_is_ok_and_absent() {
    for rc in [0, 2] {
        let m = adv("", rc).unwrap();
        assert_eq!(m.tip(b"refs/heads/a"), None);
    }
}

#[test]
fn advert_exact_lookup_by_bytes() {
    let body = format!("{}\tHEAD\n{}\trefs/heads/a\n", oid('a', 40), oid('b', 40));
    let m = adv(&body, 0).unwrap();
    assert!(m.tip(b"refs/heads/a").is_some());
    assert_eq!(m.tip(b"refs/heads/"), None);
    assert_eq!(m.tip(b"refs/heads/ab"), None);
    assert_eq!(m.tip(b"refs/heads/A"), None);
    assert!(m.tip(b"HEAD").is_some());
}

#[test]
fn advert_peeled_entries_are_their_own_keys() {
    let body = format!(
        "{}\trefs/tags/v1\n{}\trefs/tags/v1^{{}}\n",
        oid('a', 40),
        oid('b', 40)
    );
    let m = adv(&body, 0).unwrap();
    assert_eq!(m.tip(b"refs/tags/v1^{}"), Some(oid('b', 40).as_str()));
}

#[test]
fn advert_rejections_name_the_918c_reason() {
    let a = oid('a', 40);
    let cases = [
        (format!("{a} refs/heads/a\n"), AdvertError::Malformed),
        (format!("{a}\theads/a\n"), AdvertError::Malformed),
        (
            format!("{a}\trefs/heads/a\textra\n"),
            AdvertError::Malformed,
        ),
        (format!("{a}\trefs/heads/a\nzz\n"), AdvertError::Malformed),
        (format!("{a}\trefs/heads/a\n\n"), AdvertError::Malformed),
        (
            format!("{}\trefs/heads/a\n", oid('A', 40)),
            AdvertError::InvalidOid,
        ),
        (
            format!("{}\trefs/heads/a\n", oid('a', 39)),
            AdvertError::InvalidOid,
        ),
        (
            format!("{a}\trefs/heads/a\n{a}\trefs/heads/a\n"),
            AdvertError::Duplicate,
        ),
        (format!("{a}\trefs/heads/a\n"), AdvertError::AmbiguousAbsent),
    ];
    for (i, (body, expected)) in cases.iter().enumerate() {
        let rc = if *expected == AdvertError::AmbiguousAbsent {
            2
        } else {
            0
        };
        assert_eq!(adv(body, rc).unwrap_err(), *expected, "case {i}: {body:?}");
    }
}

#[test]
fn advert_other_transport_statuses_are_the_callers_concern() {
    // Both 918c callers reject a status outside 0 and 2 before parsing.
    assert!(adv("", 1).is_ok());
    assert!(adv("", 128).is_ok());
}

#[test]
fn advert_non_utf8_refnames_are_kept_as_bytes() {
    let mut body = oid('a', 40).into_bytes();
    body.extend_from_slice(b"\trefs/heads/\xff\n");
    body.extend_from_slice(oid('b', 40).as_bytes());
    body.extend_from_slice(b"\trefs/heads/\xfe\n");
    let m = AdvertMap::parse(&body, 0).unwrap();
    assert_eq!(m.tip(b"refs/heads/\xff"), Some(oid('a', 40).as_str()));
    assert_eq!(m.tip(b"refs/heads/\xfe"), Some(oid('b', 40).as_str()));
}

// ----------------------------------------------------------------- records

#[test]
fn records_encode_eight_and_four_nul_terminated_fields() {
    let e = nul_record([b"a".as_slice(), b"", b"c", b"d", b"e", b"f", b"g", b"h"]).unwrap();
    assert_eq!(e, b"a\0\0c\0d\0e\0f\0g\0h\0");
    let e = nul_record([b"h".as_slice(), b"", b"", b"refs/remotes/origin/main"]).unwrap();
    assert_eq!(e, b"h\0\0\0refs/remotes/origin/main\0");
}

#[test]
fn records_refuse_a_field_containing_nul() {
    assert!(nul_record([b"a\0b".as_slice(), b"", b"", b""]).is_err());
}

#[test]
fn records_keep_non_utf8_paths() {
    let p = b"/t/caf\xe9/authority.json";
    let e = nul_record([p.as_slice(), b"1:2", b"h", b"i", b"H", b"", b"", b"refs/x"]).unwrap();
    assert!(e.starts_with(b"/t/caf\xe9/authority.json\0"));
}

// -------------------------------------------------------------------- json

fn hash_of(s: &str) -> String {
    let v = cjson::parse(s.as_bytes()).unwrap();
    String::from_utf8(cjson::serial(&v)).unwrap()
}

#[test]
fn json_hash_style_matches_python_serial() {
    assert_eq!(
        hash_of(r#"{"b":1,"a":[true,null,"x"]}"#),
        r#"{"a":[true,null,"x"],"b":1}"#
    );
    assert_eq!(
        hash_of(r#"{"k":"caf\u00e9 \ud83d\ude00"}"#),
        r#"{"k":"caf\u00e9 \ud83d\ude00"}"#
    );
    assert_eq!(
        hash_of(r#"{"s":"\u0000\u001f\"\\/\b\f\n\r\t\u007f"}"#),
        r#"{"s":"\u0000\u001f\"\\/\b\f\n\r\t\u007f"}"#
    );
}

#[test]
fn json_hash_keys_sort_by_code_point_not_utf16() {
    // U+FFFF sorts before U+1F600 by code point; UTF-16 order would invert it.
    assert_eq!(
        hash_of(r#"{"\ud83d\ude00":1,"\uffff":2}"#),
        r#"{"\uffff":2,"\ud83d\ude00":1}"#
    );
}

#[test]
fn json_refusals_match_the_python_hooks() {
    let huge = format!("[{}]", "9".repeat(400)); // int -> float -> inf -> refused
    for bad in [
        r#"{"a":1,"a":2}"#,
        r#"{"x":{"a":1,"a":2}}"#,
        r#"[{"a":1,"a":2}]"#,
        "[NaN]",
        "[Infinity]",
        "[-Infinity]",
        "[1e400]",
        huge.as_str(),
        r#"["\ud800"]"#,
        r#"["\udc00x"]"#,
        "{} x",
        "",
    ] {
        assert!(cjson::parse(bad.as_bytes()).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn json_dumps_style_is_python_default_dumps() {
    let dumps = |raw: &str| {
        cjson::py_dumps(&cjson::py_loads(&cjson::surrogateescape(raw.as_bytes())).unwrap())
    };
    assert_eq!(
        dumps(r#"{"pushed":"false","sha":"","branch":"x","reason":"r"}"#),
        r#"{"pushed": "false", "sha": "", "branch": "x", "reason": "r"}"#
    );
    assert_eq!(
        dumps(r#"{"skipped":true,"admitted":false}"#),
        r#"{"skipped": true, "admitted": false}"#
    );
    assert_eq!(
        dumps("{\"reason\":\"caf\u{e9}\"}"),
        "{\"reason\": \"caf\\u00e9\"}"
    );
}

// ------------------------------------------------------------ path identity

fn dec(s: &str) -> Result<Vec<u8>, Fail> {
    cjson::unb64(&Value::String(s.to_owned()))
}

#[test]
fn base64_vectors_match_python_unb64() {
    // Python: b64decode(validate=True), canonical re-encode, then literal().
    assert_eq!(dec("YQ==").unwrap(), b"a");
    assert_eq!(dec("YS9i").unwrap(), b"a/b");
    assert_eq!(dec("//79").unwrap(), b"\xff\xfe\xfd");
    for refused in [
        "",     // decodes to b'' -> literal() refuses empty
        "Lg==", // '.'
        "L2E=", // absolute '/a'
        "QR==", // non-canonical final character
        "===",  // leading padding
        "Q",    // bad length
        "QQ",   // missing padding
        "QQ=",  // bad padding
        "-_8=", // URL-safe alphabet
        "QQ==\n", "QQ ==", "!!!!",
    ] {
        assert!(dec(refused).is_err(), "{refused:?} accepted");
    }
}

#[test]
fn base64_encode_matches_python_b64() {
    assert_eq!(cjson::b64(b"\xff\xfe"), "//4=");
    assert_eq!(cjson::b64(b"a/\xff"), "YS//");
}

#[test]
fn distinct_non_utf8_paths_stay_distinct_identities() {
    let a = cjson::b64(b"d/\xff");
    let b = cjson::b64(b"d/\xfe");
    assert_ne!(a, b);
    assert_eq!(dec(&a).unwrap(), b"d/\xff");
    assert_eq!(dec(&b).unwrap(), b"d/\xfe");
}

#[test]
fn literal_paths_refuse_dot_components_and_nul() {
    for raw in [b"a//b".as_slice(), b"a/./b", b"a/../b", b"a/", b"a\0b"] {
        assert!(dec(&cjson::b64(raw)).is_err(), "{raw:?} accepted");
    }
}

// ------------------------------------------------------------ observations

#[test]
fn observations_are_written_only_after_admission() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = super::Ctx::default();
    // Before admission there is no private directory: nothing is written.
    super::authority::observation(&ctx, &json!({"boundary": "refusal"})).unwrap();
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    // After admission the refusal is appended to operations.jsonl.
    ctx.observation_directory = Some(tmp.path().to_path_buf());
    super::authority::observation(&ctx, &json!({"boundary": "refusal", "reason": "r"})).unwrap();
    let log = std::fs::read(tmp.path().join("operations.jsonl")).unwrap();
    assert_eq!(log, b"{\"boundary\":\"refusal\",\"reason\":\"r\"}\n");
}

#[test]
fn an_observation_write_error_is_reported_to_the_caller_only() {
    // run() ignores this error; the refusal keeps status 2. The missing
    // parent lies inside this test's own temporary directory.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = super::Ctx {
        observation_directory: Some(tmp.path().join("missing-parent/checkpoint")),
        ..Default::default()
    };
    assert!(super::authority::observation(&ctx, &json!({})).is_err());
}
