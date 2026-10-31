//! Issue #1804 (R4): the login check that runs before any move onto an
//! API-key profile. Reads files under a temporary profile root only; never
//! runs `apiKeyHelper`, never makes a network request. See "API-key login
//! check" in docs/reference/profile-command.md.
//!
//! Contract (`rotate::key_check`):
//! - `configured(&ProfileStore, &ProfileName) -> Result<&'static str,
//!   String>`. Ok when one of these is a non-empty string after trimming:
//!   `primaryApiKey` in `<root>/<name>/.claude.json` (detail
//!   `primaryApiKey set`), `env.ANTHROPIC_API_KEY` in `settings.json`
//!   (`ANTHROPIC_API_KEY set`) or `apiKeyHelper` in `settings.json`
//!   (`apiKeyHelper set`).
//! - Err otherwise, with `no primaryApiKey, ANTHROPIC_API_KEY or
//!   apiKeyHelper setting`, or, when a file may not be read as evidence,
//!   `<file> is a symlink` / `<file> is writable by group or others`.
//! - The key value never appears in the detail or the reason.
//! - `reachable(home)` applies the same rules to `<home>/.claude.json` and
//!   `<home>/.claude/settings.json`, where plain `claude` reads after a swap.
//!   Its "no key" reason names those two files.
//! - `KeyCheckFn` is the seam type stored in `cycle::Ctx.key_check`; it takes
//!   the home directory.

use super::key_check::{KeyCheckFn, configured, reachable};
use super::tests_swap::private;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// A value the tests search output for. Deliberately not shaped like an
/// Anthropic key, so secret scanners do not flag the test source.
const SENTINEL: &str = "SENTINEL-1804-KEY";
const NONE: &str = "no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting";
const NOT_REACHABLE: &str = "no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting \
                             in ~/.claude.json or ~/.claude/settings.json";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

struct Env {
    _t: tempfile::TempDir,
    store: ProfileStore,
}

impl Env {
    /// `api-key`, with a `{}` login file and a `.claude.json` that names no
    /// key: the profile from the issue.
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = t.path().join("root");
        let store = ProfileStore::new(root.clone());
        store.add(&n("api-key")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        private(&root.join("api-key/.credentials.json"), "{}");
        private(&root.join("api-key/.claude.json"), r#"{"numStartups":3}"#);
        Self { _t: t, store }
    }

    fn write(&self, file: &str, body: &str) {
        private(&self.store.root().join("api-key").join(file), body);
    }

    fn check(&self) -> Result<&'static str, String> {
        configured(&self.store, &n("api-key"))
    }
}

fn no_sentinel(r: &Result<&'static str, String>) {
    let text = match r {
        Ok(d) => d.to_string(),
        Err(e) => e.clone(),
    };
    assert!(!text.contains(SENTINEL), "the key value leaked: {text}");
}

#[test]
fn an_empty_credentials_file_with_no_key_setting_fails() {
    let e = Env::new();
    let r = e.check();
    assert_eq!(r, Err(NONE.to_string()));
}

