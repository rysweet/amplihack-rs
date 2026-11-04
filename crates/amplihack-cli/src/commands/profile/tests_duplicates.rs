//! `duplicates`: two logins of the same account and organization (bug 2,
//! D19, D21). Only `.claude.json` files and saved `oauthAccount` blocks are
//! read; login fixtures are mode 0000.
//!
//! Contract:
//! - `Holder::{Profile(ProfileName), Home { live: Option<ProfileName> }}`.
//! - `accounts(&ProfileStore, home: Option<&Path>) -> Vec<(Holder, Account)>`:
//!   every profile in `store.list()` order whose account is known (its own
//!   `.claude.json`, or its saved block while it hosts another account), then
//!   `Home` when `$HOME/.claude.json` has an account. `live` is
//!   `default_live::live_name`.
//! - `pairs(&[(Holder, Account)]) -> Vec<(Holder, Holder)>`: every pair for
//!   which `Account::same` holds, profile pairs in list order, pairs with
//!   `Home` last as `(Home, Profile)`. The live profile is never paired with
//!   `Home`.
//! - `warning(a: &Holder, b: &Holder, wrong: Option<&Holder>) -> String`,
//!   without a prefix (issue #1845, D3): with `Some(w)` the D21 text whose
//!   fix names `w`; with `None` the cannot-tell text.
//! - `warnings(Option<&State>, &[(Holder, Account)]) -> Vec<String>`: one
//!   line per `pairs()` entry, in that order. A profile pair names the one
//!   profile that is mismatched (it has a recorded account its current
//!   account does not match), and is cannot-tell otherwise. A pair with
//!   `Home` judges `~/.claude` by the record of the rotator's live profile:
//!   it names the one mismatched holder (`~/.claude` with the `/login`
//!   text), never a profile that matches its record, the profile when it
//!   has no record and `~/.claude` matches or no live profile is known, and
//!   is cannot-tell otherwise. Without a state, no record is read.
//! - `record_warnings(Option<&State>, &[(Holder, Account)]) -> Vec<String>`:
//!   one `record_warning` line per profile whose account is not its
//!   recorded account (same user and organization), duplicate or not.
//! - `holder_label(&Holder) -> String`: the D20 holder text.
//! - `HOME_LABEL == "~/.claude"`.

