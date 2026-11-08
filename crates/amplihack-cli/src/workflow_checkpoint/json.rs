//! Strict JSON input and canonical JSON output for checkpoint records.
//!
//! Records are hashed on their exact serialised bytes, so output must be one
//! canonical form: sorted keys, no whitespace, ASCII only. Input refuses
//! duplicate object keys, which a lenient parser would silently collapse.

use super::{Fail, refuse};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::fmt;

struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Strict {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if result.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON field"));
            }
            let value = map.next_value_seed(Strict)?;
            result.insert(key, value);
        }
        Ok(Value::Object(result))
    }
}

/// Parse one JSON document as 918c's `parse` did: its refusals (syntax,
/// duplicate keys, non-finite numbers, lone surrogates) carry CPython's text,
/// from [`super::json_errors`]. Non-UTF-8 input (17.138 item 1) and nesting
/// past that scanner's depth are refused by the value parser.
pub(crate) fn parse(raw: &[u8]) -> Result<Value, Fail> {
    if let Ok(text) = std::str::from_utf8(raw)
        && let super::json_errors::Diagnosis::Refused(reason) = super::json_errors::diagnose(text)
    {
        return Err(Fail::Refusal(reason));
    }
    let mut de = serde_json::Deserializer::from_slice(raw);
    let value = Strict
        .deserialize(&mut de)
        .and_then(|v| de.end().map(|()| v))
        .map_err(|e| Fail::Refusal(format!("invalid JSON: {e}")))?;
    Ok(value)
}

fn escape_into(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write(out: &mut String, value: &Value) {
    write_with(out, value, ",", ":", true);
}

fn write_with(out: &mut String, value: &Value, item: &str, key_sep: &str, sort: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => escape_into(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, element) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(item);
                }
                write_with(out, element, item, key_sep, sort);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            if sort {
                keys.sort();
            }
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(item);
                }
                escape_into(out, key);
                out.push_str(key_sep);
                write_with(out, &map[key], item, key_sep, sort);
            }
            out.push('}');
        }
    }
}

/// A JSON number as Python's `json.dumps` writes it: integers exactly,
/// floats with `float.__repr__` (`1e-05`, `1e+16`, `100.0`).
fn number(n: &Number) -> String {
    match (n.as_i64(), n.as_u64(), n.as_f64()) {
        (Some(i), _, _) => i.to_string(),
        (None, Some(u), _) => u.to_string(),
        (None, None, Some(f)) => super::num::py_repr_float(f),
        (None, None, None) => n.to_string(),
    }
}

/// A value as CPython's `json.loads` returns it, for the two snippets that
/// echoed step-04 JSON back (`ADMISSION_RESULT_PY`, the recipe
/// `worktree_path` extraction). Unlike [`parse`] it keeps what `json.loads`
/// keeps: duplicate keys (last value, first position), `NaN`/`Infinity`,
/// integers of any size, document key order and lone surrogates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PyValue {
    None,
    Bool(bool),
    /// Decimal digits with an optional leading `-`, kept exactly.
    Int(String),
    Float(f64),
    /// Code points; lone surrogates are allowed, as in a Python `str`.
    Str(Vec<u32>),
    List(Vec<PyValue>),
    Dict(Vec<(Vec<u32>, PyValue)>),
}

