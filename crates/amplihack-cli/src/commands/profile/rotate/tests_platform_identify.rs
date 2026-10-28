//! Issue #1700: the structural reads for API-key (Claude Platform) profiles,
//! all in `rotate/identify.rs`, and how `live` and `candidates` treat an
//! API-key profile. Only key *names* are checked; every value in these
//! fixtures is a canary that must never come back out.
//!
//! Contract:
//! - `identify::LoginKind::{OAuth, NoOAuth, Unreadable, Missing}`.
//! - `identify::login_kind(path) -> LoginKind`:
//!   - `Missing`: nothing at `path`;
//!   - `OAuth`: a readable JSON object with a top-level `claudeAiOauth` key;
//!   - `NoOAuth`: a readable JSON object that is `{}` or lacks that key;
//!   - `Unreadable`: anything else at `path`: a symlink (even to a valid
//!     file), a FIFO or directory, a file not owned by the euid, one that
//!     cannot be opened, one over 64 KiB, invalid JSON, or JSON that is not
//!     an object. Never blocks on a FIFO.
//! - `identify::api_key_setting_names(dir) -> bool`: `settings.json` has
//!   `env.ANTHROPIC_API_KEY` or `apiKeyHelper`, or `.claude.json` has
//!   `primaryApiKey`. A file that is a symlink, writable by group or others,
//!   over 64 KiB or not a JSON object counts as no names. Values are never
//!   looked at (an empty string still counts as present).
//! - `Skip::NotOAuth` (`as_str` `"not oauth"`): a subscription profile whose
//!   login file is not an OAuth login (`NoOAuth` under a `subscription`
//!   override, or `Unreadable`) is never a candidate.
//! - An API-key profile is a candidate without an `oauthAccount`
//!   (`Skip::NoAccount` does not apply), but still needs a movable login
//!   file (`Skip::NoLogin` when it has none; nothing is created for it).
//! - `live`: when `~/.claude/.credentials.json` is a readable object with no
//!   `claudeAiOauth`, the ledger's `live` API-key profile is `Known` without
//!   any account check; with no ledger, the single API-key profile is
//!   `Known`; several are `Ambiguous`; none is `NoLogin`. An unreadable home
//!   login is `NoLogin`.

use super::identify::{
    CandidateInput, LiveId, LoginKind, Skip, api_key_setting_names, candidates, live, login_kind,
};
use super::ledger::{RotateLedger, RotateState};
use crate::commands::profile::auto_switch::kit::{Ledger, Liveness};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const CANARY: &str = "CANARY-sk-ant-0000";
const OAUTH: &str =
    r#"{"claudeAiOauth":{"accessToken":"CANARY-sk-ant-0000","refreshToken":"CANARY-sk-ant-0000"}}"#;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn tmp() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    t
}

fn not_root() -> bool {
    crate::commands::profile::auto_switch::euid() != 0
}

// --- login_kind ----------------------------------------------------------------

#[test]
fn an_oauth_login_is_oauth() {
    let t = tmp();
    let p = t.path().join(".credentials.json");
    private(&p, OAUTH);
    assert_eq!(login_kind(&p), LoginKind::OAuth);
}

#[test]
fn an_empty_object_or_one_without_claude_ai_oauth_is_no_oauth() {
    let t = tmp();
    let p = t.path().join(".credentials.json");
    for body in [
        "{}",
        "{}\n",
        r#"{"somethingElse":{"x":"CANARY-sk-ant-0000"}}"#,
        // Nested, not top-level: still no OAuth login.
        r#"{"wrap":{"claudeAiOauth":{}}}"#,
    ] {
        private(&p, body);
        assert_eq!(login_kind(&p), LoginKind::NoOAuth, "{body}");
    }
}

#[test]
fn a_missing_file_is_missing() {
    let t = tmp();
    assert_eq!(
        login_kind(&t.path().join(".credentials.json")),
        LoginKind::Missing
    );
}

