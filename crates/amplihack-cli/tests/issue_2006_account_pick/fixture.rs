//! Shared fixture for the issue #2006 (`amplihack account pick`) tests,
//! used as `crate::issue_2006_fixture` by the `issue_2006_*` modules.
//!
//! Every run has its own temporary `HOME`, Codex profiles root
//! (`CODEX_PROFILES_ROOT`) and lease directory
//! (`AMPLIHACK_ACCOUNT_LEASE_DIR`). The Codex is a fake `sh` script:
//!
//! - as `codex app-server` it answers the live rate-limit read for the
//!   account named by `basename "$CODEX_HOME"`, with the used percentage in
//!   `$CANNED/<name>.used`, or "not signed in" when that file is missing;
//! - as anything else (a launch) it records `CODEX_HOME` and
//!   `AMPLIHACK_CODEX_HOME` to `$RECORD/launch.env`, plus the leases on disk
//!   at that moment to `$RECORD/leases-during`, and exits 0.
//!
//! Each account home holds the Codex login file name as a named pipe
//! ([`Trap`]). Nothing may open it: the trap's writer sets a flag after
//! writing and before closing, and a reader cannot reach end-of-file until
//! the writer closes, so any finished read has already set the flag. No
//! timer is involved. No real `codex`, `~/.codex` or network is used.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use crate::support::amplihack_bin::amplihack_bin;

/// The Codex login file name, built from parts.
pub fn codex_login_name() -> String {
    ["auth", ".json"].concat()
}

const FAKE_CODEX: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'codex-cli 0.166.0'; exit 0;;
  plugin)
    case "$2" in
      list) echo '{"installed":[]}';;
      *) echo '{"ok":true}';;
    esac
    exit 0;;
  app-server)
    name=$(basename "$CODEX_HOME")
    IFS= read -r init || exit 65
    printf '%s\n' '{"id":1,"result":{"userAgent":"fake-codex/0.0"}}'
    IFS= read -r note || exit 66
    IFS= read -r req || exit 67
    if [ -f "$CANNED/$name.used" ]; then
      u=$(cat "$CANNED/$name.used")
      printf '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":%s,"windowDurationMins":300,"resetsAt":1794001200},"secondary":null,"credits":null,"planType":"plus"}}}\n' "$u"
    else
      printf '%s\n' '{"error":{"code":-32600,"message":"codex account authentication required to read rate limits"},"id":2}'
    fi
    exec cat >/dev/null;;
esac
{
  printf 'CODEX_HOME=%s\n' "${CODEX_HOME-<unset>}"
  printf 'AMPLIHACK_CODEX_HOME=%s\n' "${AMPLIHACK_CODEX_HOME-<unset>}"
} > "$RECORD/launch.env"
ls "$AMPLIHACK_ACCOUNT_LEASE_DIR/codex" 2>/dev/null | grep '\.json$' > "$RECORD/leases-during" || true
cat > /dev/null
exit 0
"#;

pub struct Trap {
    path: PathBuf,
    opened: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl Trap {
    pub fn new(path: &Path) -> Self {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let opened = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&opened);
        let p = path.to_path_buf();
        let writer = std::thread::spawn(move || {
            if let Ok(mut w) = fs::OpenOptions::new().write(true).open(&p) {
                let _ = w.write_all(b"{\"tokens\":\"SENTINEL-TOKEN-2006\"}");
                flag.store(true, Ordering::SeqCst);
            }
        });
        Self {
            path: path.to_path_buf(),
            opened,
            writer: Some(writer),
        }
    }

    pub fn assert_never_opened(&self) {
        assert!(
            !self.opened.load(Ordering::SeqCst),
            "{} was opened; the picker must never read credentials",
            self.path.display()
        );
    }
}

impl Drop for Trap {
    fn drop(&mut self) {
        // Release a writer still waiting for a reader, then join it. If the
        // pipe is already gone nothing can release the writer, so it is left
        // detached rather than joined (a join would never return).
        let released = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.path)
            .is_ok();
        if let Some(h) = self.writer.take()
            && released
        {
            let _ = h.join();
        }
    }
}

pub struct Fx {
    /// Declared first so the traps drop before `tmp` deletes their pipes.
    traps: Vec<Trap>,
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub profiles: PathBuf,
    pub leases: PathBuf,
    pub canned: PathBuf,
    pub record: PathBuf,
    pub tools: PathBuf,
}