use super::auto_switch::kit::{Ledger, State};
use super::duplicates::{
    HOME_LABEL, Holder, accounts, holder_label, pairs, record_warning, record_warnings, warning,
    warnings,
};
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn login(path: &Path) {
    // Issue #1700: an OAuth login (its top-level keys are read for the kind).
    private(path, r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#);
}

fn acct(uuid: &str, org: Option<&str>) -> String {
    let mut block = serde_json::json!({"accountUuid": uuid, "emailAddress": "me@example.com"});
    if let Some(o) = org {
        block["organizationUuid"] = o.into();
    }
    serde_json::json!({ "oauthAccount": block }).to_string()
}

/// Logged-in profiles `a`, `b`, `c` with distinct accounts; `~/.claude`
/// holds a login of an account no profile has.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let store = ProfileStore::new(tmp.path().join("root"));
    for p in ["a", "b", "c"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let e = Env {
        _tmp: tmp,
        home,
        store,
    };
    for p in ["a", "b", "c"] {
        e.profile(p, &acct(&format!("u-{p}"), Some("o1")));
        login(&e.store.root().join(p).join(".credentials.json"));
    }
    login(&e.home.join(".claude/.credentials.json"));
    private(&e.home.join(".claude.json"), &acct("u-home", Some("o1")));
    e
}

impl Env {
    fn profile(&self, p: &str, body: &str) {
        private(&self.store.root().join(p).join(".claude.json"), body);
    }

    fn pairs(&self) -> Vec<(Holder, Holder)> {
        pairs(&accounts(&self.store, Some(&self.home)))
    }

    fn state(&self) -> State {
        State::new(self.store.root()).unwrap()
    }

    fn warnings(&self) -> Vec<String> {
        warnings(
            Some(&self.state()),
            &accounts(&self.store, Some(&self.home)),
        )
    }

    /// Records `uuid` in organization `o1`, the one [`env`] gives every
    /// login, as the account profile `p` is meant to hold.
    fn record(&self, p: &str, uuid: &str) {
        self.record_in(p, uuid, "o1");
    }

    /// Records `uuid` in organization `org` for profile `p`.
    fn record_in(&self, p: &str, uuid: &str, org: &str) {
        let a = Account::from_block(&serde_json::json!({
            "accountUuid": uuid,
            "organizationUuid": org,
        }))
        .unwrap();
        super::expected::record(&self.state(), &n(p), &a).unwrap();
    }

    fn record_warnings(&self) -> Vec<String> {
        record_warnings(
            Some(&self.state()),
            &accounts(&self.store, Some(&self.home)),
        )
    }
}

fn p(s: &str) -> Holder {
    Holder::Profile(n(s))
}

#[test]
fn distinct_accounts_have_no_pairs() {
    assert!(env().pairs().is_empty());
}

#[test]
fn the_same_user_in_another_org_is_not_a_duplicate() {
    // Bug 1: ryan-mistt-org and ryan-mistt-personal.
    let e = env();
    e.profile("a", &acct("u-same", Some("o-team")));
    e.profile("b", &acct("u-same", Some("o-personal")));
    assert!(e.pairs().is_empty(), "{:?}", e.warnings());
}

#[test]
fn an_org_on_one_side_only_is_not_a_duplicate() {
    let e = env();
    e.profile("a", &acct("u-same", Some("o1")));
    e.profile("b", &acct("u-same", None));
    assert!(e.pairs().is_empty());
}

#[test]
fn the_same_user_and_org_is_a_duplicate_with_the_documented_text() {
    let e = env();
    e.profile("a", &acct("u-same", Some("o1")));
    e.profile("c", &acct("u-same", Some("o1")));
    assert_eq!(e.pairs(), vec![(p("a"), p("c"))]);
    // Issue #1845: with no record, amplihack cannot tell which is wrong.
    assert_eq!(e.warnings(), vec![cannot_tell("a", "c")]);
}

#[test]
fn the_same_user_without_orgs_is_a_duplicate() {
    let e = env();
    e.profile("a", &acct("u-same", None));
    e.profile("b", &acct("u-same", None));
    assert_eq!(e.pairs(), vec![(p("a"), p("b"))]);
}

#[test]
fn three_copies_give_every_pair_in_list_order() {
    let e = env();
    for x in ["a", "b", "c"] {
        e.profile(x, &acct("u-same", Some("o1")));
    }
    assert_eq!(
        e.pairs(),
        vec![(p("a"), p("b")), (p("a"), p("c")), (p("b"), p("c"))]
    );
}

#[test]
fn a_profile_matching_the_home_login_is_paired_with_home_last() {
    let e = env();
    e.profile("a", &acct("u-same", Some("o1")));
    e.profile("b", &acct("u-same", Some("o1")));
    e.profile("c", &acct("u-home", Some("o1")));
    let got = e.pairs();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0], (p("a"), p("b")));
    assert_eq!(got[1], (Holder::Home { live: None }, p("c")));
    assert_eq!(
        warning(&got[1].0, &got[1].1, Some(&got[1].1)),
        "profiles '~/.claude' and 'c' are signed in to the same account and organization \
         (duplicate login); fix: run 'amplihack profile login c' with the browser signed \
         out of claude.ai or in a private window."
    );
}

#[test]
fn the_rotators_live_profile_is_never_paired_with_home() {
    // `a` is live: its login is in `~/.claude`, its own directory has none.
    let e = env();
    fs::remove_file(e.store.root().join("a/.credentials.json")).unwrap();
    e.profile("a", &acct("u-home", Some("o1")));
    let list = accounts(&e.store, Some(&e.home));
    assert!(
        list.iter()
            .any(|(h, _)| *h == Holder::Home { live: Some(n("a")) }),
        "Home names the live profile"
    );
    assert!(pairs(&list).is_empty(), "{:?}", pairs(&list));
}

