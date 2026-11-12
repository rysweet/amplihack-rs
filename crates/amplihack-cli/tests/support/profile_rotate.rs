//! Shared fixture for the issue #1592 (`profile rotate`) integration tests,
//! used as `crate::support::profile_rotate` by the `issue_1592_*` modules.
//!
//! Every run uses a temporary `HOME` and profile root, asserted before each
//! command, so nothing touches the real `~/.claude`, `~/.claude-profiles` or
//! a running `claude-rotator`. The fake `claude` never reads a login file:
//! it reads the account e-mail from `$CLAUDE_CONFIG_DIR/.claude.json` (or
//! `$HOME/.claude.json` when the variable is unset) and prints canned
//! `/usage` output for that e-mail. Login fixtures are private OAuth JSON
//! objects holding `SENTINEL-TOKEN` values, which must never be printed.
//!
//! Issue #1698: the usage request (`--input-format` in argv) is answered only
//! when `<email>.json` exists; without it the fake exits 1 at once, before
//! recording anything, so every probe falls back to `/usage`. With
//! `$CANNED/marks`, each `/usage` probe appends `S <email>` and `E <email>`
//! to `$RECORD_DIR/marks` around a 0.5 s sleep.
//!
//! Issue #1973: every command raises the identity budget and read timeout
//! to the wait ceiling, the identity endpoint serves each connection on its
//! own thread, and `$CANNED/hold` (see [`Fixture::hold_probes`]) holds each
//! `/usage` probe after it has recorded its environment.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub use super::amplihack_bin::amplihack_bin;
use super::exec_stub::write_executable;
use super::wait::{CEILING, output_within, wait_until};

/// Debug-build knobs that raise the identity budget and read timeout.
pub const IDENTITY_BUDGET_ENV: &str = "AMPLIHACK_TEST_IDENTITY_BUDGET_SECS";
pub const IDENTITY_READ_TIMEOUT_ENV: &str = "AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS";
/// What `profile rotate stop` reports when the rotator outlives its 10 s wait.
pub const STOP_REFUSAL: &str = "did not exit within 10 seconds";

pub const CREDS: &str = ".credentials.json";
pub const ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";
pub const PROFILES: [&str; 3] = ["work", "spare", "extra"];
const BANNER: &str = "You are currently using your subscription to power your Claude Code usage";

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
if [ -n "${CLAUDE_CONFIG_DIR+x}" ]; then cj="$CLAUDE_CONFIG_DIR/.claude.json"; else cj="$HOME/.claude.json"; fi
email=$(sed -n 's/.*"emailAddress" *: *"\([^"]*\)".*/\1/p' "$cj" 2>/dev/null | head -n 1)
[ -n "$email" ] || email=none
case " $* " in
  *" --input-format "*)
    [ -f "$CANNED/$email.json" ] || exit 1
    cat > "$RECORD_DIR/stdin.$email.$$"
    cat "$CANNED/$email.json"
    exit 0 ;;
esac
{
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  env | grep '^AMPLIHACK_ROTATE' || true
} > "$RECORD_DIR/env.$email.$$"
while [ -f "$CANNED/hold" ]; do sleep 0.05; done
if [ -f "$CANNED/marks" ]; then
  echo "S $email" >> "$RECORD_DIR/marks"
  sleep 0.5
  echo "E $email" >> "$RECORD_DIR/marks"