#[test]
fn a_damaged_login_file_is_unreadable_never_no_oauth() {
    let t = tmp();
    let p = t.path().join(".credentials.json");
    for body in [
        "",
        "not json CANARY-sk-ant-0000",
        "[]",
        "\"{}\"",
        "null",
        "42",
        "{\"claudeAiOauth\":",
    ] {
        private(&p, body);
        assert_eq!(login_kind(&p), LoginKind::Unreadable, "{body:?}");
    }
}

#[test]
fn an_oversized_login_file_is_unreadable() {
    let t = tmp();
    let p = t.path().join(".credentials.json");
    let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(65 * 1024));
    private(&p, &big);
    assert_eq!(login_kind(&p), LoginKind::Unreadable);
}

#[test]
fn a_symlink_is_unreadable_even_to_a_valid_empty_object() {
    let t = tmp();
    let target = t.path().join("elsewhere.json");
    private(&target, "{}");
    let p = t.path().join(".credentials.json");
    symlink(&target, &p).unwrap();
    assert_eq!(login_kind(&p), LoginKind::Unreadable);
    let dangling = t.path().join("dangling.json");
    symlink(t.path().join("nothing"), &dangling).unwrap();
    assert_eq!(login_kind(&dangling), LoginKind::Unreadable);
}

#[test]
fn a_fifo_or_directory_is_unreadable_without_blocking() {
    let t = tmp();
    let fifo = t.path().join("fifo.json");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert_eq!(login_kind(&fifo), LoginKind::Unreadable);
    assert!(start.elapsed().as_secs() < 2, "a FIFO must not block");
    let dir = t.path().join("dir.json");
    fs::create_dir(&dir).unwrap();
    assert_eq!(login_kind(&dir), LoginKind::Unreadable);
}

#[test]
fn a_file_that_cannot_be_opened_is_unreadable() {
    if !not_root() {
        return;
    }
    let t = tmp();
    let p = t.path().join(".credentials.json");
    private(&p, "{}");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(login_kind(&p), LoginKind::Unreadable);
}

// --- api_key_setting_names ---------------------------------------------------------

#[test]
fn each_api_key_setting_name_counts() {
    for (file, body) in [
        (
            "settings.json",
            r#"{"env":{"ANTHROPIC_API_KEY":"CANARY-sk-ant-0000"}}"#,
        ),
        (
            "settings.json",
            r#"{"apiKeyHelper":"echo CANARY-sk-ant-0000"}"#,
        ),
        (".claude.json", r#"{"primaryApiKey":"CANARY-sk-ant-0000"}"#),
        // Presence only: an empty value still counts.
        ("settings.json", r#"{"env":{"ANTHROPIC_API_KEY":""}}"#),
    ] {
        let t = tmp();
        private(&t.path().join(file), body);
        assert!(api_key_setting_names(t.path()), "{file}: {body}");
    }
}

#[test]
fn other_settings_are_not_api_key_settings() {
    let t = tmp();
    assert!(!api_key_setting_names(t.path()), "no files");
    private(
        &t.path().join("settings.json"),
        r#"{"env":{"ANTHROPIC_BASE_URL":"x","OTHER":"CANARY-sk-ant-0000"},"model":"haiku"}"#,
    );
    private(
        &t.path().join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"a@example.com"},"customApiKeyResponses":{"approved":["x"]}}"#,
    );
    assert!(!api_key_setting_names(t.path()));
    // A top-level ANTHROPIC_API_KEY is not under `env`.
    private(
        &t.path().join("settings.json"),
        r#"{"ANTHROPIC_API_KEY":"CANARY-sk-ant-0000"}"#,
    );
    assert!(!api_key_setting_names(t.path()));
}

