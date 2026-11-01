//! `login_check`: `amplihack profile login <name>` refuses a login that
//! duplicates another profile's or `~/.claude`'s (bug 2, D14-D20). The
//! closure passed to `login_checked` stands in for the `claude` child: it
//! writes the files Claude would write. Login fixtures are mode 0000, so any
//! attempt to read one fails; logins are tracked by inode only.
//!
//! Contract:
//! - `login_checked(&ProfileStore, &ProfileName, home: Option<&Path>,
//!   run: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()>`.
//! - A refusal is an error whose text is
//!   `refusal_message(name, holder, path)` + `"\nhint: "` + `refusal_hint(name)`,
//!   with no `error: ` prefix (the top-level printer adds it).
//! - `<root>/.rejected-logins/` is 0700; `.pending-<name>-<ts>/`,
//!   `<name>-<ts>/` and `<name>-<ts>-replaced/` are created new (0700) with
//!   `<ts>` as `YYYYMMDDTHHMMSSZ`; `oauthAccount.json` is 0600.

use super::login_check::{login_checked, refusal_hint, refusal_message};
use super::name::ProfileName;
use super::store::ProfileStore;
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const CREDS: &str = ".credentials.json";
const REJECTED: &str = ".rejected-logins";

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
    /// Inode of `work`'s login at setup.
    work_ino: u64,
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// An OAuth login (issue #1700: its top-level keys are read for the kind);
/// its value is a sentinel that must never be printed. Returns its inode.
fn login(path: &Path) -> u64 {
    private(
        path,
        r#"{"claudeAiOauth":{"accessToken":"SENTINEL-TOKEN"}}"#,
    );
    ino(path).unwrap()
}