fi
[ -f "$CANNED/$email.slow" ] && sleep 30
[ -f "$CANNED/$email.out" ] && cat "$CANNED/$email.out"
exit 0
"#;

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn private_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// An OAuth login file. Issue #1700: amplihack reads which top-level keys
/// a login file has (`claudeAiOauth` or not) to tell a subscription login
/// from an API-key profile, so the file is a readable private JSON object.
/// Its values are sentinels that must never be printed; its inode
/// identifies it.
pub fn login(path: &Path) -> u64 {
    assert!(path.starts_with(std::env::temp_dir()));
    let dir = path.parent().unwrap();
    let session = if dir.ends_with(".claude") {
        dir.parent().unwrap()
    } else {
        dir
    };
    let name = if dir.ends_with(".claude") {
        "work"
    } else {
        dir.file_name().unwrap().to_str().unwrap()
    };
    // A recovery refresh retains the fake incoming account frozen in the
    // task-owned journal, even while the session still displays the old one.
    let recovering = dir
        .ends_with(".claude")
        .then(|| {
            let path = session.join("profiles/.auto-switch.d/rotate/journal.json");
            fs::read(path)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .filter(|j| matches!(j["phase"].as_str(), Some("s1" | "s2" | "s3" | "s4")))
                .and_then(|j| j.get("identity")?.get("incoming").cloned())
        })
        .flatten();
    let block = recovering
        .or_else(|| {
            fs::read(session.join(".claude.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("oauthAccount").cloned())
        })
        .unwrap_or_else(|| account_json(name));
    let fake = serde_json::to_vec(&block)
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    private_file(
        path,
        &serde_json::json!({"claudeAiOauth": {
            "accessToken": format!("fake-rotation1753-{fake}"), "refreshToken": "SENTINEL-TOKEN",
            "expiresAt": 4_102_444_800_000_i64
        }})
        .to_string(),
    );
    ino(path).unwrap()
}

pub fn ino(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path).ok().map(|m| m.ino())
}

pub fn email(p: &str) -> String {
    format!("{p}@example.com")
}

pub fn account_json(p: &str) -> serde_json::Value {
    serde_json::json!({"accountUuid": format!("uuid-{p}"), "emailAddress": email(p)})
}

pub struct Fixture {
    identity: IdentityEndpoint,
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub root: PathBuf,
    pub records: PathBuf,
    pub canned: PathBuf,
    pub work_dir: PathBuf,
    pub tools: PathBuf,
    /// Login inodes at setup: `~/.claude` (work's) and each logged-in profile.
    pub live_ino: u64,
    pub spare_ino: u64,
    pub extra_ino: u64,
    /// The fake process table every command reads (issue #2042): the shared
    /// empty tree unless [`Self::with_proc`] names a test's own.
    proc_root: std::sync::Mutex<Option<PathBuf>>,
}

impl Fixture {
    /// `work` is live in `~/.claude` (90/40: over the session threshold);
    /// `spare` 10/20 and `extra` 50/50 are logged in. No rotate ledger.
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let home = tmp.path().join("home");
        let records = tmp.path().join("records");
        let canned = tmp.path().join("canned");
        let work_dir = tmp.path().join("cwd");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &canned, &work_dir, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        let mut f = Self {
            identity: IdentityEndpoint::new(canned.clone()),
            root: home.join("profiles"),
            tmp,
            home,
            records,
            canned,
            work_dir,
            tools,
            live_ino: 0,
            spare_ino: 0,
            extra_ino: 0,
            proc_root: std::sync::Mutex::new(None),
        };
        for p in PROFILES {
            f.ok(&["profile", "add", p]);
        }
        f.root = f.root.canonicalize().unwrap();
        fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
        for p in PROFILES {
            private_file(
                &f.root.join(p).join(".claude.json"),
                &serde_json::json!({"oauthAccount": account_json(p)}).to_string(),
            );
        }
        fs::create_dir_all(f.home.join(".claude")).unwrap();
        fs::set_permissions(f.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
        f.live_ino = login(&f.home_login());
        f.spare_ino = login(&f.slot("spare"));
        f.extra_ino = login(&f.slot("extra"));
        private_file(
            &f.home.join(".claude.json"),
            &serde_json::json!({
                "oauthAccount": account_json("work"),
                "primaryApiKey": "SECRET-KEY-HOME",
                "numStartups": 7
            })
            .to_string(),
        );
        let expected = f.root.join(".auto-switch.d/expected");
        fs::create_dir_all(&expected).unwrap();
        for dir in [f.root.join(".auto-switch.d"), expected.clone()] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for name in PROFILES {
            private_file(
                &expected.join(format!("{name}.json")),
                &serde_json::json!({"account": format!("uuid-{name}"), "org": null}).to_string(),
            );
        }
        f.usage("work", 90, 40);
        f.usage("spare", 10, 20);
        f.usage("extra", 50, 50);
        f
    }

    pub fn home_login(&self) -> PathBuf {
        self.home.join(".claude").join(CREDS)
    }

    pub fn slot(&self, p: &str) -> PathBuf {
        self.root.join(p).join(CREDS)
    }

    pub fn rotate_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d/rotate")
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.rotate_dir().join("rotate.log")).unwrap_or_default()
    }

    pub fn ledger(&self) -> Option<serde_json::Value> {
        let raw = fs::read_to_string(self.rotate_dir().join("ledger.json")).ok()?;
        Some(serde_json::from_str(&raw).unwrap())
    }

    /// Canned `/usage` output for `p`: a session line with no reset clause,
    /// then the all-models week line. Issue #1818: a session at 0% with no
    /// reset reads as "no session limit"; any other session percentage keeps
    /// the limit, so `usage(p, 100, 40)` is still `session_exhausted`.
    pub fn usage(&self, p: &str, session: u32, week: u32) {
        self.canned_usage(
            p,
            &format!(
                "Current session: {session}% used\n\
                 Current week (all models): {week}% used\n"
            ),
        );
    }

    /// Issue #1962: what the fake usage endpoint answers for `p`, with both
    /// windows resetting in 3 hours. A dry run reads only this, never `claude`.
    pub fn usage_direct(&self, p: &str, session: u32, week: u32) {
        let at = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        let body = serde_json::json!({
            "five_hour": {"utilization": session, "resets_at": at},
            "seven_day": {"utilization": week, "resets_at": at}
        });
        fs::write(
            self.canned.join(direct_usage_file(&email(p))),
            body.to_string(),
        )
        .unwrap();
    }

    /// Issue #1818: a plan with no session limit. Claude prints only the
    /// all-models week line.
    pub fn usage_weekly_only(&self, p: &str, week: u32) {
        self.canned_usage(p, &format!("Current week (all models): {week}% used\n"));
    }

    fn canned_usage(&self, p: &str, lines: &str) {
        fs::write(
            self.canned.join(format!("{}.out", email(p))),
            format!("{BANNER}\n\n{lines}"),
        )
        .unwrap();
    }

    /// Like [`Self::usage`], with both windows resetting `hours` from now
    /// (1 to 23), written the way Claude writes reset times, in UTC.
    pub fn usage_resetting(&self, p: &str, session: u32, week: u32, hours: i64) {
        assert!(
            (1..24).contains(&hours),
            "a time-only reset is within a day"
        );
        let at = chrono::Utc::now() + chrono::Duration::hours(hours);
        let time = at.format("%-I:%M%P");
        let date = at.format("%b %-d, %-I:%M%P");
        fs::write(
            self.canned.join(format!("{}.out", email(p))),
            format!(
                "{BANNER}\n\nCurrent session: {session}% used · resets {time} (UTC)\n\
                 Current week (all models): {week}% used · resets {date} (UTC)\n"
            ),
        )
        .unwrap();
    }

    pub fn home_account(&self) -> String {
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(self.home.join(".claude.json")).unwrap())
                .unwrap();
        v["oauthAccount"]["accountUuid"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Where each setup login is now: `(path, inode)` for every login file.
    pub fn logins(&self) -> Vec<(String, u64)> {
        let mut v = Vec::new();
        if let Some(i) = ino(&self.home_login()) {
            v.push(("~/.claude".to_string(), i));
        }
        for e in fs::read_dir(&self.root).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(i) = ino(&e.path().join(CREDS)) {
                v.push((name, i));
            }
        }
        v.sort();
        v
    }

    /// Each setup login is in exactly one place and nothing else exists.
    pub fn assert_each_login_once(&self) {
        let mut inos: Vec<u64> = self.logins().iter().map(|(_, i)| *i).collect();
        inos.sort();
        let mut want = vec![self.live_ino, self.spare_ino, self.extra_ino];
        want.sort();
        assert_eq!(inos, want, "logins: {:?}", self.logins());
    }

    pub fn where_is(&self, inode: u64) -> Option<String> {
        self.logins()
            .into_iter()
            .find(|(_, i)| *i == inode)
            .map(|(p, _)| p)
    }

    pub fn assert_isolated(&self) {
        let real = std::env::var_os("HOME").map(PathBuf::from);
        assert!(self.home.starts_with(self.tmp.path()));
        assert!(self.root.starts_with(self.tmp.path()));
        if let Some(real) = real {
            assert_ne!(self.home, real, "never the real HOME");
        }
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = self.base(args);
        c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    /// Like `cmd`, without `AMPLIHACK_CLAUDE_PROFILE_ROOT`: the root comes
    /// from `~/.amplihack/profile-root` or the default under the temp HOME.
    pub fn cmd_no_root_env(&self, args: &[&str]) -> Command {
        self.base(args)
    }

    /// Every fixture command. Both identity knobs are set to the wait
    /// ceiling after `env_clear()`, so a loaded host cannot use up the 30 s
    /// identity budget or the 5 s read timeout (issue #1973). A future test
    /// of budget exhaustion must `env_remove` both.
    fn base(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work_dir)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            // `env_clear` dropped the probe knob `isolated` set (issue #2019).
            .env(crate::isolated_amplihack::PROBE_WAIT_ENV, "exit")
            .env(crate::isolated_amplihack::PROC_ROOT_ENV, self.proc_root())
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            // Issue #2001: `profile usage` never reads a Codex account here.
            // Outside HOME: the empty-root line names the root.
            .env(
                "CODEX_PROFILES_ROOT",
                self.home.with_file_name("no-codex-profiles"),
            )
            .env("CODEX_BIN", self.home.join("no-codex"))
            .env("AMPLIHACK_PROFILE_USAGE_API_BASE", &self.identity.base)
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("{}/v1/oauth/token", self.identity.base),
            )
            // Issue #1844: these tests read through `claude`; a reading an
            // earlier command recorded must not stand in as a passive one.
            .env("AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS", "0")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECORD_DIR", &self.records)
            .env("CANNED", &self.canned)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            // Launches never start a real background rotator here (autostart).
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        let ceiling = CEILING.as_secs().to_string();
        c.env(IDENTITY_BUDGET_ENV, &ceiling)
            .env(IDENTITY_READ_TIMEOUT_ENV, &ceiling);
        c
    }

    /// Points every later command at `fake` instead of the shared empty tree
    /// (issue #2042). Keep `fake` alive while commands run: the seam reads
    /// the real `/proc` for a tree that is gone, so [`Self::base`] panics
    /// when the tree has been removed.
    #[cfg(target_os = "linux")]
    pub fn with_proc(&self, fake: &super::fake_proc::FakeProc) -> &Self {
        *self.proc_root.lock().unwrap() = Some(fake.path().to_path_buf());
        self
    }

    fn proc_root(&self) -> PathBuf {
        match &*self.proc_root.lock().unwrap() {
            Some(tree) => {
                assert!(tree.is_dir(), "the fake proc tree {tree:?} was removed");
                tree.clone()
            }
            None => crate::isolated_amplihack::shared_empty_proc_root().to_path_buf(),
        }
    }

    /// The loopback identity endpoint, `http://127.0.0.1:<port>`.
    pub fn identity_base(&self) -> &str {
        &self.identity.base
    }

    /// Holds every later `/usage` probe after it records its environment,
    /// until [`Self::release_probes`] or the guard is dropped.
    pub fn hold_probes(&self) -> ProbeHold {
        let marker = self.canned.join("hold");
        fs::write(&marker, "").unwrap();
        ProbeHold(marker)
    }

    /// Lets held probes finish.
    pub fn release_probes(&self) {
        remove_marker(&self.canned.join("hold"));
    }

    /// Runs `profile rotate stop` until the rotator `pid` is gone. Under load
    /// the rotator may take longer than `stop`'s own 10 s wait, so a refusal
    /// with that text is retried; any other failure panics. Returns the last
    /// successful output and whether any attempt was refused. Never signals
    /// a pid itself.
    pub fn stop_until_gone(&self, pid: i32) -> (Output, bool) {
        self.stop_until_gone_by(pid, || self.cmd(&["profile", "rotate", "stop"]))
    }

    /// [`Self::stop_until_gone`] with the `stop` command built by `stop`,
    /// for tests that run it from a shell whose root resolves elsewhere.
    pub fn stop_until_gone_by(
        &self,
        pid: i32,
        mut stop: impl FnMut() -> Command,
    ) -> (Output, bool) {
        let mut refused = false;
        let mut last = None;
        wait_until(&format!("`profile rotate stop` to stop pid {pid}"), || {
            let out = output_within("`profile rotate stop`", &mut stop());
            assert_no_secrets(&out);
            if out.status.success() {
                last = Some(out);
                return true;
            }
            let err = text(&out.stderr);
            assert!(
                err.contains(STOP_REFUSAL),
                "`profile rotate stop` failed: {}\nstdout: {}\nstderr: {err}",
                out.status,
                text(&out.stdout)
            );
            refused = true;
            false
        });
        wait_until(&format!("rotator pid {pid} to exit"), || !alive(pid));
        (last.expect("a successful stop"), refused)
    }

    /// Checks what `stop_until_gone` returned. Without a refusal `stop`
    /// must report this pid stopped; after one, the rotator may have exited
    /// between attempts, so `not running` or a removed stale pid file are
    /// also correct. Either way the pid file is gone.
    pub fn assert_stopped(&self, out: &Output, refused: bool, pid: i32) {
        let stdout = text(&out.stdout);
        let stopped = format!("rotate: stopped (pid {pid})");
        if refused {
            assert!(
                stdout.contains(&stopped)
                    || stdout.contains("rotate: not running")
                    || stdout.contains("rotate: removed a stale pid file"),
                "after a refusal: {stdout}"
            );
        } else {
            assert!(stdout.contains(&stopped), "{stdout}");
        }
        assert!(
            !self.rotate_dir().join("rotate.pid").exists(),
            "stop removes the pid file"
        );
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let out = output_within(
            &format!("`amplihack {}`", args.join(" ")),
            &mut self.cmd(args),
        );
        assert_no_secrets(&out);
        crate::isolated_amplihack::assert_proc_root_honoured(&text(&out.stderr));
        // A rotate swap off a full login damps it at that whole second, and
        // only a reading strictly later may choose it again (#1870). The
        // fixture cannot read a login directly, so a run in the same second
        // would find the login stale and unknown. Waiting until the clock is
        // past the newest damping time keeps that timing out of every test
        // that swaps back. A one-shot rotate prints nothing to stdout, so
        // the ledger is the record of the swap.
        if args.starts_with(&["profile", "rotate"]) {
            self.wait_past_damping();
        }
        out
    }

    /// Waits until the wall clock is past every `swapped_away` time in the
    /// rotate ledger. A missing or damaged ledger (some tests damage it on
    /// purpose) has nothing to wait for.
    pub fn wait_past_damping(&self) {
        let newest = fs::read_to_string(self.rotate_dir().join("ledger.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|l| {
                l["swapped_away"]
                    .as_object()
                    .and_then(|m| m.values().filter_map(|v| v.as_i64()).max())
            });
        if let Some(at) = newest {
            while unix_now() <= at {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    pub fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            out.status,
            text(&out.stdout),
            text(&out.stderr),
        );
        out
    }

    /// Recorded environments of every `/usage` probe so far.
    pub fn probe_envs(&self) -> Vec<String> {
        self.records_named("env.")
            .into_iter()
            .map(|(_, body)| body)
            .collect()
    }

    /// How many `/usage` probes ran for profile `p`'s account so far.
    pub fn usage_probes(&self, p: &str) -> usize {
        self.records_named(&format!("env.{}.", email(p))).len()
    }

    /// Record files whose name starts with `prefix`, with their contents.
    fn records_named(&self, prefix: &str) -> Vec<(String, String)> {
        fs::read_dir(&self.records)
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, path)| (name, fs::read_to_string(path).unwrap()))
            .collect()
    }

    /// Adds profile `p` with a login and an account, reading
    /// `session`/`week`; it is not counted in `assert_each_login_once`.
    pub fn add_logged_in(&self, p: &str, session: u32, week: u32) {
        self.ok(&["profile", "add", p]);
        private_file(
            &self.root.join(p).join(".claude.json"),
            &serde_json::json!({"oauthAccount": account_json(p)}).to_string(),
        );
        login(&self.slot(p));
        self.usage(p, session, week);
    }
}

