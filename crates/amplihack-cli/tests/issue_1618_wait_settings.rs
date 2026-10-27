//! Issue #1618, AC6: the usage-limit wait settings from
//! `AMPLIHACK_USAGE_LIMIT_WAIT`, `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` and
//! the `recipe.usage_limit_wait` / `recipe.usage_limit_max_wait_secs` keys of
//! `~/.amplihack/config.toml`, read through the user-config loader.
//!
//! Disabled, an unprofiled step behaves exactly as before (D7): one
//! invocation, the original exit code, no wait line. With no explicit
//! setting, `CI=true` turns the wait off. Invalid values warn without being
//! quoted. ST5: no account identity reaches a wait line. See
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
    for v in ["0", "false"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env(ENABLED_VAR, v);
        assert_today(&f, &f.run(&mut c));
    }
}

#[test]
fn the_config_key_disables_the_wait() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = false\n");
    assert_today(&f, &f.run(&mut f.step()));
}

#[test]
fn the_variable_overrides_the_file_in_both_directions() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = true\n");
    let mut c = f.step();
    c.env(ENABLED_VAR, "0");
    assert_today(&f, &f.run(&mut c));

    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = false\n");
    let mut c = f.step();
    c.env(ENABLED_VAR, "1");
    assert_waited(&f, &f.run(&mut c));
}

#[test]
fn ci_turns_the_wait_off_unless_it_is_set_explicitly() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("CI", "true");
    assert_today(&f, &f.run(&mut c));

    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = true\n");
    let mut c = f.step();
    c.env("CI", "true");
    assert_waited(&f, &f.run(&mut c));
}

#[test]
fn the_config_maximum_bounds_the_wait() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 0\n");
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
    f.write_config("[recipe]\nusage_limit_wait = false\n");
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
fn an_invalid_variable_warns_and_the_layers_below_decide() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 600\n");
    let mut c = f.step();
    c.env(ENABLED_VAR, "maybe-VALUE-1618").env(MAX_VAR, "60s");
    let out = f.run(&mut c);
    assert_waited(&f, &out);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(ENABLED_VAR) && stderr.contains(MAX_VAR),
        "{stderr}"
    );
    assert!(
        !stderr.contains("VALUE-1618"),
        "the value is quoted: {stderr}"
    );
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 08:10:00 -07:00 bound=max_wait"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn settings_are_not_read_unless_noninteractive_is_exactly_one() {
    for v in ["true", "0"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("AMPLIHACK_NONINTERACTIVE", v);
        assert_today(&f, &f.run(&mut c));
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
        "You've hit your session limit · resets 8:30am (America/Los_Angeles) for secret-person@example.invalid uuid-SECRET-1618",
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
