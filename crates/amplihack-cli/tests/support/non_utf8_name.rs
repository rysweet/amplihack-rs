//! Issue #2931 (EILSEQ): a file name that is not valid UTF-8.
//!
//! APFS (macOS) refuses to create such a name: the call fails with `EILSEQ`
//! (illegal byte sequence). Linux file systems accept any bytes except `/` and
//! NUL. A test that exercises the product's handling of a non-UTF-8 name
//! therefore cannot run on an Apple target, and says so on its own test, so
//! that the skip shows in the run's count:
//!
//! ```text
//! #[cfg_attr(
//!     target_vendor = "apple",
//!     ignore = "APFS refuses a file name that is not valid UTF-8 (EILSEQ), so the input cannot be made"
//! )]
//! ```
//!
//! An attribute takes a literal, so each such test repeats that reason; this
//! file is the one place that builds the name and explains it.
//!
//! This file uses only `std`.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

/// `bytes` as a file name. Panics when the bytes are valid UTF-8: a test that
/// asks for a non-UTF-8 name and gets an ordinary one proves nothing.
pub fn non_utf8(bytes: &[u8]) -> OsString {
    assert!(
        std::str::from_utf8(bytes).is_err(),
        "{bytes:?} is valid UTF-8; a non-UTF-8 name was asked for"
    );
    OsString::from_vec(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::non_utf8;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn the_bytes_come_back_unchanged() {
        let name = non_utf8(b"a\xffb");
        assert_eq!(name.as_bytes(), b"a\xffb");
    }

    #[test]
    fn the_name_is_not_valid_utf_8() {
        assert!(non_utf8(b"\xff").to_str().is_none());
        assert!(non_utf8(b"prefix-\xc3\x28").to_str().is_none());
    }

    #[test]
    #[should_panic(expected = "valid UTF-8")]
    fn bytes_that_are_valid_utf_8_are_a_mistake_in_the_test() {
        let _ = non_utf8(b"plain");
    }
}