/// Neither a token, a secret key, an account uuid nor an e-mail is printed.
pub fn assert_no_secrets(out: &Output) {
    for s in [text(&out.stdout), text(&out.stderr)] {
        assert_text_has_no_secrets(&s);
    }
}

pub fn assert_text_has_no_secrets(s: &str) {
    for secret in [
        "SENTINEL",
        "SECRET-KEY",
        "uuid-",
        "@example.com",
        "fake-rotation1753-",
        "fake-default1753-",
        "fake-task1753-",
    ] {
        assert!(!s.contains(secret), "secret {secret:?} printed: {s}");
    }
}

/// The documented log grammar: one-shot and `watch` lines start with a UTC
/// time; `--dry-run` lines do not. `switch <from>-><to>` is the line a
/// `profile switch` swap writes (issue #1691). `reread:` lines (#1962) appear
/// when a reading goes stale, as on a loaded host (#2042). `affinity:` lines
/// (#2136) follow a swap that is not within one organisation; a profile with
/// no organisation record makes every swap to or from it `unresolved`.
pub fn assert_log_grammar(line: &str, timed: bool) {
    let msg = if timed {
        let re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z (.+)$").unwrap();
        let caps = re
            .captures(line)
            .unwrap_or_else(|| panic!("no timestamp: {line:?}"));
        caps.get(1).unwrap().as_str().to_string()
    } else {
        line.to_string()
    };
    let kinds = regex::Regex::new(
        r"^(cycle live=\S+( \S+:(s=\d+%,w=(\d+%|\?)|failed|per-token|unknown, still running))*|swap \S+->\S+ live (s=\d+% w=(\d+%|\?)|failed|unknown, still running) target s=\d+% w=(\d+%|\?) reason=(threshold|projected|upgrade)|stale live reading; re-read|switch \S+->\S+|ALERT: .+|recovery: (rolled back|finished) an interrupted swap \S+->\S+|skip: .+|change: \S+ \S+ -> \S+ by pid=\d+ ppid=\d+ parent=\x22[^\x22]*\x22( recipe_run=\S+)?( invocation=\S+)?|stop: signal received; cycle abandoned|idle: auto-switch is off|sessions archive: .+|reread: \S+ (reading stale .+; fresh .+|(failed|limit reached|time limit reached); treated as unknown)|affinity: (crossing|unresolved) \S+->\S+: .+|would .+)$",
    )
    .unwrap();
    assert!(kinds.is_match(&msg), "line outside the grammar: {line:?}");
    assert_text_has_no_secrets(line);
}

