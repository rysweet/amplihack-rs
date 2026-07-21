//! TDD tests for GitHubDistributor implementation (issue #605).
//!
//! These tests define the expected contract for the fixed GitHubDistributor.
//! They verify both the specification (via source scanning) and the utility
//! contracts (via direct logic tests).
//!
//! # Test categories
//!
//! 1. **UTF-8 truncation** — `truncate_to_char_boundary` must never panic on
//!    multi-byte strings.
//! 2. **Base64 encoding** — must use the `base64` crate instead of hand-rolled.
//! 3. **JSON body construction** — `push_bundle` must produce valid GitHub
//!    Contents API payloads.
//! 4. **Idempotent push** — `push_bundle` must include `sha` when updating an
//!    existing file.
//! 5. **Repository visibility** — `create_repository` must accept a `public`
//!    parameter.
//! 6. **No CLI arg overflow** — bundle content must go through `--input` (temp
//!    file), not inline CLI args.
//! 7. **No feature gate** — `GitHubDistributor` must be available without
//!    `#[cfg(feature = ...)]`.

use std::fs;
use std::path::Path;

/// Read a file relative to the repo root.
fn read_file(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root");
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

// ---------------------------------------------------------------------------
// 1. UTF-8 safe truncation — specification tests
// ---------------------------------------------------------------------------

/// The implementation must expose a `truncate_to_char_boundary` helper (or
/// equivalent) that never splits multi-byte characters.  These tests run
/// against the expected algorithm to lock the specification.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[test]
fn truncate_ascii_within_limit() {
    let s = "hello world";
    let truncated = truncate_to_char_boundary(s, 5);
    assert_eq!(truncated, "hello");
}

#[test]
fn truncate_ascii_at_exact_limit() {
    let s = "hello";
    let truncated = truncate_to_char_boundary(s, 5);
    assert_eq!(truncated, "hello");
}

#[test]
fn truncate_ascii_beyond_limit() {
    let s = "hi";
    let truncated = truncate_to_char_boundary(s, 100);
    assert_eq!(truncated, "hi");
}

#[test]
fn truncate_multibyte_does_not_panic() {
    // 'é' is 2 bytes in UTF-8.  Cutting at byte 4 would split the 'é'.
    let s = "café";
    let truncated = truncate_to_char_boundary(s, 4);
    assert!(truncated.len() <= 4);
    assert!(truncated.is_char_boundary(truncated.len()));
    let _ = truncated.to_string(); // must be valid UTF-8
}

#[test]
fn truncate_emoji_boundary() {
    // '🦀' is 4 bytes.  Requesting 2 bytes must not split it.
    let s = "🦀rust";
    let truncated = truncate_to_char_boundary(s, 2);
    assert!(truncated.is_empty() || truncated.len() <= 2);
}

#[test]
fn truncate_cjk_characters() {
    // CJK characters are 3 bytes each.  '日本語' = 9 bytes.
    let s = "日本語";
    let truncated = truncate_to_char_boundary(s, 6);
    assert_eq!(truncated, "日本");
}

#[test]
fn truncate_empty_string() {
    assert!(truncate_to_char_boundary("", 100).is_empty());
}

#[test]
fn truncate_zero_max() {
    assert!(truncate_to_char_boundary("hello", 0).is_empty());
}

// ---------------------------------------------------------------------------
// 2. Base64 encoding via `base64` crate
// ---------------------------------------------------------------------------

#[test]
fn base64_roundtrip_simple() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let input = b"Hello, GitHub!";
    let encoded = STANDARD.encode(input);
    let decoded = STANDARD.decode(&encoded).expect("decode must succeed");
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_binary() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let input: Vec<u8> = (0..=255).collect();
    let encoded = STANDARD.encode(&input);
    let decoded = STANDARD.decode(&encoded).expect("decode must succeed");
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_large_bundle() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    // Simulate a 2MB bundle — must not overflow CLI args
    let input = vec![0x42u8; 2 * 1024 * 1024];
    let encoded = STANDARD.encode(&input);
    assert!(encoded.len() > 2_000_000, "encoded 2MB must be large");
    let decoded = STANDARD.decode(&encoded).expect("decode must succeed");
    assert_eq!(decoded.len(), input.len());
}

// ---------------------------------------------------------------------------
// 3. JSON body construction for push_bundle
// ---------------------------------------------------------------------------

/// Build the JSON body for the GitHub Contents API PUT.
/// This mirrors the expected implementation contract.
fn build_push_bundle_json(message: &str, content: &[u8], sha: Option<&str>) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let encoded = STANDARD.encode(content);
    let mut body = serde_json::json!({
        "message": message,
        "content": encoded,
    });
    if let Some(sha_val) = sha {
        body["sha"] = serde_json::Value::String(sha_val.to_string());
    }
    serde_json::to_string(&body).unwrap()
}

#[test]
fn push_bundle_json_has_required_fields() {
    let json_body = build_push_bundle_json("commit message here", b"file content", None);
    let parsed: serde_json::Value = serde_json::from_str(&json_body).unwrap();

    assert!(parsed.get("message").is_some(), "must have 'message' field");
    assert!(parsed.get("content").is_some(), "must have 'content' field");
    assert_eq!(parsed["message"].as_str().unwrap(), "commit message here");

    use base64::{Engine, engine::general_purpose::STANDARD};
    let content_b64 = parsed["content"].as_str().unwrap();
    let decoded = STANDARD
        .decode(content_b64)
        .expect("content must be valid base64");
    assert_eq!(decoded, b"file content");
}

