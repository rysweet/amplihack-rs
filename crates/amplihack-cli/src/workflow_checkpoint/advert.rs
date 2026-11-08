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