/// Whether `pid` exists (a zombie counts until it is reaped).
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Removes the probe hold marker when dropped, so a failing test never
/// leaves probes held.
pub struct ProbeHold(PathBuf);

impl Drop for ProbeHold {
    fn drop(&mut self) {
        remove_marker(&self.0);
    }
}

fn remove_marker(marker: &Path) {
    match fs::remove_file(marker) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            panic!("cannot remove {}: {e}", marker.display())
        }
        _ => {}
    }
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970");
    i64::try_from(d.as_secs()).expect("seconds fit in i64")
}

/// Loopback-only profile answers for the explicit fake moving identities.
fn direct_usage_file(email: &str) -> String {
    format!("{email}.direct.json")
}

/// What [`Fixture::usage_direct`] set for the login in a token's `block`.
fn direct_answer(canned: &Path, block: &serde_json::Value) -> Option<serde_json::Value> {
    let email = block["emailAddress"].as_str().unwrap_or("");
    let raw = fs::read(canned.join(direct_usage_file(email))).ok()?;
    Some(serde_json::from_slice(&raw).expect("canned direct usage answer is JSON"))
}

/// Each connection is served on its own thread: a slow, silent or broken
/// client ends only its own connection (issue #1973). A `/usage` request
/// for a login with a canned direct answer gets that answer (issue #1962).
struct IdentityEndpoint {
    base: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl IdentityEndpoint {
    fn new(canned: PathBuf) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let canned = canned.clone();
                        std::thread::spawn(move || serve_identity(stream, &canned));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    // A connection that failed before it was accepted (for
                    // example reset by its client) concerns that client only.
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted => {}
                    Err(e) => panic!("fake identity listener: {e}"),
                }
            }
        });
        Self {
            base,
            stop,
            worker: Some(worker),
        }
    }
}

