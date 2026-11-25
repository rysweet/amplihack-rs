//! The checks of the review round on a local-model launch, at the binary's
//! boundary: the hooks binary (item 1), the Codex version (item 2), the
//! user's approval, sandbox and rules (item 3) and caller arguments that
//! turn hooks off (item 5).
//!
//! Each test shows that the launch is rejected or announced, or that the
//! user's own setting applies, and never that the command check or the
//! sandbox is weakened without a word. A rejection exits non-zero (and not
//! with clap's usage status), starts no Codex, creates nothing under
//! `codex-local` and gives the data that caused it on stderr.

use super::harness::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[track_caller]
fn assert_rejected(launch: &Launched, f: &Fixture, what: &str, shown: &str) {
    launch.assert_refused_at_run_time(f, what);
    assert!(
        launch.stderr().contains(shown),
        "{what}: stderr gives {shown}: {}",
        launch.stderr()
    );
}

fn set_mode(path: &std::path::Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn first_command(f: &Fixture) -> String {
    let hooks = read_json(&f.generated(NAME).join("hooks.json"));
    hooks["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned()
}

// ---- item 1: the hooks binary ----

#[test]
fn a_hooks_binary_others_can_write_is_rejected() {
    let f = Fixture::new();
    let bin = f.hooks_bin();
    for (path, insecure, secure) in [
        (bin.clone(), 0o720, 0o700),
        (bin.clone(), 0o702, 0o700),
        (f.tools().to_path_buf(), 0o775, 0o755),
        (f.tools().to_path_buf(), 0o757, 0o755),
    ] {
        set_mode(&path, insecure);
        let launch = f.local(NAME, &[]);
        set_mode(&path, secure);
        assert_rejected(
            &launch,
            &f,
            &format!("mode {insecure:o}"),
            &path.display().to_string(),
        );
    }
    f.local(NAME, &[]).assert_started();
}

/// The deployed copy in the user's bin directory is what amplihack's entry
/// runs, as for an ordinary install, even when the override gives another
/// binary.
#[test]
fn the_deployed_hooks_binary_is_used() {
    let f = Fixture::new();
    let bin = f.home().join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    set_mode(&f.home().join(".local"), 0o700);
    set_mode(&bin, 0o700);
    let deployed = bin.join("amplihack-hooks");
    fs::copy(f.hooks_bin(), &deployed).unwrap();
    set_mode(&deployed, 0o700);
    f.local(NAME, &[]).assert_started();
    let command = first_command(&f);
    assert!(
        command.contains(&deployed.display().to_string()),
        "{command}"
    );
    assert!(
        !command.contains(&f.hooks_bin().display().to_string()),
        "{command}"
    );
}

// ---- item 2: the Codex version ----

fn set_codex_version(f: &Fixture, output: &str) {
    fs::write(f.home().join("codex-version"), format!("{output}\n")).unwrap();
}

#[test]
fn an_interactive_launch_on_an_unknown_codex_version_is_rejected() {
    let f = Fixture::new();
    set_codex_version(&f, "codex-cli 0.167.0");
    let launch = f.local(NAME, &[]);
    assert_rejected(&launch, &f, "Codex 0.167.0", "0.167.0");
}

/// The caller's own hook-trust bypass does not change that: the check does
/// not depend on what the caller passes.
#[test]
fn an_interactive_launch_with_the_callers_trust_bypass_is_still_checked() {
    let f = Fixture::new();
    set_codex_version(&f, "codex-cli 0.167.0");
    let launch = f.local(NAME, &[HOOK_TRUST]);
    assert_rejected(&launch, &f, "Codex 0.167.0 with a bypass", "0.167.0");
}

#[test]
fn an_interactive_launch_whose_codex_version_cannot_be_read_is_rejected() {
    let f = Fixture::new();
    set_codex_version(&f, "no version here");
    let launch = f.local(NAME, &[]);
    launch.assert_refused_at_run_time(&f, "an unreadable version");
}

#[test]
fn an_exec_launch_on_an_unknown_codex_version_goes_on() {
    let f = Fixture::new();
    set_codex_version(&f, "codex-cli 0.167.0");
    let launch = f.local(NAME, &["exec", "-"]);
    launch.assert_started();
    assert_eq!(launch.count(HOOK_TRUST), 1);
}

#[test]
fn an_ordinary_launch_on_an_unknown_codex_version_is_not_affected() {
    let f = Fixture::new();
    set_codex_version(&f, "codex-cli 0.167.0");
    f.launch(&mut f.command(&["codex"])).assert_started();
}

// ---- item 3: the user's approval, sandbox and rules ----

/// A user sandbox is passed on as the caller's choice when the profile has
/// none, so an `exec` launch keeps it instead of getting full access.
#[test]
fn the_user_sandbox_applies_when_the_profile_has_none() {
    let f = Fixture::new();
    fs::write(
        f.user_codex().join("config.toml"),
        "approval_policy = \"on-request\"\nsandbox_mode = \"read-only\"\n",
    )
    .unwrap();
    let launch = f.local(NAME, &["exec", "-"]);
    launch.assert_started();
    let argv = launch.argv();
    assert!(
        argv.iter().any(|a| a == "sandbox_mode=\"read-only\""),
        "{argv:?}"
    );
    assert!(!argv.iter().any(|a| a == "danger-full-access"), "{argv:?}");
    let config = read_toml(&f.generated(NAME).join("config.toml"));
    assert_eq!(config["approval_policy"].as_str(), Some("on-request"));
    assert_eq!(config["sandbox_mode"].as_str(), Some("read-only"));
}

/// The user's rules are copied privately, follow a refresh, and a rule the
/// user removes is removed from the home too.
#[test]
fn the_user_rules_are_copied_and_follow_a_refresh() {
    let f = Fixture::new();
    let rules = f.user_codex().join("rules");
    fs::write(
        rules.join("forbid.rules"),
        "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")\n",
    )
    .unwrap();
    f.local(NAME, &[]).assert_started();
    let generated = f.generated(NAME).join("rules");
    for name in ["default.rules", "forbid.rules"] {
        let copy = generated.join(format!("amplihack-user-{name}"));
        assert_eq!(
            fs::read(&copy).unwrap(),
            fs::read(rules.join(name)).unwrap()
        );
        assert_eq!(mode(&copy), 0o600, "{name}");
    }
    let copy = generated.join("amplihack-user-forbid.rules");
    let before = (inodes(&f.generated(NAME)), inode(&copy));
    f.local(NAME, &[]).assert_started();
    assert_eq!(
        (inodes(&f.generated(NAME)), inode(&copy)),
        before,
        "an unchanged refresh"
    );

    fs::remove_file(rules.join("forbid.rules")).unwrap();
    f.local(NAME, &[]).assert_started();
    assert!(!generated.join("amplihack-user-forbid.rules").exists());
    assert!(generated.join("amplihack-user-default.rules").exists());
}

// ---- item 5: caller arguments that turn hooks off ----

/// The caller's argument still applies, and the launch says so on stderr:
/// one more line than the same launch without it.
#[test]
fn a_caller_argument_that_turns_hooks_off_is_announced() {
    let f = Fixture::new();
    let lines = |tail: &[&str]| {
        let launch = f.local(NAME, tail);
        launch.assert_started();
        launch.stderr().lines().count()
    };
    let control = lines(&["exec", "-"]);
    for tail in [
        &["-c", "features.hooks=false", "exec", "-"][..],
        &["--disable", "hooks", "exec", "-"][..],
    ] {
        assert_eq!(lines(tail), control + 1, "{tail:?}");
    }
}
