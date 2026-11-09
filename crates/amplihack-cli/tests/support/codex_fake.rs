//! Shared fixture for the issue #2005 (Codex rotation and `codex-resume`)
//! integration tests, used as `crate::support::codex_fake` by the
//! `issue_2005_*` modules.
//!
//! It wraps the rotate [`Fixture`] (temporary HOME and Claude profile root,
//! fake `claude`) and adds, all under the same temporary directory:
//!
//! - a Codex profiles root (`<tmp>/codex-profiles`, mode 0700) whose account
//!   directories each hold a **trap** `auth.json` (a FIFO: any open blocks)
//!   and a mode-000 `config.toml`, so a read of either hangs or fails;
//! - the default Codex home link, `$HOME/.codex`, a symlink the tests point
//!   at one account;
//! - a fake `codex` (`CODEX_BIN`) that serves `app-server`
//!   `account/rateLimits/read` from canned per-account JSON under
//!   `$FAKE_DIR/readings`, and records each `resume` call (argv,
//!   `CODEX_HOME`, whether an API key reached it, the rollout it found) in
//!   `$FAKE_DIR/resume.log`;
//! - an empty fake `/proc` (`AMPLIHACK_TEST_CODEX_PROC_ROOT`), so a real
//!   Codex process on the host never makes a swap refuse here.
//!
//! The fake never opens a file inside an account directory except to
//! `find` a rollout by name; the canned readings live outside the homes.
//!
//! Test-only hooks this fixture relies on (read only in debug builds):
//! - `AMPLIHACK_TEST_CODEX_PROC_ROOT`: the `/proc` root for the in-use scan.
//! - `AMPLIHACK_TEST_CODEX_FAULT`: `after-journal` or `after-rename`; the
//!   swap stops the process at that point, as if it were killed.

#![cfg(unix)]
#![allow(dead_code)]

use super::exec_stub::write_executable;
use super::profile_rotate::{Fixture, assert_no_secrets, text};
use super::wait::output_within;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const PROC_ROOT_ENV: &str = "AMPLIHACK_TEST_CODEX_PROC_ROOT";
pub const FAULT_ENV: &str = "AMPLIHACK_TEST_CODEX_FAULT";
pub const OPENAI_SENTINEL: &str = "sk-SENTINEL-2005-openai";
pub const CODEX_KEY_SENTINEL: &str = "codex-SENTINEL-2005-key";
/// A fixed reset time (2026-11-09T03:00:00Z), so line text is exact.
pub const RESET_AT: i64 = 1_794_193_200;
/// A reset well in the past: usable as a time, and `sleep_until` returns
/// at once for it.
pub const PAST_RESET: i64 = 1_700_000_000;
/// A session id in the form Codex writes.
pub const SESSION: &str = "0199a213-81c0-7800-8aa1-bbab2a035a53";

