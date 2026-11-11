//! Issue #2218 (R6): a pin that holds the live login on an excluded overflow
//! profile never runs the held spend-limit probe. The pin still holds; the
//! cycle logs why the spend was not probed.
//!
//! In process, on the `tests_swap.rs` fixture, as in `tests_pin_overflow`.
//! `Ctx::probe_held` is a stub that panics, so a probe fails the test;
//! `Ctx::measure` panics too, since a held cycle measures nothing. The
//! end-to-end rotator, `profile usage` and `profile verify` cases are in
//! `tests/issue_2218_excluded.rs`.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::OverflowEntry;
use crate::commands::profile::rotate::TuneFlags;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use crate::commands::profile::rotate::observe::{Measurement, Snap};
use crate::commands::profile::rotate::pin::{self, PinState};
use crate::commands::profile::rotate::settings;
use crate::commands::profile::rotate::switch::{Switched, switch_live};
use crate::commands::profile::rotate::tests_swap::{Env, env, ino_at, n};
use std::fs;

fn never_measured(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("a held cycle measured")
}

fn never_probed(_: &Ctx, name: &ProfileName) -> Result<bool, ProfileError> {
    panic!("a held cycle probed excluded profile {name}")
}

fn ctx(e: &Env) -> Ctx {
    let s = settings::resolve(&TuneFlags::default(), None, &|_| None).unwrap();
    let mut c = Ctx::new(e.store.clone(), e.home.clone(), s);
    c.measure = Some(never_measured);
    c.probe_held = Some(never_probed);
    c
}

#[test]
fn a_pinned_excluded_overflow_profile_is_held_without_a_spend_probe() {
    let e = env();
    config::update(&e.root, ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n("spare"), 1));
        Ok(())
    })
    .unwrap();
    match switch_live(&e.store, &e.home, &n("spare")) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch spare failed: {other:?}"),
    }
    // Excluded after the pin was set: the pin still holds.
    config::update(&e.root, ReadMode::Editable, |c| {
        c.exclude.push(n("spare"));
        Ok(())
    })
    .unwrap();
    let held = ino_at(&e.live());
    let creds = e.live();
    let before = (
        fs::read(&creds).unwrap(),
        fs::metadata(&creds).unwrap().modified().unwrap(),
    );
    let mut c = ctx(&e);
    for i in 0..2 {
        assert_eq!(
            run_cycle(&mut c, Mode::Watch).unwrap(),
            CycleOutcome::Held,
            "cycle {i}: the pin holds"
        );
        assert_eq!(ino_at(&e.live()), held, "cycle {i} moved the login");
    }
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    let after = (
        fs::read(&creds).unwrap(),
        fs::metadata(&creds).unwrap().modified().unwrap(),
    );
    assert_eq!(after, before, "the login is not rewritten");
    let log = fs::read_to_string(&e.rs.log).unwrap_or_default();
    assert!(
        log.contains("pinned profile spare is excluded; spend not probed"),
        "{log}"
    );
}

// ---------------------------------------------------------------------------
// `rotate status`: history written before the fix (G2)
// ---------------------------------------------------------------------------

mod status_history {
    use crate::commands::profile::auto_switch::config::{self, ReadMode};
    use crate::commands::profile::rotate::history::capacity_line;
    use crate::commands::profile::rotate::tests_swap::{Env, env, n};
    use crate::commands::profile::usage_history::{self, Entry, Point, Writer, to_line};
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn pt(s: u8, w: u8) -> Point {
        Point {
            s: Some(s),
            w,
            w_reset: None,
            s_reset: None,
            no_session_limit: false,
            passive: false,
        }
    }

