//! Parity oracle for the Rust `PhaseProjection` once it stops using the
//! `regex` and `base64` crates (#1554, PR #2020, C-q, decision A1 option c).
//!
//! `tests/integration/checkpoint_projection.rs` is also compiled into the
//! `amplihack-workflows` test binary (`canonical_staging_checkpoint_test`),
//! whose manifest has neither crate and must not change. The module therefore
//! replaces its four regex sites with string scanners and its base64 engine
//! with a small codec of its own. This file keeps the original patterns and
//! the original engine configuration as reference implementations (the
//! `cli_integration` binary already depends on both crates) and requires the
//! module to produce the same results and the same refusals, byte for byte.
//!
//! Contract:
//!
//! * `p::b64_encode(&[u8]) -> String`: the standard alphabet with padding,
//!   as `base64::engine::general_purpose::STANDARD.encode`.
//! * `p::b64_decode(&[u8]) -> Result<Vec<u8>, String>`: CPython
//!   `b64decode(validate=True)`: standard alphabet, canonical padding
//!   required, non-zero trailing bits accepted. `Err` holds the `Display`
//!   text of the `base64::DecodeError` the old engine returned, so the
//!   `binascii.Error: …` message the projection reports is unchanged.
//! * `p::render_bytes`, `p::blocks`, `p::try_scalar`, `p::literal`: the same
//!   results as the regex versions below on every input, including the edge
//!   cases (`{` without a closing brace, nested braces, CRLF, a last line
//!   without LF, `\x0b`, `\x0c`, U+0085, multi-byte UTF-8, an ID spanning a
//!   newline, regex metacharacters in a key).
//!
//! No test here panics on hostile input: a panic in the module fails the test.

use base64::Engine as _;
use regex::Regex;
use std::sync::OnceLock;

use crate::checkpoint_projection as p;
use crate::checkpoint_projection::Ctx;

/// The Python `\s` class (`str.isspace`), exactly as the module spelled it.
const PY_WS_CLASS: &str = r"[\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]";

// ------------------------------------------------- reference implementations

fn ref_template_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{([^{}]+)\}\}").expect("template regex"))
}

fn ref_block_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r#"(?m)^  - id: "([^"]+)"{PY_WS_CLASS}*$"#)).expect("block regex")
    })
}

fn ref_template_bytes(value: &Ctx) -> Vec<u8> {
    if matches!(value, Ctx::Dict(_) | Ctx::List(_) | Ctx::Bool(_)) {
        p::py_json_dumps(value).into_bytes()
    } else {
        p::py_str_bytes(value)
    }
}

fn ref_lookup<'a>(ctx: &'a Ctx, key: &str) -> Option<&'a Ctx> {
    match ctx {
        Ctx::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn ref_render_bytes(template: &str, ctx: &Ctx) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(template.len());
    let mut last = 0;
    for caps in ref_template_regex().captures_iter(template) {
        let whole = caps.get(0).expect("group 0");
        let inner = caps.get(1).expect("group 1").as_str();
        out.extend_from_slice(&template.as_bytes()[last..whole.start()]);
        let mut value = ctx;
        for part in p::py_strip(inner).split('.') {
            match ref_lookup(value, part) {
                Some(next) => value = next,
                None => return Err(format!("Unbound fixture template: {inner}")),
            }
        }
        out.extend_from_slice(&ref_template_bytes(value));
        last = whole.end();
    }
    out.extend_from_slice(&template.as_bytes()[last..]);
    Ok(out)
}

fn ref_blocks(text: &str) -> Vec<(String, String)> {
    let matches: Vec<(usize, String)> = ref_block_regex()
        .captures_iter(text)
        .map(|c| {
            (
                c.get(0).expect("group 0").start(),
                c.get(1).expect("group 1").as_str().to_owned(),
            )
        })
        .collect();
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, (start, id)) in matches.iter().enumerate() {
        let end = matches.get(i + 1).map_or(text.len(), |(next, _)| *next);
        let body = text[*start..end].to_owned();
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == id) {
            slot.1 = body;
        } else {
            out.push((id.clone(), body));
        }
    }
    out
}

