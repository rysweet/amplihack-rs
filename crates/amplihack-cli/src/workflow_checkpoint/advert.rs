//! Remote advertisement validation. Pure: no file system and no processes.
//!
//! An advertisement is the stdout of `git ls-remote --exit-code`. The rules
//! are the two 918c256cf parsers, which agree on what they accept:
//! `ORIGIN_REF_PY` (`workflow_pr_head_sync.sh:83-92`) and `destination_tip`
//! (`workflow_publish_checkpoint.sh:336-360`).
//!
//! * Records are `bytes.splitlines()`: split on `\n`, `\r\n` and `\r`, with no
//!   empty final record. A last record without a line feed and a CRLF record
//!   are therefore accepted (owner decision D1).
//! * Each record is exactly `<oid>\t<name>`. `<name>` is `HEAD` or starts with
//!   `refs/`, and is kept as bytes, NUL and other control bytes included.
//! * Each `<oid>` is 40 or 64 lowercase hex digits on its own; one
//!   advertisement may mix the widths (D1). No object-format check is made.
//! * No name repeats. Transport status 2 with any record is an ambiguous
//!   absence. Any other transport status is the caller's concern.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::num::NonZeroI32;

/// `ORIGIN_REF_PY`'s `sys.exit(78)`.
pub(crate) const EXIT_78: NonZeroI32 = match NonZeroI32::new(78) {
    Some(code) => code,
    None => panic!("78 is not zero"),
};

/// Why an advertisement was refused, in the order 918c checked each record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AdvertError {
    /// Not two TAB-separated fields, or a name outside `HEAD`/`refs/`.
    Malformed,
    /// `destination_tip`'s `parts[0].decode('ascii')` raised
    /// `UnicodeDecodeError` at this byte and position.
    NonAscii { byte: u8, position: usize },
    /// Not 40 or 64 lowercase hex digits.
    InvalidOid,
    /// A name advertised twice.
    Duplicate,
    /// Transport status 2 with records.
    AmbiguousAbsent,
}

impl AdvertError {
    /// The `destination_tip` refusal text for this error.
    pub(crate) fn checkpoint_reason(&self) -> String {
        match self {
            Self::Malformed => "malformed publication advertisement".into(),
            Self::NonAscii { byte, position } => format!(
                "'ascii' codec can't decode byte 0x{byte:02x} in position {position}: ordinal not in range(128)"
            ),
            Self::InvalidOid => "invalid publication destination identity".into(),
            Self::Duplicate => "duplicate publication advertisement".into(),
            Self::AmbiguousAbsent => "ambiguous absent publication destination".into(),
        }
    }
}

/// A fully validated advertisement. Only [`AdvertMap::parse`] builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdvertMap {
    refs: BTreeMap<Vec<u8>, String>,
}

fn valid_oid(oid: &[u8]) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

impl AdvertMap {
    /// Validate `raw`, the output of a transport that exited `rc`.
    pub(crate) fn parse(raw: &[u8], rc: i32) -> Result<Self, AdvertError> {
        let mut refs = BTreeMap::new();
        for line in lines(raw) {
            let parts: Vec<&[u8]> = line.split(|&b| b == b'\t').collect();
            let [oid, name] = parts[..] else {
                return Err(AdvertError::Malformed);
            };
            if !(name == b"HEAD" || name.starts_with(b"refs/")) {
                return Err(AdvertError::Malformed);
            }
            if let Some(position) = oid.iter().position(|b| !b.is_ascii()) {
                return Err(AdvertError::NonAscii {
                    byte: oid[position],
                    position,
                });
            }
            if !valid_oid(oid) {
                return Err(AdvertError::InvalidOid);
            }
            let oid = String::from_utf8(oid.to_vec()).map_err(|_| AdvertError::InvalidOid)?;
            match refs.entry(name.to_vec()) {
                Entry::Occupied(_) => return Err(AdvertError::Duplicate),
                Entry::Vacant(slot) => {
                    slot.insert(oid);
                }
            }
        }
        if rc == 2 && !refs.is_empty() {
            return Err(AdvertError::AmbiguousAbsent);
        }
        Ok(Self { refs })
    }

    /// The exact ref's object ID, matched byte for byte.
    pub(crate) fn tip(&self, name: &[u8]) -> Option<&str> {
        self.refs.get(name).map(String::as_str)
    }

    /// Every advertised object ID.
    pub(crate) fn oids(&self) -> impl Iterator<Item = &str> {
        self.refs.values().map(String::as_str)
    }
}

/// ORIGIN_REF_PY's decision, the only first-publication decision: `Present`
/// (exit 0) when the exact `target` is advertised, `Absent` (exit 3, the one
/// status that lets the caller skip the fetch) when a fully validated
/// advertisement lacks it, and the refusal otherwise (exit 78). `rc` is the
/// transport status after normalisation; only 2 changes the result.
pub(crate) fn origin_ref_lookup(
    raw: &[u8],
    target: &[u8],
    rc: i32,
) -> Result<super::Lookup, AdvertError> {
    let advertisement = AdvertMap::parse(raw, rc)?;
    Ok(match advertisement.tip(target) {
        Some(_) => super::Lookup::Present,
        None => super::Lookup::Absent,
    })
}

