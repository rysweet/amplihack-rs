//! The refusal text 918c's `parse` printed for a document it rejected.
//!
//! 918c ran `json.loads` with the `pairs`, `invalid_constant`,
//! `finite_number` and `integer_number` hooks and then `json_strings`
//! (`workflow_publish_checkpoint.sh:15-50` at 918c256cf). This scanner
//! follows CPython 3.14's C scanner in the same order: scanning stops at the
//! first syntax error or hook refusal (the number and constant hooks fire as
//! each token is scanned, the duplicate-key hook as each object closes), and
//! the UTF-8 check runs only after a complete parse. Positions are code-point
//! indices, as `JSONDecodeError` reports them.
//!
//! It only diagnoses. [`super::json::parse`] builds the value once this
//! finds nothing to refuse.

/// Nesting deeper than this is left to the value parser, whose own limit
/// applies first.
const MAX_DEPTH: usize = 128;

pub(crate) enum Diagnosis {
    /// 918c accepted the document.
    Accepted,
    /// 918c refused it with this text.
    Refused(String),
    /// Nested past [`MAX_DEPTH`]; not diagnosed here.
    TooDeep,
}

enum Stop {
    Refused(String),
    TooDeep,
}

type Scan<T> = Result<T, Stop>;

/// `str(JSONDecodeError(msg, s, pos))`.
fn decode_error(s: &[char], msg: &str, pos: usize) -> Stop {
    let line = s[..pos].iter().filter(|c| **c == '\n').count() + 1;
    let col = match s[..pos].iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    Stop::Refused(format!("{msg}: line {line} column {col} (char {pos})"))
}

fn refuse<T>(text: impl Into<String>) -> Scan<T> {
    Err(Stop::Refused(text.into()))
}

struct Scanner<'a> {
    s: &'a [char],
    depth: usize,
    /// `json_strings`' refusal for the first string (in document order)
    /// holding a lone surrogate; reported only after a complete parse.
    surrogate: Option<String>,
}