#[test]
fn settings_writable_by_others_or_symlinked_are_no_evidence() {
    let t = tmp();
    let s = t.path().join("settings.json");
    private(&s, r#"{"apiKeyHelper":"x"}"#);
    for mode in [0o620, 0o602, 0o666] {
        fs::set_permissions(&s, fs::Permissions::from_mode(mode)).unwrap();
        assert!(!api_key_setting_names(t.path()), "mode {mode:o}");
    }
    fs::remove_file(&s).unwrap();
    let elsewhere = t.path().join("planted.json");
    private(&elsewhere, r#"{"apiKeyHelper":"x"}"#);
    symlink(&elsewhere, &s).unwrap();
    assert!(!api_key_setting_names(t.path()), "symlinked settings.json");

    let t2 = tmp();
    let cj = t2.path().join(".claude.json");
    private(&cj, r#"{"primaryApiKey":"x"}"#);
    fs::set_permissions(&cj, fs::Permissions::from_mode(0o646)).unwrap();
    assert!(!api_key_setting_names(t2.path()), ".claude.json 0646");
}

#[test]
fn damaged_settings_are_no_evidence() {
    for body in ["not json", "[]", "\"apiKeyHelper\""] {
        let t = tmp();
        private(&t.path().join("settings.json"), body);
        assert!(!api_key_setting_names(t.path()), "{body}");
    }
    let t = tmp();
    let big = format!(
        r#"{{"apiKeyHelper":"x","pad":"{}"}}"#,
        "x".repeat(65 * 1024)
    );
    private(&t.path().join("settings.json"), &big);
    assert!(!api_key_setting_names(t.path()), "over 64 KiB");
}

// --- live() and candidates() ------------------------------------------------------

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
    rs: RotateState,
}

/// `work`: subscription, live in `~/.claude` (no login file of its own),
/// account u-work. `spare`: subscription with an OAuth login. `console`:
/// API key (`{}` login, no `oauthAccount`). `~/.claude` holds work's
/// OAuth login.
fn env() -> Env {
    let t = tmp();
    let home = t.path().join("home");
    let root = t.path().join("root");
    let store = ProfileStore::new(root.clone());
    for p in ["work", "spare", "console"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let acct = |u: &str| {
        serde_json::json!({"oauthAccount": {"accountUuid": u, "emailAddress": format!("{u}@example.com")}})
            .to_string()
    };
    private(&root.join("work/.claude.json"), &acct("u-work"));
    private(&root.join("spare/.claude.json"), &acct("u-spare"));
    private(&root.join("spare/.credentials.json"), OAUTH);
    private(&root.join("console/.credentials.json"), "{}");
    private(
        &root.join("console/settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"CANARY-sk-ant-0000"}}"#,
    );
    private(&home.join(".claude/.credentials.json"), OAUTH);
    private(&home.join(".claude.json"), &acct("u-work"));
    let rs = RotateState::new(&root);
    rs.ensure().unwrap();
    Env {
        _tmp: t,
        home,
        store,
        rs,
    }
}

impl Env {
    fn root(&self) -> &Path {
        self.store.root()
    }

    fn live(&self) -> LiveId {
        live(&self.store, &self.rs, &self.home).unwrap().0
    }

    /// `console`'s `{}` is now in `~/.claude`; work's OAuth login went back
    /// to work.
    fn console_live(&self) {
        fs::rename(
            self.home.join(".claude/.credentials.json"),
            self.root().join("work/.credentials.json"),
        )
        .unwrap();
        fs::rename(
            self.root().join("console/.credentials.json"),
            self.home.join(".claude/.credentials.json"),
        )
        .unwrap();
    }

    fn ledger_live(&self, name: &str, platform: bool) {
        let l = RotateLedger {
            live: Some(name.to_string()),
            since: Some(1_760_000_000),
            live_kind: platform.then(|| "platform".to_string()),
            ..RotateLedger::default()
        };
        self.rs.save_ledger(&l).unwrap();
    }

    fn input<'a>(&self, live: Option<&'a ProfileName>) -> CandidateInput<'a> {
        CandidateInput {
            live,
            exclude: Vec::new(),
            liveness: Liveness::Known(BTreeSet::new()),
            auto_switch: Ledger::default(),
        }
    }

    fn skips(&self, live: Option<&ProfileName>) -> (Vec<String>, Vec<(String, Skip)>) {
        let (ok, skips) = candidates(&self.store, &self.input(live)).unwrap();
        (
            ok.into_iter().map(|p| p.to_string()).collect(),
            skips.into_iter().map(|(p, s)| (p.to_string(), s)).collect(),
        )
    }

    fn set_kind_line(&self, line: &str) {
        private(
            &self.root().join(".auto-switch"),
            &format!("enabled=off\nthreshold=90\nexclude=\n{line}\n"),
        );
    }
}

#[test]
fn an_api_key_profile_without_an_oauth_account_is_a_candidate() {
    let e = env();
    let work = n("work");
    let (ok, skips) = e.skips(Some(&work));
    assert!(ok.contains(&"console".to_string()), "{ok:?} {skips:?}");
    assert!(ok.contains(&"spare".to_string()));
    assert!(!skips.iter().any(|(p, _)| p == "console"), "{skips:?}");
}

#[test]
fn an_api_key_profile_without_a_login_file_is_skipped_and_nothing_is_created() {
    let e = env();
    fs::remove_file(e.root().join("console/.credentials.json")).unwrap();
    let work = n("work");
    let (_, skips) = e.skips(Some(&work));
    assert!(
        skips.contains(&("console".to_string(), Skip::NoLogin)),
        "{skips:?}"
    );
    assert!(
        !e.root().join("console/.credentials.json").exists(),
        "an empty login file is never created"
    );
}

#[test]
fn a_subscription_profile_without_an_oauth_login_is_skipped_as_not_oauth() {
    assert_eq!(Skip::NotOAuth.as_str(), "not oauth");
    let e = env();
    // An override says console is a subscription profile: its `{}` is not
    // a login and must never be moved into ~/.claude.
    e.set_kind_line("kind=console:subscription");
    let work = n("work");
    let (ok, skips) = e.skips(Some(&work));
    assert!(!ok.contains(&"console".to_string()), "{ok:?}");
    assert!(
        skips.contains(&("console".to_string(), Skip::NotOAuth)),
        "{skips:?}"
    );
}

#[test]
fn an_unreadable_login_is_skipped_as_not_oauth() {
    let e = env();
    private(&e.root().join("spare/.credentials.json"), "garbage");
    let work = n("work");
    let (ok, skips) = e.skips(Some(&work));
    assert!(!ok.contains(&"spare".to_string()), "{ok:?}");
    assert!(
        skips.contains(&("spare".to_string(), Skip::NotOAuth)),
        "{skips:?}"
    );
}

#[test]
fn a_ledger_naming_the_api_key_profile_is_known_without_an_account_check() {
    let e = env();
    e.console_live();
    e.ledger_live("console", true);
    // $HOME/.claude.json still holds work's account; console has none.
    assert_eq!(e.live(), LiveId::Known(n("console")));
}

#[test]
fn with_no_ledger_the_single_api_key_profile_is_adopted_as_live() {
    let e = env();
    e.console_live();
    assert_eq!(e.live(), LiveId::Known(n("console")));
}

#[test]
fn with_no_ledger_two_api_key_profiles_are_ambiguous() {
    let e = env();
    e.console_live();
    e.store.add(&n("console2")).unwrap();
    private(&e.root().join("console2/.credentials.json"), "{}");
    assert_eq!(e.live(), LiveId::Ambiguous);
}

#[test]
fn an_empty_home_login_with_no_api_key_profile_is_no_login() {
    let e = env();
    e.console_live();
    // console is overridden to subscription: no API-key profile remains.
    e.set_kind_line("kind=console:subscription");
    assert_eq!(e.live(), LiveId::NoLogin);
}

#[test]
fn an_unreadable_home_login_is_no_login() {
    let e = env();
    private(&e.home.join(".claude/.credentials.json"), "[1,2,3]");
    assert_eq!(e.live(), LiveId::NoLogin);
    e.ledger_live("console", true);
    assert_eq!(
        e.live(),
        LiveId::NoLogin,
        "a damaged file is never an API-key login, whatever the ledger says"
    );
}

#[test]
fn no_identity_read_quotes_a_value() {
    // The functions return only enums and bools; this guards Debug output.
    let e = env();
    let k = format!(
        "{:?}",
        login_kind(&e.root().join("spare/.credentials.json"))
    );
    assert!(!k.contains(CANARY), "{k}");
    let l = format!("{:?}", e.live());
    assert!(!l.contains(CANARY), "{l}");
}
