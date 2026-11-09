//! Test-only Rust port of `PhaseProjection` from
//! `amplifier-bundle/recipes/tests/test-checkpoint-scope.py` at f14da87ed
//! (#1554, PR #2020, consensus steps C-n and C-o).
//!
//! The projection runs only the bounded delta of a canonical phase: the new
//! deterministic capture steps, a synthetic scope verdict, the new steps
//! between the reviewer and the checkpoint, and the existing checkpoint step.
//! Every checkpoint decision is made by the real `amplihack` binary that the
//! recipe's bash blocks invoke. This module only extracts, renders and
//! sequences those blocks, and it is never linked into the binary.
//!
//! It is a textual port, not a reimplementation:
//!
//! * The recipe is split with the Python regex block reader, not a YAML
//!   parser. A change to the two-space step layout is caught by the
//!   canonical-layout unit test rather than silently changing what runs.
//! * Context values are an ordered [`Ctx`]. The workspace `serde_json` is
//!   built without `preserve_order`, and Python keeps the insertion order of
//!   keys added after load, so `serde_json::Value` cannot hold them.
//! * `py_json_dumps`, `py_str` and `py_strip` reproduce CPython's
//!   `json.dumps`, `str()` and `str.strip()` byte for byte.
//! * Projected stdout that is not valid UTF-8 is kept as [`Ctx::Raw`], as
//!   Python's `surrogateescape` decoding keeps it.
//! * A shell failure is a result, not an error: the first non-zero block
//!   stops the sequence and its real status, stdout and stderr are returned.
//! * A contract failure (where Python's unittest assertion or an uncaught
//!   exception fired) returns status 1 with the Python message as the last
//!   stderr line. The Python traceback is not reproduced.
//!
//! The #1671 shell driver reaches this code through [`write_projection_shim`]
//! and [`reentry_main`]. `crates/amplihack-cli` has `autotests = false` and
//! its manifest is frozen for this round, so there is no `[[bin]]` or
//! `[[test]]` helper target: the shim re-enters the test binary itself.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};

// ------------------------------------------------------------------ constants

/// The environment variable that turns `phase_projection_reentry` from a
/// no-op into the #1671 projection. It names the request directory.
pub const PROJECTION_REQUEST_ENV: &str = "AMPLIHACK_PROJECTION_REQUEST";

/// `PHASES` from the Python reference: asset, checkpoint step, helper and
/// checkpoint output name.
pub const PHASES: [(&str, &str, &str, &str); 3] = [
    (
        "workflow-publish",
        "step-15-commit-push",
        "workflow_publish_checkpoint.sh",
        "commit_result",
    ),
    (
        "workflow-pr-review",
        "step-18c-push-feedback-changes",
        "workflow_review_checkpoint.sh",
        "feedback_push_result",
    ),
    (
        "workflow-finalize",
        "step-20b-push-cleanup",
        "workflow_cleanup_checkpoint.sh",
        "cleanup_push_result",
    ),
];

const PRE_DELTA_ASSETS: &str =
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/checkpoint-pre-delta-assets.json";
const PRE_DELTA_ASSETS_SHA256: &str =
    "112ea7497a96d7d0257c409c74bb302df6693e6503770d01e63613783191d706";

const BINDING_FIELDS: [&str; 10] = [
    "owner_task",
    "phase",
    "invocation",
    "worktree",
    "git_dir",
    "common_dir",
    "index_path",
    "branch",
    "initial_head",
    "candidate_digest",
];

/// `os.defpath` on POSIX: what `subprocess` searches when the child
/// environment has no `PATH`.
const PY_DEFPATH: &str = "/bin:/usr/bin";

/// CPython's default recursion limit is 1000; the C JSON scanner raises
/// `RecursionError` near it. Stopping a little earlier keeps the Rust stack
/// bounded; the outcome (no value) is the same.
const MAX_JSON_DEPTH: usize = 900;

/// Python exception names that are reported as themselves. Every other
/// contract failure message is an `AssertionError`.
const EXCEPTION_PREFIXES: [&str; 10] = [
    "KeyError: ",
    "TypeError: ",
    "ValueError: ",
    "OSError: ",
    "FileNotFoundError: ",
    "UnicodeDecodeError: ",
    "UnicodeEncodeError: ",
    "binascii.Error: ",
    "json.decoder.JSONDecodeError: ",
    "StopIteration",
];

// ------------------------------------------------------------------------ Ctx

/// One projection context value. `Dict` is ordered by insertion and is
/// never sorted. `Raw` is a Python `str` that holds `surrogateescape`
/// characters, stored as the original bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Ctx {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Raw(Vec<u8>),
    List(Vec<Ctx>),
    Dict(Vec<(String, Ctx)>),
}

impl Ctx {
    /// Convert a `serde_json` value. Object keys come out in the order the
    /// workspace `serde_json` iterates them, which is sorted: exactly the
    /// order Python saw when it parsed the request JSON that Rust wrote.
    pub fn from_json_value(value: &serde_json::Value) -> Ctx {
        match value {
            serde_json::Value::Null => Ctx::Null,
            serde_json::Value::Bool(b) => Ctx::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ctx::Int(i128::from(i))
                } else if let Some(u) = n.as_u64() {
                    Ctx::Int(i128::from(u))
                } else {
                    Ctx::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            serde_json::Value::String(s) => Ctx::Str(s.clone()),
            serde_json::Value::Array(items) => {
                Ctx::List(items.iter().map(Ctx::from_json_value).collect())
            }
            serde_json::Value::Object(map) => Ctx::Dict(
                map.iter()
                    .map(|(k, v)| (k.clone(), Ctx::from_json_value(v)))
                    .collect(),
            ),
        }
    }

    /// Python `dict.get(key)` for a `Dict`; `None` for every other value.
    pub fn get(&self, key: &str) -> Option<&Ctx> {
        match self {
            Ctx::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Python `d[key] = value`: an existing key keeps its position.
    /// Returns false (and changes nothing) when `self` is not a `Dict`.
    pub fn insert(&mut self, key: &str, value: Ctx) -> bool {
        match self {
            Ctx::Dict(items) => {
                if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                    slot.1 = value;
                } else {
                    items.push((key.to_owned(), value));
                }
                true
            }
            _ => false,
        }
    }

    /// A Python `str` built from bytes with `surrogateescape`.
    pub fn from_os_bytes(raw: &[u8]) -> Ctx {
        match std::str::from_utf8(raw) {
            Ok(text) => Ctx::Str(text.to_owned()),
            Err(_) => Ctx::Raw(raw.to_vec()),
        }
    }

    /// Python truthiness.
    fn truthy(&self) -> bool {
        match self {
            Ctx::Null => false,
            Ctx::Bool(b) => *b,
            Ctx::Int(i) => *i != 0,
            Ctx::Float(f) => *f != 0.0,
            Ctx::Str(s) => !s.is_empty(),
            Ctx::Raw(r) => !r.is_empty(),
            Ctx::List(l) => !l.is_empty(),
            Ctx::Dict(d) => !d.is_empty(),
        }
    }

    fn is_container_or_bool(&self) -> bool {
        matches!(self, Ctx::Dict(_) | Ctx::List(_) | Ctx::Bool(_))
    }
}

// ----------------------------------------------------- Python text primitives

/// `str.isspace()` for one code point: the characters with bidirectional
/// class WS, B or S, or category Zs. Surrogates are never whitespace.
pub fn py_isspace(c: u32) -> bool {
    matches!(
        c,
        0x09..=0x0d
            | 0x1c..=0x1f
            | 0x20
            | 0x85
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

/// Python `str.strip()` with no argument.
pub fn py_strip(text: &str) -> String {
    text.trim_matches(|c: char| py_isspace(u32::from(c)))
        .to_owned()
}

fn strip_cps(cps: &[u32]) -> &[u32] {
    let start = cps
        .iter()
        .position(|c| !py_isspace(*c))
        .unwrap_or(cps.len());
    let end = cps
        .iter()
        .rposition(|c| !py_isspace(*c))
        .map_or(start, |i| i + 1);
    &cps[start..end.max(start)]
}

/// `bytes.decode("utf-8", errors="surrogateescape")` as code points. Each
/// byte of an invalid sequence becomes U+DC00 + byte; decoding resumes after
/// it. Continuation bytes alone are always invalid, so the result does not
/// depend on how long the decoder considers an invalid sequence to be.
pub fn decode_surrogateescape(raw: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(raw.len());
    let mut rest = raw;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.extend(text.chars().map(u32::from));
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                // SAFETY-free: the prefix was just validated.
                let text = std::str::from_utf8(&rest[..valid]).unwrap_or_default();
                out.extend(text.chars().map(u32::from));
                let bad = error.error_len().unwrap_or(rest.len() - valid);
                for byte in &rest[valid..valid + bad] {
                    out.push(0xdc00 + u32::from(*byte));
                }
                rest = &rest[valid + bad..];
            }
        }
    }
    out
}

/// `str.encode("utf-8", errors="surrogateescape")`. `None` for a surrogate
/// outside U+DC80..U+DCFF, which Python cannot encode either.
pub fn encode_surrogateescape(cps: &[u32]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(cps.len());
    for &c in cps {
        if (0xdc80..=0xdcff).contains(&c) {
            out.push((c - 0xdc00) as u8);
        } else {
            let ch = char::from_u32(c)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    Some(out)
}

fn has_surrogate(cps: &[u32]) -> bool {
    cps.iter().any(|c| (0xd800..=0xdfff).contains(c))
}

/// Projected step stdout as Python holds it after
/// `stdout.decode(errors="surrogateescape").strip()`.
pub fn decode_projected_stdout(raw: &[u8]) -> Ctx {
    let cps = decode_surrogateescape(raw);
    let stripped = strip_cps(&cps);
    if has_surrogate(stripped) {
        // surrogateescape only produces U+DC80..U+DCFF, which always encode.
        Ctx::Raw(encode_surrogateescape(stripped).unwrap_or_default())
    } else {
        Ctx::Str(stripped.iter().filter_map(|c| char::from_u32(*c)).collect())
    }
}

// --------------------------------------------------------- number formatting

/// Python `repr(float)`. A copy of the engine's `num::py_repr_float`, which is
/// `pub(crate)` and so not reachable from integration tests; both are pinned
/// by the same 13 cases.
pub fn py_repr_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.into();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .into();
    }
    let sci = format!("{value:e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let body = if point <= 0 {
            format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
        } else if point as usize >= digits.len() {
            format!("{digits}{}.0", "0".repeat(point as usize - digits.len()))
        } else {
            let (whole, fraction) = digits.split_at(point as usize);
            format!("{whole}.{fraction}")
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{exp_sign}{:02}", exponent.unsigned_abs())
    }
}