impl Scanner<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }
    fn ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }
    fn err<T>(&self, msg: &str, pos: usize) -> Scan<T> {
        Err(decode_error(self.s, msg, pos))
    }
    fn starts(&self, i: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(k, c)| self.at(i + k) == Some(c))
    }

    /// `scan_once`; returns the index just past the value.
    fn value(&mut self, i: usize) -> Scan<usize> {
        match self.at(i) {
            Some('"') => self.string(i + 1).map(|(_, end)| end),
            Some('{') => self.nested(i, Self::object),
            Some('[') => self.nested(i, Self::array),
            Some('n') if self.starts(i, "null") => Ok(i + 4),
            Some('t') if self.starts(i, "true") => Ok(i + 4),
            Some('f') if self.starts(i, "false") => Ok(i + 5),
            Some('N') if self.starts(i, "NaN") => refuse("non-JSON numeric constant"),
            Some('I') if self.starts(i, "Infinity") => refuse("non-JSON numeric constant"),
            Some('-') if self.starts(i, "-Infinity") => refuse("non-JSON numeric constant"),
            Some('-' | '0'..='9') => self.number(i),
            _ => self.err("Expecting value", i),
        }
    }

    fn nested(&mut self, i: usize, f: fn(&mut Self, usize) -> Scan<usize>) -> Scan<usize> {
        if self.depth >= MAX_DEPTH {
            return Err(Stop::TooDeep);
        }
        self.depth += 1;
        let out = f(self, i + 1);
        self.depth -= 1;
        out
    }

    fn object(&mut self, i: usize) -> Scan<usize> {
        let mut keys: Vec<Vec<u32>> = Vec::new();
        let mut i = self.ws(i);
        if self.at(i) == Some('}') {
            return Ok(i + 1);
        }
        loop {
            if self.at(i) != Some('"') {
                return self.err("Expecting property name enclosed in double quotes", i);
            }
            let (key, end) = self.string(i + 1)?;
            keys.push(key);
            i = self.ws(end);
            if self.at(i) != Some(':') {
                return self.err("Expecting ':' delimiter", i);
            }
            i = self.ws(i + 1);
            let end = self.value(i)?;
            i = self.ws(end);
            match self.at(i) {
                Some('}') => break,
                Some(',') => {}
                _ => return self.err("Expecting ',' delimiter", i),
            }
            let comma = i;
            i = self.ws(i + 1);
            if self.at(i) == Some('}') {
                return self.err("Illegal trailing comma before end of object", comma);
            }
        }
        // The `pairs` hook runs as the object closes.
        if (1..keys.len()).any(|n| keys[..n].contains(&keys[n])) {
            return refuse("duplicate JSON field");
        }
        Ok(i + 1)
    }

    fn array(&mut self, i: usize) -> Scan<usize> {
        let mut i = self.ws(i);
        if self.at(i) == Some(']') {
            return Ok(i + 1);
        }
        loop {
            let end = self.value(i)?;
            i = self.ws(end);
            match self.at(i) {
                Some(']') => return Ok(i + 1),
                Some(',') => {}
                _ => return self.err("Expecting ',' delimiter", i),
            }
            let comma = i;
            i = self.ws(i + 1);
            if self.at(i) == Some(']') {
                return self.err("Illegal trailing comma before end of array", comma);
            }
        }
    }

    /// `scanstring_unicode(strict=True)`; `start` is just after the quote.
    fn string(&mut self, start: usize) -> Scan<(Vec<u32>, usize)> {
        let begin = start - 1;
        let mut out = Vec::new();
        let mut i = start;
        loop {
            let Some(c) = self.at(i) else {
                return self.err("Unterminated string starting at", begin);
            };
            match c {
                '"' => break,
                '\\' => {}
                c if (c as u32) <= 0x1f => return self.err("Invalid control character at", i),
                c => {
                    out.push(c as u32);
                    i += 1;
                    continue;
                }
            }
            let Some(esc) = self.at(i + 1) else {
                return self.err("Unterminated string starting at", begin);
            };
            let short = match esc {
                '"' => Some('"'),
                '\\' => Some('\\'),
                '/' => Some('/'),
                'b' => Some('\u{8}'),
                'f' => Some('\u{c}'),
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                'u' => None,
                _ => return self.err("Invalid \\escape", i),
            };
            if let Some(c) = short {
                out.push(c as u32);
                i += 2;
                continue;
            }
            // CPython reports a bad \u escape at the 'u'.
            let u = i + 1;
            if u + 5 > self.s.len() {
                return self.err("Invalid \\uXXXX escape", u);
            }
            let Some(mut cp) = self.hex4(u + 1) else {
                return self.err("Invalid \\uXXXX escape", u);
            };
            let mut end = u + 5;
            if (0xD800..0xDC00).contains(&cp)
                && end + 6 < self.s.len()
                && self.at(end) == Some('\\')
                && self.at(end + 1) == Some('u')
            {
                let Some(low) = self.hex4(end + 2) else {
                    return self.err("Invalid \\uXXXX escape", end + 1);
                };
                if (0xDC00..0xE000).contains(&low) {
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                    end += 6;
                }
            }
            out.push(cp);
            i = end;
        }
        if self.surrogate.is_none()
            && let Some(pos) = out.iter().position(|c| (0xD800..0xE000).contains(c))
        {
            self.surrogate = Some(format!(
                "'utf-8' codec can't encode character '\\u{:04x}' in position {pos}: surrogates not allowed",
                out[pos]
            ));
        }
        Ok((out, i + 1))
    }

    fn hex4(&self, i: usize) -> Option<u32> {
        (0..4).try_fold(0u32, |acc, k| {
            Some(acc * 16 + self.at(i + k)?.to_digit(16)?)
        })
    }

    /// `_match_number_unicode`, then 918c's `parse_int` / `parse_float`.
    fn number(&self, start: usize) -> Scan<usize> {
        let digit = |i: usize| matches!(self.at(i), Some('0'..='9'));
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('1'..='9') => {
                while digit(i) {
                    i += 1;
                }
            }
            Some('0') => i += 1,
            _ => return self.err("Expecting value", start),
        }
        let mut float = false;
        if self.at(i) == Some('.') && digit(i + 1) {
            float = true;
            i += 1;
            while digit(i) {
                i += 1;
            }
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let e = i;
            i += 1;
            if matches!(self.at(i), Some('-' | '+')) {
                i += 1;
            }
            let first = i;
            while digit(i) {
                i += 1;
            }
            if i > first {
                float = true;
            } else {
                i = e;
            }
        }
        let raw: String = self.s[start..i].iter().collect();
        if float {
            finite_number(&raw)?;
        } else {
            integer_number(&raw)?;
        }
        Ok(i)
    }
}

fn finite_number(raw: &str) -> Scan<()> {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(()),
        _ => refuse("non-finite JSON number"),
    }
}

fn integer_number(raw: &str) -> Scan<()> {
    let digits = raw.trim_start_matches('-').len();
    if digits > 4300 {
        return refuse(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
        ));
    }
    match raw.parse::<i128>() {
        Ok(v) if (-(1i128 << 63)..(1i128 << 64)).contains(&v) => Ok(()),
        _ => finite_number(raw),
    }
}