impl Fx {
    /// Accounts `a`, `b` and `c` under the profiles root, each with a
    /// trapped login file; no used percentage yet (all "not signed in").
    pub fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("account-pick-it-")
            .tempdir()
            .unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let profiles = root.join("codex-profiles");
        let canned = root.join("canned");
        let record = root.join("record");
        let tools = root.join("tools");
        for d in [&home, &profiles, &canned, &record, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let mut traps = Vec::new();
        for n in ["a", "b", "c"] {
            let h = profiles.join(n);
            fs::create_dir_all(&h).unwrap();
            traps.push(Trap::new(&h.join(codex_login_name())));
        }
        link_tools(&tools);
        crate::support::exec_stub::write_executable(&tools.join("codex"), FAKE_CODEX, 0o700);
        crate::support::exec_stub::write_executable(
            &tools.join("node"),
            "#!/bin/sh\necho v22.14.0\n",
            0o700,
        );
        Self {
            leases: root.join("leases"),
            tmp,
            home,
            profiles,
            canned,
            record,
            tools,
            traps,
        }
    }

    /// Accounts at 80, 50 and 20 percent headroom.
    pub fn with_80_50_20() -> Self {
        let f = Self::new();
        f.used("a", 20);
        f.used("b", 50);
        f.used("c", 80);
        f
    }

    pub fn used(&self, account: &str, pct: u32) {
        fs::write(self.canned.join(format!("{account}.used")), pct.to_string()).unwrap();
    }

    pub fn account(&self, name: &str) -> PathBuf {
        self.profiles.join(name)
    }

    pub fn assert_credentials_untouched(&self) {
        for t in &self.traps {
            t.assert_never_opened();
        }
    }

    pub fn lease_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = ["codex", "claude"]
            .iter()
            .filter_map(|c| fs::read_dir(self.leases.join(c)).ok())
            .flat_map(|rd| rd.flatten().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        v.sort();
        v
    }

    pub fn lease_json(&self) -> Vec<serde_json::Value> {
        self.lease_files()
            .iter()
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
            .collect()
    }

    /// `amplihack <args>` with the fixture's environment.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env("PATH", &self.tools)
            .env("CODEX_PROFILES_ROOT", &self.profiles)
            .env("CODEX_BIN", self.tools.join("codex"))
            .env("CANNED", &self.canned)
            .env("RECORD", &self.record)
            .env("AMPLIHACK_ACCOUNT_LEASE_DIR", &self.leases)
            // Leases expire by age; the tests must not depend on how long
            // the host takes, so expiry is pushed out of reach.
            .env("AMPLIHACK_ACCOUNT_LEASE_TTL_SECS", "86400")
            .env_remove("CODEX_HOME")
            .env_remove("AMPLIHACK_CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("AMPLIHACK_CLAUDE_CONFIG_DIR")
            .env_remove("AMPLIHACK_ACCOUNT_PICK")
            .env_remove("AMPLIHACK_ACCOUNT_PICK_CALLER")
            .env_remove("AMPLIHACK_ACCOUNT_JOB_COST_PCT")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY");
        c
    }

    /// `amplihack codex -- exec -` as a recipe agent step runs it, against
    /// the fake Codex.
    pub fn codex_launch(&self) -> Command {
        let mut c = self.cmd(&["codex", "--", "exec", "-"]);
        c.env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
            .env_remove("AMPLIHACK_DEFAULT_MODEL")
            .env_remove("AMPLIHACK_SESSION_ID")
            .env_remove("AMPLIHACK_SESSION_DEPTH");
        c
    }

    /// What the fake Codex launch recorded, as `KEY=value` lines.
    pub fn launch_env(&self, out: &Output) -> Vec<String> {
        let p = self.record.join("launch.env");
        let raw = fs::read_to_string(&p).unwrap_or_else(|_| {
            panic!("the fake codex was never launched\n{}", text(out));
        });
        raw.lines().map(str::to_owned).collect()
    }
}

pub fn text(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn link_tools(tools: &Path) {
    for tool in [
        "sh", "cat", "mkdir", "rm", "mv", "cp", "chmod", "git", "uname", "basename", "ls", "grep",
        "env",
    ] {
        let found = Command::new("/bin/sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        if found.status.success() {
            std::os::unix::fs::symlink(
                String::from_utf8(found.stdout).unwrap().trim(),
                tools.join(tool),
            )
            .unwrap();
        }
    }
}
