//! Pure-function and process-contract tests for the Rust port of
//! `PhaseProjection` (#1554, PR #2020, consensus step C-n and C-o).
//!
//! These tests are written before the port. They fail to compile until
//! `tests/integration/checkpoint_projection.rs` exists with the surface below,
//! and then fail at run time until each function matches the Python reference
//! (`test-checkpoint-scope` at f14da87ed, lines 142-165 and 419-640).
//!
//! Surface under test (test code only, never linked into the binary):
//!
//! * `Ctx`: ordered context value. `Dict` is a `Vec`, so keys inserted after
//!   load keep Python's insertion order (the workspace `serde_json` is built
//!   without `preserve_order`). `Raw` holds projected stdout that is not
//!   valid UTF-8, as Python's `surrogateescape` decoding keeps it.
//! * `py_json_dumps`, `py_str`, `py_strip`: Python's default `json.dumps`,
//!   `str()` and `str.strip()` byte for byte.
//! * `projected_json_output`: the 4d5187e runner's direct, fenced and
//!   first-balanced JSON rules.
//! * `blocks`, `scalar`, `literal`, `index_source`, `index_source_bytes`: the
//!   regex block reader. No YAML parser.
//! * `render`: `{{a.b}}` substitution, JSON for dict, list and bool.
//! * `recipe_vars`: the `RECIPE_VAR_*` environment `sync_context` builds.
//! * `decode_projected_stdout`: surrogateescape decoding plus `str.strip`.
//! * `run_sequence`: bash blocks in order, stopping at the first non-zero
//!   block and returning its real status and bytes.
//! * `PhasePlan::from_texts`: new, reviewer, before and after step sets with
//!   the Python assertion messages.
//! * `write_projection_shim` and `PROJECTION_REQUEST_ENV`: the #1671 bridge.
//!
//! Every bash this file starts runs with `env_clear` and without `BASH_ENV`
//! or `ENV`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::checkpoint_projection as p;
use p::Ctx;

// ------------------------------------------------------------------ helpers

fn s(v: &str) -> Ctx {
    Ctx::Str(v.to_owned())
}

