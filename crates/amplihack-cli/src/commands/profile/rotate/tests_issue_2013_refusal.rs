//! Issue #2013 (D10): what a refused swap says.
//!
//! Contract (`rotate/refusal.rs`, pure):
//! - `Cause`, one variant per row of "Rotate refusal alerts" in
//!   docs/reference/profile-command.md, in table order, listed in
//!   `Cause::ALL`.
//! - `Cause::kind(self) -> &'static str` and
//!   `Cause::fix(self, p: Option<&ProfileName>) -> String` (the fixed
//!   action, with `<p>` filled in, `-` when there is no profile).
//! - `Cause::of(reason: &str) -> Cause`: the first row whose reason
//!   matches; `Other` when none does.
//! - `Evidence { verified, ledger, record, home }`, each `Option<String>`
//!   holding an e-mail address only.
//! - `line(Cause, Option<&ProfileName>, cause_text, &Evidence) -> String`:
//!   `ALERT: swap refused (<kind>, profile <p|->): <cause>. Evidence:
//!   credentials/verified=<a>, ledger=<b>, record=<c>, .claude.json=<d>.
//!   Fix: <action>`. `None` shows `none`; a value failing `safe_account`
//!   shows `unreadable`.

use super::ledger::safe_account;
use super::refusal::{Cause, Evidence, line};
use super::swap_expect::ACCOUNT_MISMATCH;
use super::tests_swap::n;

const REFERENCE: &str = include_str!("../../../../../../docs/reference/profile-command.md");

fn ev(verified: &str, ledger: &str, record: &str, home: &str) -> Evidence {
    let v = |s: &str| (!s.is_empty()).then(|| s.to_string());
    Evidence {
        verified: v(verified),
        ledger: v(ledger),
        record: v(record),
        home: v(home),
    }
}

#[test]
fn the_documented_account_mismatch_line() {
    let reason = format!("{ACCOUNT_MISMATCH} nnorth");
    assert_eq!(
        line(
            Cause::AccountMismatch,
            Some(&n("nnorth")),
            &reason,
            &ev(
                "ffarmer@example.com",
                "nnorth@example.com",
                "nnorth@example.com",
                "ffarmer@example.com"
            )
        ),
        "ALERT: swap refused (account-mismatch, profile nnorth): the login in ~/.claude is not \
         the account recorded for profile nnorth. Evidence: \
         credentials/verified=ffarmer@example.com, ledger=nnorth@example.com, \
         record=nnorth@example.com, .claude.json=ffarmer@example.com. Fix: run plain claude, \
         /login to the account recorded for nnorth, then wait for the next cycle"
    );
}

#[test]
fn a_missing_value_is_none_and_an_unsafe_one_is_unreadable() {
    let long = format!("{}@example.com", "a".repeat(400));
    assert!(!safe_account(&long));
    let got = line(
        Cause::IdentityUnavailable,
        None,
        "trusted login identity unavailable",
        &ev(
            "",
            &long,
            "x\u{1b}[31m@example.com",
            "bidi\u{202e}@example.com",
        ),
    );
    assert!(
        got.contains(
            "Evidence: credentials/verified=none, ledger=unreadable, record=unreadable, \
             .claude.json=unreadable. Fix: "
        ),
        "{got}"
    );
    assert!(got.starts_with("ALERT: swap refused (identity-unavailable, profile -): "));
    assert!(
        !got.contains('\u{1b}') && !got.contains('\u{202e}'),
        "{got:?}"
    );
}

