//! Account identity, duplicate logins and the rotator's live default, end
//! to end (bugs 1-3). Uses the `profile_rotate` fixture: a temporary `HOME`
//! and profile root, asserted before every command, so the real `~/.claude`
//! and profile root are never touched. `work` is live in `~/.claude` (its
//! directory has no login); `spare` and `extra` are logged in. Logins are
//! tracked by inode and are mode 0000.
//!
//! The fake `claude` used for `profile login` writes what Claude writes after
//! a browser login: a new `.credentials.json` (0600) and a `.claude.json`
//! whose `oauthAccount` is `$LOGIN_ACCOUNT`. The fake used for launches
//! records `CLAUDE_CONFIG_DIR` and `AMPLIHACK_CLAUDE_PROFILE`.

#![cfg(unix)]

use crate::support::exec_stub::write_executable;
use crate::support::profile_default::set_default;
use crate::support::profile_rotate::*;
use std::fs;
use std::path::PathBuf;
use std::process::Output;

const LOGIN_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
cj="$CLAUDE_CONFIG_DIR/.claude.json"
printf '{"oauthAccount":%s,"numStartups":9}' "$LOGIN_ACCOUNT" > "$cj.tmp" && chmod 600 "$cj.tmp" && mv "$cj.tmp" "$cj"
( umask 077; printf 'NEW-SENTINEL-TOKEN' > "$CLAUDE_CONFIG_DIR/.credentials.json" )
exit "${LOGIN_EXIT:-0}"
"#;

const LAUNCH_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
{
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'AMPLIHACK_CLAUDE_PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
} > "$RECORD_DIR/launch"
exit 0
"#;

const NOTE: &str = "note: profile 'work' is the rotator's live login, which is in ~/.claude; launching against ~/.claude.";

fn rejected(f: &Fixture) -> PathBuf {
    f.root.join(".rejected-logins")
}

fn entries(f: &Fixture) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(rejected(f))
        .map(|r| {
            r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn oauth(f: &Fixture, p: &str) -> serde_json::Value {
    let raw = fs::read_to_string(f.root.join(p).join(".claude.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&raw).unwrap()["oauthAccount"].clone()
}

fn set_account(f: &Fixture, p: &str, account: &serde_json::Value) {
    private_file(
        &f.root.join(p).join(".claude.json"),
        &serde_json::json!({ "oauthAccount": account }).to_string(),
    );
}

fn with_org(p: &str, org: &str) -> serde_json::Value {
    let mut a = account_json(p);
    a["organizationUuid"] = org.into();
    a
}

/// `amplihack profile login <name>` with a fake browser login as `account`.
fn login_as(f: &Fixture, name: &str, account: &serde_json::Value) -> Output {
    let fake = f.tools.join("login-claude");
    write_executable(&fake, LOGIN_CLAUDE, 0o755);
    let out = f
        .cmd(&["profile", "login", name])
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &fake)
        .env("LOGIN_ACCOUNT", account.to_string())
        .output()
        .unwrap();
    assert_no_secrets(&out);
    f.assert_isolated();
    out
}

/// `amplihack claude` with the recording fake; returns the output and the
/// recorded environment.
fn launch(f: &Fixture) -> (Output, String) {
    let fake = f.tools.join("launch-claude");
    write_executable(&fake, LAUNCH_CLAUDE, 0o755);
    let _ = fs::remove_file(f.records.join("launch"));
    let out = f
        .cmd(&["claude", "--subprocess-safe", "--", "-p", "hi"])
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &fake)
        .output()
        .unwrap();
    assert_no_secrets(&out);
    f.assert_isolated();
    let rec = fs::read_to_string(f.records.join("launch")).unwrap_or_else(|_| {
        panic!(
            "claude did not run: {}\nstdout: {}\nstderr: {}",
            out.status,
            text(&out.stdout),
            text(&out.stderr)
        )
    });
    (out, rec)
}

// --- Bug 2: duplicate logins are refused -------------------------------------

#[test]
fn a_login_duplicating_another_profile_is_refused() {
    let f = Fixture::new();
    let out = login_as(&f, "spare", &account_json("extra"));
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "error: profile 'spare' was signed in to the same account and organization as profile 'extra'. The new login was moved to "
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "hint: sign out of claude.ai in your browser (or use a private/incognito window), then run 'amplihack profile login spare' again."
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("note: holding the previous login of profile 'spare' in "),
        "{stderr}"
    );
    assert!(
        stderr.contains("note: moving the new login of profile 'spare' to "),
        "{stderr}"
    );
    // The previous login is back; the new one is set aside, not deleted.
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(oauth(&f, "spare"), account_json("spare"));
    let e = entries(&f);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(
        e[0].starts_with("spare-") && !e[0].ends_with("-replaced"),
        "{e:?}"
    );
    let aside = rejected(&f).join(&e[0]);
    assert!(stderr.contains(&aside.display().to_string()), "{stderr}");
    assert!(ino(&aside.join(CREDS)).is_some(), "the new login is kept");
    f.assert_each_login_once();
    // `.rejected-logins` is not a profile.
    let list = text(&f.ok(&["profile", "list"]).stdout);
    assert!(!list.contains("rejected"), "{list}");
}

#[test]
fn a_login_duplicating_the_home_login_is_refused() {
    let f = Fixture::new();
    // `~/.claude` holds an account no profile has.
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("lone")}).to_string(),
    );
    let out = login_as(&f, "spare", &account_json("lone"));
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("was signed in to the same account and organization as ~/.claude. "),
        "{stderr}"
    );
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    f.assert_each_login_once();
}

