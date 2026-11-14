//! The run id check shared by `recipe stop <run_id>` (Linux only) and
//! `recipe guide` and the run-guidance reader (all platforms). It is a pure
//! check on a string, so it has no platform gate.

/// The longest run id accepted, in bytes.
pub(crate) const MAX_RUN_ID: usize = 128;

/// A run id that can name a record or a run folder: 1 to [`MAX_RUN_ID`]
/// bytes, each an ASCII letter or digit, `.`, `_` or `-`, not starting with
/// `.`. Nothing that could name another path.
pub(crate) fn validate_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= MAX_RUN_ID
        && !run_id.starts_with('.')
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_and_the_allowed_characters_are_valid_ids() {
        assert_eq!(MAX_RUN_ID, 128);
        for id in [
            "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
            "a",
            "A_b-9",
            "a.b",
            "no-such-run",
            &"x".repeat(MAX_RUN_ID),
        ] {
            assert!(validate_run_id(id), "{id:?} must be accepted");
        }
    }

    #[test]
    fn ids_that_could_name_another_path_are_invalid() {
        let long = "x".repeat(MAX_RUN_ID + 1);
        for id in [
            "",
            "../x",
            "a/b",
            "/abs",
            ".",
            "..",
            ".hidden",
            "a\0b",
            "a b",
            "a\n",
            "é",
            "a\\b",
            "a\u{1b}[31m",
            &long,
        ] {
            assert!(!validate_run_id(id), "{id:?} must be refused");
        }
    }
}