fn ref_try_scalar(block: &str, key: &str) -> Result<Option<Ctx>, String> {
    let pattern = format!(r"(?m)^    {}: (.*?){PY_WS_CLASS}*$", regex::escape(key));
    let re = Regex::new(&pattern).map_err(|e| format!("ValueError: scalar pattern: {e}"))?;
    let Some(caps) = re.captures(block) else {
        return Ok(None);
    };
    let raw = caps.get(1).map_or("", |m| m.as_str());
    match raw {
        "true" => return Ok(Some(Ctx::Bool(true))),
        "false" => return Ok(Some(Ctx::Bool(false))),
        _ => {}
    }
    if raw.starts_with('"') {
        return p::py_json_loads(raw.as_bytes())
            .map(Some)
            .map_err(|e| format!("json.decoder.JSONDecodeError: {e}"));
    }
    Ok(Some(Ctx::Str(raw.trim_matches('\'').to_owned())))
}

fn ref_literal(block: &str, key: &str) -> Result<String, String> {
    let pattern = format!(
        r"(?m)^    {}: \|\n((?:(?: {{6,}}.*)?\n)*)",
        regex::escape(key)
    );
    let re = Regex::new(&pattern).map_err(|e| format!("ValueError: literal pattern: {e}"))?;
    match re.captures(block) {
        Some(caps) => Ok(p::py_dedent(caps.get(1).map_or("", |m| m.as_str()))),
        None => Err(format!("A canonical literal {key} block is required.")),
    }
}

/// The engine the projection used before the rewrite.
fn ref_engine() -> base64::engine::GeneralPurpose {
    base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireCanonical),
    )
}

fn ref_b64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    ref_engine().decode(input).map_err(|e| e.to_string())
}

fn ref_b64_encode(input: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(input)
}

// ------------------------------------------------------------------ inputs

/// Whitespace code points from the class, plus near misses outside it.
const WS_PROBES: [&str; 12] = [
    "\t", "\x0b", "\x0c", "\r", "\x1c", "\x1f", " ", "\u{85}", "\u{a0}", "\u{3000}",
    "\u{200b}", // ZERO WIDTH SPACE: not Python whitespace
    "\x1b",     // ESC: not Python whitespace
];

fn ctx() -> Ctx {
    Ctx::Dict(vec![
        ("a".into(), Ctx::Str("A\u{e9}".into())),
        ("b".into(), Ctx::Str("B".into())),
        ("flag".into(), Ctx::Bool(true)),
        ("n".into(), Ctx::Int(-7)),
        (
            "nested".into(),
            Ctx::Dict(vec![
                ("b".into(), Ctx::Str("x y".into())),
                ("list".into(), Ctx::List(vec![Ctx::Int(1), Ctx::Null])),
            ]),
        ),
        ("{".into(), Ctx::Str("brace".into())),
        ("\u{1F600}".into(), Ctx::Str("emoji".into())),
    ])
}

fn template_inputs() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        "plain text",
        "{{a}}",
        "{{ a }}",
        "{{a}}{{b}}",
        "{{a}}-{{b}}",
        "{{{a}}}",
        "{{a}}}",
        "{{{{a}}",
        "{{{{a}}}}",
        "x{{{{a}}}}y",
        "{{a",
        "a}}",
        "{a}",
        "{{}}",
        "{{ }}",
        "{{a{b}}",
        "{{a}b}}",
        "{{a}}{{",
        "}}{{a}}{{",
        "{{\na\n}}",
        "{{\ta\t}}",
        "\u{e9}{{a}}\u{fc}",
        "{{\u{1F600}}}",
        "{{a}}\u{85}{{b}}",
        "{{a.b}}",
        "{{nested}}",
        "{{nested.b}}",
        "{{nested.list}}",
        "{{nested.missing}}",
        "{{nested.b.c}}",
        "{{flag}}",
        "{{n}}",
        "{{missing}}",
        "{{.}}",
        "{{a.}}",
        "{{ \u{3000}a\u{3000} }}",
        "{{\u{200b}a}}",
        "{{a}}{{missing}}",
        "{{{}}}",
        "{{{{}}}}",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for ws in WS_PROBES {
        out.push(format!("{{{{{ws}a{ws}}}}}"));
    }
    out
}

fn block_texts() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        "name: fixture\n",
        "  - id: \"first\"\n    type: bash\n  - id: \"second\"\n    agent: x\n",
        "  - id: \"first\"",
        "  - id: \"first\"\n",
        "  - id: \"first\"\r\n    type: bash\r\n",
        "steps:\n  - id: \"a\"\n  - id: \"b\"\n  - id: \"a\"\n    later: 1\n",
        "  - id: 'single'\n  - id: \"double\"\n",
        "  - id: \"\"\n",
        "  - id: \"a\" x\n",
        " - id: \"a\"\n",
        "   - id: \"a\"\n",
        "\t- id: \"a\"\n",
        "x  - id: \"a\"\n",
        "  - id: \"a\nb\"\n",
        "  - id: \"a\"\"\n",
        "  - id: \"\u{e9}t\u{e9}\"\n  - id: \"\u{1F600}\"\n",
        "\r  - id: \"a\"\n",
        "\u{85}  - id: \"a\"\n",
        "  - id: \"a\"\u{85}  - id: \"b\"\n",
        "  - id: \"a\"\u{2028}\n",
        "  - id:  \"a\"\n",
        "  - id: \"a\"\n\n\n",
        "\n\n  - id: \"a\"",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for ws in WS_PROBES {
        out.push(format!(
            "  - id: \"w\"{ws}\n    k: v\n  - id: \"z\"{ws}{ws}"
        ));
    }
    out
}

