//! A recipe's config directory and usage-limit recovery across a host login swap.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, describe, toggle_login};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{OLD_CYCLE, real_now, write_cycle};
use crate::support::profile_default::set_default;
use std::fs;
use std::process::{Command, Output};

fn with_host_default() -> Fixture {
    let f = with_profiles();
    set_default(&f.profiles, "X");
    f
}

fn copy_login_to_host(f: &Fixture, profile: &str) {
    let profile = f.profiles.join(profile);
    let host_config = f.home.join(".claude");
    fs::create_dir_all(&host_config).unwrap();
    fs::copy(
        profile.join(".credentials.json"),
        host_config.join(".credentials.json"),
    )
    .unwrap();
    fs::copy(profile.join(".claude.json"), f.home.join(".claude.json")).unwrap();
}

fn recipe_step(f: &Fixture) -> Command {
    let mut cmd = f.step();
    cmd.env_remove("CLAUDE_CONFIG_DIR")
        .env("AMPLIHACK_RECIPE_RUN_ID", "test-3391")
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "0")
        // The fixture clock starts at 8am. Leave hours on the virtual clock
        // for both events, with a progress notification every three seconds.
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "20")
        .env(
            "LIMIT_MESSAGE",
            "You've hit your session limit · resets 11am (America/Los_Angeles)",
        );
    cmd
}

fn run_through_host_swap(f: &Fixture, cmd: &mut Command) -> (Output, Option<u32>) {
    // X still has its own login files, so its default pointer would send an
    // ordinary launch to the dedicated directory even with X also live.
    copy_login_to_host(f, "X");
    write_cycle(&f.profiles, "X", OLD_CYCLE);

    // Notifications synchronize fixture writes with the wait. Assertions
    // use the stand-in's calls and exit status.
    let progress = format!("{WAIT_PREFIX}progress ");
    let mut notifications = 0;
    let mut attempts_after_swap = None;
    let out = f.run_watching(cmd, |line| {
        if !line.starts_with(&progress) {
            return;
        }
        notifications += 1;
        match notifications {
            1 => {
                copy_login_to_host(f, "Y");
                write_cycle(&f.profiles, "Y", real_now());
            }
            2 => {
                attempts_after_swap = Some(f.count("0"));
                toggle_login(&f.profiles.join("X"));
            }
            _ => {}
        }
    });
    (out, attempts_after_swap)
}

fn assert_live_host_attempt(f: &Fixture, attempt: u32) {
    let call = f.call("0", attempt);
    assert_eq!(call["HOME"], f.home.display().to_string());
    // With no config override Claude reads $HOME/.claude and $HOME/.claude.json.
    assert_eq!(call["CLAUDE_CONFIG_DIR"], "<unset>");
}

#[test]
fn implicit_recipe_uses_live_host_config_and_retries_after_a_host_swap() {
    let f = with_host_default();
    let (out, attempts_after_swap) = run_through_host_swap(&f, &mut recipe_step(&f));
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_live_host_attempt(&f, 1);
    assert_live_host_attempt(&f, 2);
    assert_eq!(attempts_after_swap, None, "{}", describe(&out));
}

#[test]
fn explicit_profile_wait_survives_host_swap_and_retries_on_its_own_login_change() {
    let f = with_host_default();
    let mut cmd = recipe_step(&f);
    cmd.env("AMPLIHACK_CLAUDE_PROFILE", "X");
    let (out, attempts_after_swap) = run_through_host_swap(&f, &mut cmd);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(attempts_after_swap, Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    for attempt in [1, 2] {
        assert_eq!(
            f.call("0", attempt)["CLAUDE_CONFIG_DIR"],
            f.profiles.join("X").display().to_string()
        );
    }
}

#[test]
fn implicit_recipe_uses_live_host_config_when_waiting_is_disabled() {
    let f = with_host_default();
    let mut cmd = recipe_step(&f);
    cmd.env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        .env("AMPLIHACK_CLAUDE_PROFILE", "")
        .env("LIMIT_EXIT", "7");
    let out = f.run(&mut cmd);
    assert_eq!(out.status.code(), Some(7), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert_live_host_attempt(&f, 1);
}

#[test]
fn recipe_with_config_override_retries_on_that_directorys_login_change() {
    let f = with_host_default();
    let mut cmd = recipe_step(&f);
    cmd.env("CLAUDE_CONFIG_DIR", &f.config).env(
        "LIMIT_MESSAGE",
        "You've hit your session limit · try again later",
    );
    let out = f.run_with_a_login_change_in(&f.config, &mut cmd);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    for attempt in [1, 2] {
        assert_eq!(
            f.call("0", attempt)["CLAUDE_CONFIG_DIR"],
            f.config.display().to_string()
        );
    }
}

#[test]
fn recipe_profile_flag_overrides_inherited_profile_and_config() {
    let f = with_host_default();
    let mut cmd = f.cmd(&[
        "claude",
        "--subprocess-safe",
        "--profile",
        "Y",
        "--",
        "-p",
        "step",
    ]);
    cmd.env("AMPLIHACK_RECIPE_RUN_ID", "test-3391")
        .env("AMPLIHACK_CLAUDE_PROFILE", "X")
        .env("LIMIT_CALLS", "0");
    let out = f.run(&mut cmd);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert_eq!(
        f.call("0", 1)["CLAUDE_CONFIG_DIR"],
        f.profiles.join("Y").display().to_string()
    );
}

#[test]
fn ordinary_launch_still_uses_host_default() {
    let f = with_host_default();
    let mut cmd = f.step();
    cmd.env("LIMIT_CALLS", "0");
    let out = f.run(&mut cmd);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert_eq!(
        f.call("0", 1)["CLAUDE_CONFIG_DIR"],
        f.profiles.join("X").display().to_string()
    );
}
