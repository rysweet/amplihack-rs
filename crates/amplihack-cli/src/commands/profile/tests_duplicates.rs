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
//! - `warning(&Holder, &Holder) -> String`: the D21 text, without a prefix.
//! - `holder_label(&Holder) -> String`: the D20 holder text.
//! - `HOME_LABEL == "~/.claude"`.

use super::auto_switch::kit::{Ledger, State};
use super::duplicates::{HOME_LABEL, Holder, accounts, holder_label, pairs, warning};
use super::name::ProfileName;
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
    private(path, "SENTINEL");
    if super::auto_switch::euid() != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
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

    fn warnings(&self) -> Vec<String> {
        self.pairs().iter().map(|(a, b)| warning(a, b)).collect()
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
    assert_eq!(
        e.warnings(),
        vec![
            "profiles 'a' and 'c' are signed in to the same account and organization \
             (duplicate login); fix: run 'amplihack profile login c' with the browser \
             signed out of claude.ai or in a private window."
                .to_string()
        ]
    );
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
        warning(&got[1].0, &got[1].1),
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
        if f.ends_with("rotate/identify.rs") {
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