fn scalar_blocks() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        "    key: value\n",
        "    key: value",
        "    key: value\r\n",
        "    key: value \t\n",
        "    key: \"json \\u00e9\"\n",
        "    key: \"broken\n",
        "    key: 'single'\n",
        "    key: ''\n",
        "    key: '\n",
        "    key: true\n",
        "    key: true  \n",
        "    key: false\r\n",
        "    key: True\n",
        "    key:value\n",
        "    key: \n",
        "    key:  x\n",
        "    key: a b \u{3000}\n",
        "    key: a\u{85}b\n",
        "    key: a\u{85}\n",
        "     key: indented\n",
        "   key: short\n",
        "\tkey: tab\n",
        "    key: first\n    key: second\n",
        "    other: x\n    key: y\n",
        "    keyx: no\n    key: yes\n",
        "    a.b: dotted\n",
        "    aXb: not-dotted\n",
        "    a+b: plus\n",
        "    (k): parens\n",
        "    \u{e9}: accented\n",
        "    key: \"a\" trailing\n",
        "    key: 1\n",
        "    key: -\n",
        "    k\nv: split\n",
        "    k\n    k\nv: indented\n",
        "    k\nvx: no\n",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for ws in WS_PROBES {
        out.push(format!("    key: v{ws}\n"));
        out.push(format!("    key: v{ws}"));
        out.push(format!("    key: {ws}v\n"));
    }
    out
}

/// `"k\nv"` holds a newline: the escaped regex matched it literally, so a
/// key may span two lines of the block, and the scanner must agree.
const SCALAR_KEYS: [&str; 9] = [
    "key", "other", "a.b", "a+b", "(k)", "\u{e9}", "missing", "", "k\nv",
];

fn literal_blocks() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        "    command: |\n      echo one\n        indented\n\n      echo two\n    note: after\n",
        "    command: |\n      echo one",
        "    command: |\n      echo one\n",
        "    command: |\n",
        "    command: |",
        "    command: |  \n      echo\n",
        "    command: |\r\n      echo\r\n",
        "    command: |\n      echo\r\n      two\r\n",
        "    command: |\n     five\n",
        "    command: |\n\techo\n",
        "    command: |\n      a\n     b\n      c\n",
        "    command: |\n\n\n      late\n",
        "    command: |\n      \u{e9}\u{1F600}\n",
        "    command: |\n      a\u{85}b\n",
        "    command: |\n            deep\n      shallow\n",
        "    command: >\n      folded\n",
        "    command: |\n      first\n    command: |\n      second\n",
        "    a.b: |\n      dotted\n",
        "    aXb: |\n      not\n",
        "     command: |\n      x\n",
        "    k\nv: |\n      split\n",
        "    k\n    k\nv: |\n      split\n",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for ws in WS_PROBES {
        out.push(format!("    command: |\n      a{ws}\n      {ws}\n"));
    }
    out
}

const LITERAL_KEYS: [&str; 6] = ["command", "a.b", "missing", "", "c+", "k\nv"];

/// Every string of length 0..=max over `alphabet`.
fn all_strings(alphabet: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut out = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for _ in 0..max {
        let mut next = Vec::with_capacity(frontier.len() * alphabet.len());
        for prefix in &frontier {
            for &b in alphabet {
                let mut s = prefix.clone();
                s.push(b);
                next.push(s);
            }
        }
        out.extend(next.iter().cloned());
        frontier = next;
    }
    out
}

// -------------------------------------------------------------- the tests

