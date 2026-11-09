//! Issue #2013 (R1, D8, D9, S1, S6-S8): the home account repair.
//!
//! The candide case of 6 November: the verified login in `~/.claude`, the
//! ledger and the record all named `nnorth`, but the `oauthAccount` block in
//! `~/.claude.json` named `ffarmer`, and every cycle refused. Here `work`
//! plays `nnorth` and `spare` plays `ffarmer`.
//!
//! Contract:
//! - Each cycle (`prelude.rs`, after the snapshot and before the live
//!   ALERTs) runs `home_repair::step`. When the ledger's live profile, the
//!   verified identity of `~/.claude` and the record for that profile name
//!   the same account, and `~/.claude.json` is absent or parses, a missing
//!   or different block is rewritten from the verified account, and the
//!   cycle goes on. It logs
//!   `repair: home account metadata named <old>, verified identity is <new>; rewritten`
//!   (`<old>` is `none` for a missing block), limited by the alert limit
//!   under `repair:<p>`.
//! - Only `oauthAccount` changes; other keys, nested ones included, are
//!   kept. The write is atomic and refuses a symlink.
//! - A file that does not parse is never rewritten; the cycle refuses with
//!   kind `home-unreadable`.
//! - Disagreeing evidence is never repaired; the block never authorises a
//!   swap.
//! - `--dry-run` writes nothing.
//! - `swap_finish::write_home_account(home, &Account)` writes the block
//!   built from a verified account.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::identify::Account;
use super::observe::{Measurement, Snap, decide_measured};
use super::swap_finish::write_home_account;
use super::tests_issue_2013_common::{
    ctx_at, home_block, home_uuid, lines, rd, record, set_now, settings,
};
use super::tests_revalidate_cause::{probes_for, spent};
use super::tests_swap::{Env, env, ino_at, json, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const REPAIRED: &str = "repair: home account metadata named spare@example.com, verified identity is work@example.com; rewritten";

/// `env()` plus a record for `work` that agrees with the ledger and the
/// verified login, so only `~/.claude.json` can disagree.
fn fx() -> Env {
    let e = env();
    record(&e.root, "work", "u-work", "work@example.com");
    e
}

fn measure_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, rd(10.0, 10.0, None), probes, at)
}

fn measure_swap(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(0.0, 10.0, None))]);
    decide_measured(ctx, snap, spent(), probes, at)
}