/// Answers one request. A read error, a timeout or an early end of input
/// ends this connection without an answer; a failed write is the client's
/// concern. Request data is never echoed.
fn serve_identity(stream: std::net::TcpStream, canned: &Path) {
    use std::io::{BufRead, Write};
    if stream.set_nonblocking(false).is_err() || stream.set_read_timeout(Some(CEILING)).is_err() {
        return;
    }
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = std::io::BufReader::new(read_half);
    let mut line = String::new();
    if !matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
        return;
    }
    let profile = line.starts_with("GET /api/oauth/profile ");
    let usage = line.starts_with("GET /api/oauth/usage ");
    let mut block = None;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => {}
        }
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("authorization")
            && let Some(hex) = v.trim().strip_prefix("Bearer fake-rotation1753-")
        {
            let bytes: Option<Vec<u8>> = hex
                .as_bytes()
                .chunks_exact(2)
                .map(|b| {
                    std::str::from_utf8(b)
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                })
                .collect();
            block = bytes.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        }
    }
    let direct = block
        .as_ref()
        .filter(|_| usage)
        .and_then(|b| direct_answer(canned, b));
    let (status, body) = match block {
        Some(b) if profile => (
            "200 OK",
            serde_json::json!({"account": {
                "uuid": b["accountUuid"], "email": b["emailAddress"]
            }, "organization": {"uuid": b["organizationUuid"]}}),
        ),
        // Issue #1962: a dry run has no `claude` fallback, so a test of its
        // decision sets the direct answer ([`Fixture::usage_direct`]).
        Some(_) if direct.is_some() => ("200 OK", direct.unwrap()),
        // Valid fake credentials use the existing canned Claude fallback. A
        // 401 would request a refresh and alter the profile directory even
        // during a dry-run measurement.
        Some(_) if usage => (
            "503 Service Unavailable",
            serde_json::json!({"error":"owned fake usage uses canned fallback"}),
        ),
        _ => ("401 Unauthorized", serde_json::json!({"error":"fake-only"})),
    };
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = stream;
    // A failed write means the client has gone; it ends only this
    // connection, and the client sees no answer.
    if let Err(e) = stream.write_all(response.as_bytes()) {
        eprintln!(
            "fake identity endpoint: client gone before the answer: {}",
            e.kind()
        );
    }
}

impl Drop for IdentityEndpoint {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take()
            && let Err(e) = worker.join()
            && !std::thread::panicking()
        {
            std::panic::resume_unwind(e);
        }
    }
}