    fn line(at: i64, by: Writer, points: &[(&str, Point)], unknown: &[&str]) -> Entry {
        Entry {
            at,
            profiles: points
                .iter()
                .map(|(k, p)| (k.to_string(), p.clone()))
                .collect::<BTreeMap<_, _>>(),
            by: Some(by),
            unknown: unknown.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn seed(e: &Env, entries: &[Entry]) {
        let dir = e.root.join(".auto-switch.d");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(usage_history::FILE);
        let body: String = entries
            .iter()
            .map(|x| format!("{}\n", to_line(x)))
            .collect();
        fs::write(&file, body).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn exclude(e: &Env, name: &str) {
        config::update(&e.root, ReadMode::Editable, |c| {
            c.exclude.push(n(name));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn the_capacity_line_does_not_count_an_excluded_profile_from_an_old_line() {
        let e = env();
        let at = now() - 60;
        // Written before the fix: spare read at 0/0 alongside work at 10/40.
        seed(
            &e,
            &[line(
                at,
                Writer::Usage,
                &[("work", pt(10, 40)), ("spare", pt(0, 0))],
                &[],
            )],
        );
        let before = capacity_line(&e.root, at + 60);
        assert!(before.contains("login-weeks of 2 "), "seeded: {before}");
        exclude(&e, "spare");
        let line = capacity_line(&e.root, at + 60);
        assert!(
            line.contains("login-weeks of 1 "),
            "N leaves spare out: {line}"
        );
        assert!(line.contains("usable 1 of 1 logins"), "{line}");
        assert!(
            line.contains("headroom 0.60 "),
            "spare's 1.00 is not counted: {line}"
        );
    }

    #[test]
    fn the_capacity_line_does_not_list_an_excluded_profile_as_unknown_or_left_out() {
        let e = env();
        let at = now() - 60;
        seed(
            &e,
            &[line(
                at,
                Writer::Rotate,
                &[("work", pt(10, 40))],
                &["spare"],
            )],
        );
        let before = capacity_line(&e.root, at + 60);
        assert!(before.contains("1 unknown"), "seeded: {before}");
        exclude(&e, "spare");
        let line = capacity_line(&e.root, at + 60);
        assert!(!line.contains("unknown,"), "no unknown count: {line}");
        assert!(!line.contains("left out"), "no login left out: {line}");
        assert!(line.contains("usable 1 of 1 logins"), "{line}");
    }
}

// ---------------------------------------------------------------------------
// Local proof of an excluded live login, and the local-only identity set
// ---------------------------------------------------------------------------

/// Contract for the implementation (design steps 1-3, 8):
///
/// - `rotate::live_excluded_offline(store, rs, home)
///   -> Option<(ProfileName, Match)>` names the excluded profile in
///   `~/.claude` from local files only. `Match` is `Ledger` (rule 1: the
///   ledger's `live` is excluded; enough on its own), `LocalAccount` (rule 2:
///   `~/.claude.json` matches the excluded profile's record) or `SameBytes`
///   (rule 3: `~/.claude/.credentials.json` is byte-identical to the
///   excluded slot's). Rules 2 and 3 run whatever the ledger names.
/// - `Ctx::set_excluded_dirs(Vec<PathBuf>)` writes the local-only set held
///   on `login_identity::Budget`. For a dir in the set, `Reader::bound` takes
///   the account from the local `.claude.json` (`~/.claude.json` for
///   `~/.claude`) in place of the injected read, and keeps the budget, stamp
///   and login-kind checks. No local account gives `UNAVAILABLE`.
mod local_proof {
    use crate::commands::profile::auto_switch::config::{self, ReadMode};
    use crate::commands::profile::auto_switch::kit::State;
    use crate::commands::profile::expected;
    use crate::commands::profile::rotate::cycle::Ctx;
    use crate::commands::profile::rotate::hand_fields::HandSuspect;
    use crate::commands::profile::rotate::identify::Account;
    use crate::commands::profile::rotate::ledger::RotateLedger;
    use crate::commands::profile::rotate::login_identity::UNAVAILABLE;
    use crate::commands::profile::rotate::tests_swap::{CREDS, Env, env, login, n, private};
    use crate::commands::profile::rotate::{Match, live_excluded_offline, settings};
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn acct(uuid: &str) -> Account {
        Account::from_claude_json(
            serde_json::json!({"oauthAccount": {
                "accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")}})
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    fn exclude(e: &Env, names: &[&str]) {
        config::update(&e.root, ReadMode::Editable, |c| {
            for p in names {
                c.exclude.push(n(p));
            }
            Ok(())
        })
        .unwrap();
    }

    /// `work`, `spare` and `extra`, each with a record `u-<name>`; `extra`
    /// has a login of its own. The ledger names `work` live, as `env()` sets.
    fn fx() -> (Env, State) {
        let e = env();
        assert!(e.root.starts_with(std::env::temp_dir()));
        assert!(e.home.starts_with(std::env::temp_dir()));
        e.store.add(&n("extra")).unwrap();
        login(&e.root.join("extra").join(CREDS));
        private(
            &e.root.join("extra/.claude.json"),
            &serde_json::json!({"oauthAccount": {
                "accountUuid": "u-extra", "emailAddress": "u-extra@example.com"}})
            .to_string(),
        );
        let state = State::new(&e.root).unwrap();
        for p in ["work", "spare", "extra"] {
            expected::record(&state, &n(p), &acct(&format!("u-{p}"))).unwrap();
        }
        (e, state)
    }

    fn set_ledger(e: &Env, live: Option<&str>, suspect: Option<&str>) {
        let mut l = e.rs.load_ledger().unwrap().unwrap_or_default();
        l.live = live.map(Into::into);
        l.account = live.map(|p| format!("u-{p}"));
        l.login_identity = None;
        l.hand_suspect = suspect.map(|p| HandSuspect {
            profile: p.into(),
            first_seen: 1_760_000_000,
        });
        e.rs.save_ledger(&RotateLedger { ..l }).unwrap();
    }

    fn set_home_json(e: &Env, uuid: &str) {
        private(
            &e.home.join(".claude.json"),
            &serde_json::json!({"oauthAccount": {
                "accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")}})
            .to_string(),
        );
    }

    fn drop_home_json(e: &Env) {
        fs::remove_file(e.home.join(".claude.json")).unwrap();
    }

    /// `~/.claude/.credentials.json` made byte-identical to `<root>/<p>`'s.
    fn copy_slot_to_live(e: &Env, p: &str) {
        let bytes = fs::read(e.slot(p)).unwrap();
        fs::remove_file(e.live()).unwrap();
        fs::write(e.live(), &bytes).unwrap();
        fs::set_permissions(e.live(), fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn proof(e: &Env) -> Option<(crate::commands::profile::name::ProfileName, Match)> {
        live_excluded_offline(&e.store, &e.rs, &e.home)
    }

    // --- rule 1: the ledger ---------------------------------------------

    #[test]
    fn rule_1_a_ledger_naming_the_excluded_profile_is_enough_on_its_own() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, Some("work"), None);
        // `~/.claude.json` disagrees: rule 1 still matches (S2, fails closed).
        set_home_json(&e, "u-spare");
        assert_eq!(proof(&e), Some((n("work"), Match::Ledger)));
    }

    #[test]
    fn rule_1_holds_with_no_claude_json_at_all() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, Some("work"), None);
        drop_home_json(&e);
        assert_eq!(proof(&e), Some((n("work"), Match::Ledger)));
    }

    // --- a ledger naming a profile that is not excluded --------------------
    //
    // A hand login leaves the ledger stale, so rules 2 and 3 run whatever
    // it names, and `~/.claude` is local-only when either matches.

    use crate::commands::profile::rotate::identify::excluded_offline::{
        local_only_dirs, may_write_back,
    };

    /// `~/.claude` read through a cycle's gate with a counting reader: the
    /// number of requests it sent.
    fn requests_for_home(e: &Env) -> usize {
        CALLS.with(|c| c.set(0));
        let mut c = ctx(e);
        c.identity = counting;
        c.set_excluded_dirs(local_only_dirs(&e.store, &e.rs, &e.home));
        let _ = c.identity_reader().oauth(&home_dir(e));
        calls()
    }

    #[test]
    fn a_ledger_naming_another_profile_with_the_excluded_login_s_bytes_matches_rule_3() {
        let (e, _) = fx();
        exclude(&e, &["extra"]);
        set_ledger(&e, Some("work"), None);
        // `~/.claude.json` still names work: only the bytes say extra.
        copy_slot_to_live(&e, "extra");
        assert_eq!(proof(&e), Some((n("extra"), Match::SameBytes)));
        assert!(local_only_dirs(&e.store, &e.rs, &e.home).contains(&home_dir(&e)));
        assert_eq!(requests_for_home(&e), 0, "no request with extra's token");
    }

    #[test]
    fn a_ledger_naming_another_profile_with_claude_json_naming_the_excluded_one_matches_rule_2() {
        // A fresh `claude login` as work writes new tokens and work's
        // account into `~/.claude.json`; the ledger still says spare.
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, Some("spare"), None);
        set_home_json(&e, "u-work");
        assert_eq!(proof(&e), Some((n("work"), Match::LocalAccount)));
        assert_eq!(requests_for_home(&e), 0, "no request with work's token");
        assert!(
            !may_write_back(&e.store, &e.rs, &e.home, &n("work")),
            "a match by the local account alone never moves the login out (S4)"
        );
    }

    #[test]
    fn a_ledger_naming_another_profile_with_nothing_naming_the_excluded_one_matches_nothing() {
        // The regression guard against over-matching.
        let (e, _) = fx();
        exclude(&e, &["extra"]);
        set_ledger(&e, Some("work"), None);
        set_home_json(&e, "u-work");
        assert_ne!(
            fs::read(e.live()).unwrap(),
            fs::read(e.slot("extra")).unwrap()
        );
        assert_eq!(proof(&e), None);
        assert!(!local_only_dirs(&e.store, &e.rs, &e.home).contains(&home_dir(&e)));
        assert_eq!(requests_for_home(&e), 1, "~/.claude is read as before");
    }

    // --- rule 2: the local account ----------------------------------------

    #[test]
    fn rule_2_no_ledger_live_and_claude_json_matching_the_record() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, None, None);
        set_home_json(&e, "u-work");
        assert_eq!(proof(&e), Some((n("work"), Match::LocalAccount)));
    }

    #[test]
    fn rule_2_a_suspected_hand_change_lets_claude_json_name_the_excluded_profile() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, Some("spare"), Some("work"));
        set_home_json(&e, "u-work");
        assert_eq!(proof(&e), Some((n("work"), Match::LocalAccount)));
    }

    #[test]
    fn rule_2_an_excluded_profile_without_a_record_is_not_matched() {
        let (e, _) = fx();
        expected::forget(&e.root, &n("work"));
        exclude(&e, &["work"]);
        set_ledger(&e, None, None);
        set_home_json(&e, "u-work");
        assert_eq!(proof(&e), None, "no record, no match; no request instead");
    }

    #[test]
    fn rule_2_an_e_mail_alone_never_matches() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, None, None);
        // Same e-mail as `work`'s record, another account uuid.
        private(
            &e.home.join(".claude.json"),
            &serde_json::json!({"oauthAccount": {
                "accountUuid": "u-other", "emailAddress": "u-work@example.com"}})
            .to_string(),
        );
        assert_eq!(proof(&e), None);
    }

    #[test]
    fn rule_2_a_claude_json_naming_a_profile_that_is_not_excluded_matches_nothing() {
        let (e, _) = fx();
        exclude(&e, &["extra"]);
        set_ledger(&e, None, None);
        set_home_json(&e, "u-work");
        assert_eq!(proof(&e), None);
    }

    // --- rule 3: the same bytes -------------------------------------------

    #[test]
    fn rule_3_no_local_account_and_credentials_identical_to_the_excluded_slot() {
        let (e, _) = fx();
        exclude(&e, &["extra"]);
        set_ledger(&e, None, None);
        drop_home_json(&e);
        copy_slot_to_live(&e, "extra");
        assert_eq!(proof(&e), Some((n("extra"), Match::SameBytes)));
    }

    #[test]
    fn rule_3_credentials_that_differ_from_every_excluded_slot_match_nothing() {
        let (e, _) = fx();
        exclude(&e, &["extra"]);
        set_ledger(&e, None, None);
        drop_home_json(&e);
        assert_ne!(
            fs::read(e.live()).unwrap(),
            fs::read(e.slot("extra")).unwrap()
        );
        assert_eq!(proof(&e), None);
    }

    #[test]
    fn an_unsafe_claude_json_counts_as_missing_and_falls_to_rule_3() {
        for kind in ["group-writable", "symlink"] {
            let (e, _) = fx();
            exclude(&e, &["extra"]);
            set_ledger(&e, None, None);
            set_home_json(&e, "u-extra");
            let cj = e.home.join(".claude.json");
            match kind {
                "group-writable" => {
                    fs::set_permissions(&cj, fs::Permissions::from_mode(0o660)).unwrap()
                }
                _ => {
                    let real = e.home.join("real.claude.json");
                    fs::rename(&cj, &real).unwrap();
                    std::os::unix::fs::symlink(&real, &cj).unwrap();
                }
            }
            assert_eq!(proof(&e), None, "{kind}: not a local proof");
            copy_slot_to_live(&e, "extra");
            assert_eq!(
                proof(&e),
                Some((n("extra"), Match::SameBytes)),
                "{kind}: the byte comparison still applies"
            );
        }
    }

    // --- no exclusion -------------------------------------------------------

    #[test]
    fn nothing_excluded_matches_nothing() {
        let (e, _) = fx();
        set_ledger(&e, Some("work"), None);
        assert_eq!(proof(&e), None);
        set_ledger(&e, None, None);
        assert_eq!(proof(&e), None);
    }

    #[test]
    fn a_corrupt_exclude_config_excludes_nothing() {
        let (e, _) = fx();
        exclude(&e, &["work"]);
        set_ledger(&e, Some("work"), None);
        let cfg = e.root.join(".auto-switch");
        assert!(cfg.exists(), "the config the exclude list lives in");
        let file = if cfg.is_dir() {
            fs::read_dir(&cfg)
                .unwrap()
                .map(|d| d.unwrap().path())
                .find(|p| p.extension().is_some_and(|x| x == "json"))
                .expect("the auto-switch config file")
        } else {
            cfg
        };
        fs::write(&file, b"{ not json").unwrap();
        assert_eq!(proof(&e), None, "the existing EXCLUDE_NOT_APPLIED rule");
    }

    // --- the local-only set behind Reader::bound ---------------------------

    thread_local! {
        static CALLS: Cell<usize> = const { Cell::new(0) };
    }

    /// A reader that counts its calls and answers an account no local file
    /// holds, so a result from it is never mistaken for a local one.
    fn counting(_: &Path) -> Result<Account, &'static str> {
        CALLS.with(|c| c.set(c.get() + 1));
        Ok(acct("u-from-the-server"))
    }

    fn calls() -> usize {
        CALLS.with(Cell::get)
    }

    fn ctx(e: &Env) -> Ctx {
        let s = settings::resolve(
            &crate::commands::profile::rotate::TuneFlags::default(),
            None,
            &|_| None,
        )
        .unwrap();
        Ctx::new(e.store.clone(), e.home.clone(), s)
    }

    fn home_dir(e: &Env) -> PathBuf {
        e.home.join(".claude")
    }

    #[test]
    fn a_marked_home_gives_the_claude_json_account_without_a_read() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![home_dir(&e)]);
        let got = c.identity_reader().oauth(&home_dir(&e)).unwrap();
        assert!(got.same(&acct("u-work")), "the ~/.claude.json account");
        assert_eq!(calls(), 0, "no request with the excluded token");
    }

    #[test]
    fn the_gate_applies_when_the_reader_is_replaced_after_the_set_is_written() {
        // S7: the set lives on the budget and the read is looked up at call
        // time, so a test (or caller) replacing `identity` stays gated.
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.set_excluded_dirs(vec![home_dir(&e)]);
        c.identity = counting;
        c.identity_reader().oauth(&home_dir(&e)).unwrap();
        assert_eq!(calls(), 0);
    }

    #[test]
    fn a_marked_slot_gives_its_own_claude_json_account_without_a_read() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![e.root.join("extra")]);
        let got = c.identity_reader().oauth(&e.root.join("extra")).unwrap();
        assert!(got.same(&acct("u-extra")));
        assert_eq!(calls(), 0);
    }

    #[test]
    fn a_marked_dir_with_no_local_account_is_unavailable_and_sends_nothing() {
        let (e, _) = fx();
        fs::remove_file(e.root.join("extra/.claude.json")).unwrap();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![e.root.join("extra")]);
        assert_eq!(
            c.identity_reader().oauth(&e.root.join("extra")).err(),
            Some(UNAVAILABLE)
        );
        assert_eq!(calls(), 0, "no request in place of the local account");
    }

    #[test]
    fn an_unmarked_dir_is_read_as_before() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![e.root.join("extra")]);
        let got = c.identity_reader().oauth(&e.root.join("spare"));
        assert!(got.unwrap().same(&acct("u-from-the-server")));
        assert_eq!(calls(), 1, "the incoming target is still verified (S5)");
    }

    #[test]
    fn a_new_ctx_marks_nothing() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.identity_reader().oauth(&home_dir(&e)).unwrap();
        assert_eq!(calls(), 1, "Budget::new() has an empty local-only set");
    }

    #[test]
    fn writing_the_set_again_replaces_it() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![home_dir(&e)]);
        c.set_excluded_dirs(vec![]);
        c.identity_reader().oauth(&home_dir(&e)).unwrap();
        assert_eq!(calls(), 1, "the set is reset each cycle, not added to");
    }

    #[test]
    fn a_folder_marked_through_a_symlinked_ancestor_matches_its_resolved_path() {
        // Callers such as `identity_claims::scan` pass the resolved slot
        // path; the local-only set may hold the path through the link.
        let (e, _) = fx();
        let link = e.root.parent().unwrap().join("root-link");
        std::os::unix::fs::symlink(&e.root, &link).unwrap();
        let resolved = fs::canonicalize(e.root.join("extra")).unwrap();
        for (marked, asked) in [
            (link.join("extra"), resolved.clone()),
            (resolved.clone(), link.join("extra")),
        ] {
            CALLS.with(|c| c.set(0));
            let mut c = ctx(&e);
            c.identity = counting;
            c.set_excluded_dirs(vec![marked.clone()]);
            let got = c.identity_reader().oauth(&asked).unwrap();
            assert!(got.same(&acct("u-extra")), "{marked:?} / {asked:?}");
            assert_eq!(calls(), 0, "{marked:?} / {asked:?}: no request");
        }
    }

    #[test]
    fn recovery_of_a_swap_moving_an_excluded_login_out_sends_no_request() {
        // The swap stopped after the exchange: work's login waits in spare's
        // folder and spare's in `~/.claude`. Start-up recovery's budget, and
        // the outgoing folder wherever it is, are local-only.
        use crate::commands::profile::rotate::identify::excluded_offline::gated_budget;
        use crate::commands::profile::rotate::ledger::Phase;
        use crate::commands::profile::rotate::login_identity::Reader;
        use crate::commands::profile::rotate::recover::{RecoverOutcome, recover_with};
        let (e, _) = fx();
        exclude(&e, &["work"]);
        e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
        e.exchanged();
        CALLS.with(|c| c.set(0));
        let budget = gated_budget(&e.store, &e.rs, &e.home);
        let outcome = recover_with(
            &e.store,
            &e.rs,
            &e.home,
            Reader {
                read: &counting,
                budget: &budget,
            },
        )
        .unwrap();
        assert_eq!(calls(), 0, "no request with work's token: {outcome:?}");
        assert!(
            !matches!(outcome, RecoverOutcome::RolledForward { .. }),
            "no move on local evidence that does not match: {outcome:?}"
        );
    }

    #[test]
    fn a_marked_home_keeps_the_trust_checks() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![home_dir(&e)]);
        fs::set_permissions(e.live(), fs::Permissions::from_mode(0o666)).unwrap();
        assert!(c.identity_reader().oauth(&home_dir(&e)).is_err());
        assert_eq!(calls(), 0);
    }

    #[test]
    fn a_marked_home_keeps_the_login_kind_check() {
        let (e, _) = fx();
        CALLS.with(|c| c.set(0));
        let mut c = ctx(&e);
        c.identity = counting;
        c.set_excluded_dirs(vec![home_dir(&e)]);
        fs::remove_file(e.live()).unwrap();
        assert!(
            c.identity_reader().oauth(&home_dir(&e)).is_err(),
            "no login"
        );
        assert_eq!(calls(), 0);
    }
    // The prelude refresh gate (live_refresh::excluded_live) is covered end
    // to end by an_expired_excluded_live_login_named_only_by_claude_json_is_
    // not_refreshed in tests/issue_2218_excluded.rs.
}

// ---------------------------------------------------------------------------
// D4: a probe never made is not a failed probe
// ---------------------------------------------------------------------------

mod failed_count {
    use crate::commands::profile::rotate::damp::next_failed;
    use crate::commands::profile::rotate::decide::{Probed, Reading};

    #[test]
    fn an_excluded_live_reading_does_not_advance_the_failed_count() {
        let p = Probed::excluded();
        assert!(!matches!(p.reading, Reading::Failed));
        assert_eq!(next_failed(0, &p.reading), 0);
        assert_eq!(next_failed(3, &p.reading), 0, "no failure is counted");
    }
}
