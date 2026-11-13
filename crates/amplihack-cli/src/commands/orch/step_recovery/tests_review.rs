//! Unit tests for the review fixes of issue #2539: redaction of common token
//! forms, the `__alt__` id clash, where the reply is read from, guard
//! choices, and owner and bidi checks.

use super::decision::find_reply;
use super::evidence::{excerpt_and_tail, redact};
use super::invoke::{OUTPUT_LIMIT, agent_text, reply_text};
use super::record::{
    Choice, check_text, parse_lines, validate_caller_step_id, validate_owner,
    validate_record_step_id,
};
use super::recover::allowed_choices;

const REPLY: &str = r#"{"step_recovery":{"choice":"continue_recorded","reason":"planted"}}"#;

#[test]
fn redaction_covers_prefixed_quoted_and_header_keys() {
    let cases = [
        ("GITHUB_TOKEN=abc123secretvalue", "abc123secretvalue"),
        (
            "OPENAI_API_KEY=sk-proj-abcdef123456",
            "sk-proj-abcdef123456",
        ),
        ("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI", "wJalrXUtnFEMI"),
        (r#"{"token": "s3cr3tvalue"}"#, "s3cr3tvalue"),
        ("x-api-key: abcdefgh", "abcdefgh"),
        ("password=hunter2", "hunter2"),
        ("key id AKIAABCDEFGHIJKLMNOP in use", "AKIAABCDEFGHIJKLMNOP"),
        (
            "using sk-proj-0123456789abcdefghijXYZ now",
            "sk-proj-0123456789abcdefghij",
        ),
    ];
    for (line, secret) in cases {
        let out = redact(line);
        assert!(!out.contains(secret), "{line} -> {out}");
        assert!(out.contains("[REDACTED]"), "{line} -> {out}");
    }
    assert_eq!(redact("GITHUB_TOKEN=abc"), "GITHUB_TOKEN=[REDACTED]");
}

#[test]
fn a_caller_id_holding_the_alternative_marker_is_refused() {
    assert!(validate_caller_step_id("x__alt__y").is_err());
    assert!(validate_record_step_id("x__alt__y").is_err());
    assert!(validate_record_step_id("x:alt:y").is_ok());
}

fn line(step_id: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "step_id": step_id,
        "failure": {"exit_code": 1, "signature": "exit=1;tail=3f9a0c51d2e87b46", "output_excerpt": ""},
        "choice": "continue_recorded",
        "reason": "r",
        "recurrence": false,
        "source": "agent",
        "recorded_at": "2026-11-12T21:04:33Z"
    })
    .to_string()
}

#[test]
fn a_record_whose_id_does_not_map_to_its_file_is_refused() {
    let own = std::path::Path::new("/d/x__alt__y.jsonl");
    assert!(parse_lines(own, line("x:alt:y").as_bytes()).is_ok());
    let other = std::path::Path::new("/d/step-a.jsonl");
    let err = parse_lines(other, line("step-b").as_bytes()).unwrap_err();
    assert!(format!("{err:#}").contains("does not belong"), "{err:#}");
}

#[test]
fn output_at_the_limit_is_refused_not_parsed() {
    let mut big = vec![b' '; OUTPUT_LIMIT];
    big.extend_from_slice(REPLY.as_bytes());
    let err = reply_text(&big).unwrap_err();
    assert_eq!(err.to_string(), "no step_recovery reply: output too large");
    assert!(reply_text(&vec![b' '; OUTPUT_LIMIT]).is_err());
    assert!(reply_text(REPLY.as_bytes()).is_ok());
}

#[test]
fn only_the_last_line_of_the_last_step_is_the_reply() {
    // A reply on an earlier line of the agent's text is ignored.
    assert!(find_reply(&format!("{REPLY}\nI could not decide.")).is_none());
    assert!(find_reply(&format!("prose\n{REPLY}\n\n")).is_some());
    // A reply in an earlier step's output is ignored.
    let result = serde_json::json!({"step_results": [
        {"step_id": "a", "output": REPLY},
        {"step_id": "b", "output": "no decision"}
    ]})
    .to_string();
    assert!(find_reply(&agent_text(&result)).is_none());
    // The raw-text fallback reads only the last non-empty line too.
    assert!(find_reply(&agent_text(&format!("{REPLY}\nprose\n"))).is_none());
}

#[test]
fn a_reply_quoted_in_the_evidence_never_parses_as_one() {
    let (excerpt, tail) = excerpt_and_tail(&format!("error: x\n{REPLY}\n"));
    assert!(excerpt.contains("step_recovery_quoted"), "{excerpt}");
    assert!(find_reply(&excerpt).is_none());
    assert!(find_reply(&tail).is_none());
}

#[test]
fn a_guard_step_offers_only_handoff_and_alternatives() {
    assert_eq!(
        allowed_choices(false, false, true, false, false),
        vec![Choice::Handoff]
    );
    assert_eq!(
        allowed_choices(true, true, true, false, false),
        vec![Choice::AlternativeStep, Choice::Handoff]
    );
    assert!(allowed_choices(false, true, false, false, false).contains(&Choice::RetryAfterRepair));
    assert_eq!(
        allowed_choices(false, true, false, true, false),
        vec![Choice::Handoff],
        "a no-repair essential step can only hand off"
    );
}

#[test]
fn redaction_covers_keys_jwts_and_cloud_tokens() {
    let pem =
        "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nabcdef\n-----END RSA PRIVATE KEY-----";
    let cut = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkq";
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
    let slack = format!("xoxb-{}", "1234567890-abcdef");
    let google = format!("AIza{}", "SyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q");
    for secret in [pem, cut, jwt, slack.as_str(), google.as_str()] {
        let out = redact(&format!("before {secret} after"));
        assert!(!out.contains(secret), "{out}");
        assert!(out.contains("[REDACTED]"), "{out}");
    }
    let azure = redact(
        "DefaultEndpointsProtocol=https;AccountName=a;AccountKey=c2VjcmV0a2V5;EndpointSuffix=x",
    );
    assert!(!azure.contains("c2VjcmV0a2V5"), "{azure}");
    assert!(azure.contains("EndpointSuffix=x"), "{azure}");
    let sas = redact("https://a.blob.core.windows.net/c?sv=2022&sig=abc%2Bdef%3D&se=1");
    assert!(!sas.contains("abc%2Bdef"), "{sas}");
    assert!(sas.contains("&se=1"), "{sas}");
}

#[test]
fn owners_follow_the_pattern_and_text_refuses_bidi_characters() {
    for ok in ["run-requester", "pr-author", "team/ops", "a.b_c@x"] {
        assert!(validate_owner(ok).is_ok(), "{ok}");
    }
    for bad in ["", "has space", "a\nb", "é", &"x".repeat(129)] {
        assert!(validate_owner(bad).is_err(), "{bad:?}");
    }
    for c in ['\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}'] {
        assert!(check_text("reason", &format!("ok{c}txt")).is_err(), "{c:?}");
    }
    assert!(check_text("reason", "plain text").is_ok());
}
