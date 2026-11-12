//! Issue #2006: `amplihack codex` picks an account when no home is pinned,
//! and a pinned home is never changed (docs/reference/account-pick.md,
//! "Launch integration" and "Pins").
//!
//! Each test runs `amplihack codex -- exec -`, the command a recipe agent
//! step runs, against the fake Codex in `issue_2006_account_pick/fixture.rs`,
//! which records `CODEX_HOME` and `AMPLIHACK_CODEX_HOME` as it saw them and
//! the leases on disk while it ran.

#![cfg(unix)]

use crate::issue_2006_fixture::{Fx, stderr, text};
use std::fs;

const WARNING: &str = "warning: account pick failed: ";
const FALLBACK: &str = "; using default home";

fn var(env: &[String], key: &str) -> String {
    let prefix = format!("{key}=");
    env.iter()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("{key} not recorded: {env:?}"))
        .to_string()
}

fn leases_during(fx: &Fx) -> Vec<String> {
    fs::read_to_string(fx.record.join("leases-during"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_explicit_codex_home_is_never_changed() {
    let fx = Fx::with_80_50_20();
    let pinned = fx.account("c"); // the worst account: a pick would move it
    let out = fx
        .codex_launch()
        .env("CODEX_HOME", &pinned)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let env = fx.launch_env(&out);
    assert_eq!(var(&env, "CODEX_HOME"), pinned.to_str().unwrap());
    assert_eq!(var(&env, "AMPLIHACK_CODEX_HOME"), pinned.to_str().unwrap());
    assert!(
        leases_during(&fx).is_empty(),
        "a pinned launch wrote a lease"
    );
    assert!(fx.lease_files().is_empty());
    assert!(!stderr(&out).contains(WARNING), "{}", text(&out));
    fx.assert_credentials_untouched();
}

#[test]
fn an_explicit_codex_home_wins_even_when_it_is_not_a_profile() {
    let fx = Fx::with_80_50_20();
    let elsewhere = fx.tmp.path().join("elsewhere-codex");
    fs::create_dir(&elsewhere).unwrap();
    let out = fx
        .codex_launch()
        .env("CODEX_HOME", &elsewhere)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        var(&fx.launch_env(&out), "CODEX_HOME"),
        elsewhere.to_str().unwrap()
    );
    assert!(fx.lease_files().is_empty());
}

#[test]
fn the_alias_alone_pins_and_restores_codex_home() {
    let fx = Fx::with_80_50_20();
    let pinned = fx.account("b");
    let out = fx
        .codex_launch()
        .env("AMPLIHACK_CODEX_HOME", &pinned)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let env = fx.launch_env(&out);
    assert_eq!(var(&env, "CODEX_HOME"), pinned.to_str().unwrap());
    assert_eq!(var(&env, "AMPLIHACK_CODEX_HOME"), pinned.to_str().unwrap());
    assert!(leases_during(&fx).is_empty());
    assert!(fx.lease_files().is_empty());
}

#[test]
fn codex_home_wins_over_the_alias() {
    let fx = Fx::with_80_50_20();
    let out = fx
        .codex_launch()
        .env("CODEX_HOME", fx.account("c"))
        .env("AMPLIHACK_CODEX_HOME", fx.account("b"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        var(&fx.launch_env(&out), "CODEX_HOME"),
        fx.account("c").to_str().unwrap()
    );
}

#[test]
fn an_unpinned_launch_runs_on_the_picked_home_and_drops_its_lease_after() {
    let fx = Fx::with_80_50_20();
    let out = fx.codex_launch().output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let env = fx.launch_env(&out);
    let a = fx.account("a");
    assert_eq!(var(&env, "CODEX_HOME"), a.to_str().unwrap());
    assert_eq!(var(&env, "AMPLIHACK_CODEX_HOME"), a.to_str().unwrap());
    let during = leases_during(&fx);
    assert_eq!(
        during.len(),
        1,
        "the launcher holds one lease while Codex runs: {during:?}"
    );
    assert!(
        !during[0].starts_with("0-"),
        "a launch lease records the launcher's pid: {during:?}"
    );
    assert!(
        fx.lease_files().is_empty(),
        "the launcher deletes its lease when Codex exits: {:?}",
        fx.lease_files()
    );
    fx.assert_credentials_untouched();
}

#[test]
fn bootstrap_configures_the_picked_home_only() {
    let fx = Fx::with_80_50_20();
    let out = fx.codex_launch().output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let picked = fx.account("a").join("config.toml");
    let doc = fs::read_to_string(&picked).unwrap_or_else(|_| {
        panic!(
            "bootstrap did not write {}\n{}",
            picked.display(),
            text(&out)
        )
    });
    assert!(doc.contains("approval_policy"), "{doc}");
    for other in ["b", "c"] {
        assert!(
            !fx.account(other).join("config.toml").exists(),
            "{other} touched"
        );
    }
    assert!(
        !fx.home.join(".codex/config.toml").exists(),
        "bootstrap configured ~/.codex while Codex ran on the picked home"
    );
    fx.assert_credentials_untouched();
}

#[test]
fn a_pinned_launch_does_not_lower_the_next_pick_by_a_lease() {
    let fx = Fx::with_80_50_20();
    let out = fx
        .codex_launch()
        .env("CODEX_HOME", fx.account("a"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let next = fx
        .cmd(&["account", "pick", "--cli", "codex", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&next.stdout).unwrap();
    assert_eq!(v["account"], "a");
    assert_eq!(v["active_leases"], 0);
}

#[test]
fn no_usable_account_warns_and_launches_on_the_default_home() {
    let fx = Fx::new(); // nobody signed in
    let out = fx.codex_launch().output().unwrap();
    assert!(
        out.status.success(),
        "the launch must not be blocked\n{}",
        text(&out)
    );
    let err = stderr(&out);
    assert!(
        err.contains(WARNING) && err.contains(FALLBACK),
        "expected the fallback warning\n{}",
        text(&out)
    );
    let home = var(&fx.launch_env(&out), "CODEX_HOME");
    for n in ["a", "b", "c"] {
        assert_ne!(
            home,
            fx.account(n).to_str().unwrap(),
            "launched on unusable {n}"
        );
    }
    assert!(fx.lease_files().is_empty());
}

#[test]
fn an_unusable_lease_directory_warns_and_still_launches() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::with_80_50_20();
    fs::create_dir(&fx.leases).unwrap();
    fs::set_permissions(&fx.leases, fs::Permissions::from_mode(0o777)).unwrap();
    let out = fx.codex_launch().output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(stderr(&out).contains(WARNING), "{}", text(&out));
    assert!(fx.record.join("launch.env").exists());
}

#[test]
fn the_off_switch_restores_the_old_behaviour() {
    for value in ["off", "OFF"] {
        let fx = Fx::with_80_50_20();
        let out = fx
            .codex_launch()
            .env("AMPLIHACK_ACCOUNT_PICK", value)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out));
        let home = var(&fx.launch_env(&out), "CODEX_HOME");
        for n in ["a", "b", "c"] {
            assert_ne!(home, fx.account(n).to_str().unwrap(), "{value}: picked {n}");
        }
        assert!(
            leases_during(&fx).is_empty(),
            "{value}: a lease was written"
        );
        assert!(!stderr(&out).contains(WARNING), "{}", text(&out));
    }
}

#[test]
fn the_codex_only_switch_still_picks_for_codex() {
    let fx = Fx::with_80_50_20();
    let out = fx
        .codex_launch()
        .env("AMPLIHACK_ACCOUNT_PICK", "codex")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        var(&fx.launch_env(&out), "CODEX_HOME"),
        fx.account("a").to_str().unwrap()
    );
}