#[test]
fn a_login_of_the_same_user_in_another_org_is_accepted() {
    let f = Fixture::new();
    set_account(&f, "extra", &with_org("extra", "org-team"));
    let out = login_as(&f, "spare", &with_org("extra", "org-personal"));
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("same account"), "{stderr}");
    assert!(
        stderr.contains("note: the previous login of profile 'spare' was moved to "),
        "{stderr}"
    );
    let e = entries(&f);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(
        e[0].starts_with("spare-") && e[0].ends_with("-replaced"),
        "{e:?}"
    );
    assert_eq!(
        ino(&rejected(&f).join(&e[0]).join(CREDS)),
        Some(f.spare_ino),
        "the replaced login is moved, not deleted"
    );
    assert_ne!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert!(ino(&f.slot("spare")).is_some());
}

#[test]
fn a_failed_login_keeps_claudes_exit_and_restores_the_profile() {
    let f = Fixture::new();
    let fake = f.tools.join("login-claude");
    write_executable(&fake, LOGIN_CLAUDE, 0o755);
    let out = f
        .cmd(&["profile", "login", "spare"])
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &fake)
        .env("LOGIN_ACCOUNT", account_json("extra").to_string())
        .env("LOGIN_EXIT", "3")
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert!(!out.status.success());
    assert!(
        !text(&out.stderr).contains("same account"),
        "no duplicate check"
    );
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(oauth(&f, "spare"), account_json("spare"));
}

#[test]
fn a_login_to_the_live_profile_is_refused_before_claude_runs() {
    // `work` is live: its login is `~/.claude`. A login of its own would
    // make the rotator refuse to swap it back, so Claude never starts.
    let f = Fixture::new();
    let out = login_as(&f, "work", &account_json("work"));
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "error: profile 'work' is the rotator's live profile; its login is ~/.claude. Nothing was moved.\nhint: log in with plain 'claude', or stop the rotator first."
        ),
        "{stderr}"
    );
    assert!(ino(&f.slot("work")).is_none(), "claude did not run");
    assert!(entries(&f).is_empty(), "{:?}", entries(&f));
    f.assert_each_login_once();
}

// --- Bugs 1 and 2: warnings ---------------------------------------------------

const DUP: &str = "profiles 'extra' and 'spare' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login spare' with the browser signed out of claude.ai or in a private window.";

#[test]
fn usage_and_status_warn_about_a_duplicate_login() {
    let f = Fixture::new();
    set_account(&f, "spare", &account_json("extra"));
    let usage = f.run(&["profile", "usage"]);
    let stderr = text(&usage.stderr);
    assert!(stderr.contains(&format!("warning: {DUP}\n")), "{stderr}");
    assert!(
        !text(&usage.stdout).contains("duplicate login"),
        "stdout is unchanged"
    );
    let status = text(&f.run(&["profile", "auto-switch", "status"]).stdout);
    assert!(status.contains(&format!("note: {DUP}\n")), "{status}");
    assert!(!status.contains("readings are not reliable"), "{status}");
}

#[test]
fn usage_warns_about_every_profile_whatever_names_it_was_given() {
    let f = Fixture::new();
    set_account(&f, "spare", &account_json("extra"));
    let stderr = text(&f.run(&["profile", "usage", "work"]).stderr);
    assert!(stderr.contains(&format!("warning: {DUP}\n")), "{stderr}");
}