/// `json.dumps(float)`: `float.__repr__` except for the three non-finite
/// values, which `allow_nan=True` writes as JavaScript names.
fn py_json_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        py_repr_float(value)
    }
}

// --------------------------------------------------------------- py_json_dumps

fn dump_json_string(out: &mut String, cps: impl IntoIterator<Item = u32>) {
    out.push('"');
    for c in cps {
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
                let _ = write!(
                    out,
                    "\\u{:04x}\\u{:04x}",
                    0xd800 + (v >> 10),
                    0xdc00 + (v & 0x3ff)
                );
            }
            c => {
                let _ = write!(out, "\\u{c:04x}");
            }
        }
    }
    out.push('"');
}

fn dump_json(out: &mut String, value: &Ctx) {
    match value {
        Ctx::Null => out.push_str("null"),
        Ctx::Bool(true) => out.push_str("true"),
        Ctx::Bool(false) => out.push_str("false"),
        Ctx::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Ctx::Float(f) => out.push_str(&py_json_float(*f)),
        Ctx::Str(s) => dump_json_string(out, s.chars().map(u32::from)),
        Ctx::Raw(raw) => dump_json_string(out, decode_surrogateescape(raw)),
        Ctx::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_json(out, item);
            }
            out.push(']');
        }
        Ctx::Dict(items) => {
            out.push('{');
            for (i, (key, item)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_json_string(out, key.chars().map(u32::from));
                out.push_str(": ");
                dump_json(out, item);
            }
            out.push('}');
        }
    }
}

/// CPython `json.dumps(value)` with every default: `", "` and `": "`
/// separators, `ensure_ascii=True`, `allow_nan=True`, insertion order.
pub fn py_json_dumps(value: &Ctx) -> String {
    let mut out = String::new();
    dump_json(&mut out, value);
    out
}

// --------------------------------------------------------------------- py_str

/// Python `str.isprintable()` for one code point, used only by `repr` of a
/// container. This path is unreachable from the projection (containers are
/// always sent to `json.dumps`), so the non-ASCII table is the common subset:
/// controls, format characters, separators other than space, surrogates,
/// private use and the two noncharacters at the end of the BMP.
fn py_isprintable(c: u32) -> bool {
    !(c < 0x20
        || (0x7f..=0xa0).contains(&c)
        || c == 0xad
        || c == 0x1680
        || (0x2000..=0x200f).contains(&c)
        || (0x2028..=0x202f).contains(&c)
        || (0x205f..=0x2064).contains(&c)
        || c == 0x3000
        || (0xd800..=0xdfff).contains(&c)
        || (0xe000..=0xf8ff).contains(&c)
        || c == 0xfeff
        || (0xfff9..=0xfffb).contains(&c)
        || c == 0xfffe
        || c == 0xffff)
}

fn repr_str(out: &mut String, cps: &[u32]) {
    let has_single = cps.contains(&0x27);
    let has_double = cps.contains(&0x22);
    let quote = if has_single && !has_double { '"' } else { '\'' };
    out.push(quote);
    for &c in cps {
        match c {
            0x5c => out.push_str("\\\\"),
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            c if c == u32::from(quote) => {
                out.push('\\');
                out.push(quote);
            }
            c if c < 0x20 || c == 0x7f => {
                let _ = write!(out, "\\x{c:02x}");
            }
            c if c < 0x7f => out.push(char::from(c as u8)),
            c if py_isprintable(c) => out.push(char::from_u32(c).unwrap_or('\u{fffd}')),
            c if c <= 0xff => {
                let _ = write!(out, "\\x{c:02x}");
            }
            c if c <= 0xffff => {
                let _ = write!(out, "\\u{c:04x}");
            }
            c => {
                let _ = write!(out, "\\U{c:08x}");
            }
        }
    }
    out.push(quote);
}

fn py_repr(out: &mut String, value: &Ctx) {
    match value {
        Ctx::Str(s) => repr_str(out, &s.chars().map(u32::from).collect::<Vec<_>>()),
        Ctx::Raw(raw) => repr_str(out, &decode_surrogateescape(raw)),
        Ctx::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_repr(out, item);
            }
            out.push(']');
        }
        Ctx::Dict(items) => {
            out.push('{');
            for (i, (key, item)) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                repr_str(out, &key.chars().map(u32::from).collect::<Vec<_>>());
                out.push_str(": ");
                py_repr(out, item);
            }
            out.push('}');
        }
        scalar => out.push_str(&String::from_utf8_lossy(&py_str_bytes(scalar))),
    }
}

/// Python `str(value)` as the bytes `os.fsencode` would give it. This is the
/// form every byte-exact path (rendering, `RECIPE_VAR_*`) uses.
pub fn py_str_bytes(value: &Ctx) -> Vec<u8> {
    match value {
        Ctx::Null => b"None".to_vec(),
        Ctx::Bool(true) => b"True".to_vec(),
        Ctx::Bool(false) => b"False".to_vec(),
        Ctx::Int(i) => i.to_string().into_bytes(),
        Ctx::Float(f) => py_repr_float(*f).into_bytes(),
        Ctx::Str(s) => s.as_bytes().to_vec(),
        Ctx::Raw(raw) => raw.clone(),
        Ctx::List(_) | Ctx::Dict(_) => {
            let mut out = String::new();
            py_repr(&mut out, value);
            out.into_bytes()
        }
    }
}

/// Python `str(value)`. For `Raw` the bytes are not valid UTF-8 and the
/// result is lossy; byte-exact callers use [`py_str_bytes`].
pub fn py_str(value: &Ctx) -> String {
    String::from_utf8_lossy(&py_str_bytes(value)).into_owned()
}

/// What `sync_context` and `render` write for one value: `json.dumps` for
/// dict, list and bool, `str()` for everything else.
fn template_bytes(value: &Ctx) -> Vec<u8> {
    if value.is_container_or_bool() {
        py_json_dumps(value).into_bytes()
    } else {
        py_str_bytes(value)
    }
}

// ----------------------------------------------------------------- JSON parse

#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonMode {
    /// The 4d5187e runner rules used by `projected_json_output`: integers in
    /// `-(2**63)..2**64` stay integers, every other number must be a finite
    /// float, NaN and Infinity are refused and no string may hold a lone
    /// surrogate.
    Projected,
    /// Plain `json.loads`: NaN and Infinity are floats; integers must fit
    /// `i128` here (Python has no limit, so a larger one is a recorded
    /// contract failure); a surrogate outside U+DC80..U+DCFF is refused
    /// because `Ctx` cannot hold it.
    PythonDefault,
}

struct JsonParser<'a> {
    s: &'a [u32],
    i: usize,
    mode: JsonMode,
    depth: usize,
}

fn cp(c: char) -> u32 {
    u32::from(c)
}