#[test]
fn render_bytes_matches_the_template_regex_on_every_edge_case() {
    let ctx = ctx();
    let mut wrong = Vec::new();
    for template in template_inputs() {
        let expected = ref_render_bytes(&template, &ctx);
        let actual = p::render_bytes(&template, &ctx);
        if actual != expected {
            wrong.push(format!("{template:?}: {actual:?} != regex {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn render_bytes_matches_the_template_regex_on_every_short_brace_string() {
    // Every string up to length 7 over `{`, `}`, `a`, ` ` and `.`: overlapping,
    // unbalanced and nested braces, all left to right without rescanning.
    let ctx = Ctx::Dict(vec![
        ("a".into(), Ctx::Str("<A>".into())),
        ("a a".into(), Ctx::Str("<AA>".into())),
    ]);
    let mut wrong = Vec::new();
    for raw in all_strings(b"{}a .", 7) {
        let template = String::from_utf8(raw).expect("ascii");
        let expected = ref_render_bytes(&template, &ctx);
        let actual = p::render_bytes(&template, &ctx);
        if actual != expected {
            wrong.push(format!("{template:?}: {actual:?} != regex {expected:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} cases:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn blocks_matches_the_block_regex_on_every_edge_case() {
    let mut wrong = Vec::new();
    for text in block_texts() {
        let expected = ref_blocks(&text);
        let actual = p::blocks(&text);
        if actual != expected {
            wrong.push(format!("{text:?}: {actual:?} != regex {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn blocks_matches_the_block_regex_on_every_short_token_sequence() {
    // Every sequence of up to 6 tokens: line starts, IDs that cross a newline,
    // empty IDs, U+0085 (whitespace to the class but no line end), and
    // whitespace running onto the next line. `blocks` returns each ID with
    // the text from its match start to the next match start, so equal output
    // means equal IDs, equal spans and the same resume point after each match.
    const TOKENS: [&str; 6] = ["  - id: \"", "\"", "\n", " ", "\u{85}", "x"];
    let mut frontier = vec![String::new()];
    let mut texts = vec![String::new()];
    for _ in 0..6 {
        let mut next = Vec::with_capacity(frontier.len() * TOKENS.len());
        for prefix in &frontier {
            for token in TOKENS {
                next.push(format!("{prefix}{token}"));
            }
        }
        texts.extend(next.iter().cloned());
        frontier = next;
    }
    let mut wrong = Vec::new();
    for text in &texts {
        let expected = ref_blocks(text);
        let actual = p::blocks(text);
        if actual != expected {
            wrong.push(format!("{text:?}: {actual:?} != regex {expected:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} sequences differ:\n{}",
        wrong.len(),
        texts.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn blocks_keeps_an_id_that_spans_a_newline_as_the_regex_did() {
    // `[^"]+` crosses a newline; the scanner must too, or a block is lost.
    let text = "  - id: \"a\nb\"\n    type: bash\n";
    assert_eq!(ref_blocks(text), vec![("a\nb".to_owned(), text.to_owned())]);
    assert_eq!(p::blocks(text), ref_blocks(text));
}

#[test]
fn blocks_does_not_start_a_line_after_u0085() {
    // `$` under `(?m)` matches only before `\n` or at the end, so a U+0085 after the
    // ID is whitespace to the class but starts no new line for `^`, and the
    // following text is not a line end for `$`.
    let text = "  - id: \"a\"\u{85}  - id: \"b\"\n";
    assert_eq!(p::blocks(text), ref_blocks(text));
    assert_eq!(ref_blocks(text).len(), 0, "regex oracle: no block matches");
}

#[test]
fn try_scalar_matches_the_scalar_regex_for_every_key_and_block() {
    let mut wrong = Vec::new();
    for block in scalar_blocks() {
        for key in SCALAR_KEYS {
            let expected = ref_try_scalar(&block, key);
            let actual = p::try_scalar(&block, key);
            if actual != expected {
                wrong.push(format!(
                    "{key:?} in {block:?}: {actual:?} != regex {expected:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn try_scalar_escapes_regex_metacharacters_in_the_key() {
    // `a.b` must not match `aXb`, as `regex::escape` guaranteed.
    let block = "    aXb: wrong\n    a.b: right\n";
    assert_eq!(
        ref_try_scalar(block, "a.b"),
        Ok(Some(Ctx::Str("right".into())))
    );
    assert_eq!(p::try_scalar(block, "a.b"), ref_try_scalar(block, "a.b"));
}

#[test]
fn try_scalar_trims_exactly_the_python_whitespace_class() {
    // ZERO WIDTH SPACE and ESC are not `str.isspace`; they stay in the value.
    for (block, value) in [
        ("    key: v\u{200b}\n", "v\u{200b}"),
        ("    key: v\x1b\n", "v\x1b"),
        ("    key: v\x0b\x0c\u{3000}\n", "v"),
        ("    key: v \r\n", "v"),
    ] {
        assert_eq!(
            ref_try_scalar(block, "key"),
            Ok(Some(Ctx::Str(value.into()))),
            "regex oracle for {block:?}"
        );
        assert_eq!(
            p::try_scalar(block, "key"),
            ref_try_scalar(block, "key"),
            "{block:?}"
        );
    }
}

#[test]
fn literal_matches_the_literal_regex_for_every_key_and_block() {
    let mut wrong = Vec::new();
    for block in literal_blocks() {
        for key in LITERAL_KEYS {
            let expected = ref_literal(&block, key);
            let actual = p::literal(&block, key);
            if actual != expected {
                wrong.push(format!(
                    "{key:?} in {block:?}: {actual:?} != regex {expected:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn literal_refuses_a_missing_block_with_the_python_message() {
    let block = "    other: |\n      x\n";
    assert_eq!(
        p::literal(block, "command"),
        Err("A canonical literal command block is required.".to_owned())
    );
}

#[test]
fn b64_encode_matches_the_standard_engine() {
    let mut inputs: Vec<Vec<u8>> = all_strings(&[0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff], 4);
    inputs.push((0..=255u8).collect());
    inputs.push(b"source.txt".to_vec());
    inputs.push("new source \u{e9}.txt".as_bytes().to_vec());
    inputs.push(vec![0xff; 1000]);
    let mut wrong = Vec::new();
    for input in inputs {
        let expected = ref_b64_encode(&input);
        let actual = p::b64_encode(&input);
        if actual != expected {
            wrong.push(format!("{input:?}: {actual:?} != {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn b64_decode_accepts_what_the_engine_accepts_and_returns_the_same_bytes() {
    for (input, bytes) in [
        ("", &b""[..]),
        ("Zg==", b"f"),
        ("Zm8=", b"fo"),
        ("Zm9v", b"foo"),
        ("Zm9vYg==", b"foob"),
        ("+/+/", &[0xfb, 0xff, 0xbf][..]),
        // Non-zero trailing bits: CPython accepts, and so did the engine.
        ("Zh==", b"f"),
        ("Zm9=", b"fo"),
    ] {
        assert_eq!(
            ref_b64_decode(input.as_bytes()),
            Ok(bytes.to_vec()),
            "oracle {input:?}"
        );
        assert_eq!(
            p::b64_decode(input.as_bytes()),
            Ok(bytes.to_vec()),
            "{input:?}"
        );
    }
}

#[test]
fn b64_decode_refuses_what_the_engine_refuses_with_the_same_message() {
    for input in [
        "Zg",
        "Zg=",
        "Zg===",
        "Z===",
        "Z",
        "ZZZZZ",
        "=Zm9",
        "Zm 9v",
        "Zm9v\n",
        "\nZm9v",
        "Zm-_",
        "Zm9v====",
        "Zg==Zg==",
        "Zm9vYg",
        "A===",
        "====",
        "Zm=v",
        "Zg=\n",
        "\u{e9}Zm9",
    ] {
        let expected = ref_b64_decode(input.as_bytes());
        assert!(expected.is_err(), "oracle accepts {input:?}");
        assert_eq!(p::b64_decode(input.as_bytes()), expected, "{input:?}");
    }
}

#[test]
fn b64_decode_matches_the_engine_on_every_short_input() {
    // Every input of length 0..=4 over a mix of alphabet, padding, URL-safe,
    // whitespace and non-ASCII bytes, plus every length-8 input built from a
    // valid quad and one of those tails: the results and refusal messages
    // must be identical, and nothing may panic.
    let alphabet = b"AQgw/+=-_\n \xc3";
    let mut inputs = all_strings(alphabet, 4);
    for tail in all_strings(b"AQ=+_", 4) {
        let mut s = b"Zm9v".to_vec();
        s.extend_from_slice(&tail);
        inputs.push(s);
    }
    let mut wrong = Vec::new();
    for input in &inputs {
        let expected = ref_b64_decode(input);
        let actual = p::b64_decode(input);
        if actual != expected {
            wrong.push(format!("{input:?}: {actual:?} != {expected:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} inputs differ:\n{}",
        wrong.len(),
        inputs.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn b64_round_trips_every_byte_string_up_to_three_bytes() {
    for input in all_strings(&[0x00, 0x3e, 0x3f, 0x80, 0xff], 3) {
        let encoded = p::b64_encode(&input);
        assert_eq!(
            p::b64_decode(encoded.as_bytes()),
            Ok(input.clone()),
            "{encoded}"
        );
    }
}