#[test]
fn push_bundle_json_omits_sha_when_none() {
    let json_body = build_push_bundle_json("msg", b"data", None);
    let parsed: serde_json::Value = serde_json::from_str(&json_body).unwrap();
    assert!(
        parsed.get("sha").is_none(),
        "sha must be absent for new files"
    );
}

#[test]
fn push_bundle_json_includes_sha_when_present() {
    let existing_sha = "abc123def456";
    let json_body = build_push_bundle_json("msg", b"data", Some(existing_sha));
    let parsed: serde_json::Value = serde_json::from_str(&json_body).unwrap();
    assert_eq!(
        parsed["sha"].as_str().unwrap(),
        existing_sha,
        "sha must match existing file SHA for idempotent update"
    );
}

// ---------------------------------------------------------------------------
// 4–7. Source-level contract tests (compile against ANY state of the code)
// ---------------------------------------------------------------------------

/// GitHubDistributor must NOT be behind a feature gate.
#[test]
fn github_distributor_not_feature_gated() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");
    let struct_line = src
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("pub struct GitHubDistributor"));

    assert!(
        struct_line.is_some(),
        "GitHubDistributor struct must exist in bundle_generator.rs"
    );

    let (line_num, _) = struct_line.unwrap();
    // Check the line ABOVE the struct for cfg(feature)
    if line_num > 0 {
        let prev_line = src.lines().nth(line_num - 1).unwrap_or("");
        assert!(
            !prev_line.contains("cfg(feature"),
            "GitHubDistributor must not be behind a feature gate, found: {prev_line}"
        );
    }
}

/// `create_repository` must accept a `public` bool parameter.
#[test]
fn create_repository_has_public_parameter() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");
    let fn_line = src.lines().find(|l| l.contains("fn create_repository"));

    assert!(fn_line.is_some(), "create_repository method must exist");

    // The method must have a bool parameter for visibility
    let sig_text: String = src
        .lines()
        .skip_while(|l| !l.contains("fn create_repository"))
        .take(5)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        sig_text.contains("bool"),
        "create_repository must accept a bool parameter for visibility, got: {sig_text}"
    );
}

/// `push_bundle` must use `--input` (temp file) instead of inline CLI args.
#[test]
fn push_bundle_uses_temp_file_not_cli_arg() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");

    // Find the push_bundle method body
    let push_bundle_section: String = src
        .lines()
        .skip_while(|l| !l.contains("fn push_bundle"))
        .take(60)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !push_bundle_section.is_empty(),
        "push_bundle method must exist"
    );

    // Must use --input for the API body (temp file approach)
    assert!(
        push_bundle_section.contains("--input") || push_bundle_section.contains("input"),
        "push_bundle must use --input (temp file) instead of -f content=..., got:\n{push_bundle_section}"
    );
}

/// `push_bundle` must handle idempotent updates by fetching existing SHA.
#[test]
fn push_bundle_fetches_existing_sha() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");

    let push_bundle_section: String = src
        .lines()
        .skip_while(|l| !l.contains("fn push_bundle"))
        .take(80)
        .collect::<Vec<_>>()
        .join("\n");

    // Must contain SHA-fetching logic (GET request to check existing file)
    assert!(
        push_bundle_section.contains("sha")
            && (push_bundle_section.contains("GET")
                || push_bundle_section.contains("get")
                || push_bundle_section.contains("contents")),
        "push_bundle must fetch existing file SHA for idempotent updates"
    );
}

/// The `distribute` method must NOT be a stub returning "not yet implemented".
#[test]
fn distribute_is_not_a_stub() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");

    let distribute_section: String = src
        .lines()
        .skip_while(|l| !l.contains("fn distribute"))
        .take(40)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !distribute_section.is_empty(),
        "distribute method must exist"
    );

    assert!(
        !distribute_section.contains("not yet implemented"),
        "distribute must not be a stub, found 'not yet implemented' in:\n{distribute_section}"
    );
}

/// The implementation must use the `base64` crate, not a hand-rolled encoder.
#[test]
fn no_hand_rolled_base64_encoder() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");

    // Must not have a custom base64_encode function
    assert!(
        !src.contains("fn base64_encode"),
        "must not have a hand-rolled base64_encode function — use the base64 crate"
    );

    // Must import or use the base64 crate
    assert!(
        src.contains("base64") || src.contains("Base64"),
        "must use the base64 crate for encoding"
    );
}

/// The implementation must use char-boundary-safe truncation for descriptions.
#[test]
fn uses_char_boundary_safe_truncation() {
    let src = read_file("crates/amplihack-utils/src/bundle_generator/distributor.rs");

    // The old unsafe pattern: &description[..description.len().min(100)]
    assert!(
        !src.contains("&description[..description.len().min("),
        "must not use unsafe byte-slicing on description strings — use char-boundary-safe truncation"
    );
}

/// `Cargo.toml` must have `base64` as a dependency (not just dev-dependency).
#[test]
fn cargo_toml_has_base64_dep() {
    let toml = read_file("crates/amplihack-utils/Cargo.toml");
    let deps_section: String = toml
        .lines()
        .skip_while(|l| !l.starts_with("[dependencies]"))
        .take_while(|l| !l.starts_with("[dev-"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        deps_section.contains("base64"),
        "base64 must be a [dependencies] entry in amplihack-utils/Cargo.toml, got:\n{deps_section}"
    );
}

/// The feature gate for `github-distributor` must be removed from Cargo.toml.
#[test]
fn no_github_distributor_feature_flag() {
    let toml = read_file("crates/amplihack-utils/Cargo.toml");
    assert!(
        !toml.contains("github-distributor"),
        "github-distributor feature flag must be removed from Cargo.toml"
    );
}