fn repairs(e: &Env) -> Vec<String> {
    lines(e)
        .into_iter()
        .filter(|l| l.starts_with("repair:") || l.starts_with("would repair:"))
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn a_block_naming_another_account_is_rewritten_from_the_verified_identity() {
    let e = fx();
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed,
        "{:?}",
        lines(&e)
    );
    assert_eq!(home_uuid(&e.home).as_deref(), Some("u-work"));
    assert_eq!(repairs(&e), [REPAIRED]);
    // The fixture's `primaryApiKey` gives the usual `ALERT: home_key` line;
    // no swap is refused.
    assert!(
        !lines(&e)
            .iter()
            .any(|l| l.starts_with("ALERT: swap refused")),
        "{:?}",
        lines(&e)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_repair_keeps_every_other_key_and_the_file_mode() {
    let e = fx();
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    let path = e.home.join(".claude.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    let v = json(&path);
    assert_eq!(v["primaryApiKey"], "SECRET-KEY-HOME");
    assert_eq!(v["projects"]["/x"]["allowedTools"], serde_json::json!([]));
    assert_eq!(v["oauthAccount"]["emailAddress"], "work@example.com");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

/// The swap goes ahead in the same cycle, after the repair.
#[cfg(target_os = "linux")]
#[test]
fn after_the_repair_the_swap_that_is_due_is_made_in_the_same_cycle() {
    let e = fx();
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    let mut c = ctx_at(&e, measure_swap, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert_eq!(
        home_uuid(&e.home).as_deref(),
        Some("u-spare"),
        "s3 wrote it"
    );
    let log = lines(&e);
    let repair = log.iter().position(|l| l == REPAIRED);
    let swap = log.iter().position(|l| l.starts_with("swap work->spare "));
    assert!(repair.is_some() && repair < swap, "{log:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_missing_block_is_written() {
    let e = fx();
    home_block(&e.home, None);
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(home_uuid(&e.home).as_deref(), Some("u-work"));
    assert_eq!(
        repairs(&e),
        [
            "repair: home account metadata named none, verified identity is work@example.com; rewritten"
        ]
    );
    assert_eq!(
        json(&e.home.join(".claude.json"))["primaryApiKey"],
        "SECRET-KEY-HOME"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_missing_file_is_created_private() {
    let e = fx();
    let path = e.home.join(".claude.json");
    fs::remove_file(&path).unwrap();
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(home_uuid(&e.home).as_deref(), Some("u-work"));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_correct_block_is_left_byte_for_byte_and_nothing_is_logged() {
    let e = fx();
    let path = e.home.join(".claude.json");
    let before = fs::read(&path).unwrap();
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(repairs(&e).is_empty(), "{:?}", lines(&e));
}

/// A running session writes the stale block back on every cycle: it is
/// repaired every cycle and logged once an hour.
#[cfg(target_os = "linux")]
#[test]
fn a_block_written_back_every_cycle_is_repaired_every_cycle_and_logged_once_an_hour() {
    let e = fx();
    let t = now();
    let mut c = ctx_at(&e, measure_stay, settings(), t);
    for at in [t, t + 120, t + 240, t + 3600] {
        home_block(&e.home, Some(("u-spare", "spare@example.com")));
        set_now(at);
        run_cycle(&mut c, Mode::Watch).unwrap();
        assert_eq!(home_uuid(&e.home).as_deref(), Some("u-work"), "at {at}");
    }
    let got = repairs(&e);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0], REPAIRED);
    assert!(got[1].starts_with(REPAIRED) && got[1].contains("(repeated 2 times since "));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(l.alert_log.contains_key("repair:work"), "{:?}", l.alert_log);
    assert!(
        l.alert_log.keys().all(|k| !k.contains('@')),
        "no e-mail address in a ledger key: {:?}",
        l.alert_log
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_that_does_not_parse_is_never_rewritten_and_the_refusal_says_how_to_fix_it() {
    let e = fx();
    let path = e.home.join(".claude.json");
    private(&path, "{\"oauthAccount\": ");
    let before = fs::read(&path).unwrap();
    let mut c = ctx_at(&e, measure_swap, settings(), now());
    let _ = run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(repairs(&e).is_empty());
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing moved");
    let log = lines(&e);
    let alert = log
        .iter()
        .find(|l| l.starts_with("ALERT: swap refused (home-unreadable, profile work): "))
        .unwrap_or_else(|| panic!("{log:?}"));
    assert!(
        alert.ends_with(
            "Fix: repair ~/.claude.json so it is a regular file you own holding a JSON object; \
             it is never rewritten while it cannot be parsed"
        ),
        "{alert}"
    );
    assert!(alert.contains(".claude.json=unreadable"), "{alert}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_symlinked_home_file_is_never_written_through() {
    let e = fx();
    let path = e.home.join(".claude.json");
    let target = e.root.join("elsewhere.json");
    private(
        &target,
        &serde_json::json!({"oauthAccount": {"accountUuid": "u-spare", "emailAddress": "spare@example.com"}})
            .to_string(),
    );
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let before = fs::read(&target).unwrap();
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    let _ = run_cycle(&mut c, Mode::Watch);
    assert_eq!(
        fs::read(&target).unwrap(),
        before,
        "the target is untouched"
    );
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !repairs(&e).iter().any(|l| l.starts_with("repair:")),
        "{:?}",
        lines(&e)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_dry_run_writes_nothing() {
    let e = fx();
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    let path = e.home.join(".claude.json");
    let before = fs::read(&path).unwrap();
    let ledger = fs::read(&e.rs.ledger).unwrap();
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    let _ = run_cycle(&mut c, Mode::DryRun);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(&e.rs.ledger).unwrap(), ledger);
    assert!(
        !repairs(&e).iter().any(|l| l.starts_with("repair:")),
        "{:?}",
        lines(&e)
    );
}

/// The record names another account than the verified login: nothing is
/// repaired, and the refusal shows what each source said.
#[cfg(target_os = "linux")]
#[test]
fn disagreeing_evidence_is_refused_with_all_four_values_and_never_repaired() {
    let e = env();
    record(&e.root, "work", "u-other", "other@example.com");
    home_block(&e.home, Some(("u-spare", "spare@example.com")));
    let before = fs::read(e.home.join(".claude.json")).unwrap();
    let mut c = ctx_at(&e, measure_swap, settings(), now());
    let _ = run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(fs::read(e.home.join(".claude.json")).unwrap(), before);
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing moved");
    assert!(repairs(&e).is_empty());
    let log = lines(&e);
    let alert = log
        .iter()
        .find(|l| l.starts_with("ALERT: swap refused (account-mismatch, profile work): "))
        .unwrap_or_else(|| panic!("{log:?}"));
    for want in [
        "Evidence: credentials/verified=work@example.com, ",
        "ledger=work@example.com, ",
        ".claude.json=spare@example.com. ",
        "Fix: run plain claude, /login to the account recorded for work, then wait for the next cycle",
    ] {
        assert!(alert.contains(want), "{want} missing from {alert}");
    }
    assert!(alert.contains("record="), "{alert}");
}

/// S1: a home block that names the target account cannot authorise a swap.
/// The verified login in `~/.claude` is an intruder's; the block, the
/// ledger and the record all say `work`.
#[cfg(target_os = "linux")]
#[test]
fn a_block_naming_the_recorded_account_never_authorises_a_swap() {
    let e = fx();
    let creds = e.live();
    let body = serde_json::json!({
        "claudeAiOauth": {"accessToken": "SENTINEL"},
        "task1753FakeIdentity": {"accountUuid": "u-intruder", "emailAddress": "intruder@example.com"},
    });
    fs::write(&creds, body.to_string()).unwrap();
    let before = fs::read(e.home.join(".claude.json")).unwrap();
    let mut c = ctx_at(&e, measure_swap, settings(), now());
    let _ = run_cycle(&mut c, Mode::Watch);
    assert_eq!(ino_at(&e.live()), Some(e.l), "nothing moved");
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert_eq!(
        fs::read(e.home.join(".claude.json")).unwrap(),
        before,
        "never rewritten to the intruder"
    );
    assert!(!lines(&e).iter().any(|l| l.starts_with("swap ")));
    assert!(repairs(&e).is_empty());
}

// --- write_home_account ------------------------------------------------------------------

fn verified(uuid: &str, email: &str) -> Account {
    Account::from_block(&serde_json::json!({"accountUuid": uuid, "emailAddress": email})).unwrap()
}

#[test]
fn write_home_account_changes_only_the_block() {
    let e = env();
    write_home_account(&e.home, &verified("u-spare", "spare@example.com")).unwrap();
    let v = json(&e.home.join(".claude.json"));
    assert_eq!(v["oauthAccount"]["accountUuid"], "u-spare");
    assert_eq!(v["primaryApiKey"], "SECRET-KEY-HOME");
    assert_eq!(v["projects"]["/x"]["allowedTools"], serde_json::json!([]));
}

#[test]
fn write_home_account_refuses_a_symlink() {
    let e = env();
    let path = e.home.join(".claude.json");
    let target = e.root.join("t.json");
    private(&target, "{}");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(write_home_account(&e.home, &verified("u-spare", "spare@example.com")).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
}

#[test]
fn write_home_account_never_rewrites_a_file_that_does_not_parse() {
    let e = env();
    let path = e.home.join(".claude.json");
    private(&path, "not json");
    assert!(write_home_account(&e.home, &verified("u-spare", "spare@example.com")).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
}