/// The fake Codex program.
///
/// `app-server`: answers the JSON-RPC exchange the live reader sends. The
/// account is `basename "$CODEX_HOME"`; read `n` of it (counted in
/// `$FAKE_DIR/count.<name>`) answers `readings/<name>.<n>.json` when that
/// exists, else `readings/<name>.json`; with neither it answers "not
/// signed in". `readings/<name>.hang` makes it never answer.
///
/// `resume`: appends one record to `$FAKE_DIR/resume.log` and exits with
/// `$FAKE_RESUME_EXIT` (default 0).
const FAKE_CODEX: &str = r#"#!/bin/sh
case "$1" in
  app-server)
    name=$(basename "$CODEX_HOME")
    echo "$name" >> "$FAKE_DIR/reads.log"
    n=$(cat "$FAKE_DIR/count.$name" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo "$n" > "$FAKE_DIR/count.$name"
    if [ -n "${OPENAI_API_KEY+x}" ] || [ -n "${CODEX_API_KEY+x}" ]; then
      echo "an API key reached the app server" >&2; exit 70
    fi
    IFS= read -r init || exit 65
    printf '%s\n' '{"id":1,"result":{"userAgent":"fake-codex/0.0"}}'
    IFS= read -r note || exit 66
    IFS= read -r req || exit 67
    case "$req" in *'"method":"account/rateLimits/read"'*) ;; *) echo "bad request: $req" >&2; exit 67;; esac
    if [ -f "$FAKE_DIR/readings/$name.hang" ]; then exec sleep 600; fi
    if [ -f "$FAKE_DIR/readings/$name.$n.json" ]; then f="$FAKE_DIR/readings/$name.$n.json"
    elif [ -f "$FAKE_DIR/readings/$name.json" ]; then f="$FAKE_DIR/readings/$name.json"
    else
      printf '%s\n' '{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}'
      exec cat >/dev/null
    fi
    printf '{"id":2,"result":{"rateLimits":%s}}\n' "$(cat "$f")"
    exec cat >/dev/null ;;
  resume)
    shift
    id="$1"
    {
      printf 'ARGC=%s\n' "$#"
      for a in "$@"; do printf 'ARG=%s\n' "$a"; done
      printf 'CODEX_HOME=%s\n' "${CODEX_HOME-<unset>}"
      if [ -n "${OPENAI_API_KEY+x}" ]; then echo 'OPENAI_API_KEY=set'; else echo 'OPENAI_API_KEY=unset'; fi
      if [ -n "${CODEX_API_KEY+x}" ]; then echo 'CODEX_API_KEY=set'; else echo 'CODEX_API_KEY=unset'; fi
      r=$(find "$CODEX_HOME/sessions" -name "*-$id.jsonl" 2>/dev/null | head -n 1)
      if [ -z "$r" ]; then echo 'ROLLOUT=none'
      elif [ -L "$r" ]; then printf 'ROLLOUT=link %s -> %s\n' "${r#$CODEX_HOME/}" "$(readlink "$r")"
      else printf 'ROLLOUT=file %s\n' "${r#$CODEX_HOME/}"; fi
      echo 'END'
    } >> "$FAKE_DIR/resume.log"
    exit "${FAKE_RESUME_EXIT:-0}" ;;
  login)
    [ -f "$FAKE_DIR/readings/$(basename "$CODEX_HOME").json" ] && { echo 'Logged in using ChatGPT'; exit 0; }
    echo 'Not logged in'; exit 1 ;;
  *)
    echo "fake codex: unexpected args: $*" >&2; exit 64 ;;
esac
"#;

pub fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path and a plain mode.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo {}", path.display());
}

/// An account directory whose login files hang or fail on any read.
pub fn trapped(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    mkfifo(&dir.join("auth.json"));
    let cfg = dir.join("config.toml");
    fs::write(&cfg, "model = \"never-read\"\n").unwrap();
    fs::set_permissions(&cfg, fs::Permissions::from_mode(0o000)).unwrap();
}

/// One window of a canned reading.
pub fn window(used: u32, minutes: u64, resets_at: i64) -> serde_json::Value {
    serde_json::json!({"usedPercent": used, "windowDurationMins": minutes, "resetsAt": resets_at})
}

pub struct CodexFixture {
    pub f: Fixture,
    pub base: PathBuf,
    pub root: PathBuf,
    pub link: PathBuf,
    pub bin: PathBuf,
    pub fake: PathBuf,
    pub proc_root: PathBuf,
}

impl CodexFixture {
    /// Accounts `names` under the root, each trapped, none linked yet.
    pub fn with(names: &[&str]) -> Self {
        let f = Fixture::new();
        f.assert_isolated();
        let base = f.home.parent().unwrap().to_path_buf();
        let root = base.join("codex-profiles");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let root = root.canonicalize().unwrap();
        for n in names {
            trapped(&root.join(n));
        }
        let bin = base.join("codex-bin").join("codex");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        write_executable(&bin, FAKE_CODEX, 0o755);
        let fake = base.join("fake");
        fs::create_dir_all(fake.join("readings")).unwrap();
        let proc_root = base.join("proc");
        fs::create_dir_all(&proc_root).unwrap();
        let link = f.home.join(".codex");
        CodexFixture {
            f,
            base,
            root,
            link,
            bin,
            fake,
            proc_root,
        }
    }

    /// `ffarmer` (12/41, room 59), `ggrant` (7d 34, room 66) and `hhale`
    /// (full, credits 4200: room 0.5) signed in; `~/.codex -> ffarmer`.
    pub fn standard() -> Self {
        let c = Self::with(&["ffarmer", "ggrant", "hhale"]);
        c.reading("ffarmer", 12, 41);
        c.reading_json(
            "ggrant",
            serde_json::json!({"primary": null, "secondary": window(34, 10080, RESET_AT), "planType": "pro"}),
        );
        c.reading_json(
            "hhale",
            serde_json::json!({"primary": window(100, 300, RESET_AT), "credits": {"hasCredits": true, "balance": "4200"}, "planType": "team"}),
        );
        c.link_to("ffarmer");
        c
    }

