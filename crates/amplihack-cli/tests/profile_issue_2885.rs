//! Issue #2885: on macOS, Claude Code keeps a profile's login in the login
//! Keychain and writes no `.credentials.json`, and the launch guard used to
//! refuse such a profile as "a subscription profile with no login of its own".
//! These tests run the real `amplihack` binary under a temporary `HOME`.
//!
//! No test touches the user's Keychain. A fake `security` is first on `PATH`;
//! it appends `<HOME>|<arguments>` to `records/security.log` and exits with
//! the status the test chose. A temporary `HOME` cannot reach the login
//! Keychain in any case. The fake `claude` records the config directory it
//! was started with, so "never started" is "no `claude.*` record".
//!
//! The lookup has no deadline of its own (crusty round 2 of PR #2891): it
//! waits for `security` to end, so a fake that macOS takes seconds to check
//! on its first run is slow, never misread.
//!
//! Contract (docs/reference/profile-command.md, "Keychain logins"):
//! - macOS, no login file, an identifiable `oauthAccount` in `.claude.json`,
//!   and `security find-generic-password -s Claude Code-credentials-<h>`
//!   exits 0 (`<h>` is the first eight hex digits of the SHA-256 of the
//!   profile directory's path): the profile launches, and `kind list` says
//!   `detected: oauth login in the macOS Keychain`.
//! - Exit 44: the old refusal, unchanged.
//! - Any other status or a signal: a distinct refusal that says the Keychain
//!   could not be read, never the "no login of its own" text.
//! - `profile auto-switch status` shows `keychain` or `keychain unreadable`
//!   in its `login` column.
//! - `profile overflow add` warns that the Keychain could not be read when
//!   that is why nothing can move onto the profile, and keeps its "has no
//!   login file" warning for a profile with no login (crusty round 1 of
//!   PR #2891).
//! - No account value and no service name reaches output, even with debug
//!   logging on; `security` is never given `-w`, `-g` or `-a`.
//! - Off macOS, `security` is never started and detection is unchanged.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const ACCOUNT: &str = r#"{"oauthAccount":{"accountUuid":"uuid-SENTINEL-KEYCHAIN","emailAddress":"user-SENTINEL-KEYCHAIN@example.com"}}"#;
const SENTINEL: &str = "user-SENTINEL-KEYCHAIN";

/// What `profile overflow add` says about a profile with no login.
const NO_LOGIN_WARNING: &str =
    "warning: work has no login file, so it cannot be used as overflow until it has one";
const REFUSED: &str = "profile work is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login work', or run 'amplihack profile kind set work api-key' if per-token billing is intended.";

