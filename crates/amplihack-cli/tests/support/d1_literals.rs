// D1 legacy-parser inputs (#1554, zero-A) as run-time byte literals.
//
// One source for every user: `crates/amplihack-cli/tests/support/mod.rs`
// declares it as a module, and other crates pull it in with
// `include!(concat!(env!("CARGO_MANIFEST_DIR"), "/<rel>/d1_literals.rs"))`.
// No inner attributes and no crate imports, so `include!` works anywhere.
//
// The bytes were dumped with `od -c` from the originals in
// ~/.amplihack/work/pr2020-d1-original-bytes/ (2026-11-07). The staged
// no-final-lf fixture file had already been rewritten by the
// end-of-file-fixer hook (sha256 1ef898c4…, 57 B), which is why these inputs
// are code, not files. Each user writes them into its own tempdir at run
// time and checks length, sha256 and the D1 property before the call.

/// A final record with no trailing LF. 56 bytes.
#[allow(dead_code)]
pub const D1_NO_FINAL_LF: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x09refs/heads/task";

/// One CRLF-terminated record. 58 bytes.
#[allow(dead_code)]
pub const D1_CRLF: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x09refs/heads/task\x0d\x0a";

/// A 64-hex record followed by a 40-hex record. 132 bytes.
#[allow(dead_code)]
pub const D1_MIXED_OID_WIDTHS: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x09refs/heads/a\x0abbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\x09refs/heads/b\x0a";

/// (name, bytes, length, full sha256 of the original file).
#[allow(dead_code)]
pub const D1_INPUTS: [(&str, &[u8], usize, &str); 3] = [
    (
        "no-final-lf",
        D1_NO_FINAL_LF,
        56,
        "59161683958a9adeaacaf650be033f302e118cdf7b2f739b2b8ada2ac088b337",
    ),
    (
        "crlf",
        D1_CRLF,
        58,
        "3de27459abf113af2fb4595d897bb57b03c55592afae3ed74e8c85250f5e3f60",
    ),
    (
        "mixed-oid-widths",
        D1_MIXED_OID_WIDTHS,
        132,
        "6b3d32ef35f078ce6d11b8c3ba8e2162460c98892014accd2e5db0d74b499dea",
    ),
];

/// The property each D1 case exists to exercise. Checked before every call,
/// so a literal that a tool "fixed" can never make a D1 test pass vacuously.
#[allow(dead_code)]
pub fn d1_property_holds(name: &str, bytes: &[u8]) -> bool {
    fn is_hex_oid(field: &[u8], width: usize) -> bool {
        field.len() == width && field.iter().all(|b| b.is_ascii_hexdigit())
    }
    match name {
        "no-final-lf" => !bytes.is_empty() && !bytes.ends_with(b"\n"),
        "crlf" => bytes.windows(2).any(|w| w == b"\r\n"),
        "mixed-oid-widths" => {
            let oids: Vec<&[u8]> = bytes
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .filter_map(|l| l.split(|b| *b == b'\t').next())
                .collect();
            oids.iter().any(|o| is_hex_oid(o, 40)) && oids.iter().any(|o| is_hex_oid(o, 64))
        }
        _ => false,
    }
}