impl PyValue {
    /// `d.get(key)` on a dict; `None` for any other type.
    pub(crate) fn get(&self, key: &str) -> Option<&PyValue> {
        let key: Vec<u32> = key.chars().map(u32::from).collect();
        match self {
            Self::Dict(items) => items.iter().find(|(k, _)| *k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// `os.environ[...]` as Python sees it: UTF-8 with `surrogateescape`.
pub(crate) fn surrogateescape(raw: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for chunk in raw.utf8_chunks() {
        out.extend(chunk.valid().chars().map(u32::from));
        out.extend(chunk.invalid().iter().map(|b| 0xdc00 + u32::from(*b)));
    }
    out
}

struct PyParser<'a> {
    s: &'a [u32],
    i: usize,
}

const WS: [u32; 4] = [0x20, 0x09, 0x0a, 0x0d];

impl PyParser<'_> {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.i).copied()
    }
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| WS.contains(&c)) {
            self.i += 1;
        }
    }
    fn eat(&mut self, text: &str) -> bool {
        let want: Vec<u32> = text.chars().map(u32::from).collect();
        if self.s[self.i..].starts_with(&want) {
            self.i += want.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self, depth: usize) -> Option<PyValue> {
        if depth > 900 {
            return None;
        }
        match char::from_u32(self.peek()?)? {
            '{' => self.dict(depth),
            '[' => self.list(depth),
            '"' => self.string().map(PyValue::Str),
            'n' if self.eat("null") => Some(PyValue::None),
            't' if self.eat("true") => Some(PyValue::Bool(true)),
            'f' if self.eat("false") => Some(PyValue::Bool(false)),
            'N' if self.eat("NaN") => Some(PyValue::Float(f64::NAN)),
            'I' if self.eat("Infinity") => Some(PyValue::Float(f64::INFINITY)),
            '-' if self.eat("-Infinity") => Some(PyValue::Float(f64::NEG_INFINITY)),
            _ => self.number(),
        }
    }
    fn digits(&mut self) -> usize {
        let start = self.i;
        while self.peek().is_some_and(|c| (0x30..=0x39).contains(&c)) {
            self.i += 1;
        }
        self.i - start
    }
    fn number(&mut self) -> Option<PyValue> {
        let start = self.i;
        if self.peek() == Some(0x2d) {
            self.i += 1;
        }
        match self.peek() {
            Some(0x30) => self.i += 1,
            Some(c) if (0x31..=0x39).contains(&c) => {
                self.digits();
            }
            _ => return None,
        }
        let int_end = self.i;
        let mut float = false;
        if self.peek() == Some(0x2e)
            && self
                .s
                .get(self.i + 1)
                .is_some_and(|c| (0x30..=0x39).contains(c))
        {
            self.i += 1;
            self.digits();
            float = true;
        }
        if matches!(self.peek(), Some(0x65 | 0x45)) {
            let save = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(0x2b | 0x2d)) {
                self.i += 1;
            }
            if self.digits() == 0 {
                self.i = save;
            } else {
                float = true;
            }
        }
        let text: String = self.s[start..self.i]
            .iter()
            .filter_map(|c| char::from_u32(*c))
            .collect();
        if float {
            text.parse().ok().map(PyValue::Float)
        } else {
            let _ = int_end;
            let text = text
                .strip_prefix("-0")
                .map_or(text.clone(), |_| "0".to_owned());
            Some(PyValue::Int(text))
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let slice = self.s.get(self.i..self.i + 4)?;
        let text: String = slice.iter().filter_map(|c| char::from_u32(*c)).collect();
        if text.len() != 4 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.i += 4;
        u32::from_str_radix(&text, 16).ok()
    }
    fn string(&mut self) -> Option<Vec<u32>> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                0x22 => return Some(out),
                0x5c => {
                    let e = self.peek()?;
                    self.i += 1;
                    let unit = match char::from_u32(e)? {
                        '"' => 0x22,
                        '\\' => 0x5c,
                        '/' => 0x2f,
                        'b' => 0x08,
                        'f' => 0x0c,
                        'n' => 0x0a,
                        'r' => 0x0d,
                        't' => 0x09,
                        'u' => {
                            let high = self.hex4()?;
                            if (0xd800..0xdc00).contains(&high)
                                && self.s[self.i..].starts_with(&[0x5c, 0x75])
                            {
                                let save = self.i;
                                self.i += 2;
                                match self.hex4() {
                                    Some(low) if (0xdc00..0xe000).contains(&low) => {
                                        0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                                    }
                                    _ => {
                                        self.i = save;
                                        high
                                    }
                                }
                            } else {
                                high
                            }
                        }
                        _ => return None,
                    };
                    out.push(unit);
                }
                c if c < 0x20 => return None,
                c => out.push(c),
            }
        }
    }
    fn list(&mut self, depth: usize) -> Option<PyValue> {
        self.i += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(0x5d) {
            self.i += 1;
            return Some(PyValue::List(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek()? {
                0x2c => self.i += 1,
                0x5d => {
                    self.i += 1;
                    return Some(PyValue::List(items));
                }
                _ => return None,
            }
        }
    }
    fn dict(&mut self, depth: usize) -> Option<PyValue> {
        self.i += 1;
        let mut items: Vec<(Vec<u32>, PyValue)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(0x7d) {
            self.i += 1;
            return Some(PyValue::Dict(items));
        }
        loop {
            self.skip_ws();
            if self.peek()? != 0x22 {
                return None;
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek()? != 0x3a {
                return None;
            }
            self.i += 1;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            match items.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => items.push((key, value)),
            }
            self.skip_ws();
            match self.peek()? {
                0x2c => self.i += 1,
                0x7d => {
                    self.i += 1;
                    return Some(PyValue::Dict(items));
                }
                _ => return None,
            }
        }
    }
}

/// CPython's `json.loads(text)`; `None` is the `ValueError` it raised.
pub(crate) fn py_loads(text: &[u32]) -> Option<PyValue> {
    let mut parser = PyParser { s: text, i: 0 };
    parser.skip_ws();
    let value = parser.value(0)?;
    parser.skip_ws();
    (parser.i == text.len()).then_some(value)
}

fn py_float_text(value: f64, json: bool) -> String {
    match (value.is_nan(), value.is_infinite(), json) {
        (true, _, true) => "NaN".into(),
        (true, _, false) => "nan".into(),
        (_, true, true) => if value > 0.0 { "Infinity" } else { "-Infinity" }.into(),
        (_, true, false) => if value > 0.0 { "inf" } else { "-inf" }.into(),
        _ => super::num::py_repr_float(value),
    }
}

fn dumps_str(out: &mut String, text: &[u32]) {
    out.push('"');
    for &c in text {
        match c {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x20..=0x7e => out.push(char::from(c as u8)),
            c if c >= 0x10000 => {
                let v = c - 0x10000;
                out.push_str(&format!(
                    "\\u{:04x}\\u{:04x}",
                    0xd800 + (v >> 10),
                    0xdc00 + (v & 0x3ff)
                ));
            }
            c => out.push_str(&format!("\\u{c:04x}")),
        }
    }
    out.push('"');
}

/// CPython's `json.dumps(value)` with its defaults.
pub(crate) fn py_dumps(value: &PyValue) -> String {
    let mut out = String::new();
    match value {
        PyValue::None => out.push_str("null"),
        PyValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        PyValue::Int(digits) => out.push_str(digits),
        PyValue::Float(f) => out.push_str(&py_float_text(*f, true)),
        PyValue::Str(s) => dumps_str(&mut out, s),
        PyValue::List(items) => {
            let parts: Vec<String> = items.iter().map(py_dumps).collect();
            out = format!("[{}]", parts.join(", "));
        }
        PyValue::Dict(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|(k, v)| {
                    let mut key = String::new();
                    dumps_str(&mut key, k);
                    format!("{key}: {}", py_dumps(v))
                })
                .collect();
            out = format!("{{{}}}", parts.join(", "));
        }
    }
    out
}