#[test]
fn every_kind_has_its_documented_fixed_action() {
    let p = n("nnorth");
    let want = [
        (
            Cause::IdentityUnavailable,
            "identity-unavailable",
            "none needed; the next cycle reads the identity again; if this repeats, run: amplihack profile verify",
        ),
        (
            Cause::AccountMismatch,
            "account-mismatch",
            "run plain claude, /login to the account recorded for nnorth, then wait for the next cycle",
        ),
        (
            Cause::LedgerMismatch,
            "ledger-mismatch",
            "none needed; the next cycle checks for a manual /login in ~/.claude; if this repeats, run: amplihack profile verify",
        ),
        (
            Cause::NoRecord,
            "no-record",
            "run: amplihack profile seed-record nnorth, check the account it shows, then run it again with --write",
        ),
        (
            Cause::SameAccount,
            "same-account",
            "run: amplihack profile verify, then log nnorth in to its own account with: amplihack profile login nnorth",
        ),
        (
            Cause::HomeUnreadable,
            "home-unreadable",
            "repair ~/.claude.json so it is a regular file you own holding a JSON object; it is never rewritten while it cannot be parsed",
        ),
        (
            Cause::TargetNoLogin,
            "target-no-login",
            "run: amplihack profile login nnorth",
        ),
        (
            Cause::HasLogin,
            "has-login",
            "decide which login nnorth keeps, then run: amplihack profile login nnorth",
        ),
        (
            Cause::FileUnsafe,
            "file-unsafe",
            "fix the owner and mode of the path named before 'Evidence:' (chmod 600 for a login file, chmod 700 for the profile root)",
        ),
        (
            Cause::RefreshLock,
            "refresh-lock",
            "none needed; the next cycle tries again",
        ),
        (
            Cause::Unsupported,
            "unsupported",
            "move the profile root onto the same Linux filesystem as your home directory",
        ),
        (
            Cause::ApiKey,
            "api-key",
            "see \"Use an API key as the last resort\" in docs/howto/claude-profile-rotation.md",
        ),
        (
            Cause::Other,
            "other",
            "see \"Swap refused\" in docs/howto/claude-profile-rotation.md",
        ),
    ];
    assert_eq!(Cause::ALL.len(), want.len());
    for (i, (cause, kind, fix)) in want.into_iter().enumerate() {
        assert_eq!(Cause::ALL[i], cause, "table order");
        assert_eq!(cause.kind(), kind);
        assert_eq!(cause.fix(Some(&p)), fix, "{kind}");
    }
}

#[test]
fn the_refusal_kind_for_a_target_without_a_login_is_not_no_login() {
    assert_eq!(Cause::TargetNoLogin.kind(), "target-no-login");
    assert!(Cause::ALL.iter().all(|c| c.kind() != "no-login"));
}

#[test]
fn reasons_map_to_their_kind_and_anything_else_is_other() {
    use super::swap::DIFFERENT_DEVICES;
    let cases = [
        (format!("{ACCOUNT_MISMATCH} work"), Cause::AccountMismatch),
        (
            super::login_identity::UNAVAILABLE.to_string(),
            Cause::IdentityUnavailable,
        ),
        (
            "trusted login identity: outgoing ownership is not established; run: amplihack profile seed-record work"
                .to_string(),
            Cause::NoRecord,
        ),
        (
            "~/.claude.json is not a JSON object".to_string(),
            Cause::HomeUnreadable,
        ),
        (
            "Claude holds its refresh lock; the swap waits for the next cycle".to_string(),
            Cause::RefreshLock,
        ),
        (DIFFERENT_DEVICES.to_string(), Cause::Unsupported),
        (
            "spare has no stored login; run `amplihack profile login spare`".to_string(),
            Cause::TargetNoLogin,
        ),
        ("work already has a login".to_string(), Cause::HasLogin),
        (
            "a login file is not a private regular file with one link".to_string(),
            Cause::FileUnsafe,
        ),
        (
            "x has no oauth login and is not an API-key profile".to_string(),
            Cause::ApiKey,
        ),
        ("something nobody planned for".to_string(), Cause::Other),
    ];
    for (reason, want) in cases {
        assert_eq!(Cause::of(&reason), want, "{reason}");
    }
}

#[test]
fn the_reference_lists_every_kind_and_its_fix() {
    for cause in Cause::ALL {
        let row = format!("| `{}` |", cause.kind());
        assert!(REFERENCE.contains(&row), "no row for {}", cause.kind());
        let fix = cause
            .fix(Some(&n("PLACEHOLDER")))
            .replace("PLACEHOLDER", "<p>");
        assert!(
            REFERENCE.contains(&format!("`{fix}`")),
            "the reference does not quote the fix for {}: {fix}",
            cause.kind()
        );
    }
}

#[test]
fn every_fix_names_only_subcommands_that_exist() {
    for cause in Cause::ALL {
        let fix = cause.fix(Some(&n("p")));
        for (i, _) in fix.match_indices("amplihack profile ") {
            let sub = fix[i + "amplihack profile ".len()..]
                .split([' ', ',', ';'])
                .next()
                .unwrap();
            assert!(
                ["login", "verify", "seed-record"].contains(&sub),
                "{}: unknown subcommand {sub}",
                cause.kind()
            );
        }
    }
}