/// Records the config directory it was started with and exits 0.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}" > "$RECORD_DIR/claude.$$"
exit 0
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    /// The canonical profile root.
    root: PathBuf,
    records: PathBuf,
    work: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    /// `work`: an `oauthAccount` and no login file. `plain`: neither.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let home = base.join("home");
        let records = base.join("records");
        let work = base.join("work");
        let tools = base.join("tools");
        for d in [&home, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        let mut f = Self {
            _tmp: tmp,
            root: home.join("profiles"),
            home,
            records,
            work,
            tools,
        };
        for name in ["work", "plain"] {
            f.ok(&["profile", "add", name]);
        }
        f.root = f.root.canonicalize().unwrap();
        fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
        let json = f.root.join("work").join(".claude.json");
        fs::write(&json, ACCOUNT).unwrap();
        fs::set_permissions(&json, fs::Permissions::from_mode(0o600)).unwrap();
        f
    }

    /// A fake `security` that records and then runs `tail`.
    fn security(&self, tail: &str) {
        let log = self.records.join("security.log");
        let body = format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$HOME\" \"$*\" >> '{}'\n{tail}\n",
            log.display()
        );
        write_executable(&self.tools.join("security"), &body, 0o755);
    }

    fn security_exits(&self, code: i32) {
        self.security(&format!("echo 'acct=\"{SENTINEL}\"'\nexit {code}"));
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env(crate::isolated_amplihack::PROBE_WAIT_ENV, "exit")
            .env(
                crate::isolated_amplihack::PROC_ROOT_ENV,
                crate::isolated_amplihack::shared_empty_proc_root(),
            )
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root)
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            out.status,
            text(&out.stdout),
            text(&out.stderr)
        );
        out
    }

    /// The recorded `security` lines; none when it was never started.
    fn security_lines(&self) -> Vec<String> {
        fs::read_to_string(self.records.join("security.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// How many times the fake `claude` was started for a launch.
    fn claude_runs(&self) -> usize {
        fs::read_dir(&self.records)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("claude.")
            })
            .count()
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn all_output(out: &Output) -> String {
    format!("{}{}", text(&out.stdout), text(&out.stderr))
}

/// The `login` column of `auto-switch status` for `name`, as the row's text.
fn status_row(f: &Fixture, name: &str) -> String {
    let out = f.ok(&["profile", "auto-switch", "status"]);
    text(&out.stdout)
        .lines()
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no row for {name}:\n{}", text(&out.stdout)))
        .to_string()
}

// --- macOS: the Keychain is asked ---------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use std::process::Stdio;

    impl Fixture {
        fn claude_config_dir(&self) -> String {
            let entry = fs::read_dir(&self.records)
                .unwrap()
                .map(|e| e.unwrap())
                .find(|e| e.file_name().to_string_lossy().starts_with("claude."))
                .expect("claude was started");
            fs::read_to_string(entry.path()).unwrap()
        }
    }

    /// `Claude Code-credentials-<first 8 hex of SHA-256 of dir>`, computed with
    /// the system `shasum`, independently of the code under test.
    fn service_for(dir: &Path) -> String {
        let mut child = Command::new("shasum")
            .args(["-a", "256"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("shasum");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(dir.to_str().unwrap().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        format!("Claude Code-credentials-{}", &text(&out.stdout)[..8])
    }

    #[test]
    fn a_profile_with_a_keychain_item_launches() {
        let f = Fixture::new();
        f.security_exits(0);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        assert!(out.status.success(), "{}", all_output(&out));
        assert_eq!(f.claude_runs(), 1, "claude was started once");
        assert!(f.claude_config_dir().contains(&format!(
            "CLAUDE_CONFIG_DIR={}",
            f.root.join("work").display()
        )));
        let want = format!(
            "{}|find-generic-password -s {}",
            f.home.display(),
            service_for(&f.root.join("work"))
        );
        let lines = f.security_lines();
        assert!(!lines.is_empty(), "the Keychain was asked");
        for line in &lines {
            assert_eq!(line, &want, "exactly the documented command, no -w -g -a");
        }
        assert!(!all_output(&out).contains(SENTINEL), "{}", all_output(&out));
    }

    #[test]
    fn kind_list_names_the_keychain_as_the_source() {
        let f = Fixture::new();
        f.security_exits(0);
        let out = f.ok(&["profile", "kind", "list"]);
        let row = text(&out.stdout)
            .lines()
            .find(|l| l.starts_with("work"))
            .unwrap()
            .to_string();
        assert!(row.contains("subscription"), "{row}");
        assert!(
            row.contains("(detected: oauth login in the macOS Keychain)"),
            "{row}"
        );
    }

    #[test]
    fn an_absent_item_is_refused_with_the_old_message() {
        let f = Fixture::new();
        f.security_exits(44);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
        assert!(text(&out.stderr).contains(REFUSED), "{}", text(&out.stderr));
        assert_eq!(f.claude_runs(), 0, "claude never started");
        assert!(!f.security_lines().is_empty());
    }

    #[test]
    fn a_locked_keychain_is_refused_with_its_own_message() {
        let f = Fixture::new();
        f.security_exits(51);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
        assert!(err.contains("profile work has no login file"), "{err}");
        assert!(err.contains("Keychain could not be read"), "{err}");
        assert!(err.contains("`security` exit status 51"), "{err}");
        assert!(
            err.contains("hint: unlock the login Keychain and run again, or: amplihack profile kind set <name> api-key"),
            "{err}"
        );
        assert!(
            !err.contains("no login of its own"),
            "not the old text: {err}"
        );
        assert!(!all_output(&out).contains(SENTINEL));
        assert_eq!(f.claude_runs(), 0, "claude never started");
    }

    #[test]
    fn a_keychain_that_is_locked_is_never_taken_for_an_api_key() {
        let f = Fixture::new();
        // An API-key setting in the profile does not turn a locked Keychain
        // into per-token billing: the launch is still refused.
        let settings = f.root.join("work").join("settings.json");
        fs::write(&settings, r#"{"apiKeyHelper":"echo CANARY-LEAK-PROBE"}"#).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        f.security_exits(51);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
        assert!(text(&out.stderr).contains("could not be read"));
        assert_eq!(f.claude_runs(), 0);
        let kinds = f.ok(&["profile", "kind", "list"]);
        let row = text(&kinds.stdout)
            .lines()
            .find(|l| l.starts_with("work"))
            .unwrap()
            .to_string();
        assert!(row.contains("subscription"), "{row}");
        assert!(row.contains("detected: macOS Keychain unreadable"), "{row}");
    }

    #[test]
    fn a_security_ended_by_a_signal_is_refused_as_unreadable() {
        let f = Fixture::new();
        f.security("kill -9 $$");
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
        assert!(err.contains("`security` was ended by a signal"), "{err}");
        assert_eq!(f.claude_runs(), 0);
    }

    #[test]
    fn a_profile_without_an_account_is_refused_and_security_is_never_started() {
        let f = Fixture::new();
        f.security_exits(0);
        let out = f.run(&["profile", "run", "plain", "--", "-p", "hi"]);
        assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
        assert!(
            text(&out.stderr)
                .contains("profile plain is a subscription profile with no login of its own"),
            "{}",
            text(&out.stderr)
        );
        assert!(f.security_lines().is_empty(), "no identity, no lookup");
    }

    #[test]
    fn an_api_key_override_launches_without_asking_the_keychain() {
        let f = Fixture::new();
        f.security_exits(51);
        f.ok(&["profile", "kind", "set", "work", "api-key"]);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        assert!(out.status.success(), "{}", all_output(&out));
        assert!(f.security_lines().is_empty(), "api-key never asks");
        assert_eq!(f.claude_runs(), 1);
    }

    #[test]
    fn a_subscription_override_launches_on_a_keychain_login() {
        let f = Fixture::new();
        f.security_exits(0);
        f.ok(&["profile", "kind", "set", "work", "subscription"]);
        let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
        assert!(out.status.success(), "{}", all_output(&out));
        assert_eq!(f.claude_runs(), 1);
    }

    #[test]
    fn status_shows_keychain_in_the_login_column() {
        let f = Fixture::new();
        f.security_exits(0);
        let row = status_row(&f, "work");
        assert!(row.contains("keychain"), "{row}");
        assert!(!row.contains("keychain unreadable"), "{row}");
    }

    #[test]
    fn status_shows_keychain_unreadable_when_it_cannot_be_read() {
        let f = Fixture::new();
        f.security_exits(51);
        let row = status_row(&f, "work");
        assert!(row.contains("keychain unreadable"), "{row}");
    }

    #[test]
    fn status_shows_none_for_an_absent_item_and_for_no_account() {
        let f = Fixture::new();
        f.security_exits(44);
        for name in ["work", "plain"] {
            let row = status_row(&f, name);
            assert!(!row.contains("keychain"), "{name}: {row}");
            assert!(row.contains("none"), "{name}: {row}");
        }
    }

    /// The warning must send the user to the right remedy: unlock the
    /// Keychain, not log the profile in.
    #[test]
    fn overflow_add_names_an_unreadable_keychain_and_not_a_missing_login() {
        let f = Fixture::new();
        f.security_exits(51);
        let out = f.ok(&["profile", "overflow", "add", "work"]);
        let err = text(&out.stderr);
        assert!(
            err.contains(
                "warning: work has no login file and the macOS Keychain could not be read (`security` exit status 51), so it cannot be used as overflow until the Keychain can be read"
            ),
            "{err}"
        );
        assert!(err.contains("Unlock the login Keychain"), "{err}");
        assert!(!err.contains(NO_LOGIN_WARNING), "{err}");
        assert!(!all_output(&out).contains(SENTINEL), "{}", all_output(&out));
    }

    #[test]
    fn overflow_add_keeps_the_no_login_warning_for_an_absent_item_only() {
        let f = Fixture::new();
        f.security_exits(44);
        let out = f.ok(&["profile", "overflow", "add", "work"]);
        let err = text(&out.stderr);
        assert!(err.contains(NO_LOGIN_WARNING), "{err}");
        assert!(!err.contains("Keychain"), "{err}");

        f.ok(&["profile", "overflow", "remove", "work"]);
        f.security_exits(0);
        let out = f.ok(&["profile", "overflow", "add", "work"]);
        let err = text(&out.stderr);
        assert!(
            !err.contains("has no login file"),
            "a Keychain login: {err}"
        );
    }

    /// Debug logging shows what the program does; it never shows an account
    /// value or the service name (which carries a hash of a private path).
    #[test]
    fn debug_logging_never_shows_an_account_value_or_the_service_name() {
        for code in [0, 44, 51] {
            let f = Fixture::new();
            f.security_exits(code);
            let service = service_for(&f.root.join("work"));
            let hex = service.rsplit('-').next().unwrap().to_string();
            for args in [
                vec!["profile", "run", "work", "--", "-p", "hi"],
                vec!["profile", "kind", "list"],
                vec!["profile", "auto-switch", "status"],
            ] {
                let out = f.cmd(&args).env("RUST_LOG", "debug").output().unwrap();
                let all = all_output(&out);
                assert!(!all.contains(SENTINEL), "exit {code} {args:?}: {all}");
                assert!(!all.contains("SENTINEL-KEYCHAIN"), "exit {code} {args:?}");
                assert!(!all.contains(&service), "exit {code} {args:?}: {all}");
                assert!(!all.contains(&hex), "exit {code} {args:?}: {all}");
            }
        }
    }
}

// --- Off macOS: nothing changes ----------------------------------------------------------

/// A recording `security` on `PATH` is never started: Linux and the other
/// Unix systems have no Keychain, so detection and the refusal are as before.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_macos_security_is_never_started_and_the_refusal_is_unchanged() {
    let f = Fixture::new();
    f.security_exits(0);
    let out = f.run(&["profile", "run", "work", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(1), "{}", all_output(&out));
    assert!(text(&out.stderr).contains(REFUSED), "{}", text(&out.stderr));
    assert_eq!(f.claude_runs(), 0);
    f.ok(&["profile", "kind", "list"]);
    let row = status_row(&f, "work");
    assert!(!row.contains("keychain"), "{row}");
    assert!(row.contains("none"), "{row}");
    // `overflow add` keeps its warning, and never names a Keychain.
    let added = f.ok(&["profile", "overflow", "add", "work"]);
    let err = text(&added.stderr);
    assert!(err.contains(NO_LOGIN_WARNING), "{err}");
    assert!(!err.contains("Keychain"), "{err}");
    assert!(f.security_lines().is_empty(), "security was never started");
}