/// CPython's `repr(str)` of bytes decoded with `surrogateescape`, as an
/// `OSError` message quotes a filename.
pub(crate) fn py_repr_os(raw: &[u8]) -> String {
    py_str_repr(&surrogateescape(raw))
}

/// CPython's `repr(bytes)`: `b'...'`, double quotes only when the bytes
/// hold a single quote and no double quote.
pub(crate) fn py_bytes_repr(raw: &[u8]) -> String {
    let quote = if raw.contains(&b'\'') && !raw.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::from("b");
    out.push(char::from(quote));
    for &b in raw {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if b == quote => {
                out.push('\\');
                out.push(char::from(b));
            }
            b if !(0x20..0x7f).contains(&b) => out.push_str(&format!("\\x{b:02x}")),
            b => out.push(char::from(b)),
        }
    }
    out.push(char::from(quote));
    out
}

/// CPython's `repr(str)`.
fn py_str_repr(text: &[u32]) -> String {
    let has = |c: u32| text.contains(&c);
    let quote = if has(0x27) && !has(0x22) { 0x22 } else { 0x27 };
    let mut out = String::new();
    out.push(char::from(quote as u8));
    for &c in text {
        match c {
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(char::from(c as u8));
            }
            // CPython: anything `str.isprintable()` rejects is escaped, by
            // width: \xhh up to U+00FF, \uhhhh up to U+FFFF, else \U.
            c if !super::py_printable::py_isprintable(c) => {
                if c <= 0xff {
                    out.push_str(&format!("\\x{c:02x}"));
                } else if c <= 0xffff {
                    out.push_str(&format!("\\u{c:04x}"));
                } else {
                    out.push_str(&format!("\\U{c:08x}"));
                }
            }
            c => match char::from_u32(c) {
                Some(ch) => out.push(ch),
                // Unreachable: surrogates are non-printable above.
                None => out.push_str(&format!("\\u{c:04x}")),
            },
        }
    }
    out.push(char::from(quote as u8));
    out
}

