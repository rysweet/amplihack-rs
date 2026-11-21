//! Issue #2885 (crusty round 1 of PR #2891): the rotator's capacity write
//! leaves an overflow profile out when its macOS Keychain could not be read.
//! Before, it did so without a word, so the log could not tell that profile
//! from one with no login, while the launch guard named the Keychain.
//!
//! Contract:
//! - `overflow_record::refresh_capacity(&Ctx, ..) -> Vec<String>` returns
//!   one line for each such profile: `auto-switch: profile <name> was passed
//!   over: it has no login file and the macOS Keychain could not be read
//!   (<cause>)`, the line selection prints.
//! - `overflow_record::note_keychain(&mut Ctx, &mut Sink, t, notes)` logs
//!   only the lines the previous cycle did not give, and keeps this cycle's
//!   in `Ctx.keychain_notes`. A Keychain that stays locked is logged once,
//!   not once a cycle; one that was readable for a cycle and is locked again
//!   is logged again. The cycle calls the two one after the other.
//! - A Keychain that answers, either way, logs nothing.
//!
//! Every test names the platform through `keychain::test_seam`, so none
//! starts the system `security`. The fake reads its exit status from a file,
//! so a test changes the answer without writing a new script.

use super::super::cycle::Ctx;
use super::super::decide::Reading;
use super::super::ledger::RotateState;
use super::super::log::{Sink, open_or_warn};
use super::super::overflow_record::{note_keychain, refresh_capacity};
use super::super::settings;
use super::super::tests_swap::{login, private};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::keychain::test_seam;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::test_support::{EnvGuard, env_lock, write_executable};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const ACCOUNT: &str = r#"{"oauthAccount":{"accountUuid":"uuid-SENTINEL-KEYCHAIN","emailAddress":"user-SENTINEL-KEYCHAIN@example.com"}}"#;
const LOCKED: &str = "auto-switch: profile spare was passed over: it has no login file and the macOS Keychain could not be read (`security` exit status 51)";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// `work` holds a login file. `spare` has an account and no login file, and
/// is marked as overflow, so only the Keychain can say whether it has a
/// login.
struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
    rs: RotateState,
    bin: PathBuf,
    code: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (home, root, bin) = (base.join("home"), base.join("root"), base.join("bin"));
    for d in [&home, &bin] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = ProfileStore::new(root.clone());
    for name in ["work", "spare"] {
        store.add(&n(name)).unwrap();
        private(&root.join(name).join(".claude.json"), ACCOUNT);
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    login(&root.join("work").join(".credentials.json"));
    private(&root.join(".auto-switch"), "enabled=on\noverflow=spare\n");
    let code = base.join("security.code");
    write_executable(
        &bin.join("security"),
        // `read`, a builtin: the lookup's `PATH` holds this directory alone.
        &format!(
            "#!/bin/sh\nread -r code < '{}'\nexit \"$code\"\n",
            code.display()
        ),
    );
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    Fx {
        _tmp: tmp,
        home,
        store,
        rs,
        bin,
        code,
    }
}

impl Fx {
    fn ctx(&self) -> Ctx {
        let s = settings::resolve(&super::super::TuneFlags::default(), None, &|_| None).unwrap();
        Ctx::new(self.store.clone(), self.home.clone(), s)
    }

    /// One capacity write with the Keychain answering `code`, as a cycle
    /// makes it: every plan login is used up, and the log is the real one.
    fn cycle(&self, ctx: &mut Ctx, code: i32) {
        fs::write(&self.code, format!("{code}\n")).unwrap();
        let mut out = self.sink();
        let work = n("work");
        let live = Reading::EXHAUSTED;
        let t = now();
        let notes = refresh_capacity(ctx, Some(&work), &live, &[], t, None, None);
        note_keychain(ctx, &mut out, t, notes);
    }

    fn sink(&self) -> Sink {
        Sink::new(open_or_warn(&self.rs.log, &self.rs.log1), false, true)
    }

    fn logged(&self) -> Vec<String> {
        fs::read_to_string(&self.rs.log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once(' ').unwrap().1.to_string())
            .collect()
    }
}

#[test]
fn a_locked_keychain_is_logged_once_and_again_when_it_is_locked_again() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fx();
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _seam = test_seam(&f.bin);
    let mut ctx = f.ctx();

    f.cycle(&mut ctx, 51);
    assert_eq!(f.logged(), [LOCKED], "the cause is in the log");
    f.cycle(&mut ctx, 51);
    assert_eq!(
        f.logged(),
        [LOCKED],
        "a second locked cycle repeats nothing"
    );

    // The Keychain answers, with an item and then with none: nothing to say.
    for code in [0, 44] {
        f.cycle(&mut ctx, code);
        assert_eq!(f.logged(), [LOCKED], "exit status {code}");
    }

    f.cycle(&mut ctx, 51);
    assert_eq!(f.logged(), [LOCKED, LOCKED], "locked again, said again");
    for line in f.logged() {
        assert!(!line.contains("SENTINEL"), "{line}");
    }
}

#[test]
fn note_keychain_logs_only_the_lines_the_last_cycle_did_not_give() {
    let f = fx();
    let mut ctx = f.ctx();
    let mut out = f.sink();
    for notes in [vec!["a"], vec!["a", "b"], vec!["b"], vec![], vec!["a", "b"]] {
        let notes = notes.into_iter().map(str::to_string).collect();
        note_keychain(&mut ctx, &mut out, now(), notes);
    }
    assert_eq!(f.logged(), ["a", "b", "a", "b"]);
    assert_eq!(ctx.keychain_notes, ["a", "b"]);
}