#[test]
fn a_profile_duplicating_the_live_login_is_flagged_against_home() {
    let e = env();
    fs::remove_file(e.store.root().join("a/.credentials.json")).unwrap();
    e.profile("a", &acct("u-home", Some("o1")));
    e.profile("b", &acct("u-home", Some("o1")));
    // `a` (live, login-less) and `b` (its own login) are the same account:
    // `b` is a duplicate of the login in `~/.claude`. `a` and `b` are also
    // reported as a profile pair.
    let got = e.pairs();
    assert!(
        got.contains(&(Holder::Home { live: Some(n("a")) }, p("b"))),
        "{got:?}"
    );
    assert!(
        !got.iter()
            .any(|(x, y)| *x == Holder::Home { live: Some(n("a")) } && *y == p("a"))
    );
}

#[test]
fn a_hosting_profile_is_compared_by_its_saved_block() {
    // `b`'s directory hosts `a`'s login, so its `.claude.json` names `a`'s
    // account; `b`'s own account is the saved block.
    let e = env();
    let state = State::new(e.store.root()).unwrap();
    state.ensure().unwrap();
    let ledger: Ledger = serde_json::from_value(serde_json::json!({
        "version": 1,
        "hosts": {"b": {"live": "a", "swapped_at": 1, "pre_swap_fingerprint": null}}
    }))
    .unwrap();
    state.save_ledger(&ledger).unwrap();
    state
        .write_sub_json(
            "accounts",
            "b",
            &serde_json::json!({"accountUuid": "u-b", "organizationUuid": "o1"}),
        )
        .unwrap();
    e.profile("b", &acct("u-a", Some("o1")));
    assert!(e.pairs().is_empty(), "{:?}", e.warnings());

    // And a saved block that duplicates another profile is flagged.
    state
        .write_sub_json(
            "accounts",
            "b",
            &serde_json::json!({"accountUuid": "u-c", "organizationUuid": "o1"}),
        )
        .unwrap();
    assert_eq!(e.pairs(), vec![(p("b"), p("c"))]);
}

#[test]
fn without_home_only_profiles_are_compared() {
    let e = env();
    e.profile("a", &acct("u-home", Some("o1")));
    let list = accounts(&e.store, None);
    assert!(!list.iter().any(|(h, _)| matches!(h, Holder::Home { .. })));
    assert!(pairs(&list).is_empty());
}

#[test]
fn home_without_an_account_is_left_out() {
    let e = env();
    private(&e.home.join(".claude.json"), "{}");
    let list = accounts(&e.store, Some(&e.home));
    assert!(!list.iter().any(|(h, _)| matches!(h, Holder::Home { .. })));
}

#[test]
fn profiles_without_an_account_are_left_out() {
    let e = env();
    private(&e.store.root().join("a/.claude.json"), "not json");
    fs::remove_file(e.store.root().join("b/.claude.json")).unwrap();
    let list = accounts(&e.store, Some(&e.home));
    assert!(!list.iter().any(|(h, _)| *h == p("a") || *h == p("b")));
    assert!(list.iter().any(|(h, _)| *h == p("c")));
}

#[test]
fn holder_labels_follow_the_refusal_text() {
    assert_eq!(HOME_LABEL, "~/.claude");
    assert_eq!(holder_label(&p("work")), "profile 'work'");
    assert_eq!(
        holder_label(&Holder::Home {
            live: Some(n("team"))
        }),
        "~/.claude (rotator live profile 'team')"
    );
    assert_eq!(holder_label(&Holder::Home { live: None }), "~/.claude");
}

