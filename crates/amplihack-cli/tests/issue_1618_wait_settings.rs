//! Issue #1618, AC6: the usage-limit wait setting from
//! `AMPLIHACK_USAGE_LIMIT_WAIT` and the `recipe.usage_limit_wait` key of
//! `~/.amplihack/config.toml`, read through the user-config loader. There is
//! no length setting (issue #2911): `recipe.usage_limit_max_wait_secs` is an
//! unknown key and `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` is not read.
//!
//! Disabled, an unprofiled step behaves exactly as before (D7): one
//! invocation, the original exit code, no wait line. `CI=true` does not turn
//! the wait off (issue #2911); only the key or its variable does. Invalid
//! values warn without being quoted. ST5: no account identity reaches a wait
//! line. See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events, private_file, text, wait_lines};

const ENABLED_VAR: &str = "AMPLIHACK_USAGE_LIMIT_WAIT";
const MAX_VAR: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";
/// A session limit that states no reset. Its wait is a capacity wait with no
/// deadline, so it is the wait on which a length setting would show.
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";

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

/// Issue #2911: a run that meets a limit does not end because of it, and
/// `CI` is not a setting. It used to turn the wait off by default, and
/// amplihack's own delegation layer exports `CI=true` to every delegated
/// `claude -p`, so a recipe run started from one ended at its first limit.
/// The explicit opt-out still applies under `CI`.
#[test]
fn issue_2911_ci_does_not_turn_the_wait_off() {
    for ci in ["true", "1", "github"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("CI", ci);
        assert_waited(&f, &f.run(&mut c));
    }

    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = false\n");
    let mut c = f.step();
    c.env("CI", "true");
    let out = f.run(&mut c);
    assert_today(&f, &out);
    assert!(
        !text(&out.stderr).contains("reason=ci"),
        "no ci reason is written: {}",
        describe(&out)
    );
}

/// Issue #2911: a config file that still sets the removed length key gives
/// a wait with no reset no deadline: the wait ends on the login change, and
/// on nothing timed.
#[test]
fn the_removed_config_length_does_not_bound_the_wait() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 0\n");
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert_waited(&f, &out);
    assert!(
        events(&out)[1].ends_with(" until=none bound=none"),
        "{:?}",
        events(&out)
    );
    assert_eq!(events(&out)[2], "end reason=login changed");
}

/// Nor does it bound a reset wait, which ends at its reset.
#[test]
fn the_removed_config_length_does_not_bound_a_reset_wait() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_max_wait_secs = 0\n");
    let out = f.run(&mut f.step());
    assert_waited(&f, &out);
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "{:?}",
        events(&out)
    );
    assert_eq!(events(&out)[1], "end reason=reset reached");
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

/// An invalid enabling variable is reported without its value and the file
/// below it decides. The old length variable, exported with any value, is
/// reported as no longer read, again without its value.
#[test]
fn an_invalid_variable_warns_and_the_layers_below_decide() {
    let f = Fixture::new();
    f.write_config("[recipe]\nusage_limit_wait = true\n");
    let mut c = f.step();
    c.env(ENABLED_VAR, "maybe-VALUE-1618")
        .env(MAX_VAR, "60s-VALUE-2911")
        .env("LIMIT_MESSAGE", NO_RESET_TEXT);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);
    assert_waited(&f, &out);
    let stderr = text(&out.stderr);
    assert!(stderr.contains(ENABLED_VAR), "{stderr}");
    assert!(
        stderr.contains(&format!("warning: {MAX_VAR} is no longer read")),
        "{stderr}"
    );
    assert!(
        !stderr.contains("VALUE-1618") && !stderr.contains("VALUE-2911"),
        "a value is quoted: {stderr}"
    );
    assert!(
        events(&out)[1].ends_with(" until=none bound=none"),
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
