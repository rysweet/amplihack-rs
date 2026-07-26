//! Validation for values interpolated into remote shell commands.

use crate::error::RemoteError;

/// Validate a session identifier for safe interpolation into a remote shell
/// command. Rejects — rather than silently mangles — any value that is not a
/// strict identifier (ASCII alphanumerics plus `-`, `_`, `.`). This prevents
/// constructing a wrong-but-valid command from unexpected input.
pub(crate) fn validate_session_id(value: &str) -> Result<&str, RemoteError> {
    if value.is_empty() {
        return Err(RemoteError::validation("session id must not be empty"));
    }
    let all_valid = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if all_valid {
        Ok(value)
    } else {
        Err(RemoteError::validation(format!(
            "invalid session id {value:?}: only ASCII alphanumerics and '-', '_', '.' are allowed"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_identifiers() {
        assert_eq!(
            validate_session_id("hello-world_2.0").unwrap(),
            "hello-world_2.0"
        );
        assert_eq!(
            validate_session_id("amplihack-user-123").unwrap(),
            "amplihack-user-123"
        );
        assert_eq!(validate_session_id("S1").unwrap(), "S1");
    }

    #[test]
    fn rejects_invalid_input() {
        // Rejects rather than silently mangling — no wrong-but-valid command.
        assert!(validate_session_id("test;rm -rf /").is_err());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id("!@#$%^&*()").is_err());
        assert!(validate_session_id("has space").is_err());
        assert!(validate_session_id("$(whoami)").is_err());
        assert!(validate_session_id("a\"b").is_err());

        match validate_session_id("bad;id") {
            Err(RemoteError::Validation(_)) => {}
            other => panic!("expected Validation error, got {other:?}"),
        }
    }
}