fn d(pairs: Vec<(&str, Ctx)>) -> Ctx {
    Ctx::Dict(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn json_ok(text: &str) -> Ctx {
    p::projected_json_output(text.as_bytes())
        .unwrap_or_else(|e| panic!("{text:?} must be accepted: {e}"))
}

fn json_refused(text: &str) {
    assert!(
        p::projected_json_output(text.as_bytes()).is_err(),
        "{text:?} must be refused"
    );
}

/// A hermetic environment for projected bash blocks: no inherited
/// variables, a private HOME and TMPDIR, and the Git isolation set.
fn hermetic_env(home: &Path) -> Vec<(OsString, OsString)> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"));
    vec![
        ("PATH".into(), path),
        ("HOME".into(), home.as_os_str().to_owned()),
        ("TMPDIR".into(), home.as_os_str().to_owned()),
        ("LC_ALL".into(), "C".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_ALLOW_PROTOCOL".into(), "file".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
    ]
}

// ------------------------------------------------------------ py_json_dumps

#[test]
fn py_json_dumps_uses_python_default_separators_and_insertion_order() {
    let value = d(vec![
        ("z", Ctx::Int(1)),
        (
            "a",
            Ctx::List(vec![Ctx::Int(2), Ctx::Null, Ctx::Bool(true)]),
        ),
        ("m", d(vec![])),
        ("n", Ctx::List(vec![])),
    ]);
    assert_eq!(
        p::py_json_dumps(&value),
        r#"{"z": 1, "a": [2, null, true], "m": {}, "n": []}"#
    );
}

#[test]
fn py_json_dumps_escapes_non_ascii_with_ensure_ascii_and_surrogate_pairs() {
    assert_eq!(
        p::py_json_dumps(&d(vec![("a", s("é"))])),
        "{\"a\": \"\\u00e9\"}"
    );
    assert_eq!(p::py_json_dumps(&s("\u{1F600}")), "\"\\ud83d\\ude00\"");
    assert_eq!(p::py_json_dumps(&s("\u{3000}x")), "\"\\u3000x\"");
}

#[test]
fn py_json_dumps_escapes_controls_quote_backslash_and_del_but_not_slash() {
    assert_eq!(
        p::py_json_dumps(&s("a\nb\t\"\\/\u{1}\u{7f}\r\u{8}\u{c}")),
        r#""a\nb\t\"\\/\u0001\u007f\r\b\f""#
    );
}

#[test]
fn py_json_dumps_keeps_non_utf8_raw_bytes_as_lone_low_surrogates() {
    // Python: b"\xc3\xa9\xff".decode(errors="surrogateescape") then
    // json.dumps gives the escaped e-acute followed by the escaped U+DCFF.
    assert_eq!(
        p::py_json_dumps(&Ctx::Raw(b"\xc3\xa9\xff".to_vec())),
        "\"\\u00e9\\udcff\""
    );
}

#[test]
fn py_json_dumps_formats_numbers_as_python_does() {
    let cases: [(Ctx, &str); 13] = [
        (Ctx::Float(1e-05), "1e-05"),
        (Ctx::Float(1e-06), "1e-06"),
        (Ctx::Float(1e-07), "1e-07"),
        (Ctx::Float(0.0001), "0.0001"),
        (Ctx::Float(1e16), "1e+16"),
        (Ctx::Float(100.0), "100.0"),
        (Ctx::Float(-0.0), "-0.0"),
        (Ctx::Float(1.5), "1.5"),
        (Ctx::Float(2.5e-05), "2.5e-05"),
        (Ctx::Float(1e15), "1000000000000000.0"),
        (
            Ctx::Float(18_446_744_073_709_551_616.0),
            "1.8446744073709552e+19",
        ),
        (Ctx::Int(18_446_744_073_709_551_615), "18446744073709551615"),
        (Ctx::Int(-9_223_372_036_854_775_808), "-9223372036854775808"),
    ];
    for (value, want) in cases {
        assert_eq!(p::py_json_dumps(&value), want, "{value:?}");
    }
}

#[test]
fn py_float_extremes_match_cpython_3_14() {
    // json.dumps and str() under CPython 3.14.10, the oracle's interpreter.
    let cases: [(f64, &str, &str); 5] = [
        (5e-324, "5e-324", "5e-324"),
        (
            1.7976931348623157e308,
            "1.7976931348623157e+308",
            "1.7976931348623157e+308",
        ),
        (f64::NAN, "NaN", "nan"),
        (f64::INFINITY, "Infinity", "inf"),
        (f64::NEG_INFINITY, "-Infinity", "-inf"),
    ];
    for (value, dumps, text) in cases {
        assert_eq!(p::py_json_dumps(&Ctx::Float(value)), dumps, "{value:?}");
        assert_eq!(p::py_str(&Ctx::Float(value)), text, "{value:?}");
    }
}

/// The body of `fn py_repr_float` in `rel`, without comment lines.
fn py_repr_float_body(rel: &str) -> String {
    let src = fs::read_to_string(root().join(rel)).unwrap();
    let start = src
        .find("fn py_repr_float(value: f64) -> String {")
        .unwrap_or_else(|| panic!("{rel} has no py_repr_float"));
    let body = &src[start..];
    let end = body.find("\n}\n").expect("end of py_repr_float") + 3;
    body[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn py_repr_float_copy_agrees_with_the_engine() {
    // The engine's `num::py_repr_float` is `pub(crate)`, so the projection
    // keeps a copy. The copy must stay the same code, not drift into a
    // second implementation.
    assert_eq!(
        py_repr_float_body("tests/integration/checkpoint_projection.rs"),
        py_repr_float_body("crates/amplihack-cli/src/workflow_checkpoint/num.rs"),
        "checkpoint_projection::py_repr_float differs from the engine's num::py_repr_float"
    );
    // repr(float) is the shortest text that reads back as the same float.
    // The xorshift seed is fixed, so every run checks the same 20,000 values;
    // a failure names the seed and the value's bits.
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut state = SEED;
    for _ in 0..20_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if !value.is_finite() {
            continue;
        }
        let text = p::py_repr_float(value);
        let bits = state;
        let back: f64 = text
            .parse()
            .unwrap_or_else(|e| panic!("{text}: {e} (seed {SEED:#x}, bits {bits:#x})"));
        assert_eq!(
            back.to_bits(),
            bits,
            "{text} (seed {SEED:#x}, bits {bits:#x})"
        );
    }
}

// ------------------------------------------------------------------- py_str

#[test]
fn py_str_matches_python_str_for_scalars() {
    let cases: [(Ctx, &str); 10] = [
        (Ctx::Bool(true), "True"),
        (Ctx::Bool(false), "False"),
        (Ctx::Null, "None"),
        (Ctx::Int(-3), "-3"),
        (Ctx::Float(1e-05), "1e-05"),
        (Ctx::Float(1.0), "1.0"),
        (Ctx::Float(-0.0), "-0.0"),
        (Ctx::Float(1e16), "1e+16"),
        (s("plain é"), "plain é"),
        (s(""), ""),
    ];
    for (value, want) in cases {
        assert_eq!(p::py_str(&value), want, "{value:?}");
    }
}

// ----------------------------------------------------------------- py_strip

#[test]
fn py_strip_removes_python_unicode_whitespace_including_u001c_to_u001f() {
    for ws in [
        '\t', '\n', '\u{b}', '\u{c}', '\r', '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}', ' ', '\u{85}',
        '\u{a0}', '\u{1680}', '\u{2000}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}',
    ] {
        let text = format!("{ws}{ws}value{ws}");
        assert_eq!(p::py_strip(&text), "value", "U+{:04X}", u32::from(ws));
    }
}

#[test]
fn py_strip_keeps_characters_python_does_not_treat_as_space() {
    for kept in ['\u{200b}', '\u{feff}', '\u{0}', '\u{180e}'] {
        let text = format!("{kept}v{kept}");
        assert_eq!(p::py_strip(&text), text, "U+{:04X}", u32::from(kept));
    }
}

// ---------------------------------------------------- decode_projected_stdout

#[test]
fn projected_stdout_that_is_utf8_is_stripped_to_a_string() {
    assert_eq!(
        p::decode_projected_stdout(b"\x1c\n  ok \xc2\x85\n"),
        s("ok")
    );
}

#[test]
fn projected_stdout_that_is_not_utf8_is_kept_as_raw_bytes() {
    // A lone 0x85 byte becomes U+DC85 under surrogateescape, which is not
    // whitespace, so it is not stripped; the ASCII newline is.
    assert_eq!(
        p::decode_projected_stdout(b"\x85ok\xff\n"),
        Ctx::Raw(b"\x85ok\xff".to_vec())
    );
}

// ---------------------------------------------------- projected_json_output

// case: amplifier-bundle/recipes/tests/test-checkpoint-scope.py::ProjectionJSONTests.test_direct_and_original_hygiene_prefix_preserve_exact_result
#[test]
fn projection_json_direct_and_hygiene_prefix_preserve_exact_result() {
    let want = d(vec![
        ("pushed", s("true")),
        ("sha", s(&"a".repeat(40))),
        ("branch", s("feature")),
        ("reason", s("already-pushed")),
    ]);
    let body = format!(
        r#"{{"pushed": "true", "sha": "{}", "branch": "feature", "reason": "already-pushed"}}"#,
        "a".repeat(40)
    );
    for prefix in ["", "Artifact check clean.\nArtifact check clean.\n"] {
        assert_eq!(json_ok(&format!("{prefix}{body}")), want, "{prefix:?}");
    }
}

// case: amplifier-bundle/recipes/tests/test-checkpoint-scope.py::ProjectionJSONTests.test_fenced_and_balanced_outputs_preserve_nested_and_quoted_data
#[test]
fn projection_json_fenced_and_balanced_outputs_preserve_nested_and_quoted_data() {
    let want = d(vec![(
        "nested",
        Ctx::List(vec![
            Ctx::Int(1),
            d(vec![("message", s(r#"a quoted "}" remains data"#))]),
        ]),
    )]);
    let body = r#"{"nested": [1, {"message": "a quoted \"}\" remains data"}]}"#;
    for text in [
        format!("```json\n{body}\n```"),
        format!("```\n{body}\n```"),
        format!("Info {body} done"),
    ] {
        assert_eq!(json_ok(&text), want, "{text:?}");
    }
}

// case: amplifier-bundle/recipes/tests/test-checkpoint-scope.py::ProjectionJSONTests.test_empty_truncated_and_malformed_outputs_remain_refused
#[test]
fn projection_json_empty_truncated_and_malformed_outputs_remain_refused() {
    for text in [
        "",
        "Artifact check clean.\n",
        r#"{"pushed":"#,
        "prefix {broken} then {}",
        "NaN",
        "Infinity",
        "-Infinity",
        r#"{"value": 1e400}"#,
        r#"{"value": NaN}"#,
    ] {
        json_refused(text);
    }
}

// case: amplifier-bundle/recipes/tests/test-checkpoint-scope.py::ProjectionJSONTests.test_unicode_pairs_pass_and_lone_surrogates_remain_refused
#[test]
fn projection_json_unicode_pairs_pass_and_lone_surrogates_remain_refused() {
    assert_eq!(
        json_ok("{\"value\":\"\\ud83d\\ude00\"}"),
        d(vec![("value", s("\u{1F600}"))])
    );
    for invalid in [
        r#"{"value":"\ud800"}"#,
        r#"{"value":"\udc00"}"#,
        r#"{"\ud800":"value"}"#,
        r#"{"nested":["\udc00"]}"#,
    ] {
        for text in [
            invalid.to_owned(),
            format!("Information.\n{invalid}"),
            format!("```json\n{invalid}\n```"),
        ] {
            json_refused(&text);
        }
    }
}

// case: amplifier-bundle/recipes/tests/test-checkpoint-scope.py::ProjectionJSONTests.test_duplicate_values_are_validated_before_last_key_wins
#[test]
fn projection_json_duplicate_values_are_validated_before_last_key_wins() {
    for invalid in [
        r#"{"value":"\ud800","value":"valid"}"#,
        r#"{"value":{"nested":"\ud800"},"value":"valid"}"#,
    ] {
        json_refused(invalid);
    }
    assert_eq!(
        json_ok(r#"{"value":"first","value":"valid"}"#),
        d(vec![("value", s("valid"))])
    );
    assert_eq!(
        json_ok(r#"{"value":"\\ud800"}"#),
        d(vec![("value", s(r"\ud800"))])
    );
}

#[test]
fn projection_json_duplicate_key_keeps_the_first_position_and_the_last_value() {
    // Python dict(pairs): the key stays where it first appeared.
    assert_eq!(
        json_ok(r#"{"a":1,"b":2,"a":3}"#),
        d(vec![("a", Ctx::Int(3)), ("b", Ctx::Int(2))])
    );
}

#[test]
fn projection_json_integers_in_i64_to_u64_range_stay_integers_else_finite_floats() {
    assert_eq!(
        json_ok("9223372036854775807"),
        Ctx::Int(9_223_372_036_854_775_807)
    );
    assert_eq!(
        json_ok("-9223372036854775808"),
        Ctx::Int(-9_223_372_036_854_775_808)
    );
    assert_eq!(
        json_ok("18446744073709551615"),
        Ctx::Int(18_446_744_073_709_551_615)
    );
    assert_eq!(
        json_ok("18446744073709551616"),
        Ctx::Float(18_446_744_073_709_551_616.0)
    );
    assert_eq!(
        json_ok("-9223372036854775809"),
        Ctx::Float(-9_223_372_036_854_775_809.0)
    );
    json_refused(&"9".repeat(400));
}

#[test]
fn projection_json_accepts_scalars_and_strips_surrounding_whitespace() {
    assert_eq!(json_ok(" \n5\n "), Ctx::Int(5));
    assert_eq!(json_ok("\u{3000}true\u{1c}"), Ctx::Bool(true));
    assert_eq!(json_ok("1e-05"), Ctx::Float(1e-05));
}

#[test]
fn projection_json_refuses_raw_non_utf8_inside_a_string() {
    assert!(p::projected_json_output(b"{\"a\":\"\xff\"}").is_err());
}

#[test]
fn projection_json_balanced_scan_falls_back_from_objects_to_arrays() {
    // The first balanced object fails to decode, so the scan breaks out of
    // objects and tries the first balanced array.
    assert_eq!(
        json_ok("note {not json} then [1, 2]"),
        Ctx::List(vec![Ctx::Int(1), Ctx::Int(2)])
    );
}

// ------------------------------------------------------------------- render

fn render_ctx() -> Ctx {
    d(vec![
        (
            "task_description",
            s("$(touch PWNED) `id` it's\nmultiline é"),
        ),
        (
            "worktree_setup",
            d(vec![
                ("worktree_path", s("/w")),
                ("branch_name", s("task/x")),
            ]),
        ),
        ("flag", Ctx::Bool(true)),
        ("nothing", Ctx::Null),
        ("count", Ctx::Int(5)),
        ("ratio", Ctx::Float(1e-05)),
        ("items", Ctx::List(vec![s("é"), Ctx::Int(1)])),
    ])
}

#[test]
fn render_substitutes_dotted_paths_and_strips_inner_whitespace() {
    let ctx = render_ctx();
    assert_eq!(
        p::render(
            "cd {{ worktree_setup.worktree_path }} && git switch {{worktree_setup.branch_name}}",
            &ctx
        )
        .unwrap(),
        "cd /w && git switch task/x"
    );
}

#[test]
fn render_uses_json_for_dict_list_and_bool_and_str_for_the_rest() {
    let ctx = render_ctx();
    let cases = [
        (
            "{{worktree_setup}}",
            r#"{"worktree_path": "/w", "branch_name": "task/x"}"#,
        ),
        ("{{items}}", "[\"\\u00e9\", 1]"),
        ("{{flag}}", "true"),
        ("{{nothing}}", "None"),
        ("{{count}}", "5"),
        ("{{ratio}}", "1e-05"),
    ];
    for (template, want) in cases {
        assert_eq!(p::render(template, &ctx).unwrap(), want, "{template}");
    }
}

#[test]
fn render_is_purely_textual_for_shell_metacharacters() {
    // T3: no quoting is added and nothing is executed while rendering.
    let ctx = render_ctx();
    assert_eq!(
        p::render("echo {{task_description}}", &ctx).unwrap(),
        "echo $(touch PWNED) `id` it's\nmultiline é"
    );
}

#[test]
fn render_refuses_an_unbound_key_with_the_python_message() {
    let ctx = render_ctx();
    assert_eq!(
        p::render("x {{missing}} y", &ctx).unwrap_err(),
        "Unbound fixture template: missing"
    );
    // The message keeps the unstripped expression, as Python's match[1] does.
    assert_eq!(
        p::render("{{ missing.key }}", &ctx).unwrap_err(),
        "Unbound fixture template:  missing.key "
    );
    // A path through a non-dict value is unbound too.
    assert_eq!(
        p::render("{{count.value}}", &ctx).unwrap_err(),
        "Unbound fixture template: count.value"
    );
}

#[test]
fn render_leaves_text_the_template_regex_does_not_match() {
    let ctx = render_ctx();
    assert_eq!(
        p::render("{{}} {x} ${HOME}", &ctx).unwrap(),
        "{{}} {x} ${HOME}"
    );
    assert_eq!(p::render("{{{count}}}", &ctx).unwrap(), "{5}");
}

// -------------------------------------------------------------- recipe_vars

#[test]
fn recipe_vars_follow_sync_context_encoding_and_insertion_order() {
    let ctx = d(vec![
        ("zeta", s("z")),
        ("alpha", Ctx::Bool(false)),
        ("setup", d(vec![("b", Ctx::Int(1)), ("a", s("é"))])),
        ("none", Ctx::Null),
        ("ratio", Ctx::Float(1e-06)),
        ("raw", Ctx::Raw(b"a\xffb".to_vec())),
    ]);
    let vars = p::recipe_vars(&ctx);
    let names: Vec<_> = vars
        .iter()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "RECIPE_VAR_zeta",
            "RECIPE_VAR_alpha",
            "RECIPE_VAR_setup",
            "RECIPE_VAR_none",
            "RECIPE_VAR_ratio",
            "RECIPE_VAR_raw"
        ]
    );
    let values: Vec<&[u8]> = vars.iter().map(|(_, v)| v.as_bytes()).collect();
    assert_eq!(values[0], b"z");
    assert_eq!(values[1], b"false");
    assert_eq!(values[2], b"{\"b\": 1, \"a\": \"\\u00e9\"}");
    assert_eq!(values[3], b"None");
    assert_eq!(values[4], b"1e-06");
    assert_eq!(
        values[5], b"a\xffb",
        "raw bytes round-trip through OsString"
    );
}

// ------------------------------------------------- blocks, scalar, literal

const RECIPE: &str = "name: fixture\nsteps:\n  - id: \"first\"  \n    type: bash\n    output: \"captured_\\u00e9\"\n    parse_json: true\n    command: |\n      echo one\n        indented\n\n      echo two\n    note: after\n  - id: 'single-quoted-is-not-a-step'\n  - id: \"second\"\n    agent: 'amplihack:reviewer'\n    nested:\n      type: hidden\n";

#[test]
fn blocks_split_on_the_two_space_double_quoted_id_layout_in_order() {
    let found = p::blocks(RECIPE);
    let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["first", "second"]);
    assert!(found[0].1.starts_with("  - id: \"first\""));
    assert!(found[0].1.contains("single-quoted-is-not-a-step"));
    assert!(found[1].1.ends_with("      type: hidden\n"));
    assert!(p::blocks("steps: []\n").is_empty());
}

#[test]
fn scalar_reads_booleans_json_strings_and_quote_stripped_text() {
    let found = p::blocks(RECIPE);
    let first = &found[0].1;
    let second = &found[1].1;
    assert_eq!(p::scalar(first, "type"), Some(s("bash")));
    assert_eq!(p::scalar(first, "parse_json"), Some(Ctx::Bool(true)));
    assert_eq!(p::scalar(first, "output"), Some(s("captured_é")));
    assert_eq!(p::scalar(second, "agent"), Some(s("amplihack:reviewer")));
    // Six-space keys belong to nested maps and are not scalars of the step.
    assert_eq!(p::scalar(second, "type"), None);
    assert_eq!(p::scalar(first, "absent"), None);
}

#[test]
fn literal_dedents_like_textwrap_and_ends_at_a_shallower_line() {
    let first = &p::blocks(RECIPE)[0].1;
    assert_eq!(
        p::literal(first, "command").unwrap(),
        "echo one\n  indented\n\necho two\n"
    );
}

#[test]
fn literal_refuses_a_missing_block_with_the_python_message() {
    let second = &p::blocks(RECIPE)[1].1;
    assert_eq!(
        p::literal(second, "command").unwrap_err(),
        "A canonical literal command block is required."
    );
}

// ------------------------------------------------------------- index_source

#[test]
fn index_source_reads_the_hash_bound_retained_asset() {
    let text = p::index_source(&root(), "amplifier-bundle/recipes/workflow-publish.yaml")
        .expect("retained pre-delta asset");
    assert!(!p::blocks(&text).is_empty());
}

#[test]
fn index_source_refuses_a_changed_fixture_file() {
    let path = root().join(
        "crates/amplihack-workflows/tests/fixtures/canonical_staging/checkpoint-pre-delta-assets.json",
    );
    let mut raw = fs::read(path).unwrap();
    raw.push(b'\n');
    assert_eq!(
        p::index_source_bytes(&raw, "amplifier-bundle/recipes/workflow-publish.yaml").unwrap_err(),
        "The retained pre-delta asset fixture changed."
    );
}

// --------------------------------------------------------------- PhasePlan

fn step(id: &str, extra: &str) -> String {
    format!("  - id: \"{id}\"\n{extra}")
}

fn bash_step(id: &str) -> String {
    step(id, "    type: bash\n    command: |\n      true\n")
}

fn agent_step(id: &str) -> String {
    step(
        id,
        "    agent: \"amplihack:reviewer\"\n    output: \"verdict\"\n",
    )
}

fn recipe(steps: &[String]) -> String {
    format!("name: fixture\nsteps:\n{}", steps.concat())
}

#[test]
fn phase_plan_finds_new_steps_the_one_reviewer_and_the_before_and_after_sets() {
    let original = recipe(&[bash_step("old"), bash_step("ckpt")]);
    let current = recipe(&[
        bash_step("old"),
        bash_step("capture"),
        agent_step("review"),
        bash_step("post"),
        bash_step("ckpt"),
    ]);
    let plan = p::PhasePlan::from_texts(&current, &original, "ckpt").unwrap();
    assert_eq!(plan.new, ["capture", "review", "post"]);
    assert_eq!(plan.reviewer, "review");
    assert_eq!(plan.before, ["capture"]);
    assert_eq!(plan.after, ["post"]);
}

#[test]
fn phase_plan_refuses_zero_or_two_new_reviewers() {
    let original = recipe(&[bash_step("ckpt")]);
    let none = recipe(&[bash_step("capture"), bash_step("ckpt")]);
    let two = recipe(&[
        bash_step("capture"),
        agent_step("r1"),
        agent_step("r2"),
        bash_step("ckpt"),
    ]);
    for current in [none, two] {
        let err = p::PhasePlan::from_texts(&current, &original, "ckpt").unwrap_err();
        assert!(
            err.contains("Each phase needs exactly one new read-only scope reviewer."),
            "{err}"
        );
    }
}

#[test]
fn phase_plan_refuses_a_reviewer_after_the_checkpoint() {
    let original = recipe(&[bash_step("ckpt")]);
    let current = recipe(&[
        bash_step("capture"),
        bash_step("ckpt"),
        agent_step("review"),
    ]);
    assert!(p::PhasePlan::from_texts(&current, &original, "ckpt").is_err());
}

#[test]
fn phase_plan_refuses_an_empty_capture_set() {
    let original = recipe(&[bash_step("ckpt")]);
    let current = recipe(&[agent_step("review"), bash_step("ckpt")]);
    let err = p::PhasePlan::from_texts(&current, &original, "ckpt").unwrap_err();
    assert!(
        err.contains("A deterministic capture must precede the scope reviewer."),
        "{err}"
    );
}

/// A layout change in a canonical recipe must fail here, not silently
/// change what the projection runs.
#[test]
fn canonical_recipes_split_into_the_expected_phase_steps() {
    let phases = [
        ("workflow-publish", "step-15-commit-push"),
        ("workflow-pr-review", "step-18c-push-feedback-changes"),
        ("workflow-finalize", "step-20b-push-cleanup"),
    ];
    for (asset, checkpoint) in phases {
        let rel = format!("amplifier-bundle/recipes/{asset}.yaml");
        let current = fs::read_to_string(root().join(&rel)).unwrap();
        let original = p::index_source(&root(), &rel).unwrap();
        assert!(
            p::blocks(&current).iter().any(|(id, _)| id == checkpoint),
            "{asset}: {checkpoint} is not a block"
        );
        let plan = p::PhasePlan::from_texts(&current, &original, checkpoint)
            .unwrap_or_else(|e| panic!("{asset}: {e}"));
        assert!(!plan.before.is_empty(), "{asset}");
        for sid in plan.before.iter().chain(&plan.after) {
            let block = &p::blocks(&current)
                .into_iter()
                .find(|(id, _)| id == sid)
                .unwrap()
                .1;
            assert_eq!(p::scalar(block, "type"), Some(s("bash")), "{asset}::{sid}");
            p::literal(block, "command").unwrap_or_else(|e| panic!("{asset}::{sid}: {e}"));
        }
    }
}

// ------------------------------------------------------------ run_sequence

#[test]
fn run_sequence_stops_at_the_first_failing_block_and_returns_its_real_status() {
    let dir = tempfile::tempdir().unwrap();
    let bodies = [
        "printf first; : > ran-1".to_owned(),
        "printf 'out\\377'; printf 'err-79' >&2; exit 79".to_owned(),
        ": > ran-3".to_owned(),
    ];
    let result = p::run_sequence(&bodies, dir.path(), &hermetic_env(dir.path()));
    assert_eq!(result.rc, 79);
    assert_eq!(result.stdout, b"out\xff", "only the failing block's stdout");
    assert_eq!(result.stderr, b"err-79");
    assert!(dir.path().join("ran-1").exists());
    assert!(
        !dir.path().join("ran-3").exists(),
        "no block after a failure runs"
    );
}

#[test]
fn run_sequence_returns_the_last_block_when_every_block_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let bodies = ["printf a".to_owned(), "printf b; printf e >&2".to_owned()];
    let result = p::run_sequence(&bodies, dir.path(), &hermetic_env(dir.path()));
    assert_eq!(
        (
            result.rc,
            result.stdout.as_slice(),
            result.stderr.as_slice()
        ),
        (0, &b"b"[..], &b"e"[..])
    );
}

#[test]
fn run_sequence_runs_in_the_task_directory_with_only_the_given_environment() {
    let dir = tempfile::tempdir().unwrap();
    let bodies = ["pwd -P; printf '%s' \"${CARGO_MANIFEST_DIR-unset}\"".to_owned()];
    let result = p::run_sequence(&bodies, dir.path(), &hermetic_env(dir.path()));
    let want = format!("{}\nunset", fs::canonicalize(dir.path()).unwrap().display());
    assert_eq!(String::from_utf8(result.stdout).unwrap(), want);
}

#[test]
fn shell_result_converts_to_the_owner_project_output_shape() {
    let out: Output = p::ShellResult {
        rc: 79,
        stdout: b"o".to_vec(),
        stderr: b"e".to_vec(),
    }
    .into();
    assert_eq!(out.status.code(), Some(79));
    assert_eq!((out.stdout, out.stderr), (b"o".to_vec(), b"e".to_vec()));
}

// ------------------------------------------------- #1671 re-entry shim

/// The bash these shim tests write into the `#!` line, resolved from the
/// test's own PATH. The shim is Bash 3.2 compatible, so any bash will do.
fn unit_bash() -> PathBuf {
    let env = vec![(
        OsString::from("PATH"),
        std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin")),
    )];
    p::resolve_program("bash", &env, Path::new("/")).expect("bash on PATH")
}

/// A stand-in for the test binary: sources `BASH_ENV` and `ENV` if they
/// reached it (as a careless re-entry would), records its arguments and
/// environment, then writes the files the real re-entry test would write.
fn fake_test_binary(dir: &Path, write: &str) -> PathBuf {
    let path = dir.join("fake-test-binary");
    fs::write(
        &path,
        format!(
            "#!{bash}\nset -eu\nif [ -n \"${{BASH_ENV:-}}\" ]; then . \"$BASH_ENV\"; fi\nif [ -n \"${{ENV:-}}\" ]; then . \"$ENV\"; fi\nprintf '%s\\n' \"$@\" > \"{args}\"\nenv > \"{env}\"\nreq=\"${{{var}:?}}\"\n{write}\n",
            bash = unit_bash().display(),
            args = dir.join("args").display(),
            env = dir.join("env").display(),
            var = p::PROJECTION_REQUEST_ENV,
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The shim's own directory, where its request directories are created.
fn shim_dir(dir: &Path) -> PathBuf {
    let shim = dir.join("shim");
    fs::create_dir_all(&shim).unwrap();
    shim
}

/// Run the shim as the #1671 driver does: under `env -u BASH_ENV -u ENV`.
/// `extra` stands for variables inherited from the driver's environment.
fn run_shim(dir: &Path, write: &str, extra: &[(&str, &OsStr)]) -> Output {
    run_shim_as(dir, write, extra, true)
}

fn run_shim_as(dir: &Path, write: &str, extra: &[(&str, &OsStr)], driver: bool) -> Output {
    let binary = fake_test_binary(dir, write);
    let shim = p::write_projection_shim(
        &shim_dir(dir),
        &unit_bash(),
        &binary,
        "issue_1671::phase_projection_reentry",
    );
    let case = dir.join("case");
    fs::create_dir_all(&case).unwrap();
    let mut command = if driver {
        let mut c = Command::new("env");
        c.args(["-u", "BASH_ENV", "-u", "ENV"]).arg(&shim);
        c
    } else {
        Command::new(&shim)
    };
    command
        .env_clear()
        .envs(hermetic_env(dir))
        .envs(extra.iter().copied())
        .stdin(Stdio::null())
        .arg(root())
        .arg(&case);
    command.output().expect("run projection shim")
}

fn entries(dir: &Path) -> Vec<OsString> {
    let mut names: Vec<OsString> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn projection_shim_passes_through_bytes_and_status_from_the_reentry_test() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shim(
        dir.path(),
        "printf 'OUT\\377' > \"$req/out.bin\"; printf 'ERR' > \"$req/err.bin\"; printf 79 > \"$req/rc\"; printf 'libtest noise\\n'",
        &[],
    );
    assert_eq!(out.status.code(), Some(79));
    assert_eq!(
        out.stdout, b"OUT\xff",
        "no libtest output in the projected stream"
    );
    assert_eq!(out.stderr, b"ERR");
    let args = fs::read_to_string(dir.path().join("args")).unwrap();
    for want in ["--exact", "issue_1671::phase_projection_reentry", "--quiet"] {
        assert!(
            args.lines().any(|l| l == want),
            "{want} missing from {args:?}"
        );
    }
    // A passing run removes its request directory; nothing is ever written
    // into the case directory the driver inspects.
    assert!(entries(&dir.path().join("case")).is_empty());
    assert_eq!(
        entries(&dir.path().join("shim")),
        vec![OsString::from("projection-shim.sh")]
    );
}

#[test]
fn projection_shim_request_carries_only_the_two_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shim(
        dir.path(),
        "cp \"$req/request.json\" \"$req/../../request.copy\"; : > \"$req/out.bin\"; : > \"$req/err.bin\"; printf 0 > \"$req/rc\"",
        &[],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let request: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join("request.copy")).unwrap()).unwrap();
    let keys: Vec<&String> = request.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 3, "{request}");
    assert_eq!(request["version"], serde_json::json!(2));
    let hex = |raw: &[u8]| raw.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert_eq!(request["root_hex"], hex(root().as_os_str().as_bytes()));
    assert_eq!(
        request["dir_hex"],
        hex(dir.path().join("case").as_os_str().as_bytes())
    );
}

#[test]
fn projection_shim_fails_closed_without_out_bin_and_names_its_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_shim(dir.path(), "printf 0 > \"$req/rc\"", &[]);
    assert_eq!(out.status.code(), Some(70));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    let kept = entries(&dir.path().join("shim"))
        .into_iter()
        .find(|n| n.as_bytes().starts_with(b"projection-request."))
        .expect("the failed request directory is kept");
    let kept = dir.path().join("shim").join(kept);
    assert!(
        stderr.contains(&format!("evidence in {}", kept.display())),
        "{stderr}"
    );
    assert!(kept.join("request.json").is_file());
    assert!(entries(&dir.path().join("case")).is_empty());
}

#[test]
fn projection_shim_fails_closed_on_a_malformed_or_empty_rc() {
    for rc in ["7x", "", "-1", " 0", "256", "1000"] {
        let dir = tempfile::tempdir().unwrap();
        let write = format!(
            "printf ok > \"$req/out.bin\"; : > \"$req/err.bin\"; printf '%s' '{rc}' > \"$req/rc\""
        );
        let out = run_shim(dir.path(), &write, &[]);
        assert_eq!(out.status.code(), Some(70), "rc {rc:?}");
        assert!(out.stdout.is_empty(), "rc {rc:?}");
    }
}

/// A hostile startup file, and proof that the fake binary really sources
/// `ENV` and `BASH_ENV` when they reach it, so the sentinel checks below
/// cannot pass vacuously.
fn hostile(dir: &Path) -> (PathBuf, PathBuf) {
    let sentinel = dir.join("hostile-ran");
    let hostile = dir.join("hostile.sh");
    fs::write(&hostile, format!(": > '{}'\n", sentinel.display())).unwrap();
    (hostile, sentinel)
}

#[test]
fn fake_test_binary_sources_a_hostile_env_and_bash_env_when_given_them() {
    for name in ["ENV", "BASH_ENV"] {
        let dir = tempfile::tempdir().unwrap();
        let (file, sentinel) = hostile(dir.path());
        let binary = fake_test_binary(dir.path(), ": > \"$req/out.bin\"");
        let req = dir.path().join("req");
        fs::create_dir(&req).unwrap();
        let status = Command::new(&binary)
            .env_clear()
            .envs(hermetic_env(dir.path()))
            .env(name, &file)
            .env(p::PROJECTION_REQUEST_ENV, &req)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(sentinel.exists(), "the fake binary must source {name}");
    }
}

#[test]
fn projection_shim_under_the_driver_runs_no_hostile_bash_env_or_env() {
    let dir = tempfile::tempdir().unwrap();
    let (file, sentinel) = hostile(dir.path());
    let out = run_shim(
        dir.path(),
        "printf ok > \"$req/out.bin\"; : > \"$req/err.bin\"; printf 0 > \"$req/rc\"",
        &[("BASH_ENV", file.as_os_str()), ("ENV", file.as_os_str())],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = fs::read(dir.path().join("env")).unwrap();
    for line in env.split(|b| *b == b'\n') {
        assert!(
            !line.starts_with(b"BASH_ENV="),
            "BASH_ENV reached the re-entry"
        );
        assert!(!line.starts_with(b"ENV="), "ENV reached the re-entry");
    }
    assert!(!sentinel.exists(), "a hostile startup file ran");
}

#[test]
fn projection_shim_started_directly_still_withholds_env_from_the_reentry() {
    // Non-interactive bash does not read ENV, so a direct start shows the
    // shim's own `unset` is what keeps ENV from the re-entry.
    let dir = tempfile::tempdir().unwrap();
    let (file, sentinel) = hostile(dir.path());
    let out = run_shim_as(
        dir.path(),
        "printf ok > \"$req/out.bin\"; : > \"$req/err.bin\"; printf 0 > \"$req/rc\"",
        &[("ENV", file.as_os_str())],
        false,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!sentinel.exists(), "a hostile ENV file ran");
}

#[test]
fn reentry_request_variable_is_named_as_designed() {
    assert_eq!(p::PROJECTION_REQUEST_ENV, "AMPLIHACK_PROJECTION_REQUEST");
}

// ------------------------------------------------------------------ sha256

#[test]
fn sha256_matches_the_fips_vectors() {
    let cases: [(&[u8], &str); 4] = [
        (
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            &[b'a'; 1000],
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(p::sha256_hex(input), want, "{} bytes", input.len());
    }
}

// ------------------------------------------- Owner::project uses the port

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

const FIXTURE: &str = "tests/integration/canonical_staging_checkpoint_fixture.rs";
const PROJECTION: &str = "tests/integration/checkpoint_projection.rs";

#[test]
fn owner_project_no_longer_embeds_or_spawns_the_python_projection() {
    let src = read(FIXTURE);
    for banned in [
        "const PROJECTION",
        "python3",
        "importlib",
        "PYTHONDONTWRITEBYTECODE",
        "test-checkpoint-scope",
    ] {
        assert!(!src.contains(banned), "{FIXTURE} still contains {banned:?}");
    }
    assert!(
        src.contains("crate::checkpoint_projection::project(&request, &env)"),
        "{FIXTURE} must call the Rust projection"
    );
}

#[test]
fn rust_projection_uses_no_yaml_parser_and_no_python() {
    let src = read(PROJECTION);
    for banned in [
        "serde_yaml",
        "python3",
        "importlib",
        "amplihack_workflows::recipe",
    ] {
        assert!(!src.contains(banned), "{PROJECTION} contains {banned:?}");
    }
    // The block reader is the Python regex, with `\s` spelled out as the
    // Unicode whitespace class Python's `str` patterns use.
    assert!(src.contains(r##"r#"(?m)^  - id: "([^"]+)"{PY_WS_CLASS}*$"#"##));
    // The SHA-256 pin of the retained pre-delta asset is kept.
    assert!(src.contains("112ea7497a96d7d0257c409c74bb302df6693e6503770d01e63613783191d706"));
}

#[test]
fn every_owner_project_caller_still_calls_the_unchanged_entry_point() {
    // Every caller found at f14da87ed; each keeps calling OWNER.project.
    for caller in [
        "tests/integration/workflow_publish_resilience.rs",
        "tests/integration/issue_1714_finalization_handoff/step20b_remote.rs",
        "tests/integration/issue_978_step15_rebase_base_test.rs",
        "tests/integration/issue_1762_caller_branch_alias/push.rs",
        "tests/integration/artifact_guard_workflow_tests.rs",
    ] {
        assert!(read(caller).contains("OWNER.project("), "{caller}");
    }
    assert!(
        read(FIXTURE).contains("-> Output {"),
        "Owner::project keeps returning std::process::Output"
    );
}

#[test]
fn step20b_tests_keep_a_test_owned_tmpdir() {
    let src = read("tests/integration/issue_1714_finalization_handoff/step20b_remote.rs");
    assert!(src.contains(".env(\"TMPDIR\", self.tmpdir())"));
}

// ------------------------------------------------- #1671 driver and bridge

const DRIVER_1671: &str = "amplifier-bundle/recipes/tests/test-issue-1671-hook-rejection-repair.sh";
const BRIDGE_1671: &str = "tests/integration/issue_1671_hook_rejection_repair_test.rs";

#[test]
fn issue_1671_driver_runs_no_python() {
    let src = read(DRIVER_1671);
    for line in src.lines() {
        let code = line.split_once(" #").map_or(line, |(code, _)| code);
        if code.trim_start().starts_with('#') {
            continue;
        }
        assert!(
            !code.contains("python3 ") && !code.contains(" python3"),
            "{DRIVER_1671} still runs python3: {line}"
        );
    }
    // The python3 preflight became a jq, sed and cmp preflight that also
    // refuses a sed that is not GNU sed.
    assert!(src.contains("for tool in git jq sed cmp; do"));
    assert!(src.contains("GNU sed is required"));
    assert!(!src.contains("command -v python3"));
}

#[test]
fn issue_1671_driver_reaches_the_rust_projection_through_the_shim_only() {
    let src = read(DRIVER_1671);
    for banned in [
        "publish-projection",
        "importlib",
        "test-checkpoint-scope",
        "PY_PROJECTION",
    ] {
        assert!(
            !src.contains(banned),
            "{DRIVER_1671} still contains {banned:?}"
        );
    }
    assert!(
        src.contains("step_command=(env -u BASH_ENV -u ENV \"${PROJECTION_SHIM:?"),
        "step-15 must run through PROJECTION_SHIM without BASH_ENV or ENV"
    );
    assert!(
        src.contains("\" \"${REPO_ROOT}\" \"${dir}\")"),
        "the shim takes REPO_ROOT and the case directory"
    );
    // An unset shim is a refusal, never a fallback.
    assert!(
        src.contains("${PROJECTION_SHIM:?"),
        "the driver must refuse to run without PROJECTION_SHIM"
    );
}

#[test]
fn issue_1671_bridge_test_sets_the_shim_records_gnu_bash_and_clears_startup_files() {
    let src = read(BRIDGE_1671);
    for want in [
        "PROJECTION_SHIM",
        "phase_projection_reentry",
        "AMPLIHACK_PROJECTION_REQUEST",
        "current_exe",
        "--version",
        "env_remove(\"BASH_ENV\")",
        "env_remove(\"ENV\")",
    ] {
        assert!(src.contains(want), "{BRIDGE_1671} lacks {want:?}");
    }
}

#[test]
fn issue_1671_reentry_test_is_a_no_op_without_its_variable() {
    let src = read(BRIDGE_1671);
    let body = src
        .split_once("fn phase_projection_reentry()")
        .expect("phase_projection_reentry test")
        .1;
    let head: String = body.chars().take(600).collect();
    assert!(
        head.contains("AMPLIHACK_PROJECTION_REQUEST") && head.contains("return"),
        "the re-entry test must return early when its variable is unset"
    );
}
