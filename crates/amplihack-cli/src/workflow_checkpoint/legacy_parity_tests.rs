//! Legacy-parity unit tests for the checkpoint engine (#1554, owner rule D1
//! and the 918c parity rule).
//!
//! The oracle is the Python at 918c256cf. Every expected value below was
//! recorded on 2026-11-07 by running the 918c fragment under `python3 -I`
//! outside the repository (see
//! `crates/amplihack-cli/tests/fixtures/workflow_checkpoint/README.md`):
//!
//! * `ORIGIN_REF_PY`, `workflow_pr_head_sync.sh:83-92`, SHA-256
//!   e4fbb280abb1693c1c9e263bd70b076cca4220e2219368f72fd993c28f0b6472;
//! * `unb64` and `literal`, `workflow_publish_checkpoint.sh:57-69`.
//!
//! The 918c parser returns 78 only for a malformed body; 0 and 3 both mean
//! "parsed". These tests therefore assert parse acceptance or rejection and
//! leave the status mapping to the binary-level tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::advert::{AdvertMap, lines};
use super::json;
use crate::command_error::exit_code;

const NO_FINAL_LF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/workflow_checkpoint/legacy_parser/no-final-lf.advert"
));
const CRLF: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/workflow_checkpoint/legacy_parser/crlf.advert"
));
const MIXED_OID_WIDTHS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/workflow_checkpoint/legacy_parser/mixed-oid-widths.advert"
));

fn oid(c: u8, width: usize) -> Vec<u8> {
    vec![c; width]
}