#[test]
fn nothing_printed_names_an_account() {
    let e = env();
    e.profile("a", &acct("u-same", Some("org-same")));
    e.profile("b", &acct("u-same", Some("org-same")));
    for w in e.warnings() {
        for secret in ["u-same", "org-same", "@example.com"] {
            assert!(!w.contains(secret), "{w}");
        }
    }
    let list = accounts(&e.store, Some(&e.home));
    let shown = format!("{list:?}");
    assert!(
        !shown.contains("u-same") && !shown.contains("org-same"),
        "{shown}"
    );
}

/// D2/D4: `Account::same` is the only identity comparison. The JSON keys
/// that hold an identity are read only in `rotate/identify.rs`.
#[test]
fn identity_keys_are_read_only_in_identify() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("tests_")
            {
                out.push(p);
            }
        }
    }
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile");
    let mut files = Vec::new();
    walk(&base, &mut files);
    let mut offenders = Vec::new();
    for f in &files {
        // `Account` moved from `identify` to `account` (issue #1886).
        if f.ends_with("rotate/identify.rs") || f.ends_with("rotate/account.rs") {
            continue;
        }
        let src = fs::read_to_string(f).unwrap();
        for key in ["\"accountUuid\"", "\"organizationUuid\""] {
            if src.contains(key) {
                offenders.push(format!("{} reads {key}", f.display()));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

/// Login files are never opened, read or copied here: the module reads
/// `.claude.json` and saved blocks only.
#[test]
fn duplicates_never_touches_a_login_file() {
    let src = include_str!("duplicates.rs");
    for line in src.lines() {
        let l = line.to_ascii_lowercase();
        if l.contains("creds") || l.contains("credentials") {
            for bad in ["read", "open", "copy", "remove_file", "set_permissions"] {
                assert!(!l.contains(bad), "login file handled: {line}");
            }
        }
    }
}

// --- Issue #1845: which profile the warning names (D3, AC1, AC2) -----------

/// The D21 text naming `wrong` in its fix.
fn names(a: &str, b: &str, wrong: &str) -> String {
    format!(
        "profiles '{a}' and '{b}' are signed in to the same account and organization \
         (duplicate login); fix: run 'amplihack profile login {wrong}' with the browser \
         signed out of claude.ai or in a private window."
    )
}

/// The D3 cannot-tell text, word for word.
fn cannot_tell(a: &str, b: &str) -> String {
    format!(
        "profiles '{a}' and '{b}' are signed in to the same account and organization \
         (duplicate login); amplihack cannot tell which of them holds the wrong login; \
         fix: run 'amplihack profile login <name>' for the one that should hold a \
         different account, with the browser signed out of claude.ai or in a private \
         window."
    )
}

/// `a` and `c` both hold `u-same`.
fn dup_a_c() -> Env {
    let e = env();
    e.profile("a", &acct("u-same", Some("o1")));
    e.profile("c", &acct("u-same", Some("o1")));
    e
}

#[test]
fn warning_names_the_holder_it_is_given_or_cannot_tell() {
    assert_eq!(
        warning(&p("a"), &p("b"), Some(&p("b"))),
        names("a", "b", "b")
    );
    assert_eq!(
        warning(&p("a"), &p("b"), Some(&p("a"))),
        names("a", "b", "a")
    );
    assert_eq!(warning(&p("a"), &p("b"), None), cannot_tell("a", "b"));
}

#[test]
fn the_one_mismatched_profile_is_named_when_it_is_second() {
    let e = dup_a_c();
    e.record("a", "u-same");
    e.record("c", "u-c-recorded");
    assert_eq!(e.warnings(), vec![names("a", "c", "c")]);
}

#[test]
fn the_one_mismatched_profile_is_named_when_it_is_first() {
    // The bug: the fix always named the second profile, here the right one.
    let e = dup_a_c();
    e.record("a", "u-a-recorded");
    e.record("c", "u-same");
    assert_eq!(e.warnings(), vec![names("a", "c", "a")]);
}

#[test]
fn a_mismatched_profile_paired_with_an_unrecorded_one_is_named() {
    let e = dup_a_c();
    e.record("a", "u-a-recorded");
    assert_eq!(e.warnings(), vec![names("a", "c", "a")]);

    let e = dup_a_c();
    e.record("c", "u-c-recorded");
    assert_eq!(e.warnings(), vec![names("a", "c", "c")]);
}

#[test]
fn without_records_amplihack_cannot_tell() {
    let e = dup_a_c();
    assert_eq!(e.warnings(), vec![cannot_tell("a", "c")]);
}

#[test]
fn a_matching_profile_paired_with_an_unrecorded_one_names_the_unrecorded_one() {
    // Issue #1845's own case: after 'ryan-mistt-personal' is logged in
    // again it has a matching record, and 'bbuck', which a backfill will
    // not record while another profile holds the account, is named.
    let e = dup_a_c();
    e.record("a", "u-same");
    assert_eq!(e.warnings(), vec![names("a", "c", "c")]);

    let e = dup_a_c();
    e.record("c", "u-same");
    assert_eq!(e.warnings(), vec![names("a", "c", "a")]);
}

#[test]
fn when_both_match_their_records_amplihack_cannot_tell() {
    let e = dup_a_c();
    e.record("a", "u-same");
    e.record("c", "u-same");
    assert_eq!(e.warnings(), vec![cannot_tell("a", "c")]);
}

#[test]
fn when_both_are_mismatched_amplihack_cannot_tell() {
    let e = dup_a_c();
    e.record("a", "u-a-recorded");
    e.record("c", "u-c-recorded");
    assert_eq!(e.warnings(), vec![cannot_tell("a", "c")]);
}

#[test]
fn a_damaged_record_counts_as_none() {
    // Undamaged, c's record would name c; damaged, neither has a record.
    let e = dup_a_c();
    e.record("c", "u-c-recorded");
    let file = e.store.root().join(".auto-switch.d/expected/c.json");
    private(&file, "not json");
    assert_eq!(e.warnings(), vec![cannot_tell("a", "c")]);
}

#[test]
fn without_a_state_no_record_is_read() {
    let e = dup_a_c();
    e.record("c", "u-c-recorded");
    let list = accounts(&e.store, Some(&e.home));
    assert_eq!(warnings(None, &list), vec![cannot_tell("a", "c")]);
}

#[test]
fn every_pair_gets_its_own_line_in_pair_order() {
    let e = env();
    for x in ["a", "b", "c"] {
        e.profile(x, &acct("u-same", Some("o1")));
    }
    e.record("a", "u-same");
    e.record("b", "u-b-recorded");
    assert_eq!(
        e.warnings(),
        vec![
            names("a", "b", "b"),
            names("a", "c", "c"),
            names("b", "c", "b"),
        ]
    );
}

/// The cannot-tell text for a pair with `~/.claude`, word for word.
fn home_cannot_tell(b: &str) -> String {
    format!(
        "profiles '~/.claude' and '{b}' are signed in to the same account and organization \
         (duplicate login); amplihack cannot tell which of them holds the wrong login; fix: \
         run 'amplihack profile login {b}' if '{b}' should hold a different account, or plain \
         'claude' and '/login' if ~/.claude should, with the browser signed out of claude.ai \
         or in a private window."
    )
}

/// The text naming `~/.claude`, which is off the record of live profile
/// `live`, word for word.
fn home_named(b: &str, live: &str) -> String {
    format!(
        "profiles '~/.claude' and '{b}' are signed in to the same account and organization \
         (duplicate login); the login in ~/.claude is not the account recorded for profile \
         '{live}'; fix: run plain 'claude' and '/login' to the account '{live}' is meant for, \
         with the browser signed out of claude.ai or in a private window."
    )
}

#[test]
fn without_a_live_profile_a_home_pair_names_the_profile_unless_it_matches() {
    let e = env();
    e.profile("c", &acct("u-home", Some("o1")));
    let list = accounts(&e.store, Some(&e.home));
    assert!(list.iter().any(|(h, _)| *h == Holder::Home { live: None }));
    let named = vec![names("~/.claude", "c", "c")];
    assert_eq!(e.warnings(), named, "no record");
    assert_eq!(warnings(None, &list), named, "no state");
    e.record("c", "u-c-recorded");
    assert_eq!(e.warnings(), named, "a mismatched record");
    e.record("c", "u-home");
    assert_eq!(
        e.warnings(),
        vec![home_cannot_tell("c")],
        "a profile matching its record is never named"
    );
}

/// `a` is the rotator's live profile by the rotate ledger; its folder has
/// no login and still names `u-a`. `~/.claude` and `b` both hold `u-b`.
fn live_a_home_dup_b() -> Env {
    let e = env();
    fs::remove_file(e.store.root().join("a/.credentials.json")).unwrap();
    private(
        &e.store.root().join(".auto-switch.d/rotate/ledger.json"),
        r#"{"version":1,"live":"a","since":10}"#,
    );
    for d in [".auto-switch.d", ".auto-switch.d/rotate"] {
        fs::set_permissions(e.store.root().join(d), fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(&e.home.join(".claude.json"), &acct("u-b", Some("o1")));
    let list = accounts(&e.store, Some(&e.home));
    assert!(
        list.iter()
            .any(|(h, _)| *h == Holder::Home { live: Some(n("a")) }),
        "{list:?}"
    );
    assert_eq!(
        pairs(&list),
        vec![(Holder::Home { live: Some(n("a")) }, p("b"))]
    );
    e
}

#[test]
fn home_off_the_live_profiles_record_is_named_not_the_matching_profile() {
    // Issue #1845: the swap guard refuses this state ("the login in
    // ~/.claude is not the account recorded for profile a"); the warning
    // must agree with it and must not send the user to log `b` in again.
    let e = live_a_home_dup_b();
    e.record("a", "u-a");
    e.record("b", "u-b");
    let got = e.warnings();
    assert_eq!(got, vec![home_named("b", "a")]);
    assert!(!got[0].contains("profile login b"), "{got:?}");
    // `b` has no record: `~/.claude` is still the one off its record.
    let e = live_a_home_dup_b();
    e.record("a", "u-a");
    assert_eq!(e.warnings(), vec![home_named("b", "a")]);
}

#[test]
fn a_profile_off_its_record_is_named_when_home_matches_the_live_record() {
    let e = live_a_home_dup_b();
    e.record("a", "u-b");
    assert_eq!(
        e.warnings(),
        vec![names("~/.claude", "b", "b")],
        "b has no record"
    );
    e.record("b", "u-b-recorded");
    assert_eq!(
        e.warnings(),
        vec![names("~/.claude", "b", "b")],
        "b is off its record"
    );
}

#[test]
fn a_home_pair_with_a_live_profile_cannot_be_told_without_evidence() {
    let e = live_a_home_dup_b();
    assert_eq!(e.warnings(), vec![home_cannot_tell("b")], "no record");
    let list = accounts(&e.store, Some(&e.home));
    assert_eq!(
        warnings(None, &list),
        vec![home_cannot_tell("b")],
        "no state"
    );
    e.record("a", "u-b");
    e.record("b", "u-b");
    assert_eq!(e.warnings(), vec![home_cannot_tell("b")], "both match");
    e.record("a", "u-a");
    e.record("b", "u-b-recorded");
    assert_eq!(e.warnings(), vec![home_cannot_tell("b")], "both mismatched");
}

#[test]
fn a_hosting_profile_is_judged_by_its_saved_block() {
    // `b` hosts `a`'s login, so its `.claude.json` names `a`'s account; its
    // own account, the saved block, duplicates `c` and is not its record.
    let e = env();
    let state = e.state();
    state.ensure().unwrap();
    let ledger: Ledger = serde_json::from_value(serde_json::json!({
        "version": 1,
        "hosts": {"b": {"live": "a", "swapped_at": 1, "pre_swap_fingerprint": null}}
    }))
    .unwrap();
    state.save_ledger(&ledger).unwrap();
    state
        .write_sub_json(
            "accounts",
            "b",
            &serde_json::json!({"accountUuid": "u-c", "organizationUuid": "o1"}),
        )
        .unwrap();
    e.profile("b", &acct("u-a", Some("o1")));
    e.record("b", "u-b");
    e.record("c", "u-c");
    assert_eq!(e.warnings(), vec![names("b", "c", "b")]);
    // Its `.claude.json` (`u-a`) is not what is compared with the record.
    e.record("b", "u-a");
    assert_eq!(e.warnings(), vec![names("b", "c", "b")]);
}

#[test]
fn neither_form_prints_an_account_or_a_record() {
    let e = env();
    e.profile("a", &acct("u-SENTINEL-same", Some("org-SENTINEL")));
    e.profile("b", &acct("u-SENTINEL-same", Some("org-SENTINEL")));
    e.profile("c", &acct("u-SENTINEL-same", Some("org-SENTINEL")));
    e.record("a", "u-SENTINEL-recorded");
    e.record("b", "u-SENTINEL-same");
    let got = e.warnings();
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got.iter().any(|w| w.contains("cannot tell")), "{got:?}");
    assert!(got.iter().any(|w| !w.contains("cannot tell")), "{got:?}");
    for w in got {
        for secret in ["SENTINEL", "@example.com", "sk-ant", "accessToken"] {
            assert!(!w.contains(secret), "{secret:?} in {w}");
        }
    }
}

#[test]
fn warnings_never_write_a_record() {
    let e = dup_a_c();
    let _ = e.warnings();
    assert!(
        !e.store.root().join(".auto-switch.d/expected").exists(),
        "the warning only reads"
    );
}

// --- Organizations and per-profile record checks (#1845 review) -----------

#[test]
fn the_same_user_in_another_organization_is_mismatched() {
    // ryan-mistt-org recorded in its team organization, holding the same
    // user's personal-organization login.
    let e = dup_a_c();
    e.record("a", "u-same");
    e.record_in("c", "u-same", "o-personal");
    assert_eq!(e.warnings(), vec![names("a", "c", "c")]);
}

#[test]
fn a_profile_off_its_record_is_warned_about_without_a_duplicate() {
    // The #1845 writer: a session with CLAUDE_CONFIG_DIR on `b`'s folder
    // wrote another account's login there, which no other profile holds.
    let e = env();
    e.record("a", "u-a");
    e.record("b", "u-b");
    e.profile("b", &acct("u-elsewhere", Some("o1")));
    assert!(e.warnings().is_empty(), "no duplicate pair");
    assert_eq!(e.record_warnings(), vec![record_warning(&p("b"))]);
}

#[test]
fn a_profile_in_another_organization_than_recorded_is_warned_about() {
    let e = env();
    e.record_in("a", "u-a", "o-team");
    assert_eq!(e.record_warnings(), vec![record_warning(&p("a"))]);
}

#[test]
fn record_warnings_skip_matching_and_unrecorded_profiles_and_need_a_state() {
    let e = env();
    e.record("a", "u-a");
    assert!(e.record_warnings().is_empty());
    e.record("c", "u-c-recorded");
    let list = accounts(&e.store, Some(&e.home));
    assert!(
        record_warnings(None, &list).is_empty(),
        "no state, no record"
    );
    assert_eq!(e.record_warnings(), vec![record_warning(&p("c"))]);
}

#[test]
fn the_record_warning_names_the_profile_and_no_account() {
    let w = record_warning(&p("bbuck"));
    assert_eq!(
        w,
        "profile 'bbuck' holds a login that is not the account recorded for it; fix: run \
         'amplihack profile login bbuck' with the browser signed out of claude.ai or in a \
         private window."
    );
}