    pub fn account(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// A 5h window at `five`% and a 7d window at `week`%, both resetting at
    /// [`RESET_AT`].
    pub fn reading(&self, name: &str, five: u32, week: u32) {
        self.reading_json(
            name,
            serde_json::json!({
                "primary": window(five, 300, RESET_AT),
                "secondary": window(week, 10080, RESET_AT),
                "planType": "plus",
                "rateLimitReachedType": null,
                "ordinaryUsageAllowed": true
            }),
        );
    }

    /// The account answers `rate_limits` (the `rateLimits` object).
    pub fn reading_json(&self, name: &str, rate_limits: serde_json::Value) {
        fs::write(
            self.fake.join("readings").join(format!("{name}.json")),
            serde_json::to_string(&rate_limits).unwrap(),
        )
        .unwrap();
    }

    /// Read `n` (1-based, per account and per run of the fixture) answers
    /// `rate_limits` instead of the default reading.
    pub fn reading_nth(&self, name: &str, n: u32, rate_limits: serde_json::Value) {
        fs::write(
            self.fake.join("readings").join(format!("{name}.{n}.json")),
            serde_json::to_string(&rate_limits).unwrap(),
        )
        .unwrap();
    }

    /// The account is fully used and Codex says its limit is reached.
    pub fn limited(&self, name: &str, resets_at: i64) {
        self.reading_json(
            name,
            serde_json::json!({
                "primary": window(100, 300, resets_at),
                "secondary": window(60, 10080, resets_at + 86_400),
                "rateLimitReachedType": "primary",
                "ordinaryUsageAllowed": false
            }),
        );
    }

    pub fn not_signed_in(&self, name: &str) {
        let _ = fs::remove_file(self.fake.join("readings").join(format!("{name}.json")));
    }

    pub fn hangs(&self, name: &str) {
        fs::write(self.fake.join("readings").join(format!("{name}.hang")), "").unwrap();
    }

    /// Points the default home link at `name` (absolute target).
    pub fn link_to(&self, name: &str) {
        let _ = fs::remove_file(&self.link);
        std::os::unix::fs::symlink(self.account(name), &self.link).unwrap();
    }

    /// The account label the link points at, if it points under the root.
    pub fn live(&self) -> Option<String> {
        let t = fs::read_link(&self.link).ok()?;
        let t = if t.is_absolute() {
            t
        } else {
            self.link.parent()?.join(t)
        };
        let parent = t.parent()?.canonicalize().ok()?;
        (parent == self.root).then(|| t.file_name().unwrap().to_string_lossy().into_owned())
    }

    /// `(name, inode)` of every account directory, sorted.
    pub fn account_inodes(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = fs::read_dir(&self.root)
            .unwrap()
            .map(|e| e.unwrap())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::symlink_metadata(e.path()).unwrap().ino(),
                )
            })
            .collect();
        v.sort();
        v
    }

    /// The `auth.json` trap of every account is still a FIFO with the same
    /// inode, so no login was moved, copied or replaced.
    pub fn auth_traps(&self) -> Vec<(String, u64, bool)> {
        let mut v = Vec::new();
        for e in fs::read_dir(&self.root).unwrap() {
            let e = e.unwrap();
            let p = e.path().join("auth.json");
            if let Ok(m) = fs::symlink_metadata(&p) {
                use std::os::unix::fs::FileTypeExt;
                v.push((
                    e.file_name().to_string_lossy().into_owned(),
                    m.ino(),
                    m.file_type().is_fifo(),
                ));
            }
        }
        v.sort();
        v
    }

    pub fn state_path(&self) -> PathBuf {
        self.f.rotate_dir().join("codex-state.json")
    }

    pub fn state(&self) -> Option<serde_json::Value> {
        let raw = fs::read_to_string(self.state_path()).ok()?;
        Some(serde_json::from_str(&raw).unwrap())
    }

    /// Writes `codex-state.json` (creating the rotate directory privately).
    pub fn write_state(&self, v: &serde_json::Value) {
        let dir = self.f.rotate_dir();
        fs::create_dir_all(&dir).unwrap();
        for d in [self.f.root.join(".auto-switch.d"), dir.clone()] {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        super::profile_rotate::private_file(&dir.join("codex-state.json"), &v.to_string());
    }

    /// A rollout for `id` under `account`'s `sessions`, mode 000 (a trap:
    /// it may be linked, never opened). Returns its path relative to the
    /// account directory.
    pub fn rollout(&self, account: &str, id: &str) -> PathBuf {
        let rel = PathBuf::from(format!(
            "sessions/2026/11/08/rollout-2026-11-08T10-00-00-{id}.jsonl"
        ));
        let p = self.account(account).join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{\"never\":\"read\"}\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
        rel
    }

    /// Lines of the rotate log that belong to the Codex rotation.
    pub fn codex_log(&self) -> Vec<String> {
        codex_lines(&self.f.log())
    }

    /// The `resume` records the fake wrote, one `Vec` of lines per call.
    pub fn resumes(&self) -> Vec<Vec<String>> {
        let raw = fs::read_to_string(self.fake.join("resume.log")).unwrap_or_default();
        raw.split("END\n")
            .filter(|b| !b.trim().is_empty())
            .map(|b| b.lines().map(String::from).collect())
            .collect()
    }

    /// Accounts the fake app server was started for, in order.
    pub fn reads(&self) -> Vec<String> {
        fs::read_to_string(self.fake.join("reads.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = self.f.cmd(args);
        c.env("CODEX_PROFILES_ROOT", &self.root)
            .env("CODEX_BIN", &self.bin)
            .env("FAKE_DIR", &self.fake)
            .env("AMPLIHACK_ROTATE_CODEX", "on")
            .env(PROC_ROOT_ENV, &self.proc_root)
            .env("OPENAI_API_KEY", OPENAI_SENTINEL)
            .env("CODEX_API_KEY", CODEX_KEY_SENTINEL);
        c
    }

    pub fn run_cmd(&self, what: &str, mut c: Command) -> Output {
        let out = output_within(what, &mut c);
        assert_no_secrets(&out);
        for s in [text(&out.stdout), text(&out.stderr), self.f.log()] {
            assert!(!s.contains("SENTINEL-2005"), "an API key was printed: {s}");
        }
        out
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.run_cmd(&format!("`amplihack {}`", args.join(" ")), self.cmd(args))
    }

    pub fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "`amplihack {}` failed: {}\nstdout: {}\nstderr: {}\nlog: {}",
            args.join(" "),
            out.status,
            text(&out.stdout),
            text(&out.stderr),
            self.f.log()
        );
        out
    }

    /// One-shot `profile rotate` (Claude part, then Codex part).
    pub fn rotate(&self) -> Output {
        self.ok(&["profile", "rotate"])
    }

    /// `profile rotate --dry-run`.
    pub fn dry_run(&self) -> Output {
        self.ok(&["profile", "rotate", "--dry-run"])
    }

    /// `profile codex-resume <args>`.
    pub fn resume(&self, args: &[&str]) -> Output {
        let mut full = vec!["profile", "codex-resume"];
        full.extend_from_slice(args);
        self.run(&full)
    }
}

/// The lines of `log` (with or without a leading UTC time) whose message
/// starts with `codex ` or `dry-run would-`, without the time.
pub fn codex_lines(log: &str) -> Vec<String> {
    let ts = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z ").unwrap();
    log.lines()
        .map(|l| ts.replace(l, "").into_owned())
        .filter(|m| m.starts_with("codex ") || m.starts_with("dry-run would-"))
        .collect()
}

/// The one `codex cycle` line of `lines`.
pub fn cycle_line(lines: &[String]) -> String {
    let c: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("codex cycle "))
        .collect();
    assert_eq!(c.len(), 1, "exactly one codex cycle line: {lines:?}");
    c[0].clone()
}

/// `MM-DDTHH:MMZ` of an epoch time, as the cycle line writes resets.
pub fn reset_text(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap()
        .format("%m-%dT%H:%MZ")
        .to_string()
}

/// A fake `/proc/<pid>` owned by the test user. `exe` is a symlink to
/// `exe` (need not exist), `cmdline` and `environ` are NUL-joined.
pub fn fake_proc(
    proc_root: &Path,
    pid: u32,
    comm: &str,
    exe: &str,
    cmdline: &[&str],
    environ: &[&str],
    cwd: &Path,
) -> PathBuf {
    let d = proc_root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
    std::os::unix::fs::symlink(exe, d.join("exe")).unwrap();
    let join = |v: &[&str]| {
        let mut b = Vec::new();
        for s in v {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b
    };
    fs::write(d.join("cmdline"), join(cmdline)).unwrap();
    fs::write(d.join("environ"), join(environ)).unwrap();
    std::os::unix::fs::symlink(cwd, d.join("cwd")).unwrap();
    d
}