#[test]
fn primary_api_key_passes_with_a_fixed_detail() {
    let e = Env::new();
    e.write(
        ".claude.json",
        &format!(r#"{{"primaryApiKey":"{SENTINEL}"}}"#),
    );
    let r = e.check();
    assert_eq!(r, Ok("primaryApiKey set"));
    no_sentinel(&r);
}

#[test]
fn anthropic_api_key_in_settings_env_passes_with_a_fixed_detail() {
    let e = Env::new();
    e.write(
        "settings.json",
        &format!(r#"{{"env":{{"ANTHROPIC_API_KEY":"{SENTINEL}"}}}}"#),
    );
    let r = e.check();
    assert_eq!(r, Ok("ANTHROPIC_API_KEY set"));
    no_sentinel(&r);
}

#[test]
fn api_key_helper_passes_with_a_fixed_detail_and_is_never_run() {
    let e = Env::new();
    let marker = e.store.root().join("helper-ran");
    e.write(
        "settings.json",
        &format!(r#"{{"apiKeyHelper":"touch {}"}}"#, marker.display()),
    );
    assert_eq!(e.check(), Ok("apiKeyHelper set"));
    assert!(!marker.exists(), "apiKeyHelper must never be run");
}

#[test]
fn empty_whitespace_and_non_string_values_fail() {
    for v in [r#""""#, r#""   ""#, "42", "null", "{}", "[]", "true"] {
        let e = Env::new();
        e.write(".claude.json", &format!(r#"{{"primaryApiKey":{v}}}"#));
        e.write(
            "settings.json",
            &format!(r#"{{"apiKeyHelper":{v},"env":{{"ANTHROPIC_API_KEY":{v}}}}}"#),
        );
        assert_eq!(e.check(), Err(NONE.to_string()), "value {v}");
    }
}

#[test]
fn a_key_name_at_the_top_level_of_settings_is_not_the_env_setting() {
    let e = Env::new();
    e.write(
        "settings.json",
        &format!(r#"{{"ANTHROPIC_API_KEY":"{SENTINEL}"}}"#),
    );
    assert_eq!(e.check(), Err(NONE.to_string()));
}

#[test]
fn malformed_json_fails_without_echoing_the_file() {
    let e = Env::new();
    e.write(
        "settings.json",
        &format!(r#"{{"apiKeyHelper":"{SENTINEL}""#),
    );
    let r = e.check();
    assert!(r.is_err(), "{r:?}");
    no_sentinel(&r);
}

#[test]
fn a_group_writable_settings_file_fails_and_names_the_file() {
    let e = Env::new();
    e.write(
        "settings.json",
        &format!(r#"{{"apiKeyHelper":"echo {SENTINEL}"}}"#),
    );
    let p = e.store.root().join("api-key/settings.json");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o620)).unwrap();
    let r = e.check();
    assert_eq!(
        r,
        Err("settings.json is writable by group or others".to_string())
    );
    no_sentinel(&r);
}

#[test]
fn a_symlinked_settings_file_fails_and_names_the_file() {
    let e = Env::new();
    let elsewhere = e.store.root().join("elsewhere.json");
    private(
        &elsewhere,
        &format!(r#"{{"apiKeyHelper":"echo {SENTINEL}"}}"#),
    );
    std::os::unix::fs::symlink(&elsewhere, e.store.root().join("api-key/settings.json")).unwrap();
    let r = e.check();
    assert_eq!(r, Err("settings.json is a symlink".to_string()));
    no_sentinel(&r);
}

#[test]
fn a_symlinked_claude_json_fails_and_names_the_file() {
    let e = Env::new();
    let target = e.store.root().join("api-key/.claude.json");
    fs::remove_file(&target).unwrap();
    let elsewhere = e.store.root().join("elsewhere.json");
    private(&elsewhere, &format!(r#"{{"primaryApiKey":"{SENTINEL}"}}"#));
    std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
    let r = e.check();
    assert_eq!(r, Err(".claude.json is a symlink".to_string()));
    no_sentinel(&r);
}

/// What Claude 2.1.316 printed for `-p /usage` with each key setting in
/// `$HOME` and a plan login live: no reading, so the live login is `Failed`
/// and the per-token tier is not tried from it. See "API-key login check".
#[test]
fn claudes_cost_text_under_a_home_key_is_a_failed_reading() {
    use super::decide::{Probed, Reading};
    use crate::commands::profile::usage_parse::parse_usage;
    let printed = "Total cost:            $0.0000\n\
                   Total duration (API):  0s\n\
                   Total duration (wall): 1s\n\
                   Total code changes:    0 lines added, 0 lines removed\n\
                   Usage:                 0 input, 0 output, 0 cache read, 0 cache write\n";
    let report = parse_usage(printed, "", 1_793_389_376);
    assert_eq!(Probed::from_report(&report).reading, Reading::Failed);
}

#[test]
fn the_seam_type_accepts_a_fake_check() {
    // The cycle tests replace the check through `Ctx.key_check`.
    let pass: KeyCheckFn = |_| Ok("fake");
    let fail: KeyCheckFn = |_| Err("fake failure".to_string());
    let t = tempfile::tempdir().unwrap();
    assert_eq!(pass(t.path()), Ok("fake"));
    assert!(fail(t.path()).is_err());
    let real: KeyCheckFn = reachable;
    assert_eq!(real(t.path()), Err(NOT_REACHABLE.to_string()));
}

/// A home directory with the `~/.claude` of a plan login and no key.
fn home() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join(".claude")).unwrap();
    private(&t.path().join(".claude.json"), r#"{"numStartups":3}"#);
    t
}

#[test]
fn reachable_finds_each_setting_in_the_files_plain_claude_reads() {
    let t = home();
    assert_eq!(reachable(t.path()), Err(NOT_REACHABLE.to_string()));
    let settings = t.path().join(".claude/settings.json");
    private(&settings, r#"{"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#);
    assert_eq!(reachable(t.path()), Ok("apiKeyHelper set"));
    private(
        &settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"SENTINEL-1804-KEY"}}"#,
    );
    assert_eq!(reachable(t.path()), Ok("ANTHROPIC_API_KEY set"));
    private(
        &t.path().join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    assert_eq!(reachable(t.path()), Ok("primaryApiKey set"));
}

#[test]
fn reachable_ignores_a_settings_json_at_the_top_of_home() {
    // `<home>/settings.json` is not a file Claude reads.
    let t = home();
    private(&t.path().join("settings.json"), r#"{"apiKeyHelper":"x"}"#);
    assert_eq!(reachable(t.path()), Err(NOT_REACHABLE.to_string()));
}

#[test]
fn reachable_keeps_the_evidence_rules_and_never_shows_a_value() {
    let t = home();
    let settings = t.path().join(".claude/settings.json");
    private(&settings, r#"{"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#);
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o660)).unwrap();
    let r = reachable(t.path());
    assert_eq!(
        r,
        Err("settings.json is writable by group or others".to_string())
    );
    no_sentinel(&r);
}

#[test]
fn an_other_writable_claude_json_fails_and_names_the_file() {
    let e = Env::new();
    e.write(
        ".claude.json",
        &format!(r#"{{"primaryApiKey":"{SENTINEL}"}}"#),
    );
    let p = e.store.root().join("api-key/.claude.json");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o602)).unwrap();
    let r = e.check();
    assert_eq!(
        r,
        Err(".claude.json is writable by group or others".to_string())
    );
    no_sentinel(&r);
}

#[test]
fn an_untrusted_claude_json_fails_even_when_settings_names_a_key() {
    // .claude.json is checked first; an untrusted one ends the check.
    let e = Env::new();
    e.write("settings.json", r#"{"apiKeyHelper":"echo key"}"#);
    let p = e.store.root().join("api-key/.claude.json");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(
        e.check(),
        Err(".claude.json is writable by group or others".to_string())
    );
}

#[test]
fn an_oversized_settings_file_fails_and_names_the_file() {
    let e = Env::new();
    let pad = "x".repeat(70 * 1024);
    e.write(
        "settings.json",
        &format!(r#"{{"apiKeyHelper":"{SENTINEL}","pad":"{pad}"}}"#),
    );
    let r = e.check();
    assert_eq!(r, Err("settings.json is too large".to_string()));
    no_sentinel(&r);
}

#[test]
fn an_unparsable_settings_file_fails_with_cannot_be_parsed() {
    let e = Env::new();
    e.write(
        "settings.json",
        &format!("{{\"apiKeyHelper\": \"{SENTINEL}\""),
    );
    let r = e.check();
    assert_eq!(r, Err("settings.json cannot be parsed".to_string()));
    no_sentinel(&r);
}

#[test]
fn an_unparsable_claude_json_fails_with_cannot_be_parsed() {
    let e = Env::new();
    e.write(
        ".claude.json",
        &format!("{{\"primaryApiKey\": \"{SENTINEL}\""),
    );
    let r = e.check();
    assert_eq!(r, Err(".claude.json cannot be parsed".to_string()));
    no_sentinel(&r);
}

#[test]
fn a_settings_file_that_is_not_a_json_object_fails() {
    for body in [r#"["apiKeyHelper"]"#, r#""apiKeyHelper""#, "null", "42"] {
        let e = Env::new();
        e.write("settings.json", body);
        assert!(e.check().is_err(), "body {body}");
    }
}

#[test]
fn an_empty_object_in_both_files_fails() {
    let e = Env::new();
    e.write(".claude.json", "{}");
    e.write("settings.json", "{}");
    assert_eq!(e.check(), Err(NONE.to_string()));
}

#[test]
fn a_login_file_alone_never_passes_whatever_it_holds() {
    // D2: Claude Code does not read a key from .credentials.json.
    let e = Env::new();
    e.write(
        ".credentials.json",
        &format!(r#"{{"primaryApiKey":"{SENTINEL}","apiKey":"{SENTINEL}"}}"#),
    );
    let r = e.check();
    assert_eq!(r, Err(NONE.to_string()));
    no_sentinel(&r);
}

#[test]
fn missing_files_fail_with_the_no_setting_reason() {
    let e = Env::new();
    fs::remove_file(e.store.root().join("api-key/.claude.json")).unwrap();
    assert_eq!(e.check(), Err(NONE.to_string()));
}