fn rec(oid_bytes: &[u8], name: &[u8], end: &[u8]) -> Vec<u8> {
    let mut out = oid_bytes.to_vec();
    out.push(b'\t');
    out.extend_from_slice(name);
    out.extend_from_slice(end);
    out
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

// ------------------------------------------------------------------ D1

#[test]
fn d1_fixture_bytes_are_the_packet_bytes() {
    // A checkout that normalised line endings would hollow out the D1 rows.
    assert_eq!(NO_FINAL_LF.len(), 56);
    assert!(!NO_FINAL_LF.ends_with(b"\n"));
    assert_eq!(CRLF.len(), 58);
    assert!(CRLF.ends_with(b"\r\n"));
    assert_eq!(MIXED_OID_WIDTHS.len(), 132);
    assert_eq!(&MIXED_OID_WIDTHS[..64], &oid(b'a', 64)[..]);
    assert_eq!(&MIXED_OID_WIDTHS[78..118], &oid(b'b', 40)[..]);
}

#[test]
fn d1_advertisement_without_a_final_line_feed_is_accepted() {
    let map = AdvertMap::parse(NO_FINAL_LF, 0).expect("918c accepts no final LF (exit 0)");
    assert_eq!(map.oids().count(), 1);
}

#[test]
fn d1_crlf_advertisement_is_accepted() {
    let map = AdvertMap::parse(CRLF, 0).expect("918c accepts a CRLF record (exit 0)");
    assert_eq!(map.oids().count(), 1);
}

#[test]
fn d1_mixed_40_and_64_digit_object_ids_are_accepted() {
    let map = AdvertMap::parse(MIXED_OID_WIDTHS, 0)
        .expect("918c checks each OID on its own, so mixed widths exit 0");
    assert_eq!(map.oids().count(), 2);
}

// -------------------------------------------------- legacy grammar table

/// (case, bytes, transport rc, 918c accepts the body).
fn grammar_rows() -> Vec<(&'static str, Vec<u8>, i32, bool)> {
    let a = oid(b'a', 40);
    let b = oid(b'b', 64);
    let c = oid(b'c', 40);
    vec![
        ("empty_rc0", Vec::new(), 0, true),
        ("empty_rc2", Vec::new(), 2, true),
        ("present", rec(&a, b"refs/heads/x", b"\n"), 0, true),
        ("head_only", rec(&a, b"HEAD", b"\n"), 0, true),
        ("sha256_width", rec(&b, b"refs/heads/x", b"\n"), 0, true),
        (
            "rc2_with_records",
            rec(&a, b"refs/heads/x", b"\n"),
            2,
            false,
        ),
        // The shell rejects any other transport status before parsing; the
        // 918c parser itself only singles out rc == 2.
        (
            "rc5_is_not_the_parsers_concern",
            rec(&a, b"refs/heads/x", b"\n"),
            5,
            true,
        ),
        (
            "blank_interior_line",
            cat(&[
                rec(&a, b"refs/heads/x", b"\n\n"),
                rec(&c, b"refs/heads/y", b"\n"),
            ]),
            0,
            false,
        ),
        (
            "trailing_double_lf",
            rec(&a, b"refs/heads/x", b"\n\n"),
            0,
            false,
        ),
        (
            "lone_cr_separates_two_records",
            cat(&[
                rec(&a, b"refs/heads/a", b"\r"),
                rec(&c, b"refs/heads/b", b"\n"),
            ]),
            0,
            true,
        ),
        (
            "lone_cr_leaves_a_one_field_record",
            cat(&[rec(&a, b"refs/heads/a", b"\r"), b"junk\n".to_vec()]),
            0,
            false,
        ),
        (
            "crlf_then_lf",
            cat(&[
                rec(&a, b"refs/heads/x", b"\r\n"),
                rec(&c, b"refs/heads/y", b"\n"),
            ]),
            0,
            true,
        ),
        (
            "three_fields",
            rec(&a, b"refs/heads/x\textra", b"\n"),
            0,
            false,
        ),
        ("one_field", cat(&[a.clone(), b"\n".to_vec()]), 0, false),
        (
            "space_separator",
            cat(&[a.clone(), b" refs/heads/x\n".to_vec()]),
            0,
            false,
        ),
        ("empty_oid", rec(b"", b"refs/heads/x", b"\n"), 0, false),
        ("bad_prefix", rec(&a, b"heads/x", b"\n"), 0, false),
        (
            "uppercase_oid",
            rec(&oid(b'A', 40), b"refs/heads/x", b"\n"),
            0,
            false,
        ),
        (
            "oid_39_digits",
            rec(&oid(b'a', 39), b"refs/heads/x", b"\n"),
            0,
            false,
        ),
        (
            "oid_41_digits",
            rec(&oid(b'a', 41), b"refs/heads/x", b"\n"),
            0,
            false,
        ),
        (
            "duplicate_ref_same_oid",
            cat(&[
                rec(&a, b"refs/heads/x", b"\n"),
                rec(&a, b"refs/heads/x", b"\n"),
            ]),
            0,
            false,
        ),
        (
            "duplicate_ref_different_oid",
            cat(&[
                rec(&a, b"refs/heads/x", b"\n"),
                rec(&c, b"refs/heads/x", b"\n"),
            ]),
            0,
            false,
        ),
        (
            "nul_inside_ref_is_kept",
            rec(&a, b"refs/heads/x\0z", b"\n"),
            0,
            true,
        ),
        (
            "vt_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x0bb", b"\n"),
            0,
            true,
        ),
        (
            "ff_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x0cb", b"\n"),
            0,
            true,
        ),
        (
            "fs_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x1cb", b"\n"),
            0,
            true,
        ),
        (
            "gs_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x1db", b"\n"),
            0,
            true,
        ),
        (
            "rs_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x1eb", b"\n"),
            0,
            true,
        ),
        (
            "nel_inside_ref_is_kept",
            rec(&a, b"refs/heads/a\x85b", b"\n"),
            0,
            true,
        ),
        (
            "non_utf8_ref_is_kept",
            rec(&a, b"refs/heads/\xff\xfe", b"\n"),
            0,
            true,
        ),
    ]
}