fn ino(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path).ok().map(|m| m.ino())
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn block(uuid: &str, org: &str) -> serde_json::Value {
    serde_json::json!({
        "accountUuid": uuid,
        "organizationUuid": org,
        "emailAddress": format!("mail-{uuid}@x.invalid"),
    })
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Profiles `work` (logged in, user-SENTINEL-work in org-SENTINEL-1) and
/// `other` (logged in, user-SENTINEL-other in the same org); `fresh` has
/// never logged in. `~/.claude` holds user-SENTINEL-home in that org. Every
/// account value contains `SENTINEL`, which no temporary path can.
fn env_in(tmp: tempfile::TempDir, root_name: &str) -> Env {
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = tmp.path().join("home");
    let store = ProfileStore::new(tmp.path().join(root_name));
    for p in ["work", "other", "fresh"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    for (p, u) in [
        ("work", "user-SENTINEL-work"),
        ("other", "user-SENTINEL-other"),
    ] {
        private(
            &store.root().join(p).join(".claude.json"),
            &serde_json::json!({"oauthAccount": block(u, "org-SENTINEL-1"), "numStartups": 1})
                .to_string(),
        );
    }
    let work_ino = login(&store.root().join("work").join(CREDS));
    login(&store.root().join("other").join(CREDS));
    login(&home.join(".claude").join(CREDS));
    private(
        &home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": block("user-SENTINEL-home", "org-SENTINEL-1")})
            .to_string(),
    );
    Env {
        _tmp: tmp,
        home,
        store,
        work_ino,
    }
}

fn env() -> Env {
    env_in(tempfile::tempdir().unwrap(), "root")
}

impl Env {
    fn dir(&self, p: &str) -> PathBuf {
        self.store.root().join(p)
    }

    fn rejected(&self) -> PathBuf {
        self.store.root().join(REJECTED)
    }

    /// Entry names under `.rejected-logins`, sorted.
    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.rejected())
            .map(|r| {
                r.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn entry(&self, prefix: &str, suffix: &str) -> PathBuf {
        let found: Vec<String> = self
            .entries()
            .into_iter()
            .filter(|e| e.starts_with(prefix) && e.ends_with(suffix))
            .collect();
        assert_eq!(found.len(), 1, "entries: {:?}", self.entries());
        self.rejected().join(&found[0])
    }

    fn check(&self, name: &str, run: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
        login_checked(&self.store, &n(name), Some(&self.home), run)
    }

    fn oauth(&self, p: &str) -> Option<serde_json::Value> {
        read_json(&self.dir(p).join(".claude.json"))
            .get("oauthAccount")
            .cloned()
    }

    /// What Claude does on a successful `/login`: a new login file and a
    /// new `oauthAccount`, plus an unrelated key it changes. Returns the new
    /// login's inode.
    fn claude_logs_in(&self, p: &str, uuid: &str, org: &str) -> u64 {
        let dir = self.dir(p);
        let path = dir.join(".claude.json");
        let mut v = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        v["oauthAccount"] = block(uuid, org);
        v["projects"] = serde_json::json!({"/tmp/x": {"allowedTools": []}});
        private(&path, &v.to_string());
        login(&dir.join(CREDS))
    }
}

fn ts_ok(s: &str) -> bool {
    regex::Regex::new(r"^\d{8}T\d{6}Z$").unwrap().is_match(s)
}

fn assert_no_secrets(s: &str) {
    for secret in ["SENTINEL", "@x.invalid"] {
        assert!(!s.contains(secret), "{secret:?} printed: {s}");
    }
}

/// Makes `name` the rotator's live profile through the prototype ledger.
fn make_live(e: &Env, name: &str) {
    let rot = e.store.root().join(".rotator");
    fs::create_dir_all(&rot).unwrap();
    fs::set_permissions(&rot, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &rot.join("ledger.json"),
        &format!(r#"{{"live":"{name}","since":1}}"#),
    );
}

// --- Accepted ---------------------------------------------------------------

#[test]
fn a_first_login_of_a_new_account_is_accepted() {
    let e = env();
    let mut new_ino = 0;
    e.check("fresh", || {
        new_ino = e.claude_logs_in("fresh", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(ino(&e.dir("fresh").join(CREDS)), Some(new_ino));
    assert!(
        !e.entries().iter().any(|x| x.starts_with(".pending-")),
        "{:?}",
        e.entries()
    );
    assert_eq!(
        e.oauth("fresh"),
        Some(block("user-SENTINEL-new", "org-SENTINEL-1"))
    );
}

#[test]
fn an_accepted_login_moves_the_previous_one_to_replaced() {
    let e = env();
    let mut new_ino = 0;
    e.check("work", || {
        new_ino = e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(new_ino));
    let replaced = e.entry("work-", "-replaced");
    let ts = replaced
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_start_matches("work-")
        .trim_end_matches("-replaced")
        .to_string();
    assert!(ts_ok(&ts), "{ts}");
    assert_eq!(
        ino(&replaced.join(CREDS)),
        Some(e.work_ino),
        "moved, not copied"
    );
    assert_eq!(
        fs::symlink_metadata(replaced.join(CREDS)).unwrap().nlink(),
        1
    );
    assert_eq!(mode(&replaced), 0o700);
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
}

#[test]
fn the_same_user_in_another_org_is_accepted() {
    // R2.2: ryan-mistt-org and ryan-mistt-personal.
    let e = env();
    e.check("fresh", || {
        e.claude_logs_in("fresh", "user-SENTINEL-other", "org-SENTINEL-personal");
        Ok(())
    })
    .unwrap();
    assert!(ino(&e.dir("fresh").join(CREDS)).is_some());
}

#[test]
fn a_missing_org_on_one_side_is_accepted() {
    let e = env();
    e.check("fresh", || {
        let dir = e.dir("fresh");
        private(
            &dir.join(".claude.json"),
            &serde_json::json!({"oauthAccount": {"accountUuid": "user-SENTINEL-other"}})
                .to_string(),
        );
        login(&dir.join(CREDS));
        Ok(())
    })
    .unwrap();
}

#[test]
fn logging_in_again_to_the_same_account_is_accepted() {
    // A profile is never compared with itself.
    let e = env();
    e.check("work", || {
        e.claude_logs_in("work", "user-SENTINEL-work", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
}

// --- Refused ----------------------------------------------------------------

#[test]
fn a_duplicate_of_another_profile_is_refused_and_the_profile_restored() {
    let e = env();
    let before = e.oauth("work");
    let cj_mode = mode(&e.dir("work").join(".claude.json"));
    let mut new_ino = 0;
    let err = e
        .check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();

    let x = e.entry("work-", "");
    let x_name = x.file_name().unwrap().to_string_lossy().into_owned();
    assert!(ts_ok(x_name.trim_start_matches("work-")), "{x_name}");
    assert_eq!(
        err,
        format!(
            "{}\nhint: {}",
            refusal_message("work", "profile 'other'", &x.display().to_string()),
            refusal_hint("work")
        )
    );
    assert!(!err.starts_with("error:"), "the top-level printer adds it");
    // The new login is moved aside, never deleted; the old one is back.
    assert_eq!(ino(&x.join(CREDS)), Some(new_ino));
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(
        read_json(&x.join("oauthAccount.json")),
        block("user-SENTINEL-other", "org-SENTINEL-1")
    );
    assert_eq!(mode(&x.join("oauthAccount.json")), 0o600);
    assert_eq!(mode(&x), 0o700);
    assert_eq!(mode(&e.rejected()), 0o700);
    // Only `oauthAccount` is put back; Claude's other changes stay.
    assert_eq!(e.oauth("work"), before);
    let cj = read_json(&e.dir("work").join(".claude.json"));
    assert!(cj.get("projects").is_some(), "{cj}");
    assert_eq!(cj["numStartups"], 1);
    assert_eq!(mode(&e.dir("work").join(".claude.json")), cj_mode);
    // The pending directory is gone.
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
    assert_no_secrets(&err);
    // The other profile is untouched.
    assert_eq!(
        e.oauth("other"),
        Some(block("user-SENTINEL-other", "org-SENTINEL-1"))
    );
}

#[test]
fn a_duplicate_refused_without_a_previous_login_removes_the_key() {
    let e = env();
    private(&e.dir("fresh").join(".claude.json"), r#"{"numStartups":3}"#);
    let mut new_ino = 0;
    let err = e
        .check("fresh", || {
            new_ino = e.claude_logs_in("fresh", "user-SENTINEL-other", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("profile 'other'"), "{err}");
    assert_eq!(
        ino(&e.dir("fresh").join(CREDS)),
        None,
        "no login to restore"
    );
    assert_eq!(ino(&e.entry("fresh-", "").join(CREDS)), Some(new_ino));
    let cj = read_json(&e.dir("fresh").join(".claude.json"));
    assert!(cj.get("oauthAccount").is_none(), "{cj}");
    assert_eq!(cj["numStartups"], 3);
}

#[test]
fn a_login_without_any_claude_json_is_accepted_and_none_is_created() {
    // Claude wrote a login but no `.claude.json`: with no account to
    // compare the check accepts (fail safe), and never creates the file.
    let e = env();
    fs::remove_file(e.dir("fresh").join(".claude.json")).ok();
    e.check("fresh", || {
        login(&e.dir("fresh").join(CREDS));
        Ok(())
    })
    .unwrap();
    assert!(!e.dir("fresh").join(".claude.json").exists());
}

#[test]
fn a_duplicate_of_the_home_login_is_refused() {
    let e = env();
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-home", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    let x = e.entry("work-", "");
    assert!(
        err.starts_with(&refusal_message(
            "work",
            "~/.claude",
            &x.display().to_string()
        )),
        "{err}"
    );
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
}

#[test]
fn a_duplicate_of_the_rotators_live_login_names_the_live_profile() {
    // D19: `team` is live per the prototype ledger; its directory has no
    // login and no account, so only `~/.claude` matches the non-live `work`.
    let e = env();
    e.store.add(&n("team")).unwrap();
    make_live(&e, "team");
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-home", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(" as ~/.claude (rotator live profile 'team'). "),
        "{err}"
    );
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
}

#[test]
fn a_login_to_the_live_profile_is_refused_before_claude_runs() {
    // The live profile's login is `~/.claude`; one of its own would make
    // the rotator refuse to swap it back. Refused up front, so nothing is
    // held or moved and Claude never starts.
    let e = env();
    e.store.add(&n("team")).unwrap();
    private(
        &e.dir("team").join(".claude.json"),
        &serde_json::json!({"oauthAccount": block("user-SENTINEL-home", "org-SENTINEL-1")})
            .to_string(),
    );
    let ran = Cell::new(false);
    let err = e
        .check("team", || {
            ran.set(true);
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(!ran.get(), "claude must not start");
    assert_eq!(
        err,
        format!(
            "{}\nhint: {}",
            super::login_check::live_refusal_message("team"),
            super::login_check::LIVE_REFUSAL_HINT
        )
    );
    assert_no_secrets(&err);
    assert_eq!(ino(&e.dir("team").join(CREDS)), None);
    assert!(e.entries().is_empty(), "{:?}", e.entries());
}

#[test]
fn a_profile_swapped_in_as_live_while_claude_runs_stops_without_moving_anything() {
    // Security M-1: the rotator takes no lease. While Claude runs it moves
    // the new login of `work` into `~/.claude` and records `work` as live.
    let e = env();
    let swapped = e.home.join("swapped-login");
    let mut new_ino = 0;
    let err = e
        .check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
            fs::rename(e.dir("work").join(CREDS), &swapped).unwrap();
            make_live(&e, "work");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("swapped in as the rotator's live profile"),
        "{err}"
    );
    let pending = e.entry(".pending-work-", "");
    assert!(err.contains(&pending.display().to_string()), "{err}");
    assert_eq!(ino(&pending.join(CREDS)), Some(e.work_ino), "still held");
    assert_eq!(ino(&e.dir("work").join(CREDS)), None, "nothing put back");
    assert_eq!(ino(&swapped), Some(new_ino));
    assert!(e.entries().iter().all(|x| !x.starts_with("work-")));
    assert_no_secrets(&err);
}

#[test]
fn the_duplicate_check_fails_closed_when_the_profiles_cannot_be_listed() {
    // Forbidden pattern 1: an unreadable root is not "no other profiles".
    // Mode 0300 stops the listing but not the renames inside the root.
    if super::auto_switch::euid() == 0 {
        return;
    }
    let e = env();
    let before = e.oauth("work");
    let mut new_ino = 0;
    let result = e.check("work", || {
        new_ino = e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
        fs::set_permissions(e.store.root(), fs::Permissions::from_mode(0o300)).unwrap();
        Ok(())
    });
    fs::set_permissions(e.store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("could not check profile 'work' for duplicate logins"),
        "{err}"
    );
    let aside = e.entry("work-", "");
    assert!(err.contains(&aside.display().to_string()), "{err}");
    assert_eq!(ino(&aside.join(CREDS)), Some(new_ino));
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(e.oauth("work"), before);
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
    assert_no_secrets(&err);
}

#[test]
fn without_home_only_profiles_are_compared() {
    let e = env();
    login_checked(&e.store, &n("fresh"), None, || {
        e.claude_logs_in("fresh", "user-SENTINEL-home", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
}

// --- Claude fails or writes no login ----------------------------------------

#[test]
fn a_failed_child_restores_the_previous_login_and_keeps_the_new_one_aside() {
    let e = env();
    let before = e.oauth("work");
    let mut new_ino = 0;
    let err = e
        .check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
            anyhow::bail!("claude exited with exit status: 3")
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("claude exited with exit status: 3"), "{err}");
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(ino(&e.entry("work-", "").join(CREDS)), Some(new_ino));
    assert_eq!(e.oauth("work"), before);
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
}

#[test]
fn a_failed_child_without_a_new_login_restores_and_moves_nothing_else() {
    let e = env();
    let before = e.oauth("work");
    let err = e
        .check("work", || {
            anyhow::bail!("claude exited with exit status: 1")
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("claude exited"), "{err}");
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(e.oauth("work"), before);
    assert!(
        e.entries().iter().all(|x| !x.starts_with("work-")),
        "{:?}",
        e.entries()
    );
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
}

#[test]
fn a_failed_child_does_not_run_the_duplicate_check() {
    let e = env();
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            anyhow::bail!("claude exited with exit status: 2")
        })
        .unwrap_err()
        .to_string();
    assert!(!err.contains("same account"), "{err}");
}

#[test]
fn no_new_login_after_a_previous_one_restores_everything_and_refuses() {
    // D2/C3: with a previous login file, the platform writes login files,
    // so a missing one is a failed login: the login and `oauthAccount`
    // both go back, so they describe the same account.
    let e = env();
    let before = e.oauth("work");
    let err = e
        .check("work", || {
            let path = e.dir("work").join(".claude.json");
            let mut v = read_json(&path);
            v["oauthAccount"] = block("user-SENTINEL-new", "org-SENTINEL-1");
            private(&path, &v.to_string());
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with(
            "Claude exited without writing a login file; the previous login of profile \
             'work' was restored"
        ),
        "{err}"
    );
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(e.oauth("work"), before);
    assert!(e.entries().is_empty(), "{:?}", e.entries());
}

#[test]
fn no_new_login_without_a_previous_one_leaves_oauth_account() {
    // C3 (macOS Keychain): exit 0, no login file before or after.
    let e = env();
    e.check("fresh", || {
        private(
            &e.dir("fresh").join(".claude.json"),
            &serde_json::json!({"oauthAccount": block("user-SENTINEL-new", "org-SENTINEL-1")})
                .to_string(),
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(ino(&e.dir("fresh").join(CREDS)), None);
    assert_eq!(
        e.oauth("fresh"),
        Some(block("user-SENTINEL-new", "org-SENTINEL-1")),
        "left as Claude wrote it"
    );
    assert!(e.entries().is_empty(), "{:?}", e.entries());
}

#[test]
fn no_new_login_with_a_duplicate_account_is_refused_with_nothing_to_move() {
    let e = env();
    let err = e
        .check("fresh", || {
            private(
                &e.dir("fresh").join(".claude.json"),
                &serde_json::json!({"oauthAccount": block("user-SENTINEL-other", "org-SENTINEL-1")})
                    .to_string(),
            );
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("profile 'other'"), "{err}");
    assert!(err.contains("Claude wrote no login file"), "{err}");
    assert_eq!(ino(&e.dir("fresh").join(CREDS)), None);
    assert!(e.entries().iter().all(|x| !x.starts_with("fresh-")));
    assert_no_secrets(&err);
}

#[test]
fn a_block_that_cannot_be_put_back_keeps_the_pending_directory_and_claudes_error() {
    // Forbidden pattern 3 and D18: Claude failed after replacing
    // `.claude.json` with a non-object, so the saved block cannot go back.
    // Its only copy stays, and Claude's own error is still reported.
    let e = env();
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
            private(&e.dir("work").join(".claude.json"), "[]");
            anyhow::bail!("claude exited with exit status: 7")
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("claude exited with an error first: claude exited with exit status: 7\n"),
        "{err}"
    );
    let pending = e.entry(".pending-work-", "");
    assert!(err.contains("could not be updated"), "{err}");
    assert!(
        err.contains(&pending.join("oauthAccount.json").display().to_string()),
        "{err}"
    );
    assert_eq!(
        read_json(&pending.join("oauthAccount.json")),
        block("user-SENTINEL-work", "org-SENTINEL-1")
    );
    // The login itself is back, and the hint names where the new one went.
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    let aside = e.entry("work-", "");
    assert!(
        err.contains(&aside.join(CREDS).display().to_string()),
        "{err}"
    );
    assert_no_secrets(&err);
}

// --- Unsafe files -----------------------------------------------------------

#[test]
fn a_new_login_that_is_not_private_is_not_moved_or_chmodded() {
    // S-6.
    if super::auto_switch::euid() == 0 {
        return;
    }
    let e = env();
    let mut new_ino = 0;
    let err = e
        .check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            fs::set_permissions(e.dir("work").join(CREDS), fs::Permissions::from_mode(0o644))
                .unwrap();
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("0644"), "reports the mode: {err}");
    assert!(err.contains("exposed"), "{err}");
    let pending = e.entry(".pending-work-", "");
    assert!(err.contains(&pending.display().to_string()), "{err}");
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(new_ino));
    assert_eq!(mode(&e.dir("work").join(CREDS)), 0o644, "mode unchanged");
    assert_eq!(
        ino(&pending.join(CREDS)),
        Some(e.work_ino),
        "held, not lost"
    );
}

#[test]
fn a_symlinked_previous_login_stops_before_claude_runs() {
    let e = env();
    let target = e.dir("work").join("real-login");
    fs::rename(e.dir("work").join(CREDS), &target).unwrap();
    std::os::unix::fs::symlink(&target, e.dir("work").join(CREDS)).unwrap();
    let ran = Cell::new(false);
    assert!(
        e.check("work", || {
            ran.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!ran.get(), "claude must not start");
    assert!(
        fs::symlink_metadata(e.dir("work").join(CREDS))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!e.entries().iter().any(|x| x.starts_with(".pending-")));
}

#[test]
fn a_hard_linked_previous_login_stops_before_claude_runs() {
    let e = env();
    fs::hard_link(e.dir("work").join(CREDS), e.dir("work").join("second-link")).unwrap();
    let ran = Cell::new(false);
    assert!(
        e.check("work", || {
            ran.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!ran.get());
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
}

#[test]
fn a_symlinked_rejected_logins_directory_is_refused() {
    let e = env();
    let elsewhere = e.home.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, e.rejected()).unwrap();
    let ran = Cell::new(false);
    assert!(
        e.check("work", || {
            ran.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!ran.get());
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn an_existing_entry_directory_is_an_error_and_nothing_moves_into_it() {
    // S-2: entry directories are created new. Pre-create every name the
    // check could pick in the next few seconds.
    let e = env();
    fs::create_dir_all(e.rejected()).unwrap();
    fs::set_permissions(e.rejected(), fs::Permissions::from_mode(0o700)).unwrap();
    let now = chrono::Utc::now();
    let mut planted = Vec::new();
    for s in -2..30 {
        let ts = (now + chrono::Duration::seconds(s)).format("%Y%m%dT%H%M%SZ");
        let d = e.rejected().join(format!("work-{ts}"));
        fs::create_dir(&d).unwrap();
        planted.push(d);
    }
    let mut new_ino = 0;
    assert!(
        e.check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            Ok(())
        })
        .is_err()
    );
    for d in &planted {
        assert_eq!(fs::read_dir(d).unwrap().count(), 0, "{}", d.display());
    }
    // The new login is not lost: it is still in the profile directory.
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(new_ino));
}

// --- Locks, leases and signals ----------------------------------------------

#[test]
fn a_lease_taken_while_claude_runs_stops_the_check_without_moving_anything() {
    // S-1: the lease check runs again after the child exits.
    use super::auto_switch::kit::{Ledger, State};
    let e = env();
    let state = State::new(e.store.root()).unwrap();
    state.ensure().unwrap();
    let mut new_ino = 0;
    let err = e
        .check("work", || {
            new_ino = e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            let ledger: Ledger = serde_json::from_value(serde_json::json!({
                "version": 1,
                "hosts": {"other": {"live": "work", "swapped_at": 1, "pre_swap_fingerprint": null}}
            }))
            .unwrap();
            state.save_ledger(&ledger).unwrap();
            Ok(())
        })
        .unwrap_err()
        .to_string();
    let pending = e.entry(".pending-work-", "");
    assert!(err.contains(&pending.display().to_string()), "{err}");
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(new_ino));
    assert_eq!(ino(&pending.join(CREDS)), Some(e.work_ino));
    assert!(e.entries().iter().all(|x| !x.starts_with("work-")));
}

/// Whether SIGINT would end this process right now.
fn sigint_is_default() -> bool {
    // SAFETY: a null `act` only queries the current disposition.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old);
        old.sa_sigaction == libc::SIG_DFL
    }
}

#[test]
fn ctrl_c_during_the_login_still_restores() {
    // S-3: amplihack does not die on SIGINT while the child runs, so the
    // restore after Ctrl-C still happens. The signal is raised only once it
    // is known to be caught or ignored, so a failure does not kill the
    // test binary.
    let e = env();
    let before = e.oauth("work");
    let default_while_running = Cell::new(true);
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
            default_while_running.set(sigint_is_default());
            if !default_while_running.get() {
                // SAFETY: raise has no preconditions.
                unsafe { libc::raise(libc::SIGINT) };
            }
            anyhow::bail!("claude exited with signal: 2 (SIGINT)")
        })
        .unwrap_err()
        .to_string();
    assert!(
        !default_while_running.get(),
        "SIGINT must be ignored or caught while claude runs"
    );
    assert!(err.contains("claude exited"), "{err}");
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
    assert_eq!(e.oauth("work"), before);
}

#[test]
fn the_pending_directory_is_private_while_claude_runs() {
    let e = env();
    e.check("work", || {
        let pending = e.entry(".pending-work-", "");
        let ts = pending
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(".pending-work-")
            .to_string();
        assert!(ts_ok(&ts), "{ts}");
        assert_eq!(mode(&e.rejected()), 0o700);
        assert_eq!(mode(&pending), 0o700);
        assert_eq!(ino(&pending.join(CREDS)), Some(e.work_ino));
        assert_eq!(ino(&e.dir("work").join(CREDS)), None, "moved, not copied");
        assert_eq!(
            read_json(&pending.join("oauthAccount.json")),
            block("user-SENTINEL-work", "org-SENTINEL-1")
        );
        assert_eq!(mode(&pending.join("oauthAccount.json")), 0o600);
        e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
}

// --- Output -----------------------------------------------------------------

#[test]
fn messages_clean_control_characters_from_paths() {
    // S-5: a root path with an escape sequence and a bidi override.
    let tmp = tempfile::tempdir().unwrap();
    let e = env_in(tmp, "root\u{1b}[31m\u{202e}x");
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert!(
        !err.contains('\u{1b}') && !err.contains('\u{202e}'),
        "{err:?}"
    );
    assert_eq!(ino(&e.dir("work").join(CREDS)), Some(e.work_ino));
}

#[test]
fn nothing_under_rejected_logins_or_in_messages_names_an_account() {
    let e = env();
    let err = e
        .check("work", || {
            e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
            Ok(())
        })
        .unwrap_err()
        .to_string();
    assert_no_secrets(&err);
    for entry in e.entries() {
        assert_no_secrets(&entry);
    }
}

#[test]
fn the_refusal_text_is_the_documented_one() {
    assert_eq!(
        refusal_message("<name>", "<holder>", "<path>"),
        "profile '<name>' was signed in to the same account and organization as <holder>. \
         The new login was moved to <path>; '<name>' is back to its previous login."
    );
    assert_eq!(
        refusal_hint("<name>"),
        "sign out of claude.ai in your browser (or use a private/incognito window), \
         then run 'amplihack profile login <name>' again."
    );
}

/// `login_check` only checks metadata of and renames login files: no line
/// that names one may read, open, copy, chmod or delete, and nothing but
/// amplihack's own saved block is ever deleted or chmod-ed.
#[test]
fn login_check_never_opens_a_login_file() {
    let src = include_str!("login_check.rs");
    for line in src.lines() {
        let l = line.to_ascii_lowercase();
        if l.trim_start().starts_with("//") {
            continue;
        }
        if l.contains("creds") || l.contains("credentials") {
            for bad in [
                "read",
                "open",
                "copy",
                "remove_file",
                "set_permissions",
                "hash",
            ] {
                assert!(!l.contains(bad), "login file handled: {line}");
            }
        }
        if l.contains("remove_file") || l.contains("set_permissions") {
            assert!(
                l.contains("saved") || l.contains("saved_block"),
                "only the saved block may be deleted or chmod-ed: {line}"
            );
        }
    }
}

// --- Issue #1845: the recorded account (D2, AC7) ----------------------------
//
// Contract: an accepted login records the account in the profile's new
// `.claude.json` with `expected::record`; a later accepted login overwrites
// it. The Keychain case (no login file before or after) records the same way
// when `.claude.json` names an account and is not a duplicate. Nothing is
// recorded for a refused, failed or restored login, nor when `.claude.json`
// names no account. A record that cannot be saved prints a warning and does
// not fail the login.

fn state(e: &Env) -> super::auto_switch::kit::State {
    super::auto_switch::kit::State::new(e.store.root()).unwrap()
}

fn recorded(e: &Env, p: &str) -> Option<String> {
    super::expected::recorded(&state(e), &n(p))
}

fn record_file(e: &Env, p: &str) -> PathBuf {
    e.store
        .root()
        .join(".auto-switch.d/expected")
        .join(format!("{p}.json"))
}

fn pre_record(e: &Env, p: &str, uuid: &str) {
    let a = super::rotate::identify::Account::from_block(&block(uuid, "org-SENTINEL-1")).unwrap();
    super::expected::record(&state(e), &n(p), &a).unwrap();
}

#[test]
fn an_accepted_first_login_records_its_account() {
    let e = env();
    e.check("fresh", || {
        e.claude_logs_in("fresh", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "fresh").as_deref(), Some("user-SENTINEL-new"));
    let path = record_file(&e, "fresh");
    assert_eq!(
        read_json(&path),
        serde_json::json!({"account": "user-SENTINEL-new"}),
        "the key only: no organization, e-mail or token"
    );
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn an_accepted_login_to_another_account_overwrites_the_record() {
    let e = env();
    pre_record(&e, "work", "user-SENTINEL-work");
    e.check("work", || {
        e.claude_logs_in("work", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "work").as_deref(), Some("user-SENTINEL-new"));
}

#[test]
fn logging_in_again_to_the_same_account_records_it() {
    let e = env();
    e.check("work", || {
        e.claude_logs_in("work", "user-SENTINEL-work", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "work").as_deref(), Some("user-SENTINEL-work"));
}

#[test]
fn a_refused_duplicate_records_nothing_and_keeps_the_old_record() {
    let e = env();
    pre_record(&e, "work", "user-SENTINEL-work");
    e.check("work", || {
        e.claude_logs_in("work", "user-SENTINEL-other", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap_err();
    assert_eq!(recorded(&e, "work").as_deref(), Some("user-SENTINEL-work"));
    e.check("fresh", || {
        e.claude_logs_in("fresh", "user-SENTINEL-other", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap_err();
    assert_eq!(recorded(&e, "fresh"), None);
}

#[test]
fn a_failed_child_records_nothing() {
    let e = env();
    e.check("fresh", || {
        e.claude_logs_in("fresh", "user-SENTINEL-new", "org-SENTINEL-1");
        anyhow::bail!("claude exited with exit status: 3")
    })
    .unwrap_err();
    assert_eq!(recorded(&e, "fresh"), None);
}

#[test]
fn a_restored_login_records_nothing() {
    // Exit 0 with no new login file after a previous one: restored.
    let e = env();
    e.check("work", || {
        let path = e.dir("work").join(".claude.json");
        let mut v = read_json(&path);
        v["oauthAccount"] = block("user-SENTINEL-new", "org-SENTINEL-1");
        private(&path, &v.to_string());
        Ok(())
    })
    .unwrap_err();
    assert_eq!(recorded(&e, "work"), None);
}

#[test]
fn a_login_to_the_live_profile_records_nothing() {
    // `team` holds the account in `~/.claude`, so it is the live profile and
    // its login is refused before Claude runs.
    let e = env();
    e.store.add(&n("team")).unwrap();
    private(
        &e.dir("team").join(".claude.json"),
        &serde_json::json!({"oauthAccount": block("user-SENTINEL-home", "org-SENTINEL-1")})
            .to_string(),
    );
    e.check("team", || Ok(())).unwrap_err();
    assert_eq!(recorded(&e, "team"), None);
}

#[test]
fn a_login_without_an_account_records_nothing() {
    let e = env();
    fs::remove_file(e.dir("fresh").join(".claude.json")).ok();
    e.check("fresh", || {
        login(&e.dir("fresh").join(CREDS));
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "fresh"), None);
    assert!(!record_file(&e, "fresh").exists());
}

#[test]
fn a_keychain_login_records_its_account() {
    // C3 (macOS Keychain): exit 0, no login file before or after.
    let e = env();
    e.check("fresh", || {
        private(
            &e.dir("fresh").join(".claude.json"),
            &serde_json::json!({"oauthAccount": block("user-SENTINEL-new", "org-SENTINEL-1")})
                .to_string(),
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "fresh").as_deref(), Some("user-SENTINEL-new"));
}

#[test]
fn a_keychain_login_without_an_account_records_nothing() {
    let e = env();
    e.check("fresh", || {
        private(&e.dir("fresh").join(".claude.json"), r#"{"numStartups":2}"#);
        Ok(())
    })
    .unwrap();
    assert_eq!(recorded(&e, "fresh"), None);
}

#[test]
fn a_refused_keychain_duplicate_records_nothing() {
    let e = env();
    e.check("fresh", || {
        private(
            &e.dir("fresh").join(".claude.json"),
            &serde_json::json!({"oauthAccount": block("user-SENTINEL-other", "org-SENTINEL-1")})
                .to_string(),
        );
        Ok(())
    })
    .unwrap_err();
    assert_eq!(recorded(&e, "fresh"), None);
}

#[test]
fn a_record_that_cannot_be_saved_does_not_fail_the_login() {
    let e = env();
    let blocker = e.store.root().join(".auto-switch.d");
    fs::create_dir_all(&blocker).unwrap();
    fs::set_permissions(&blocker, fs::Permissions::from_mode(0o700)).unwrap();
    // `expected` is a file, so no record can be written under it.
    private(&blocker.join("expected"), "in the way");
    let mut new_ino = 0;
    e.check("fresh", || {
        new_ino = e.claude_logs_in("fresh", "user-SENTINEL-new", "org-SENTINEL-1");
        Ok(())
    })
    .unwrap();
    assert_eq!(
        ino(&e.dir("fresh").join(CREDS)),
        Some(new_ino),
        "login kept"
    );
    assert_eq!(recorded(&e, "fresh"), None);
}

/// D2 and the security notes: only an accepted login, an adopting swap and
/// a swap backfill write a record.
#[test]
fn only_login_check_and_swap_expect_write_records() {
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
    let mut writers = Vec::new();
    for f in &files {
        let src = fs::read_to_string(f).unwrap();
        let code: String = src
            .lines()
            .map(|l| l.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        if code.contains("expected::record(") || code.contains("\"expected\"") {
            let rel = f.strip_prefix(&base).unwrap().display().to_string();
            writers.push(rel);
        }
    }
    writers.sort();
    assert_eq!(
        writers,
        vec![
            "expected.rs".to_string(),
            "login_check.rs".to_string(),
            "rotate/swap_expect.rs".to_string(),
        ],
        "files that write or name the record store"
    );
}
