//! Issue #1618, AC6: the usage-limit wait settings from
//! `AMPLIHACK_USAGE_LIMIT_WAIT`, `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` and
//! the `usage_limit_wait` key of `$HOME/.amplihack/config`.
//!
//! Disabled, an unprofiled step behaves exactly as before (D7): one
//! invocation, the original exit code, no wait line. Invalid settings warn
//! once, naming the variable or key, never the value. ST5: no account
//! identity from the stub's `.claude.json` appears in a wait line. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events, private_file, text, wait_lines};

const ENABLED_VAR: &str = "AMPLIHACK_USAGE_LIMIT_WAIT";
const MAX_VAR: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";

fn assert_today(f: &Fixture, out: &std::process::Output) {
    assert_eq!(out.status.code(), Some(1), "{}", describe(out));
    assert_eq!(f.count("0"), 1, "one invocation, as before");
    assert!(wait_lines(out).is_empty(), "{:?}", wait_lines(out));
}

fn assert_waited(f: &Fixture, out: &std::process::Output) {
    assert!(out.status.success(), "{}", describe(out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(
        events(out).last().map(String::as_str),
        Some("resume attempt=2")
    );
}

#[test]
fn the_variable_disables_the_wait() {
    for v in ["0", "false", "OFF", "No"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env(ENABLED_VAR, v);
        assert_today(&f, &f.run(&mut c));
    }
}

#[test]
fn the_config_key_disables_the_wait() {
    let f = Fixture::new();
    f.write_config(r#"{"usage_limit_wait":{"enabled":false}}"#, 0o600);
    assert_today(&f, &f.run(&mut f.step()));
}

#[test]
fn the_variable_overrides_the_file_in_both_directions() {
    let f = Fixture::new();
    f.write_config(r#"{"usage_limit_wait":{"enabled":true}}"#, 0o600);
    let mut c = f.step();
    c.env(ENABLED_VAR, "off");
    assert_today(&f, &f.run(&mut c));

    let f = Fixture::new();
    f.write_config(r#"{"usage_limit_wait":{"enabled":false}}"#, 0o600);
    let mut c = f.step();
    c.env(ENABLED_VAR, "YES");
    assert_waited(&f, &f.run(&mut c));
}

#[test]
fn the_config_maximum_bounds_the_wait() {
    let f = Fixture::new();
    f.write_config(
        r#"{"usage_limit_wait":{"enabled":true,"max_wait_secs":0}}"#,
        0o600,
    );
    let out = f.run(&mut f.step());
    assert_waited(&f, &out);
    assert!(
        events(&out)[0].ends_with("bound=max_wait"),
        "{:?}",
        events(&out)
    );
    assert_eq!(events(&out)[1], "end reason=max reached");
}

#[test]
fn an_empty_variable_is_unset() {
    let f = Fixture::new();
    f.write_config(r#"{"usage_limit_wait":{"enabled":false}}"#, 0o600);
    let mut c = f.step();
    c.env(ENABLED_VAR, "");
    let out = f.run(&mut c);
    assert_today(&f, &out);
    assert!(
        !text(&out.stderr).contains(ENABLED_VAR),
        "no warning for empty"
    );
}

#[test]
fn an_invalid_enabled_variable_warns_once_and_the_default_applies() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env(ENABLED_VAR, "maybe-VALUE-1618");
    let out = f.run(&mut c);
    assert_waited(&f, &out);
    let stderr = text(&out.stderr);
    assert_eq!(stderr.matches(ENABLED_VAR).count(), 1, "{stderr}");
    assert!(
        !stderr.contains("VALUE-1618"),
        "the value is never quoted: {stderr}"
    );
}

#[test]
fn an_invalid_maximum_warns_once_and_the_file_decides() {
    for bad in ["+60", "-1", "1.5", "60s"] {
        let f = Fixture::new();
        f.write_config(r#"{"usage_limit_wait":{"max_wait_secs":600}}"#, 0o600);
        let mut c = f.step();
        c.env(MAX_VAR, bad);
        let out = f.run(&mut c);
        assert_waited(&f, &out);
        let stderr = text(&out.stderr);
        assert_eq!(stderr.matches(MAX_VAR).count(), 1, "{bad}: {stderr}");
        assert!(
            events(&out)[0].ends_with("until=2026-10-18 08:10:00 -07:00 bound=max_wait"),
            "{bad}: {:?}",
            events(&out)
        );
    }
}

#[test]
fn a_config_others_can_write_gives_defaults_with_one_warning() {
    let f = Fixture::new();
    f.write_config(
        r#"{"usage_limit_wait":{"enabled":false,"max_wait_secs":4242},"marker":"CONTENT-1618"}"#,
        0o666,
    );
    let out = f.run(&mut f.step());
    assert_waited(&f, &out);
    let stderr = text(&out.stderr);
    assert!(
        !stderr.contains("CONTENT-1618") && !stderr.contains("4242"),
        "{stderr}"
    );
    assert_eq!(
        stderr
            .lines()
            .filter(|l| l.contains("usage_limit_wait") && !l.starts_with("amplihack.recipe."))
            .count(),
        1,
        "one warning: {stderr}"
    );
}

#[test]
fn a_bad_config_key_gives_defaults_with_one_warning() {
    let f = Fixture::new();
    f.write_config(
        r#"{"usage_limit_wait":{"enabled":"nope","max_wait_secs":0}}"#,
        0o600,
    );
    let out = f.run(&mut f.step());
    assert_waited(&f, &out);
    assert!(
        events(&out)[0].ends_with("bound=reset+margin"),
        "the whole key falls back, max included: {:?}",
        events(&out)
    );
    assert!(!text(&out.stderr).contains("nope"));
}

#[test]
fn settings_are_not_read_unless_noninteractive_is_exactly_one() {
    for v in ["true", "0"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("AMPLIHACK_NONINTERACTIVE", v)
            .env(ENABLED_VAR, "maybe");
        let out = f.run(&mut c);
        assert_today(&f, &out);
        assert!(
            !text(&out.stderr).contains(ENABLED_VAR),
            "NONINTERACTIVE={v:?}: settings were read"
        );
    }
}

#[test]
fn st5_no_account_identity_reaches_a_wait_line() {
    let f = Fixture::new();
    private_file(
        &f.config.join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"uuid-SECRET-1618","emailAddress":"secret-person@example.invalid"}}"#,
    );
    private_file(
        &f.home.join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"uuid-HOME-1618"}}"#,
    );
    let mut c = f.step();
    c.env(
        "LIMIT_MESSAGE",
        "secret-person@example.invalid uuid-SECRET-1618 hit the usage limit · resets 8:30am (America/Los_Angeles)",
    );
    let out = f.run(&mut c);
    assert_waited(&f, &out);
    for l in wait_lines(&out) {
        for secret in [
            "SECRET",
            "secret-person",
            "HOME-1618",
            "example.invalid",
            &*f.root.to_string_lossy(),
        ] {
            assert!(!l.contains(secret), "{secret:?} leaked into {l:?}");
        }
    }
}