impl JsonParser<'_> {
    fn peek(&self) -> Option<u32> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.i += 1;
        }
    }

    fn starts_with(&self, word: &str) -> bool {
        let w: Vec<u32> = word.chars().map(u32::from).collect();
        self.s.len() >= self.i + w.len() && self.s[self.i..self.i + w.len()] == w[..]
    }

    fn document(mut self) -> Result<Ctx, String> {
        if self.peek() == Some(0xfeff) {
            return Err("Unexpected UTF-8 BOM (decode using utf-8-sig)".into());
        }
        self.skip_ws();
        let value = self.value()?;
        self.skip_ws();
        if self.i != self.s.len() {
            return Err(format!("Extra data: char {}", self.i));
        }
        Ok(value)
    }

    fn value(&mut self) -> Result<Ctx, String> {
        let Some(c) = self.peek() else {
            return Err(format!("Expecting value: char {}", self.i));
        };
        match c {
            0x22 => {
                self.i += 1;
                let cps = self.string_body()?;
                self.string_ctx(cps)
            }
            0x7b => self.object(),
            0x5b => self.array(),
            _ if self.starts_with("null") => {
                self.i += 4;
                Ok(Ctx::Null)
            }
            _ if self.starts_with("true") => {
                self.i += 4;
                Ok(Ctx::Bool(true))
            }
            _ if self.starts_with("false") => {
                self.i += 5;
                Ok(Ctx::Bool(false))
            }
            _ if self.starts_with("NaN") => {
                self.i += 3;
                self.constant(f64::NAN)
            }
            _ if self.starts_with("Infinity") => {
                self.i += 8;
                self.constant(f64::INFINITY)
            }
            _ if self.starts_with("-Infinity") => {
                self.i += 9;
                self.constant(f64::NEG_INFINITY)
            }
            _ => self.number(),
        }
    }

    fn constant(&self, value: f64) -> Result<Ctx, String> {
        match self.mode {
            JsonMode::Projected => Err("Non-JSON numeric constant".into()),
            JsonMode::PythonDefault => Ok(Ctx::Float(value)),
        }
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err("maximum JSON nesting depth exceeded".into());
        }
        Ok(())
    }

    fn object(&mut self) -> Result<Ctx, String> {
        self.enter()?;
        self.i += 1;
        let mut items: Vec<(String, Ctx)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(cp('}')) {
            self.i += 1;
            self.depth -= 1;
            return Ok(Ctx::Dict(items));
        }
        loop {
            if self.peek() != Some(cp('"')) {
                return Err(format!(
                    "Expecting property name enclosed in double quotes: char {}",
                    self.i
                ));
            }
            self.i += 1;
            let key_cps = self.string_body()?;
            if has_surrogate(&key_cps) {
                return Err("lone surrogate in an object key".into());
            }
            let key: String = key_cps.iter().filter_map(|c| char::from_u32(*c)).collect();
            self.skip_ws();
            if self.peek() != Some(cp(':')) {
                return Err(format!("Expecting ':' delimiter: char {}", self.i));
            }
            self.i += 1;
            self.skip_ws();
            let value = self.value()?;
            // Python `dict(pairs)`: the last value wins at the first position.
            if let Some(slot) = items.iter_mut().find(|(k, _)| *k == key) {
                slot.1 = value;
            } else {
                items.push((key, value));
            }
            self.skip_ws();
            match self.peek() {
                Some(c) if c == cp(',') => {
                    self.i += 1;
                    self.skip_ws();
                }
                Some(c) if c == cp('}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Ctx::Dict(items));
                }
                _ => return Err(format!("Expecting ',' delimiter: char {}", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Ctx, String> {
        self.enter()?;
        self.i += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(cp(']')) {
            self.i += 1;
            self.depth -= 1;
            return Ok(Ctx::List(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(c) if c == cp(',') => {
                    self.i += 1;
                    self.skip_ws();
                }
                Some(c) if c == cp(']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Ctx::List(items));
                }
                _ => return Err(format!("Expecting ',' delimiter: char {}", self.i)),
            }
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.s.get(at..at + 4)?;
        let mut value = 0u32;
        for &d in digits {
            let digit = char::from_u32(d)?.to_digit(16)?;
            value = value * 16 + digit;
        }
        Some(value)
    }

    /// CPython's C `scanstring` with `strict=True`. The opening quote has
    /// been consumed. Returns code points; lone surrogates are kept.
    fn string_body(&mut self) -> Result<Vec<u32>, String> {
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err("Unterminated string".into());
            };
            self.i += 1;
            match c {
                0x22 => return Ok(out),
                0x5c => {
                    let Some(e) = self.peek() else {
                        return Err("Unterminated string".into());
                    };
                    self.i += 1;
                    match char::from_u32(e) {
                        Some('"') => out.push(0x22),
                        Some('\\') => out.push(0x5c),
                        Some('/') => out.push(0x2f),
                        Some('b') => out.push(0x08),
                        Some('f') => out.push(0x0c),
                        Some('n') => out.push(0x0a),
                        Some('r') => out.push(0x0d),
                        Some('t') => out.push(0x09),
                        Some('u') => {
                            let Some(u) = self.hex4(self.i) else {
                                return Err("Invalid \\uXXXX escape".into());
                            };
                            self.i += 4;
                            if (0xd800..=0xdbff).contains(&u)
                                && self.s.get(self.i) == Some(&0x5c)
                                && self.s.get(self.i + 1) == Some(&cp('u'))
                            {
                                let Some(low) = self.hex4(self.i + 2) else {
                                    return Err("Invalid \\uXXXX escape".into());
                                };
                                if (0xdc00..=0xdfff).contains(&low) {
                                    self.i += 6;
                                    out.push(0x10000 + ((u - 0xd800) << 10) + (low - 0xdc00));
                                    continue;
                                }
                            }
                            out.push(u);
                        }
                        _ => return Err("Invalid \\escape".into()),
                    }
                }
                c if c < 0x20 => return Err("Invalid control character".into()),
                c => out.push(c),
            }
        }
    }

    fn string_ctx(&self, cps: Vec<u32>) -> Result<Ctx, String> {
        if !has_surrogate(&cps) {
            return Ok(Ctx::Str(
                cps.iter().filter_map(|c| char::from_u32(*c)).collect(),
            ));
        }
        match self.mode {
            JsonMode::Projected => Err("lone surrogate in a JSON string".into()),
            JsonMode::PythonDefault => encode_surrogateescape(&cps)
                .map(Ctx::Raw)
                .ok_or_else(|| "a surrogate outside U+DC80..U+DCFF cannot be held".into()),
        }
    }

    /// `-?(?:0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?` with ASCII digits only.
    fn number(&mut self) -> Result<Ctx, String> {
        let start = self.i;
        let is_digit = |c: Option<u32>| matches!(c, Some(0x30..=0x39));
        let mut j = self.i;
        if self.s.get(j) == Some(&cp('-')) {
            j += 1;
        }
        match self.s.get(j).copied() {
            Some(0x30) => j += 1,
            Some(0x31..=0x39) => {
                while is_digit(self.s.get(j).copied()) {
                    j += 1;
                }
            }
            _ => return Err(format!("Expecting value: char {start}")),
        }
        let mut is_float = false;
        if self.s.get(j) == Some(&cp('.')) && is_digit(self.s.get(j + 1).copied()) {
            j += 1;
            while is_digit(self.s.get(j).copied()) {
                j += 1;
            }
            is_float = true;
        }
        if matches!(self.s.get(j).copied(), Some(0x45 | 0x65)) {
            let mut k = j + 1;
            if matches!(self.s.get(k).copied(), Some(0x2b | 0x2d)) {
                k += 1;
            }
            if is_digit(self.s.get(k).copied()) {
                while is_digit(self.s.get(k).copied()) {
                    k += 1;
                }
                j = k;
                is_float = true;
            }
        }
        self.i = j;
        let text: String = self.s[start..j]
            .iter()
            .filter_map(|c| char::from_u32(*c))
            .collect();
        let as_float = |text: &str| -> Result<f64, String> {
            text.parse::<f64>()
                .map_err(|e| format!("could not convert {text:?} to float: {e}"))
        };
        if is_float {
            let value = as_float(&text)?;
            if self.mode == JsonMode::Projected && !value.is_finite() {
                return Err("Non-finite JSON number".into());
            }
            return Ok(Ctx::Float(value));
        }
        let lower = -(1i128 << 63);
        let upper = 1i128 << 64;
        match (text.parse::<i128>(), self.mode) {
            (Ok(n), JsonMode::Projected) if (lower..upper).contains(&n) => Ok(Ctx::Int(n)),
            (Ok(n), JsonMode::PythonDefault) => Ok(Ctx::Int(n)),
            (_, JsonMode::Projected) => {
                let value = as_float(&text)?;
                if value.is_finite() {
                    Ok(Ctx::Float(value))
                } else {
                    Err("Non-finite JSON number".into())
                }
            }
            (Err(_), JsonMode::PythonDefault) => {
                Err(format!("integer {text} is outside the range Ctx can hold"))
            }
        }
    }
}

fn parse_json(cps: &[u32], mode: JsonMode) -> Result<Ctx, String> {
    JsonParser {
        s: cps,
        i: 0,
        mode,
        depth: 0,
    }
    .document()
}

/// Plain `json.loads(text)` on a Python `str`.
pub fn py_json_loads(text: &[u8]) -> Result<Ctx, String> {
    parse_json(&decode_surrogateescape(text), JsonMode::PythonDefault)
}

fn is_tick3(t: &[u32], i: usize) -> bool {
    t.get(i..i + 3) == Some(&[0x60, 0x60, 0x60][..])
}

/// `re.search(r"```(?:json)?\s*\n?(.*?)\n?\s*```", text, re.S)[1]`.
///
/// For an opening fence at `i`, the greedy `\s*` takes the whole whitespace
/// run, the lazy group then ends at the first position followed only by
/// whitespace and a closing fence. Whitespace never contains a backtick, so
/// backtracking into `\s*` or dropping `json` can never find a match that
/// the first attempt missed.
fn find_fence(t: &[u32]) -> Option<&[u32]> {
    let json: Vec<u32> = "json".chars().map(u32::from).collect();
    for i in 0..t.len() {
        if !is_tick3(t, i) {
            continue;
        }
        let mut j = i + 3;
        if t.get(j..j + 4) == Some(&json[..]) {
            j += 4;
        }
        let mut s = j;
        while s < t.len() && py_isspace(t[s]) {
            s += 1;
        }
        if let Some(q) = (s..t.len()).find(|&q| is_tick3(t, q)) {
            let mut p = q;
            while p > s && py_isspace(t[p - 1]) {
                p -= 1;
            }
            return Some(&t[s..p]);
        }
    }
    None
}

/// The pinned runner's direct, fenced and first-balanced JSON rules, as
/// `projected_json_output` in the Python reference applies them to
/// `stdout.decode(errors="surrogateescape")`.
pub fn projected_json_output(raw: &[u8]) -> Result<Ctx, String> {
    let cps = decode_surrogateescape(raw);
    let text = strip_cps(&cps);
    let decode = |part: &[u32]| parse_json(part, JsonMode::Projected);
    if let Ok(value) = decode(text) {
        return Ok(value);
    }
    if let Some(Ok(value)) = find_fence(text).map(|group| decode(strip_cps(group))) {
        return Ok(value);
    }
    for (opening, closing) in [(cp('{'), cp('}')), (cp('['), cp(']'))] {
        let Some(start) = text.iter().position(|c| *c == opening) else {
            continue;
        };
        let (mut depth, mut quoted, mut escaped) = (0i64, false, false);
        for (offset, &c) in text[start..].iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == cp('\\') {
                escaped = true;
                continue;
            }
            if c == cp('"') {
                quoted = !quoted;
                continue;
            }
            if quoted {
                continue;
            }
            if c == opening {
                depth += 1;
            } else if c == closing {
                depth -= 1;
                if depth == 0 {
                    match decode(&text[start..start + offset + 1]) {
                        Ok(value) => return Ok(value),
                        Err(_) => break,
                    }
                }
            }
        }
    }
    Err("No valid direct, fenced or first-balanced JSON output".into())
}

// ------------------------------------------------------------------- render

/// The next `{{expr}}` at or after `from`, as the Python pattern
/// `\{\{([^{}]+)\}\}` finds it: the leftmost `{{`, then one or more
/// characters that are not braces (newlines included), then `}}`. The inner
/// run is forced (it stops at the first brace), so a failed start never
/// needs another split and the search moves on by one byte. Returns
/// `(start, inner_start, inner_end, end)`; all four are ASCII positions and
/// therefore character boundaries.
fn next_template(bytes: &[u8], from: usize) -> Option<(usize, usize, usize, usize)> {
    let mut start = from;
    while start + 1 < bytes.len() {
        if bytes[start] == b'{' && bytes[start + 1] == b'{' {
            let inner_start = start + 2;
            let mut inner_end = inner_start;
            while inner_end < bytes.len() && bytes[inner_end] != b'{' && bytes[inner_end] != b'}' {
                inner_end += 1;
            }
            if inner_end > inner_start
                && bytes.get(inner_end) == Some(&b'}')
                && bytes.get(inner_end + 1) == Some(&b'}')
            {
                return Some((start, inner_start, inner_end, inner_end + 2));
            }
        }
        start += 1;
    }
    None
}

/// `PhaseProjection.render` as bytes: each `{{expr}}` (no braces inside) is
/// replaced, left to right and without rescanning, by the value at the
/// stripped dotted path. No quoting or escaping is added.
pub fn render_bytes(template: &str, ctx: &Ctx) -> Result<Vec<u8>, String> {
    let bytes = template.as_bytes();
    let mut out = Vec::with_capacity(template.len());
    let mut last = 0;
    while let Some((start, inner_start, inner_end, end)) = next_template(bytes, last) {
        let inner = &template[inner_start..inner_end];
        out.extend_from_slice(&bytes[last..start]);
        let mut value = ctx;
        for part in py_strip(inner).split('.') {
            match value {
                Ctx::Dict(_) => match value.get(part) {
                    Some(next) => value = next,
                    None => return Err(format!("Unbound fixture template: {inner}")),
                },
                _ => return Err(format!("Unbound fixture template: {inner}")),
            }
        }
        out.extend_from_slice(&template_bytes(value));
        last = end;
    }
    out.extend_from_slice(&bytes[last..]);
    Ok(out)
}

/// [`render_bytes`] for callers that need text. A result that is not UTF-8
/// (only possible through a `Raw` value) is refused here; the projection
/// itself always uses the bytes.
pub fn render(template: &str, ctx: &Ctx) -> Result<String, String> {
    String::from_utf8(render_bytes(template, ctx)?).map_err(|_| {
        "The rendered command holds bytes that are not UTF-8; use render_bytes.".to_owned()
    })
}

/// The `RECIPE_VAR_*` pairs `sync_context` writes, in context order.
pub fn recipe_vars(ctx: &Ctx) -> Vec<(OsString, OsString)> {
    let Ctx::Dict(items) = ctx else {
        return Vec::new();
    };
    items
        .iter()
        .map(|(key, value)| {
            (
                OsString::from(format!("RECIPE_VAR_{key}")),
                OsString::from_vec(template_bytes(value)),
            )
        })
        .collect()
}

// --------------------------------------------------- blocks, scalar, literal

/// Every position where `(?m)^` matches: the start of the text and every
/// position just after a `\n` (the `regex` crate's multi-line `^` ignores a
/// lone `\r`, as Python's does).
fn line_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1))
}