#[test]
fn the_same_user_in_two_orgs_gets_no_warning() {
    let f = Fixture::new();
    set_account(&f, "extra", &with_org("extra", "org-team"));
    set_account(&f, "spare", &with_org("extra", "org-personal"));
    let usage = text(&f.run(&["profile", "usage"]).stderr);
    assert!(!usage.contains("duplicate login"), "{usage}");
    let status = text(&f.run(&["profile", "auto-switch", "status"]).stdout);
    assert!(!status.contains("duplicate login"), "{status}");
    assert!(!status.contains("same account"), "{status}");
}

#[test]
fn the_live_profile_is_not_warned_about_against_its_own_home_login() {
    // `work` is live: `~/.claude.json` and `work/.claude.json` name the same
    // account, which is its own login, not a duplicate.
    let f = Fixture::new();
    let usage = text(&f.run(&["profile", "usage"]).stderr);
    assert!(!usage.contains("duplicate login"), "{usage}");
    let status = text(&f.run(&["profile", "auto-switch", "status"]).stdout);
    assert!(!status.contains("duplicate login"), "{status}");
}

#[test]
fn a_profile_duplicating_the_home_login_is_warned_with_home_first() {
    let f = Fixture::new();
    set_account(&f, "extra", &account_json("work"));
    let stderr = text(&f.run(&["profile", "usage"]).stderr);
    assert!(
        stderr.contains(
            "warning: profiles '~/.claude' and 'extra' are signed in to the same account and organization (duplicate login); fix: run 'amplihack profile login extra' "
        ),
        "{stderr}"
    );
}

// --- Bug 3: the live default launches against ~/.claude ----------------------

#[test]
fn launching_the_live_default_uses_the_default_dir_with_one_note() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "work"]);
    let (out, rec) = launch(&f);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(rec.contains("CLAUDE_CONFIG_DIR=<unset>\n"), "{rec}");
    assert!(rec.contains("AMPLIHACK_CLAUDE_PROFILE=<unset>\n"), "{rec}");
    let stderr = text(&out.stderr);
    assert_eq!(
        stderr.lines().filter(|l| *l == NOTE).count(),
        1,
        "exactly one note: {stderr}"
    );
    f.assert_each_login_once();
}

#[test]
fn launching_the_live_profile_by_flag_is_redirected_too() {
    let f = Fixture::new();
    let fake = f.tools.join("launch-claude");
    write_executable(&fake, LAUNCH_CLAUDE, 0o755);
    let out = f
        .cmd(&[
            "claude",
            "--profile",
            "work",
            "--subprocess-safe",
            "--",
            "-p",
            "hi",
        ])
        .env("AMPLIHACK_CLAUDE_BINARY_PATH", &fake)
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let rec = fs::read_to_string(f.records.join("launch")).unwrap();
    assert!(rec.contains("CLAUDE_CONFIG_DIR=<unset>\n"), "{rec}");
    assert_eq!(text(&out.stderr).lines().filter(|l| *l == NOTE).count(), 1);
}

#[test]
fn launching_a_default_that_is_not_live_sets_its_dir() {
    let f = Fixture::new();
    // Only the pointer: `switch spare` would make spare live (issue #1691).
    set_default(&f.root, "spare");
    let (out, rec) = launch(&f);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        rec.contains(&format!(
            "CLAUDE_CONFIG_DIR={}\n",
            f.root.join("spare").display()
        )),
        "{rec}"
    );
    assert!(rec.contains("AMPLIHACK_CLAUDE_PROFILE=spare\n"), "{rec}");
    assert!(!text(&out.stderr).contains("rotator's live login"));
}

#[test]
fn after_switch_none_claude_uses_no_profile() {
    let f = Fixture::new();
    set_default(&f.root, "spare");
    let cleared = f.ok(&["profile", "switch", "--none"]);
    assert_eq!(text(&cleared.stdout), "Default profile cleared.\n");
    assert!(!f.root.join(".default").exists());
    let again = f.ok(&["profile", "switch", "--none"]);
    assert_eq!(text(&again.stdout), "No default profile was set.\n");
    let (out, rec) = launch(&f);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(rec.contains("CLAUDE_CONFIG_DIR=<unset>\n"), "{rec}");
    assert!(rec.contains("AMPLIHACK_CLAUDE_PROFILE=<unset>\n"), "{rec}");
    assert!(!text(&out.stderr).contains("rotator's live login"));
    let show = text(&f.ok(&["profile", "show"]).stdout);
    assert_eq!(show.trim(), "none");
}

#[test]
fn switch_needs_a_name_or_none_but_not_both() {
    let f = Fixture::new();
    assert_eq!(f.run(&["profile", "switch"]).status.code(), Some(2));
    assert_eq!(
        f.run(&["profile", "switch", "spare", "--none"])
            .status
            .code(),
        Some(2)
    );
}