#[test]
fn advertisement_grammar_matches_the_918c_parser_row_by_row() {
    let mut wrong = Vec::new();
    for (case, raw, rc, accepted) in grammar_rows() {
        let got = AdvertMap::parse(&raw, rc).is_ok();
        if got != accepted {
            wrong.push(format!(
                "{case}: 918c accepted={accepted}, Rust accepted={got}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "parser drift from 918c:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn vertical_tab_does_not_split_a_ref_so_the_prefix_is_absent() {
    // 918c: target refs/heads/a against "refs/heads/a\vb" exits 3.
    let raw = rec(&oid(b'a', 40), b"refs/heads/a\x0bb", b"\n");
    let map = AdvertMap::parse(&raw, 0).expect("accepted");
    assert_eq!(map.oids().count(), 1, "\\v must stay inside the field");
}

// ------------------------------------------------- bytes.splitlines parity

#[test]
fn splitlines_splits_only_on_lf_cr_and_crlf() {
    assert_eq!(
        lines(b"a\nb\rc\r\nd"),
        vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]
    );
    for sep in [&b"\x0b"[..], b"\x0c", b"\x1c", b"\x1d", b"\x1e", b"\x85"] {
        let raw = [&b"x"[..], sep, b"y\n"].concat();
        assert_eq!(
            lines(&raw),
            vec![[&b"x"[..], sep, b"y"].concat()],
            "{sep:?} split a line"
        );
    }
}

#[test]
fn splitlines_keeps_interior_and_trailing_empty_records() {
    assert_eq!(
        lines(b"a\n\nb\n"),
        vec![b"a".to_vec(), Vec::new(), b"b".to_vec()]
    );
    assert_eq!(lines(b"a\n\n"), vec![b"a".to_vec(), Vec::new()]);
    assert_eq!(lines(b"a\r\r\n"), vec![b"a".to_vec(), Vec::new()]);
    assert!(lines(b"").is_empty());
}

// ------------------------------------------------------------ crash status

#[test]
fn a_panic_maps_to_status_1_like_an_uncaught_python_exception() {
    // 918c: an exception outside CHECKPOINT_PY's except list, or any error in
    // ORIGIN_REF_PY, ends the interpreter with status 1. 17.126 does not
    // authorise 78 for a crash.
    let outcome = super::catch_panic::<()>("legacy-parity-probe", || {
        panic!("deliberate test panic");
    });
    let error = outcome.expect_err("a panic must become a coded error");
    assert_eq!(exit_code(&error), Some(1), "panic status must be 1, not 78");
}

// -------------------------------------------------------------- JSON reader

#[test]
fn json_refuses_duplicate_keys_at_every_depth() {
    for raw in [
        &br#"{"a":1,"a":1}"#[..],
        br#"{"a":{"b":1,"b":2}}"#,
        br#"{"a":[{"c":1,"c":1}]}"#,
        br#"[{"x":{"y":{"z":0,"z":0}}}]"#,
    ] {
        assert!(
            json::parse(raw).is_err(),
            "{}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn json_refuses_non_finite_constants() {
    for raw in [
        &b"NaN"[..],
        b"Infinity",
        b"-Infinity",
        br#"{"a":NaN}"#,
        br#"[1e400]"#,
    ] {
        assert!(
            json::parse(raw).is_err(),
            "{}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn json_accepts_distinct_keys_in_sibling_objects() {
    assert!(json::parse(br#"{"a":{"k":1},"b":{"k":2}}"#).is_ok());
}

// ------------------------------------------------------------ base64 paths

fn unb64(text: &str) -> Result<Vec<u8>, ()> {
    json::unb64(&serde_json::Value::String(text.to_owned())).map_err(|_| ())
}

#[test]
fn base64_valid_paths_decode_to_raw_bytes() {
    // Recorded from 918c unb64.
    let rows: [(&str, &[u8]); 7] = [
        ("YQ==", b"a"),
        ("LWE=", b"-a"),
        ("YQpi", b"a\nb"),
        ("YWJj", b"abc"),
        ("gA==", b"\x80"),
        ("YS9i", b"a/b"),
        ("4pyT", b"\xe2\x9c\x93"),
    ];
    for (text, raw) in rows {
        assert_eq!(unb64(text).as_deref(), Ok(raw), "{text}");
    }
}

#[test]
fn base64_malformed_or_non_literal_paths_are_refused() {
    // Each is a refusal (status 2) at 918c: binascii.Error is a ValueError,
    // and the re-encode and literal checks raise Refusal.
    for text in [
        "YQ", "YQ==\n", "Y Q==", "YR==", "", "L2E=", "YQBi", "YS8vYg==", "Li4=", "Lg==",
        "YS8uLi9i", "YS8=", "YQ==YQ==", "YQ=", "Y===", "_w==", "-w==",
    ] {
        assert!(unb64(text).is_err(), "{text:?} must be refused");
    }
}

#[test]
fn base64_non_string_encodings_are_refused() {
    for value in [
        serde_json::json!(1),
        serde_json::json!(null),
        serde_json::json!(["YQ=="]),
    ] {
        assert!(json::unb64(&value).is_err(), "{value}");
    }
}

#[test]
fn base64_round_trip_is_the_standard_padded_alphabet() {
    for raw in [&b"a"[..], b"-a", b"a\nb", b"\xff\xfe/x", b"dir/file.rs"] {
        let text = json::b64(raw);
        assert!(
            text.len().is_multiple_of(4) && !text.contains(['-', '_']),
            "{text}"
        );
        assert_eq!(unb64(&text).as_deref(), Ok(raw));
    }
}

// ----------------------------------------------------------- numeric bytes

/// (value, Python `json.dumps(value)`), recorded with CPython 3.14 `-I`.
const FLOAT_ROWS: [(f64, &str); 21] = [
    (1e-05, "1e-05"),
    (1e-06, "1e-06"),
    (1e-07, "1e-07"),
    (0.0001, "0.0001"),
    (0.001, "0.001"),
    (1e16, "1e+16"),
    (1e22, "1e+22"),
    (1.5, "1.5"),
    (100.0, "100.0"),
    (123_456_789_012_345.0, "123456789012345.0"),
    (1_234_567_890_123_456.0, "1234567890123456.0"),
    (12_345_678_901_234_567.0, "1.2345678901234568e+16"),
    (0.1, "0.1"),
    (2.5e-05, "2.5e-05"),
    (-1e-05, "-1e-05"),
    (-0.0, "-0.0"),
    (1.797_693_134_862_315_7e308, "1.7976931348623157e+308"),
    (5e-324, "5e-324"),
    (std::f64::consts::PI, "3.141592653589793"),
    (1e15, "1000000000000000.0"),
    (0.5e-4, "5e-05"),
];

#[test]
fn floats_are_written_as_python_repr() {
    for (value, python) in FLOAT_ROWS {
        assert_eq!(json::py_float_repr(value), python, "{value:e}");
    }
}

#[test]
fn a_numeric_branch_name_is_written_as_python_json_dumps() {
    // ADMISSION_RESULT_PY echoes branch_name through json.loads/json.dumps.
    let rows: [(&str, &str); 9] = [
        (r#"{"branch_name": 1e-5}"#, "1e-05"),
        (r#"{"branch_name": 1e-6}"#, "1e-06"),
        (r#"{"branch_name": 1e-7}"#, "1e-07"),
        (r#"{"branch_name": 1E16}"#, "1e+16"),
        (r#"{"branch_name": 100.0}"#, "100.0"),
        (r#"{"branch_name": -0}"#, "0"),
        (r#"{"branch_name": NaN}"#, "NaN"),
        (r#"{"branch_name": -1e400}"#, "-Infinity"),
        (
            r#"{"branch_name": 12345678901234567890123}"#,
            "12345678901234567890123",
        ),
    ];
    for (raw, python) in rows {
        let value = json::py_loads(&json::surrogateescape(raw.as_bytes())).unwrap();
        assert_eq!(
            json::py_dumps(value.get("branch_name").unwrap()),
            python,
            "{raw}"
        );
    }
}

#[test]
fn checkpoint_records_write_floats_as_python_serial() {
    // 918c `serial` is json.dumps(sort_keys, (',', ':')): floats use repr.
    let value = json::parse(br#"{"b":1e-7,"a":[1.0,2]}"#).unwrap();
    assert_eq!(json::serial(&value), br#"{"a":[1.0,2],"b":1e-07}"#);
}

#[test]
fn argv_bytes_are_written_as_python_surrogateescape_json() {
    assert_eq!(json::py_dumps_os(b"a\xffb\xe9"), r#""a\udcffb\udce9""#);
    assert_eq!(json::py_dumps_os("caf\u{e9}".as_bytes()), "\"caf\\u00e9\"");
}

#[test]
fn print_of_a_json_value_is_python_str() {
    let rows: [(&str, &str); 10] = [
        (r#""x""#, "x"),
        ("5", "5"),
        ("1e-5", "1e-05"),
        ("null", "None"),
        ("true", "True"),
        (r#"[1,"a"]"#, "[1, 'a']"),
        (r#"{"k":"v","a":1}"#, "{'k': 'v', 'a': 1}"),
        (r#""it's""#, "it's"),
        (r#"["it's"]"#, r#"["it's"]"#),
        (r#""a\nb""#, "a\nb"),
    ];
    for (raw, python) in rows {
        let value = json::py_loads(&json::surrogateescape(raw.as_bytes())).unwrap();
        assert_eq!(json::py_print(&value).unwrap(), python.as_bytes(), "{raw}");
    }
    // print() of a lone surrogate raised UnicodeEncodeError.
    let lone = json::py_loads(&json::surrogateescape(br#""\ud800""#)).unwrap();
    assert_eq!(json::py_print(&lone), None);
}

#[test]
fn python_int_accepts_what_int_accepts() {
    use super::actions::py_int;
    for (text, value) in [
        ("0", 0),
        (" 0 ", 0),
        ("-0", 0),
        ("+7", 7),
        ("1_0", 10),
        ("\t79\n", 79),
    ] {
        assert_eq!(py_int(text).ok(), Some(value), "{text:?}");
    }
    for text in ["", "x", "1__0", "_1", "1_", "0x10", "1.0", "- 1"] {
        assert!(py_int(text).is_err(), "{text:?}");
    }
}