/// Where `PY\s*$` (Python's `\s` class under `(?m)`) can end when it starts
/// at `from`: the greedy run of `str.isspace` characters is backed off to the
/// last position that is the end of the text or sits before a `\n`. `None`
/// when no such position exists, i.e. the pattern fails here.
fn ws_then_line_end(text: &str, from: usize) -> Option<usize> {
    let mut best = None;
    for (offset, c) in text[from..].char_indices() {
        if c == '\n' {
            best = Some(from + offset);
        }
        if !py_isspace(u32::from(c)) {
            return best;
        }
    }
    Some(text.len())
}

/// `blocks(text)`: split on `^  - id: "<id>"` lines. The Python builds a
/// dict, so a repeated ID keeps its first position and takes the last body;
/// this returns the same pairs in the same order.
///
/// The lines are found by scanning for the Python pattern
/// `(?m)^  - id: "([^"]+)"\s*$` the way a regex search does: left to right,
/// each search resuming where the previous match ended. The ID runs to the
/// first `"` and may cross a newline; the match ends where the trailing
/// whitespace reaches a line end.
pub fn blocks(text: &str) -> Vec<(String, String)> {
    const HEAD: &str = "  - id: \"";
    let mut matches: Vec<(usize, String)> = Vec::new();
    let mut resume = 0;
    for start in line_starts(text) {
        if start < resume || !text[start..].starts_with(HEAD) {
            continue;
        }
        let id_start = start + HEAD.len();
        let Some(id_len) = text[id_start..].find('"') else {
            continue;
        };
        if id_len == 0 {
            continue;
        }
        let Some(end) = ws_then_line_end(text, id_start + id_len + 1) else {
            continue;
        };
        matches.push((start, text[id_start..id_start + id_len].to_owned()));
        resume = end;
    }
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

/// `scalar(block, key)` with its failures reported: `true`/`false` become
/// booleans, a value starting with `"` is `json.loads`ed, anything else has
/// its single quotes stripped.
///
/// The value is what the Python pattern `(?m)^    KEY: (.*?)\s*$` captures:
/// on the first line that starts with `    KEY: `, the rest of that line
/// with trailing `str.isspace` characters removed.
pub fn try_scalar(block: &str, key: &str) -> Result<Option<Ctx>, String> {
    let head = format!("    {key}: ");
    let Some(start) = line_starts(block).find(|&p| block[p..].starts_with(&head)) else {
        return Ok(None);
    };
    let rest = &block[start + head.len()..];
    let line = rest.find('\n').map_or(rest, |nl| &rest[..nl]);
    let raw = line.trim_end_matches(|c: char| py_isspace(u32::from(c)));
    match raw {
        "true" => return Ok(Some(Ctx::Bool(true))),
        "false" => return Ok(Some(Ctx::Bool(false))),
        _ => {}
    }
    if raw.starts_with('"') {
        return py_json_loads(raw.as_bytes())
            .map(Some)
            .map_err(|e| format!("json.decoder.JSONDecodeError: {e}"));
    }
    Ok(Some(Ctx::Str(raw.trim_matches('\'').to_owned())))
}

/// [`try_scalar`] for tests: a malformed quoted scalar is a test failure.
#[allow(
    dead_code,
    reason = "only checkpoint_projection_unit in cli_integration calls it"
)]
pub(crate) fn scalar(block: &str, key: &str) -> Option<Ctx> {
    try_scalar(block, key).unwrap_or_else(|e| panic!("scalar {key:?}: {e}"))
}