/// What 918c's `parse(raw)` did with this text.
pub(crate) fn diagnose(raw: &str) -> Diagnosis {
    let s: Vec<char> = raw.chars().collect();
    let mut sc = Scanner {
        s: &s,
        depth: 0,
        surrogate: None,
    };
    let outcome = if s.first() == Some(&'\u{feff}') {
        Err(decode_error(
            &s,
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            0,
        ))
    } else {
        let start = sc.ws(0);
        sc.value(start).and_then(|end| {
            let end = sc.ws(end);
            if end == s.len() {
                Ok(())
            } else {
                Err(decode_error(&s, "Extra data", end))
            }
        })
    };
    match (outcome, sc.surrogate) {
        (Err(Stop::Refused(text)), _) => Diagnosis::Refused(text),
        (Err(Stop::TooDeep), _) => Diagnosis::TooDeep,
        (Ok(()), Some(text)) => Diagnosis::Refused(text),
        (Ok(()), None) => Diagnosis::Accepted,
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnosis, diagnose};

    fn text(raw: &str) -> String {
        match diagnose(raw) {
            Diagnosis::Refused(text) => text,
            Diagnosis::Accepted => format!("<accepted {raw:?}>"),
            Diagnosis::TooDeep => format!("<too deep {raw:?}>"),
        }
    }

    /// Each row is CPython 3.14's `str(e)` for 918c's `parse(raw)`.
    #[test]
    fn decode_errors_match_cpython_3_14() {
        let cases = [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            (
                "\u{feff}{}",
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
            ),
            (
                "\"\\u12\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            (
                "\"\\ud800\\u12g4x\"",
                "Invalid \\uXXXX escape: line 1 column 9 (char 8)",
            ),
            (
                "{\"a\" 1}",
                "Expecting ':' delimiter: line 1 column 6 (char 5)",
            ),
            (
                "{1:1}",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            (
                "{\"a\":1 2}",
                "Expecting ',' delimiter: line 1 column 8 (char 7)",
            ),
            (
                "[1,]",
                "Illegal trailing comma before end of array: line 1 column 3 (char 2)",
            ),
            (
                "{\"a\":1,}",
                "Illegal trailing comma before end of object: line 1 column 7 (char 6)",
            ),
            (
                "\"ab",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            (
                "\"a\nb\"",
                "Invalid control character at: line 1 column 3 (char 2)",
            ),
            ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            (
                "\"\\u12g4\"",
                "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
            ),
            ("1 2", "Extra data: line 1 column 3 (char 2)"),
            ("\n\n x", "Expecting value: line 3 column 2 (char 3)"),
            ("1.", "Extra data: line 1 column 2 (char 1)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("{\"a\":1,\"a\":2}", "duplicate JSON field"),
            ("[NaN]", "non-JSON numeric constant"),
            ("-Infinity", "non-JSON numeric constant"),
            ("1e400", "non-finite JSON number"),
            (
                "\"\\ud800\"",
                "'utf-8' codec can't encode character '\\ud800' in position 0: surrogates not allowed",
            ),
        ];
        for (raw, want) in cases {
            assert_eq!(text(raw), want, "input {raw:?}");
        }
    }

    /// The order is 918c's: a syntax error after a duplicate key in an
    /// unclosed object wins, a hook refusal before a later syntax error wins,
    /// and the surrogate check waits for a complete parse.
    #[test]
    fn the_first_refusal_in_reference_order_is_reported() {
        assert_eq!(
            text("{\"a\":1,\"a\":2"),
            "Expecting ',' delimiter: line 1 column 13 (char 12)"
        );
        assert_eq!(text("[{\"a\":1,\"a\":2}, x]"), "duplicate JSON field");
        assert_eq!(
            text("[\"\\udc00\", x]"),
            "Expecting value: line 1 column 12 (char 11)"
        );
        assert_eq!(text("[NaN, x]"), "non-JSON numeric constant");
    }

    #[test]
    fn accepted_documents_and_integer_bounds() {
        for raw in [
            "{}",
            " [1, -0, 2.5e3, \"\\ud83d\\ude00\"] ",
            "18446744073709551615",
            "-9223372036854775808",
            "1e308",
        ] {
            assert!(matches!(diagnose(raw), Diagnosis::Accepted), "{raw:?}");
        }
        assert!(matches!(diagnose(&"[".repeat(200)), Diagnosis::TooDeep));
    }
}
