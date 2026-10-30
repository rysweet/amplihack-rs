//! Unit tests for `env_record.rs`: the closed, exact allowlist (R2.4, D7,
//! S21), the value rules (S8b for `AMPLIHACK_AGENT_BINARY`), and `--env`
//! parsing (B10). The drift guard against the launcher is in
//! `tests_env_drift.rs`.
//!
//! API under test:
//! - `ALLOWLIST: &[&str]`; `allowed(name: &str) -> bool`
//! - `record(pairs: impl IntoIterator<Item = (String, String)>) ->
//!   (BTreeMap<String, String>, Vec<(String, String)>)`: the recorded
//!   variables and, for each allowlisted name whose value was refused,
//!   `(name, rule)`. A name off the list is dropped silently: it is never
//!   mentioned, because it may be a credential.
//! - `parse_flag(kv: &str) -> Result<(String, String), String>`; the error is
//!   the whole message (`--env: ...`)

use super::env_record::*;

const LIST: [&str; 11] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "MAX_THINKING_TOKENS",
    "AMPLIHACK_DEFAULT_MODEL",
    "AMPLIHACK_AGENT_BINARY",
];

const SECRET: &str = "sk-ant-api03-SECRETSECRETSECRET";

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_allowlist_is_exactly_the_eleven_names() {
    let mut got = ALLOWLIST.to_vec();
    got.sort_unstable();
    let mut want = LIST.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "any change needs a security review (S21)");
    assert!(LIST.iter().all(|n| allowed(n)));
}

#[test]
fn names_are_matched_exactly() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_CUSTOM_HEADERS",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "HTTPS_PROXY",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
        "AMPLIHACK_SESSION_ID",
        "AMPLIHACK_DEPTH",
        "CLAUDECODE",
        "TMUX",
        "PATH",
        "HOME",
        "anthropic_model",
        "ANTHROPIC_MODEL_X",
        "XANTHROPIC_MODEL",
        "ANTHROPIC_",
        "",
    ] {
        assert!(!allowed(name), "{name:?} must not be recorded");
    }
}

#[test]
fn record_keeps_only_listed_names_and_never_mentions_the_others() {
    let (rec, refused) = record(pairs(&[
        ("ANTHROPIC_MODEL", "claude-opus-5-5"),
        ("CLAUDE_CODE_EFFORT_LEVEL", "high"),
        ("ANTHROPIC_API_KEY", SECRET),
        ("GH_TOKEN", "ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
        ("ANTHROPIC_BASE_URL", "https://proxy.example"),
        ("CLAUDE_CONFIG_DIR", "/home/u/.claude-profiles/work"),
        ("AMPLIHACK_CLAUDE_PROFILE", "work"),
        ("PATH", "/usr/bin"),
    ]));
    let names: Vec<&str> = rec.keys().map(String::as_str).collect();
    assert_eq!(names, ["ANTHROPIC_MODEL", "CLAUDE_CODE_EFFORT_LEVEL"]);
    assert_eq!(rec["CLAUDE_CODE_EFFORT_LEVEL"], "high");
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn refused_values_are_reported_by_name_and_rule_only() {
    let (long, max) = ("x".repeat(4097), "y".repeat(4096));
    let (rec, refused) = record(pairs(&[
        ("ANTHROPIC_MODEL", "opus\nSECRET-LINE"),
        ("MAX_THINKING_TOKENS", long.as_str()),
        ("CLAUDE_CODE_SUBAGENT_MODEL", "a\0b"),
        ("AMPLIHACK_AGENT_BINARY", "/bin/sh"),
        ("AMPLIHACK_DEFAULT_MODEL", max.as_str()),
    ]));
    assert_eq!(rec.keys().collect::<Vec<_>>(), ["AMPLIHACK_DEFAULT_MODEL"]);
    let mut names: Vec<&str> = refused.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    let want = [
        "AMPLIHACK_AGENT_BINARY",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "MAX_THINKING_TOKENS",
    ];
    assert_eq!(names, want);
    for (_, rule) in &refused {
        assert!(!rule.contains("SECRET") && !rule.contains("/bin/sh") && !rule.contains(&long));
    }
}

#[test]
fn the_agent_binary_must_be_one_of_the_four_clis() {
    for ok in ["claude", "copilot", "codex", "amplifier"] {
        let (rec, refused) = record(pairs(&[("AMPLIHACK_AGENT_BINARY", ok)]));
        assert_eq!(
            rec.get("AMPLIHACK_AGENT_BINARY").map(String::as_str),
            Some(ok)
        );
        assert!(refused.is_empty());
    }
    for bad in ["/bin/sh", "sh", "claude;id", "../claude", ""] {
        let (rec, _) = record(pairs(&[("AMPLIHACK_AGENT_BINARY", bad)]));
        assert!(rec.is_empty(), "S8b: {bad:?}");
    }
}

#[test]
fn env_flag_parsing() {
    assert_eq!(
        parse_flag("CLAUDE_CODE_EFFORT_LEVEL=high").unwrap(),
        ("CLAUDE_CODE_EFFORT_LEVEL".to_string(), "high".to_string())
    );
    assert_eq!(
        parse_flag("ANTHROPIC_MODEL=a=b").unwrap().1,
        "a=b",
        "split at the first '='"
    );
    assert_eq!(
        parse_flag("ANTHROPIC_MODEL").unwrap_err(),
        "--env: expected KEY=VALUE"
    );
    let rule = "--env: value must be at most 4096 bytes, with no newline or NUL";
    assert_eq!(parse_flag("ANTHROPIC_MODEL=a\nb").unwrap_err(), rule);
    assert_eq!(
        parse_flag(&format!("ANTHROPIC_MODEL={}", "v".repeat(4097))).unwrap_err(),
        rule
    );
    let e = parse_flag("AMPLIHACK_AGENT_BINARY=/bin/sh").unwrap_err();
    assert!(
        e.starts_with("--env: ") && !e.contains("/bin/sh"),
        "S8b: {e}"
    );
}

#[test]
fn a_name_off_the_list_is_refused_without_repeating_the_argument() {
    for arg in [
        format!("ANTHROPIC_API_KEY={SECRET}"),
        format!("{SECRET}=x"),
        "CLAUDE_CONFIG_DIR=/x".to_string(),
    ] {
        let e = parse_flag(&arg).unwrap_err();
        let (list, rest) = e
            .split_once(": only these variables can be recorded: ")
            .unwrap_or_else(|| panic!("B10 text: {e}"));
        assert_eq!(list, "--env");
        assert_eq!(
            rest.split(", ").collect::<Vec<_>>(),
            ALLOWLIST,
            "the whole list"
        );
        assert!(
            !e.contains(SECRET) && !e.contains("/x") && !e.contains("API_KEY"),
            "{e}"
        );
    }
}