fn py_repr(value: &PyValue) -> String {
    match value {
        PyValue::None => "None".into(),
        PyValue::Bool(b) => if *b { "True" } else { "False" }.into(),
        PyValue::Int(digits) => digits.clone(),
        PyValue::Float(f) => py_float_text(*f, false),
        PyValue::Str(s) => py_str_repr(s),
        PyValue::List(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        PyValue::Dict(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// CPython's `print(value)` bytes without the line feed: `str(value)`
/// encoded as UTF-8. `None` is the `UnicodeEncodeError` a lone surrogate
/// raised.
pub(crate) fn py_print(value: &PyValue) -> Option<Vec<u8>> {
    let text: Vec<u32> = match value {
        PyValue::Str(s) => s.clone(),
        other => py_repr(other).chars().map(u32::from).collect(),
    };
    let text: String = text
        .iter()
        .map(|c| char::from_u32(*c))
        .collect::<Option<String>>()?;
    Some(text.into_bytes())
}

/// Bytes as Python's `json.dumps` writes an argv string decoded with
/// `surrogateescape`: each byte that is not part of valid UTF-8 becomes
/// `\udcXX`.
pub(crate) fn py_dumps_os(raw: &[u8]) -> String {
    let mut out = String::from("\"");
    for chunk in raw.utf8_chunks() {
        let mut valid = String::new();
        escape_into(&mut valid, chunk.valid());
        out.push_str(&valid[1..valid.len() - 1]);
        for byte in chunk.invalid() {
            out.push_str(&format!("\\udc{byte:02x}"));
        }
    }
    out.push('"');
    out
}

/// Canonical bytes: sorted keys, compact separators, ASCII only.
pub(crate) fn serial(value: &Value) -> Vec<u8> {
    let mut out = String::new();
    write(&mut out, value);
    out.into_bytes()
}

pub(crate) fn digest(raw: &[u8]) -> String {
    hex(&Sha256::digest(raw))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Standard, padded base64 of a path's bytes, as Python's `b64encode`.
pub(crate) fn b64(raw: &[u8]) -> String {
    STANDARD.encode(raw)
}

/// Decode canonical standard base64 into a literal worktree-relative path.
/// The decoder refuses non-alphabet bytes and bad padding; the re-encode
/// check refuses non-canonical trailing bits, as Python's
/// `b64decode(validate=True)` plus the re-encode comparison did.
pub(crate) fn unb64(value: &Value) -> Result<Vec<u8>, Fail> {
    let text = value
        .as_str()
        .ok_or_else(|| Fail::Refusal("path encoding must be a string".into()))?;
    let raw = STANDARD
        .decode(text.as_bytes())
        .map_err(|_| Fail::Refusal("invalid base64 path encoding".into()))?;
    refuse(b64(&raw) == text, "non-canonical path encoding")?;
    literal(&raw)?;
    Ok(raw)
}

/// A literal path: relative, non-empty, no NUL and no empty, `.` or `..` part.
pub(crate) fn literal(path: &[u8]) -> Result<(), Fail> {
    refuse(
        !path.is_empty()
            && !path.starts_with(b"/")
            && !path.contains(&0)
            && path
                .split(|&b| b == b'/')
                .all(|part| !part.is_empty() && part != b"." && part != b".."),
        "invalid literal path",
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_are_refused() {
        assert!(parse(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse(br#"{"a":{"b":1,"b":1}}"#).is_err());
        assert!(parse(br#"{"a":1}"#).is_ok());
    }

    #[test]
    fn trailing_data_and_constants_are_refused() {
        assert!(parse(b"{} {}").is_err());
        assert!(parse(b"NaN").is_err());
        assert!(parse(b"[Infinity]").is_err());
        assert!(parse(br#""\ud800""#).is_err());
    }

    #[test]
    fn serial_is_sorted_compact_ascii() {
        let value = parse("{\"b\":[1,true,null],\"a\":\"\u{e9}\u{1f600}\\n\"}".as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(serial(&value)).unwrap(),
            r#"{"a":"\u00e9\ud83d\ude00\n","b":[1,true,null]}"#
        );
    }

    #[test]
    fn base64_round_trips_and_rejects_non_canonical_forms() {
        for raw in [&b"a"[..], b"ab", b"abc", b"dir/file name", b"\xff\xfe"] {
            assert_eq!(unb64(&Value::String(b64(raw))).unwrap(), raw);
        }
        assert!(unb64(&Value::String("YQ".into())).is_err());
        assert!(unb64(&Value::String("YR==".into())).is_err());
        assert!(unb64(&Value::String("Y=Q=".into())).is_err());
        assert!(unb64(&Value::String(b64(b"../x"))).is_err());
        assert!(unb64(&Value::String(b64(b"/x"))).is_err());
        assert!(unb64(&Value::Bool(true)).is_err());
    }
}