/// `textwrap.dedent` as CPython 3.14 implements it (the oracle's version).
pub fn py_dedent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let is_space_line = |l: &str| !l.is_empty() && l.chars().all(|c| py_isspace(u32::from(c)));
    let non_blank: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.is_empty() && !is_space_line(l))
        .collect();
    let l1 = non_blank.iter().copied().min().unwrap_or("");
    let l2 = non_blank.iter().copied().max().unwrap_or("");
    let l2_chars: Vec<char> = l2.chars().collect();
    let mut margin = 0usize;
    for (m, c) in l1.chars().enumerate() {
        margin = m;
        if l2_chars.get(m) != Some(&c) || !(c == ' ' || c == '\t') {
            break;
        }
    }
    lines
        .iter()
        .map(|l| {
            if is_space_line(l) {
                String::new()
            } else {
                l.chars().skip(margin).collect()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `literal(block, key)`: a `key: |` block of lines indented by six or more
/// spaces (or empty), dedented. The message is the Python one.
///
/// This is the Python pattern `(?m)^    KEY: \|\n((?:(?: {6,}.*)?\n)*)`
/// scanned directly: the first line start that begins with `    KEY: |` and
/// a newline, then every following complete line (one that ends in `\n`)
/// that is empty or starts with six spaces, up to the first that is not.
pub fn literal(block: &str, key: &str) -> Result<String, String> {
    let head = format!("    {key}: |\n");
    let Some(start) = line_starts(block).find(|&p| block[p..].starts_with(&head)) else {
        return Err(format!("A canonical literal {key} block is required."));
    };
    let body_start = start + head.len();
    let mut body_end = body_start;
    while let Some(nl) = block[body_end..].find('\n') {
        let line = &block[body_end..body_end + nl];
        if !(line.is_empty() || line.starts_with("      ")) {
            break;
        }
        body_end += nl + 1;
    }
    Ok(py_dedent(&block[body_start..body_end]))
}

/// FIPS 180-4 SHA-256. This module is also compiled into the `amplihack`
/// package's integration binary (for the #1671 re-entry test), which has no
/// `sha2` dependency, and the manifests are frozen for this round. Checked
/// against the standard vectors by `sha256_matches_the_fips_vectors`.
pub fn sha256_hex(raw: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = raw.to_vec();
    let bit_len = (raw.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(out, "{word:08x}");
    }
    out
}

/// `index_source(path)`: the hash-bound pre-delta copy of a recipe, read from
/// the retained fixture rather than the Git index.
pub fn index_source(root: &Path, path: &str) -> Result<String, String> {
    let fixture = root.join(PRE_DELTA_ASSETS);
    let raw =
        fs::read(&fixture).map_err(|e| format!("FileNotFoundError: {}: {e}", fixture.display()))?;
    index_source_bytes(&raw, path)
}

/// [`index_source`] on bytes already read, with both SHA-256 checks.
pub fn index_source_bytes(raw: &[u8], path: &str) -> Result<String, String> {
    if sha256_hex(raw) != PRE_DELTA_ASSETS_SHA256 {
        return Err("The retained pre-delta asset fixture changed.".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| format!("json.decoder.JSONDecodeError: {e}"))?;
    let entry = value
        .get("assets")
        .ok_or_else(|| "KeyError: 'assets'".to_owned())?
        .get(path)
        .ok_or_else(|| format!("KeyError: '{path}'"))?;
    let text = entry
        .get("text")
        .ok_or_else(|| "KeyError: 'text'".to_owned())?
        .as_str()
        .ok_or_else(|| "TypeError: 'text' is not a string".to_owned())?;
    let pinned = entry
        .get("sha256")
        .ok_or_else(|| "KeyError: 'sha256'".to_owned())?;
    if pinned.as_str() != Some(sha256_hex(text.as_bytes()).as_str()) {
        return Err(format!(
            "The retained pre-delta asset bytes changed: {path}"
        ));
    }
    Ok(text.to_owned())
}

// ---------------------------------------------------------------- PhasePlan

/// The step sets `PhaseProjection.__init__` derives, with its assertions.
#[derive(Debug, Clone)]
pub struct PhasePlan {
    pub current: Vec<(String, String)>,
    pub checkpoint: String,
    pub new: Vec<String>,
    pub reviewer: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

impl PhasePlan {
    pub fn from_texts(current: &str, original: &str, checkpoint: &str) -> Result<Self, String> {
        let current_blocks = blocks(current);
        let original_ids: BTreeSet<String> =
            blocks(original).into_iter().map(|(id, _)| id).collect();
        let ordered: Vec<&str> = current_blocks.iter().map(|(id, _)| id.as_str()).collect();
        let new: Vec<String> = ordered
            .iter()
            .filter(|id| !original_ids.contains(**id))
            .map(|id| (*id).to_owned())
            .collect();
        let mut reviewers = Vec::new();
        for sid in &new {
            let body = &current_blocks
                .iter()
                .find(|(id, _)| id == sid)
                .expect("new IDs come from the current blocks")
                .1;
            if try_scalar(body, "agent")?.is_some() {
                reviewers.push(sid.clone());
            }
        }
        if reviewers.len() != 1 {
            return Err(format!(
                "{} != 1 : Each phase needs exactly one new read-only scope reviewer.",
                reviewers.len()
            ));
        }
        let reviewer = reviewers.remove(0);
        let index = |sid: &str| -> Result<usize, String> {
            ordered
                .iter()
                .position(|id| *id == sid)
                .ok_or_else(|| format!("ValueError: '{sid}' is not in list"))
        };
        let review_at = index(&reviewer)?;
        let checkpoint_at = index(checkpoint)?;
        if review_at >= checkpoint_at {
            return Err(format!("{review_at} not less than {checkpoint_at}"));
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for sid in &new {
            let at = index(sid)?;
            if at < review_at {
                before.push(sid.clone());
            } else if review_at < at && at < checkpoint_at {
                after.push(sid.clone());
            }
        }
        if before.is_empty() {
            return Err(
                "[] is not true : A deterministic capture must precede the scope reviewer.".into(),
            );
        }
        Ok(PhasePlan {
            current: current_blocks,
            checkpoint: checkpoint.to_owned(),
            new,
            reviewer,
            before,
            after,
        })
    }

    fn block(&self, sid: &str) -> Result<&str, String> {
        self.current
            .iter()
            .find(|(id, _)| id == sid)
            .map(|(_, body)| body.as_str())
            .ok_or_else(|| format!("KeyError: '{sid}'"))
    }
}

// ------------------------------------------------------------ shell results

/// What `subprocess.run(..., capture_output=True)` returns: Python's
/// `returncode` (negative for a signal) and the raw output bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResult {
    pub rc: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ShellResult {
    /// The status the Python projection process exits with: `sys.exit(rc)`
    /// keeps the low eight bits, so a signal `-n` becomes `256 - n`.
    pub fn exit_code(&self) -> u8 {
        (self.rc as u32 & 0xff) as u8
    }

    /// A contract failure: status 1, no stdout, and the Python exception line
    /// (an `AssertionError` unless the message names another exception).
    pub fn contract_failure(message: &str) -> ShellResult {
        let line = if EXCEPTION_PREFIXES.iter().any(|p| message.starts_with(p)) {
            message.to_owned()
        } else {
            format!("AssertionError: {message}")
        };
        ShellResult {
            rc: 1,
            stdout: Vec::new(),
            stderr: format!("{line}\n").into_bytes(),
        }
    }
}

impl From<ShellResult> for Output {
    fn from(result: ShellResult) -> Output {
        Output {
            status: ExitStatus::from_raw(i32::from(result.exit_code()) << 8),
            stdout: result.stdout,
            stderr: result.stderr,
        }
    }
}

fn last_value<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
    env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// How `subprocess` finds a bare program name: each entry of the child
/// environment's `PATH` (or `os.defpath`), an empty or relative entry taken
/// from the child's working directory, first executable regular file wins.
pub fn resolve_program(name: &str, env: &[(OsString, OsString)], cwd: &Path) -> Option<PathBuf> {
    let path = last_value(env, "PATH")
        .cloned()
        .unwrap_or_else(|| OsString::from(PY_DEFPATH));
    for dir in path.as_bytes().split(|b| *b == b':') {
        let dir = Path::new(OsStr::from_bytes(dir));
        let candidate = if dir.is_absolute() {
            dir.join(name)
        } else {
            cwd.join(dir).join(name)
        };
        if fs::metadata(&candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        {
            return Some(candidate);
        }
    }
    None
}

fn check_spawnable(argv: &[&[u8]], env: &[(OsString, OsString)]) -> Result<(), String> {
    if argv.iter().any(|a| a.contains(&0)) {
        return Err("ValueError: embedded null byte".into());
    }
    for (key, value) in env {
        if key.as_bytes().contains(&b'=') || key.is_empty() {
            return Err("ValueError: illegal environment variable name".into());
        }
        if key.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
            return Err("ValueError: embedded null byte".into());
        }
    }
    Ok(())
}

/// Run one program as `subprocess.run([name, *args], cwd=cwd, env=env,
/// capture_output=True)` would: only the given environment, `argv[0]` set
/// to the bare name, stdin from `/dev/null`, output captured as bytes.
fn run_program(
    name: &str,
    args: &[&[u8]],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<ShellResult, String> {
    check_spawnable(args, env)?;
    let program = resolve_program(name, env, cwd).ok_or_else(|| {
        format!("FileNotFoundError: [Errno 2] No such file or directory: '{name}'")
    })?;
    let mut command = Command::new(&program);
    command
        .arg0(name)
        .args(args.iter().map(|a| OsStr::from_bytes(a)))
        .env_clear()
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|e| format!("OSError: {name} in {}: {e}", cwd.display()))?;
    let rc = match (output.status.code(), output.status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => -signal,
        (None, None) => {
            return Err(format!(
                "OSError: {name} ended without a status: {:?}",
                output.status
            ));
        }
    };
    Ok(ShellResult {
        rc,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// One bash block: `bash -c <body>`.
pub fn run_block(
    body: &[u8],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<ShellResult, String> {
    run_program("bash", &[b"-c", body], cwd, env)
}

/// Run bash blocks in order. The first non-zero block stops the sequence and
/// its real status and bytes are returned; later blocks never run. When all
/// succeed the last block's result is returned. A block that cannot be
/// started, or an empty sequence, is a contract failure, never a success.
pub fn run_sequence(bodies: &[String], cwd: &Path, env: &[(OsString, OsString)]) -> ShellResult {
    if bodies.is_empty() {
        return ShellResult::contract_failure("run_sequence needs at least one block.");
    }
    let mut last = None;
    for body in bodies {
        match run_block(body.as_bytes(), cwd, env) {
            Ok(result) if result.rc != 0 => return result,
            Ok(result) => last = Some(result),
            Err(message) => return ShellResult::contract_failure(&message),
        }
    }
    last.expect("at least one block ran")
}

// --------------------------------------------------------------- the fixture

/// The parts of the Python `Fixture` the projection reads.
struct Fixture {
    context: Ctx,
    /// `Path(...)` of the task worktree; `str(self.task)` is `task_str`.
    task: PathBuf,
    task_str: Ctx,
    branch: Ctx,
    owner: Ctx,
    allowed: BTreeSet<Vec<u8>>,
    /// `self.env`, in insertion order.
    env: Vec<(OsString, OsString)>,
}

fn set_env(env: &mut Vec<(OsString, OsString)>, key: OsString, value: OsString) {
    if let Some(slot) = env.iter_mut().find(|(k, _)| *k == key) {
        slot.1 = value;
    } else {
        env.push((key, value));
    }
}

fn env_from_list(list: &[(OsString, Option<OsString>)]) -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    for (key, value) in list {
        if let Some(value) = value {
            set_env(&mut env, key.clone(), value.clone());
        }
    }
    env
}

/// `str(PurePosixPath(raw))`: repeated and trailing slashes and `.`
/// components are removed; exactly two leading slashes are kept.
fn py_path_str(raw: &[u8]) -> Vec<u8> {
    let leading = raw.iter().take_while(|b| **b == b'/').count();
    let mut out: Vec<u8> = match leading {
        0 => Vec::new(),
        2 => b"//".to_vec(),
        _ => b"/".to_vec(),
    };
    let parts: Vec<&[u8]> = raw
        .split(|b| *b == b'/')
        .filter(|p| !p.is_empty() && *p != b".")
        .collect();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `Path(p).resolve()` (non-strict): the longest existing prefix is
/// canonicalised and the rest appended, with `..` applied lexically.
fn py_resolve(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir()?.join(path)
    };
    if let Ok(real) = fs::canonicalize(&absolute) {
        return Ok(real);
    }
    let components: Vec<Component> = absolute.components().collect();
    for split in (1..components.len()).rev() {
        let head: PathBuf = components[..split].iter().collect();
        if let Ok(mut real) = fs::canonicalize(&head) {
            for component in &components[split..] {
                match component {
                    Component::ParentDir => {
                        real.pop();
                    }
                    Component::CurDir => {}
                    other => real.push(other.as_os_str()),
                }
            }
            return Ok(real);
        }
    }
    Ok(absolute)
}

/// `os.getcwd()`: a deleted or unreadable working directory is the
/// `FileNotFoundError` Python raised, so it ends the projection as a contract
/// failure instead of silently standing in `/`.
fn current_dir() -> Result<PathBuf, String> {
    std::env::current_dir()
        .map_err(|e| format!("FileNotFoundError: cannot read the current directory: {e}"))
}

fn path_ctx(path: &Path) -> Ctx {
    Ctx::from_os_bytes(path.as_os_str().as_bytes())
}

fn ctx_repr(value: Option<&Ctx>) -> String {
    let mut out = String::new();
    match value {
        None => out.push_str("None"),
        Some(v) => py_repr(&mut out, v),
    }
    out
}

fn assert_equal(first: Option<&Ctx>, second: &Ctx) -> Result<(), String> {
    if first == Some(second) {
        Ok(())
    } else {
        Err(format!("{} != {}", ctx_repr(first), ctx_repr(Some(second))))
    }
}

fn key_error(key: &str) -> String {
    format!("KeyError: '{key}'")
}

fn require<'a>(value: &'a Ctx, key: &str) -> Result<&'a Ctx, String> {
    match value {
        Ctx::Dict(_) => value.get(key).ok_or_else(|| key_error(key)),
        _ => Err(format!(
            "TypeError: cannot read {key:?} from {}",
            ctx_repr(Some(value))
        )),
    }
}

impl Fixture {
    fn sync_context(&mut self) -> Result<(), String> {
        let Ctx::Dict(items) = &self.context else {
            return Err("TypeError: the fixture context is not a dict".into());
        };
        let pairs: Vec<(OsString, OsString)> = items
            .iter()
            .map(|(key, value)| {
                (
                    OsString::from(format!("RECIPE_VAR_{key}")),
                    OsString::from_vec(template_bytes(value)),
                )
            })
            .collect();
        for (key, value) in pairs {
            set_env(&mut self.env, key, value);
        }
        Ok(())
    }

    /// The `Fixture` of the `Owner::project` driver at f14da87ed.
    fn for_owner(
        req: &ProjectionRequest,
        env: &[(OsString, Option<OsString>)],
    ) -> Result<Self, String> {
        let context = req.context.clone();
        let setup = require(&context, "worktree_setup")?;
        let worktree = match require(setup, "worktree_path")? {
            Ctx::Str(s) => s.as_bytes().to_vec(),
            Ctx::Raw(r) => r.clone(),
            other => {
                return Err(format!(
                    "TypeError: expected str, bytes or os.PathLike object, not {}",
                    ctx_repr(Some(other))
                ));
            }
        };
        let task_bytes = py_path_str(&worktree);
        let task = PathBuf::from(OsString::from_vec(task_bytes.clone()));
        let branch = require(setup, req.branch_key)?.clone();
        let owner = require(&context, "task_description")?.clone();
        let mut fixture = Fixture {
            context,
            task,
            task_str: Ctx::from_os_bytes(&task_bytes),
            branch,
            owner,
            allowed: req.owner_paths.iter().cloned().collect(),
            env: env_from_list(env),
        };
        fixture.sync_context()?;
        Ok(fixture)
    }

    /// The `Fixture` of the #1671 `publish-projection.py` heredoc.
    fn for_1671(directory: &Path, env: &[(OsString, Option<OsString>)]) -> Result<Self, String> {
        let base_env = env_from_list(env);
        let task = py_resolve(&directory.join("repo"))?;
        let branch_out = git_in(&task, &["branch", "--show-current"], &base_env)?;
        let branch_text = String::from_utf8(branch_out).map_err(|e| {
            format!("UnicodeDecodeError: 'utf-8' codec can't decode git output: {e}")
        })?;
        let branch = Ctx::Str(py_strip(&branch_text));
        let owner = last_value(&base_env, "TASK_DESCRIPTION").map_or_else(
            || Ctx::Str("add the widget".into()),
            |v| Ctx::from_os_bytes(v.as_bytes()),
        );
        let version_bump = match last_value(&base_env, "RECIPE_VAR_version_bump") {
            Some(raw) => py_json_loads(raw.as_bytes())
                .map_err(|e| format!("json.decoder.JSONDecodeError: {e}"))?,
            None => Ctx::Dict(Vec::new()),
        };
        let task_str = path_ctx(&task);
        let context = Ctx::Dict(vec![
            ("task_description".into(), owner.clone()),
            ("final_requirements".into(), owner.clone()),
            (
                "repo_path".into(),
                path_ctx(&py_resolve(&directory.join("main"))?),
            ),
            (
                "worktree_setup".into(),
                Ctx::Dict(vec![
                    ("worktree_path".into(), task_str.clone()),
                    ("branch_name".into(), branch.clone()),
                    ("local_branch".into(), branch.clone()),
                    (
                        "base_ref".into(),
                        Ctx::Str("refs/remotes/origin/main".into()),
                    ),
                ]),
            ),
            ("version_bump".into(), version_bump),
        ]);
        let mut fixture = Fixture {
            context,
            task,
            task_str,
            branch,
            owner,
            allowed: [b"src.txt".to_vec()].into_iter().collect(),
            env: base_env,
        };
        fixture.sync_context()?;
        Ok(fixture)
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        git_in(&self.task, args, &self.env)
    }
}

/// `Fixture.git`: `git -C <repo> ...` with the fixture environment exactly as
/// Python passed it. It deliberately does not use `amplihack_git::command`:
/// Python forwarded inherited repository selectors (P1), and this port must
/// forward the same environment. The program is resolved from that
/// environment's `PATH`, as `subprocess` resolves it.
fn git_in(repo: &Path, args: &[&str], env: &[(OsString, OsString)]) -> Result<Vec<u8>, String> {
    let mut argv: Vec<&[u8]> = vec![b"-C", repo.as_os_str().as_bytes()];
    argv.extend(args.iter().map(|a| a.as_bytes()));
    // Python passes no cwd to Fixture.git; the child inherits the projection
    // process's directory, which also anchors relative PATH entries.
    let here = current_dir()?;
    let result = run_program("git", &argv, &here, env)?;
    if result.rc != 0 {
        return Err(format!(
            "{} != 0 : {}",
            result.rc,
            ctx_repr(Some(&Ctx::from_os_bytes(&result.stderr)))
        ));
    }
    Ok(result.stdout)
}

// ----------------------------------------------------------- PhaseProjection

/// The `Owner::project` request: the same fields the Python driver read.
#[derive(Debug, Clone)]
pub struct ProjectionRequest {
    pub root: PathBuf,
    pub asset: String,
    /// `fixture_owner_paths`, already `os.fsencode`d.
    pub owner_paths: Vec<Vec<u8>>,
    /// The request's `context`, in the order Python parsed it.
    pub context: Ctx,
    /// The `worktree_setup` key that names the task branch. The owner
    /// projection (`Owner::project`) reads `local_branch`; the bounded
    /// checkpoint projection of `test-pragmatic-checkpoints.py`
    /// (`bounded_fixture_projection`) read `branch_name`.
    pub branch_key: &'static str,
}

struct Projection<'a> {
    f: &'a mut Fixture,
    plan: PhasePlan,
    capture: Option<Ctx>,
    skipped: Option<Ctx>,
}

/// The standard base64 alphabet (RFC 4648 section 4).
const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_value(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

fn b64_invalid_symbol(offset: usize, byte: u8) -> String {
    format!("Invalid symbol {byte}, offset {offset}.")
}

/// Standard base64 with padding, as Python's `base64.b64encode(...).decode()`.
pub fn b64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        let symbols = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, symbol) in symbols.iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(B64_ALPHABET[*symbol as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// CPython 3.14 `base64.b64decode(s, validate=True)`: the standard alphabet
/// only, canonical `=` padding required, non-zero trailing bits accepted.
///
/// The refusals and their messages are the ones the `base64` 0.22 engine
/// this replaces reported (`GeneralPurposeConfig` with
/// `decode_allow_trailing_bits(true)` and `DecodePaddingMode::RequireCanonical`),
/// checked in the same order: a stray last byte of a `4n + 1` input first,
/// then every complete quad before the last one, then the final one to four
/// bytes. `checkpoint_projection_parity` compares the two on every short
/// input.
pub fn b64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let rem = input.len() % 4;
    if rem == 1 {
        let last = input[input.len() - 1];
        if last != b'=' && b64_value(last).is_none() {
            return Err(b64_invalid_symbol(input.len() - 1, last));
        }
    }
    let quads_len = input
        .len()
        .saturating_sub(rem)
        .saturating_sub(if rem == 0 { 4 } else { 0 });
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);
    for (quad, chunk) in input[..quads_len].chunks_exact(4).enumerate() {
        let mut n = 0u32;
        for (i, &byte) in chunk.iter().enumerate() {
            let value = b64_value(byte).ok_or_else(|| b64_invalid_symbol(quad * 4 + i, byte))?;
            n |= value << (26 - 6 * i);
        }
        out.extend_from_slice(&n.to_be_bytes()[..3]);
    }
    let mut morsels = [0u32; 4];
    let mut morsel_count = 0usize;
    let mut padding = 0usize;
    let mut first_padding = 0usize;
    for (i, &byte) in input[quads_len..].iter().enumerate() {
        if byte == b'=' {
            if i < 2 {
                return Err(b64_invalid_symbol(quads_len + i, byte));
            }
            if padding == 0 {
                first_padding = i;
            }
            padding += 1;
            continue;
        }
        if padding > 0 {
            return Err(b64_invalid_symbol(quads_len + first_padding, b'='));
        }
        let value = b64_value(byte).ok_or_else(|| b64_invalid_symbol(quads_len + i, byte))?;
        morsels[morsel_count] = value;
        morsel_count += 1;
    }
    if !input.is_empty() && morsel_count < 2 {
        return Err(format!(
            "Invalid input length: {}",
            quads_len + morsel_count
        ));
    }
    if !(padding + morsel_count).is_multiple_of(4) {
        return Err("Invalid padding".to_owned());
    }
    let n = (morsels[0] << 26) | (morsels[1] << 20) | (morsels[2] << 14) | (morsels[3] << 8);
    out.extend_from_slice(&n.to_be_bytes()[..morsel_count * 6 / 8]);
    Ok(out)
}

impl Projection<'_> {
    fn execute(&mut self, sid: &str) -> Result<ShellResult, String> {
        let block = self.plan.block(sid)?.to_owned();
        assert_equal(
            try_scalar(&block, "type")?.as_ref(),
            &Ctx::Str("bash".into()),
        )?;
        let command = literal(&block, "command")?;
        let rendered = render_bytes(&command, &self.f.context)?;
        let result = run_block(&rendered, &self.f.task, &self.f.env)?;
        let output = try_scalar(&block, "output")?;
        if let Some(output) = output.filter(Ctx::truthy).filter(|_| result.rc == 0) {
            let value = if try_scalar(&block, "parse_json")? == Some(Ctx::Bool(true)) {
                projected_json_output(&result.stdout)
                    .map_err(|error| format!("Malformed deterministic output for {sid}: {error}"))?
            } else {
                decode_projected_stdout(&result.stdout)
            };
            let Ctx::Str(key) = &output else {
                return Err(format!(
                    "TypeError: can only concatenate str (not {}) to str",
                    ctx_repr(Some(&output))
                ));
            };
            self.f.context.insert(key, value);
            self.f.sync_context()?;
        }
        Ok(result)
    }

    fn output_key(&self, sid: &str) -> Result<Option<Ctx>, String> {
        try_scalar(self.plan.block(sid)?, "output")
    }

    fn capture_scope(&mut self) -> Result<ShellResult, String> {
        let mut result = None;
        for sid in self.plan.before.clone() {
            let step = self.execute(&sid)?;
            if step.rc != 0 {
                return Ok(step);
            }
            let value = match self.output_key(&sid)? {
                Some(Ctx::Str(key)) => self.f.context.get(&key).cloned(),
                _ => None,
            };
            if let Some(value @ Ctx::Dict(_)) = value {
                if value.get("skipped") == Some(&Ctx::Bool(true)) {
                    if value.get("admitted") != Some(&Ctx::Bool(false)) {
                        return Err(format!("{} is not False", ctx_repr(value.get("admitted"))));
                    }
                    let keys: BTreeSet<&str> = match &value {
                        Ctx::Dict(items) => items.iter().map(|(k, _)| k.as_str()).collect(),
                        _ => BTreeSet::new(),
                    };
                    let want: BTreeSet<&str> = ["skipped", "admitted", "reason"].into();
                    if keys != want {
                        return Err(format!("Items differ: {keys:?} != {want:?}"));
                    }
                    self.skipped = Some(value);
                    return Ok(step);
                }
                if ["binding", "paths", "outgoing_commits"]
                    .iter()
                    .all(|k| value.get(k).is_some())
                {
                    self.capture = Some(value);
                }
            }
            result = Some(step);
        }
        if self.capture.is_none() {
            return Err(
                "unexpectedly None : The deterministic capture must expose typed, lossless identities."
                    .into(),
            );
        }
        result.ok_or_else(|| "A deterministic capture must precede the scope reviewer.".into())
    }

    fn verdict(&self) -> Result<Ctx, String> {
        let capture = self.capture.as_ref().ok_or("unexpectedly None")?;
        let rows = match require(capture, "paths")? {
            Ctx::List(rows) => rows,
            other => {
                return Err(format!(
                    "TypeError: capture paths are not a list: {}",
                    ctx_repr(Some(other))
                ));
            }
        };
        let mut paths: Vec<Vec<u8>> = Vec::with_capacity(rows.len());
        for row in rows {
            let encoded = match require(row, "path_b64")? {
                Ctx::Str(s) if s.is_ascii() => s.clone(),
                Ctx::Str(_) | Ctx::Raw(_) => {
                    return Err(
                        "ValueError: string argument should contain only ASCII characters".into(),
                    );
                }
                other => {
                    return Err(format!(
                        "TypeError: argument should be a bytes-like object or ASCII string, not {}",
                        ctx_repr(Some(other))
                    ));
                }
            };
            paths.push(b64_decode(encoded.as_bytes()).map_err(|e| format!("binascii.Error: {e}"))?);
        }
        let unique: BTreeSet<Vec<u8>> = paths.iter().cloned().collect();
        let sorted: Vec<Vec<u8>> = unique.iter().cloned().collect();
        if paths != sorted {
            return Err(
                "Lists differ : Capture ordering must preserve unique filename bytes.".into(),
            );
        }
        let binding = require(capture, "binding")?;
        if !matches!(binding, Ctx::Dict(_)) {
            return Err("AttributeError: the binding has no keys()".into());
        }
        if !BINDING_FIELDS.iter().all(|k| binding.get(k).is_some()) {
            return Err("False is not true".into());
        }
        assert_equal(binding.get("owner_task"), &self.f.owner)?;
        assert_equal(binding.get("worktree"), &self.f.task_str)?;
        assert_equal(binding.get("branch"), &self.f.branch)?;
        let head = self.f.git(&["rev-parse", "HEAD"])?;
        assert_equal(binding.get("initial_head"), &ascii_strip_decode(&head)?)?;
        let git_dir = self.f.git(&["rev-parse", "--absolute-git-dir"])?;
        assert_equal(binding.get("git_dir"), &ascii_strip_decode(&git_dir)?)?;
        match binding.get("candidate_digest") {
            Some(Ctx::Str(digest)) => {
                // `re.search(r"^[0-9a-f]{64}$", s)`: `$` also matches before a
                // final newline.
                let body = digest.strip_suffix('\n').unwrap_or(digest);
                if !(body.len() == 64
                    && body.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                {
                    return Err(format!(
                        "Regex didn't match: '^[0-9a-f]{{64}}$' not found in {}",
                        ctx_repr(Some(&Ctx::Str(digest.clone())))
                    ));
                }
            }
            other => {
                return Err(format!(
                    "TypeError: expected string or bytes-like object, got {}",
                    ctx_repr(other)
                ));
            }
        }
        let approved = unique.iter().all(|p| self.f.allowed.contains(p));
        let approved_paths: Vec<Ctx> = paths
            .iter()
            .filter(|p| self.f.allowed.contains(*p))
            .map(|p| Ctx::Str(b64_encode(p)))
            .collect();
        Ok(Ctx::Dict(vec![
            ("approved".into(), Ctx::Bool(approved)),
            ("binding".into(), binding.clone()),
            ("approved_paths".into(), Ctx::List(approved_paths)),
            (
                "reason".into(),
                Ctx::Str(
                    "The fixture owner grants these finite task paths; this is not code approval."
                        .into(),
                ),
            ),
        ]))
    }

    fn finish(&mut self, verdict: Ctx) -> Result<ShellResult, String> {
        let reviewer = self.plan.reviewer.clone();
        let output = self.output_key(&reviewer)?.ok_or("unexpectedly None")?;
        let Ctx::Str(key) = &output else {
            return Err(format!(
                "TypeError: can only concatenate str (not {}) to str",
                ctx_repr(Some(&output))
            ));
        };
        self.f.context.insert(key, verdict);
        self.f.sync_context()?;
        let mut steps = self.plan.after.clone();
        steps.push(self.plan.checkpoint.clone());
        let mut result = None;
        for sid in steps {
            let step = self.execute(&sid)?;
            if step.rc != 0 {
                return Ok(step);
            }
            result = Some(step);
        }
        result.ok_or_else(|| "the checkpoint did not run".into())
    }

    fn run(&mut self) -> Result<ShellResult, String> {
        let result = self.capture_scope()?;
        if result.rc != 0 {
            return Ok(result);
        }
        if self.skipped.is_some() {
            // An existing no-operation cleanup skip never invents a reviewer
            // vote, path permission or admitted checkpoint.
            let checkpoint = self.plan.checkpoint.clone();
            return self.execute(&checkpoint);
        }
        let verdict = self.verdict()?;
        self.finish(verdict)
    }
}

/// `bytes.strip().decode()`: ASCII whitespace only, then strict UTF-8.
fn ascii_strip_decode(raw: &[u8]) -> Result<Ctx, String> {
    let is_ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = raw.iter().position(|b| !is_ws(b)).unwrap_or(raw.len());
    let end = raw.iter().rposition(|b| !is_ws(b)).map_or(start, |i| i + 1);
    std::str::from_utf8(&raw[start..end.max(start)])
        .map(|s| Ctx::Str(s.to_owned()))
        .map_err(|e| format!("UnicodeDecodeError: 'utf-8' codec can't decode git output: {e}"))
}

fn run_phase(f: &mut Fixture, root: &Path, asset: &str) -> Result<ShellResult, String> {
    let (_, checkpoint, _, _) = PHASES
        .iter()
        .find(|p| p.0 == asset)
        .copied()
        .ok_or_else(|| "StopIteration".to_owned())?;
    let rel = format!("amplifier-bundle/recipes/{asset}.yaml");
    let current_path = root.join(&rel);
    let current = fs::read_to_string(&current_path)
        .map_err(|e| format!("FileNotFoundError: {}: {e}", current_path.display()))?;
    let original = index_source(root, &rel)?;
    let plan = PhasePlan::from_texts(&current, &original, checkpoint)?;
    Projection {
        f,
        plan,
        capture: None,
        skipped: None,
    }
    .run()
}

/// `PhaseProjection(fixture, phase).run()` for the `Owner::project` driver.
/// `env` is the projection process's environment (`os.environ` in the
/// Python driver); `None` entries are ignored and nothing else is removed.
pub fn project(req: &ProjectionRequest, env: &[(OsString, Option<OsString>)]) -> ShellResult {
    let outcome =
        Fixture::for_owner(req, env).and_then(|mut f| run_phase(&mut f, &req.root, &req.asset));
    outcome.unwrap_or_else(|message| ShellResult::contract_failure(&message))
}

/// The #1671 `publish-projection.py` heredoc: the workflow-publish phase for
/// the case directory `directory`, with `env` standing for `os.environ`.
pub fn project_1671(
    root: &Path,
    directory: &Path,
    env: &[(OsString, Option<OsString>)],
) -> ShellResult {
    let outcome =
        Fixture::for_1671(directory, env).and_then(|mut f| run_phase(&mut f, root, PHASES[0].0));
    outcome.unwrap_or_else(|message| ShellResult::contract_failure(&message))
}

/// PEP 538 locale coercion as the isolated (`-I`) Python driver applied it to its own
/// environment before any child starts: with `LC_ALL` unset or empty and a
/// C or POSIX `LC_CTYPE` locale, `LC_CTYPE=C.UTF-8` is added. The oracle's
/// `env -0` probe confirms or corrects this per call site; `-I` makes
/// `PYTHONCOERCECLOCALE` ineffective.
pub fn with_python_locale_coercion(
    env: &[(OsString, Option<OsString>)],
) -> Vec<(OsString, Option<OsString>)> {
    let get = |key: &str| -> Option<OsString> {
        env.iter()
            .rev()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let mut out = env.to_vec();
    if get("LC_ALL").is_some() {
        return out;
    }
    let ctype = get("LC_CTYPE").or_else(|| get("LANG"));
    let is_c = ctype.as_deref().is_none_or(|v| v == "C" || v == "POSIX");
    if is_c {
        out.push(("LC_CTYPE".into(), Some("C.UTF-8".into())));
    }
    out
}

// ------------------------------------------------------- #1671 re-entry shim

/// The libtest path of the re-entry test in the `amplihack` integration
/// binary. The bridge test, the shim and the inventory exclusion all use it.
pub const REENTRY_TEST: &str = "issue_1671_hook_rejection_repair::phase_projection_reentry";

/// Single-quote a value for the shim's own text.
fn sh_quote(raw: &[u8]) -> Vec<u8> {
    let mut out = b"'".to_vec();
    for &b in raw {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// The shim body. Arguments: REPO_ROOT DIR. It writes a request into a new
/// private directory under `request_root` (the shim's own directory, never
/// the case directory the driver inspects), re-runs `test_binary` with only
/// `test_name` selected and `AMPLIHACK_PROJECTION_REQUEST` naming that
/// directory, and then fails closed: a missing `out.bin` or `err.bin`, an
/// `rc` that is not 0..255, or a non-zero test-binary status exits 70 with a
/// message naming the request directory, which is kept as evidence. The test
/// binary's own output goes to files, never to the projected stream.
///
/// The driver starts the shim with `env -u BASH_ENV -u ENV`, and the shim
/// removes both again before the re-entry, so no startup file runs in the
/// shim, the test binary or the projection's own bash blocks. The request
/// carries only the two arguments.
fn shim_text(bash: &Path, request_root: &Path, test_binary: &Path, test_name: &str) -> Vec<u8> {
    let mut text = Vec::new();
    text.extend_from_slice(b"#!");
    text.extend_from_slice(bash.as_os_str().as_bytes());
    text.extend_from_slice(b"\n");
    text.extend_from_slice(
        br#"# #1671 projection shim, written by the Rust bridge test (#1554).
# There is no [[bin]] or [[test]] helper target: crates/amplihack-cli has
# autotests = false and a frozen manifest, so this re-enters the test binary.
# Bash 3.2 compatible. Fails closed: no output file, a malformed rc or a
# failed re-entry is exit 70, never a pass.
set -u
unset BASH_ENV ENV
"#,
    );
    text.extend_from_slice(b"test_bin=");
    text.extend_from_slice(&sh_quote(test_binary.as_os_str().as_bytes()));
    text.extend_from_slice(b"\ntest_name=");
    text.extend_from_slice(&sh_quote(test_name.as_bytes()));
    text.extend_from_slice(b"\nrequest_root=");
    text.extend_from_slice(&sh_quote(request_root.as_os_str().as_bytes()));
    text.extend_from_slice(b"\nrequest_var=");
    text.extend_from_slice(&sh_quote(PROJECTION_REQUEST_ENV.as_bytes()));
    text.extend_from_slice(
        br#"
fail() {
    printf 'projection shim: %s\n' "$1" >&2
    exit 70
}
hex() {
    printf '%s' "$1" | od -An -v -tx1 | tr -d ' \n'
}
[ "$#" -eq 2 ] || fail "usage: <REPO_ROOT> <DIR> (got $# arguments)"
[ -d "$2" ] || fail "case directory $2 does not exist"
[ -x "$test_bin" ] || fail "test binary $test_bin is not executable"
[ -d "$request_root" ] || fail "request directory root $request_root does not exist"
req=$(mktemp -d "$request_root/projection-request.XXXXXX") || fail "cannot create a request directory in $request_root"
[ -n "$req" ] && [ -d "$req" ] || fail "mktemp gave no request directory"
printf '{"version":2,"root_hex":"%s","dir_hex":"%s"}\n' "$(hex "$1")" "$(hex "$2")" > "$req/request.json" \
    || fail "cannot write $req/request.json"
(
    export "$request_var=$req"
    exec "$test_bin" --exact "$test_name" --quiet --test-threads=1
) < /dev/null > "$req/libtest.out" 2> "$req/libtest.err"
status=$?
[ "$status" -eq 0 ] || fail "re-entry test exited $status; evidence in $req"
[ -f "$req/out.bin" ] || fail "re-entry wrote no out.bin; evidence in $req"
[ -f "$req/err.bin" ] || fail "re-entry wrote no err.bin; evidence in $req"
[ -f "$req/rc" ] || fail "re-entry wrote no rc; evidence in $req"
rc=$(cat "$req/rc"; printf x) || fail "cannot read $req/rc; evidence in $req"
rc=${rc%x}
case "$rc" in
    ''|*[!0-9]*) fail "malformed rc '$rc'; evidence in $req" ;;
esac
[ "${#rc}" -le 3 ] || fail "malformed rc '$rc'; evidence in $req"
rc=$((10#$rc))
[ "$rc" -le 255 ] || fail "rc $rc is out of range; evidence in $req"
cat "$req/out.bin" || fail "cannot copy out.bin; evidence in $req"
cat "$req/err.bin" >&2 || fail "cannot copy err.bin; evidence in $req"
rm -rf -- "$req"
exit "$rc"
"#,
    );
    text
}

/// Write the #1671 shim into `dir` and return its path. `bash` is the GNU
/// bash for the `#!` line, chosen and recorded by the caller. `test_binary`
/// must be the running test binary (`std::env::current_exe()`), never a name
/// looked up on `PATH`. Request directories are created in `dir`.
pub fn write_projection_shim(
    dir: &Path,
    bash: &Path,
    test_binary: &Path,
    test_name: &str,
) -> PathBuf {
    assert!(
        bash.is_absolute() && test_binary.is_absolute() && dir.is_absolute(),
        "the shim needs absolute bash, test binary and directory paths"
    );
    assert!(
        !test_name.is_empty() && !test_name.contains(['\n', '\0']),
        "bad re-entry test name {test_name:?}"
    );
    let path = dir.join("projection-shim.sh");
    fs::write(&path, shim_text(bash, dir, test_binary, test_name))
        .expect("write the projection shim");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("make the shim executable");
    path
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("malformed hex field {text:?}"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Write `name` in `dir` atomically: a temporary file, then a rename.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) {
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut file =
        fs::File::create(&tmp).unwrap_or_else(|e| panic!("create {}: {e}", tmp.display()));
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .unwrap_or_else(|e| panic!("write {}: {e}", tmp.display()));
    drop(file);
    fs::rename(&tmp, dir.join(name)).unwrap_or_else(|e| panic!("rename {}: {e}", tmp.display()));
}

/// The body of the `phase_projection_reentry` test. Without
/// `AMPLIHACK_PROJECTION_REQUEST` it returns at once (an ordinary
/// `cargo test` run). With it, it reads the shim's request, runs the #1671
/// projection with this process's environment standing for `os.environ`
/// (without `BASH_ENV` and `ENV`, which the shim removed), and writes `out.bin`, `err.bin` and then `rc`, each atomically. Any
/// malformed request panics before `rc` exists, so the shim fails closed.
pub fn reentry_main() {
    let Some(dir) = std::env::var_os(PROJECTION_REQUEST_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let raw = fs::read(dir.join("request.json"))
        .unwrap_or_else(|e| panic!("read {}/request.json: {e}", dir.display()));
    let request: serde_json::Value =
        serde_json::from_slice(&raw).unwrap_or_else(|e| panic!("request.json: {e}"));
    let object = request.as_object().expect("request.json is an object");
    let expected: BTreeSet<&str> = ["version", "root_hex", "dir_hex"].into();
    let found: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(found, expected, "request.json fields");
    assert_eq!(
        object["version"],
        serde_json::json!(2),
        "request.json version"
    );
    let field = |key: &str| -> Vec<u8> {
        match &object[key] {
            serde_json::Value::String(s) => unhex(s).unwrap_or_else(|e| panic!("{key}: {e}")),
            other => panic!("{key} must be a hex string, not {other}"),
        }
    };
    let root = PathBuf::from(OsString::from_vec(field("root_hex")));
    let case = PathBuf::from(OsString::from_vec(field("dir_hex")));
    // `map(pathlib.Path, sys.argv[1:])` normalises both arguments.
    let root = PathBuf::from(OsString::from_vec(py_path_str(root.as_os_str().as_bytes())));
    let case = PathBuf::from(OsString::from_vec(py_path_str(case.as_os_str().as_bytes())));
    let env: Vec<(OsString, Option<OsString>)> = std::env::vars_os()
        .filter(|(k, _)| k != PROJECTION_REQUEST_ENV && k != "BASH_ENV" && k != "ENV")
        .map(|(k, v)| (k, Some(v)))
        .collect();
    let env = with_python_locale_coercion(&env);
    let result = project_1671(&root, &case, &env);
    write_atomic(&dir, "out.bin", &result.stdout);
    write_atomic(&dir, "err.bin", &result.stderr);
    write_atomic(&dir, "rc", result.exit_code().to_string().as_bytes());
}