/// Lines as Python's `bytes.splitlines` returns them: split on `\n`, `\r\n`
/// and `\r` only, keeping interior empty lines, with no empty final line.
pub(crate) fn lines(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut ended = true;
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\n' => {
                out.push(std::mem::take(&mut current));
                ended = true;
            }
            b'\r' => {
                out.push(std::mem::take(&mut current));
                if raw.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                ended = true;
            }
            b => {
                current.push(b);
                ended = false;
            }
        }
        i += 1;
    }
    if !ended {
        out.push(current);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod first_publication_tests {
    use super::super::Lookup;
    use super::*;

    const TASK: &[u8] = b"refs/heads/task";

    fn record(oid_byte: u8, width: usize, name: &str) -> Vec<u8> {
        let mut v = vec![oid_byte; width];
        v.push(b'\t');
        v.extend_from_slice(name.as_bytes());
        v
    }

    fn lookup(raw: &[u8], rc: i32) -> Result<Lookup, AdvertError> {
        origin_ref_lookup(raw, TASK, rc)
    }

    /// The three D1 record shapes at unit level, on synthetic records. These
    /// are not the R8 reference bytes: the binary-level D1 tests build those
    /// from `tests/support/d1_literals.rs` and check their SHA-256.
    #[test]
    fn lookup_accepts_an_unterminated_a_crlf_and_a_mixed_width_record() {
        let no_lf = record(b'a', 40, "refs/heads/task");
        assert_ne!(no_lf.last(), Some(&b'\n'));
        assert_eq!(lookup(&no_lf, 0), Ok(Lookup::Present));

        let mut crlf = no_lf.clone();
        crlf.extend_from_slice(b"\r\n");
        assert!(crlf.windows(2).any(|w| w == b"\r\n"));
        assert_eq!(lookup(&crlf, 0), Ok(Lookup::Present));

        let mut mixed = record(b'a', 64, "refs/heads/a");
        mixed.push(b'\n');
        mixed.extend(record(b'b', 40, "refs/heads/task"));
        mixed.push(b'\n');
        assert_eq!(lookup(&mixed, 0), Ok(Lookup::Present));
    }

    /// The R8 D1 reference input (`no-final-lf`, the original bytes kept in
    /// a local capture, not in the repo): its
    /// length, SHA-256 and missing final LF are checked before the call, so a
    /// literal that a hook "fixed" fails here instead of passing vacuously.
    #[test]
    fn lookup_accepts_the_r8_no_final_lf_reference_bytes() {
        mod d1 {
            include!("../../tests/support/d1_literals.rs");
        }
        let bytes = d1::D1_NO_FINAL_LF;
        assert_eq!(bytes.len(), 56, "R8 no-final-lf length");
        assert_eq!(
            super::super::json::digest(bytes),
            "59161683958a9adeaacaf650be033f302e118cdf7b2f739b2b8ada2ac088b337",
            "R8 no-final-lf sha256"
        );
        assert_ne!(bytes.last(), Some(&b'\n'), "R8 input must not end in LF");
        assert!(d1::d1_property_holds("no-final-lf", bytes));
        assert_eq!(lookup(bytes, 0), Ok(Lookup::Present));
    }

    /// Only a fully valid advertisement without the exact ref is `Absent`;
    /// every refused row would otherwise let the caller skip the fetch.
    #[test]
    fn only_a_fully_valid_advertisement_without_the_ref_is_absent() {
        let mut other = record(b'a', 40, "refs/heads/other");
        other.push(b'\n');
        assert_eq!(lookup(b"", 2), Ok(Lookup::Absent), "empty, rc 2");
        assert_eq!(lookup(b"", 0), Ok(Lookup::Absent), "empty, rc 0");
        assert_eq!(lookup(&other, 0), Ok(Lookup::Absent), "other ref only");

        let refused: Vec<(&str, Vec<u8>, i32)> = vec![
            ("rc 2 with records", other.clone(), 2),
            ("duplicate", [other.clone(), other.clone()].concat(), 0),
            (
                "39 digits",
                [record(b'a', 39, "refs/heads/x"), b"\n".to_vec()].concat(),
                0,
            ),
            (
                "uppercase hex",
                [record(b'A', 40, "refs/heads/x"), b"\n".to_vec()].concat(),
                0,
            ),
            ("symref line", b"ref: refs/heads/main\tHEAD\n".to_vec(), 0),
            (
                "three fields",
                [record(b'a', 40, "refs/heads/x\textra"), b"\n".to_vec()].concat(),
                0,
            ),
            (
                "interior empty record",
                [other.clone(), b"\n".to_vec()].concat(),
                0,
            ),
            (
                "name outside refs/",
                [record(b'a', 40, "bogus"), b"\n".to_vec()].concat(),
                0,
            ),
            (
                "valid then bad",
                [other.clone(), record(b'a', 40, "bogus")].concat(),
                0,
            ),
        ];
        for (label, raw, rc) in refused {
            assert!(
                lookup(&raw, rc).is_err(),
                "{label} must be refused, not skipped"
            );
        }
    }

    /// The ref is matched byte for byte, never by prefix, suffix or case.
    #[test]
    fn the_task_ref_is_matched_exactly() {
        for name in [
            "refs/heads/task-2",
            "refs/heads/tas",
            "refs/heads/Task",
            "refs/remotes/origin/task",
        ] {
            let raw = record(b'a', 40, name);
            assert_eq!(lookup(&raw, 0), Ok(Lookup::Absent), "{name}");
        }
    }
}
