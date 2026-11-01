//! Shared fixture for the issue #1811 capacity-governor tests.
//!
//! Runs the real `amplihack` binary through `isolated(..)` with
//! `env_clear`, a temporary `HOME` and profile root, and stub `claude` and
//! `recipe-runner-rs` binaries written with `exec_stub`. Every variable is
//! set on the child command only; nothing touches the real `~/.claude`,
//! `~/.amplihack`, profile root or tmux server. The governor's wait is 0
//! unless a test says otherwise, so no test sleeps on admission.
//!
//! The stub `claude` answers `--version`, appends `S <ns>` and `E <ns>`
//! around an optional `$STUB_SLEEP` to `$RECORD_DIR/marks`, writes its argv
//! to `$RECORD_DIR/args.<pid>`, prints `STUB-OUT` and exits 0.

#![cfg(unix)]
#![allow(dead_code)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use crate::support::profile_default::{give_logins, set_default};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
echo "S $(date +%s%N)" >> "$RECORD_DIR/marks"
printf '%s\n' "$@" > "$RECORD_DIR/args.$$"
env | grep -E '^AMPLIHACK_(TREE_ID|SESSION_DEPTH)=' > "$RECORD_DIR/env.$$" || true
[ -n "$STUB_SLEEP" ] && sleep "$STUB_SLEEP"
echo "E $(date +%s%N)" >> "$RECORD_DIR/marks"
echo STUB-OUT
exit 0
"#;

/// Runs two agent steps through `amplihack claude`, as `recipe-runner-rs`
/// does, and records that it started. `$STUB_BEFORE_STEPS`, when set, runs
/// after admission and before the first step, so a test can move the
/// readings part-way through a run.
pub const STUB_RUNNER: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'recipe-runner-rs 0.0.0 (stub)'; exit 0 ;;
esac
echo ran >> "$RECORD_DIR/runner"
env | grep -E '^AMPLIHACK_GOVERNOR_(PARENT|RECIPE)=' > "$RECORD_DIR/runner.env" || true
[ -n "$STUB_BEFORE_STEPS" ] && sh -c "$STUB_BEFORE_STEPS"
for s in 1 2; do
  "$AMPLIHACK_UNDER_TEST" claude --subprocess-safe -- -p "step $s" </dev/null >> "$RECORD_DIR/runner.out" 2>&1 || {
    printf '{"recipe_name":"gov","success":false,"step_results":[],"context":{}}\n'; exit 1; }
done
printf '%s\n' '{"recipe_name":"gov","success":true,"step_results":[],"context":{}}'
"#;

pub const RECIPE: &str = "name: gov\ndescription: governor test\nversion: 1.0.0\ncontext: {}\nsteps:\n  - id: s1\n    type: bash\n    command: \"true\"\n";

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn describe(o: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        o.status,
        text(&o.stdout),
        text(&o.stderr)
    )
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub struct Fixture {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub root: PathBuf,
    pub records: PathBuf,
    pub work: PathBuf,
    pub tools: PathBuf,
}

impl Fixture {
    /// Profiles `alpha` (active) and `beta`, both logged in, no readings.
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let records = tmp.path().join("records");
        let work = tmp.path().join("work");
        let tools = tmp.path().join("tools");
        for d in [&home, &records, &work, &tools] {
            fs::create_dir_all(d).unwrap();
        }
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        write_executable(&tools.join("recipe-runner-rs"), STUB_RUNNER, 0o755);
        let f = Self {
            root: home.join("profiles"),
            tmp,
            home,
            records,
            work,
            tools,
        };
        for p in ["alpha", "beta"] {
            let out = f.cmd(&["profile", "add", p]).output().unwrap();
            assert!(out.status.success(), "profile add {p}: {}", describe(&out));
        }
        give_logins(&f.home, &f.root, "alpha", &["beta"]);
        set_default(&f.root, "alpha");
        f
    }

    /// Writes `usage.json` readings observed now: `(name, session %, weekly
    /// %)`, with a negative session for a weekly-only profile.
    pub fn readings(&self, profiles: &[(&str, f64, f64)]) {
        let t = now();
        let mut accounts = serde_json::Map::new();
        for (name, session, weekly) in profiles {
            let mut r = serde_json::json!({
                "seven_day": {"used_percentage": weekly, "resets_at": t + 3 * 86_400},
                "observed_at": t,
                "source": "statusline",
            });
            if *session >= 0.0 {
                r["five_hour"] =
                    serde_json::json!({"used_percentage": session, "resets_at": t + 7200});
            }
            accounts.insert(name.to_string(), r);
        }
        let dir = self.root.join(".auto-switch.d");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(
            &dir.join("usage.json"),
            &serde_json::json!({"version": 1, "accounts": accounts}).to_string(),
        );
    }

    /// Writes `~/.amplihack/config.toml` with a `[governor]` table.
    pub fn governor_config(&self, body: &str) {
        let dir = self.home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(&dir.join("config.toml"), &format!("[governor]\n{body}\n"));
    }

    /// The readings file [`Fixture::readings`] writes.
    pub fn usage_path(&self) -> PathBuf {
        self.root.join(".auto-switch.d").join("usage.json")
    }

    pub fn gov(&self) -> PathBuf {
        self.root.join(".governor.d")
    }

    /// `amplihack <args>` with only the fixture's environment.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_clear()
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_UNDER_TEST", amplihack_bin())
            .env("RECORD_DIR", &self.records)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .env("AMPLIHACK_GOVERNOR_WAIT", "0")
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.root);
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    pub fn recipe_path(&self) -> PathBuf {
        let p = self.work.join("gov.yaml");
        fs::write(&p, RECIPE).unwrap();
        p
    }

    /// How many times the stub `claude` started.
    pub fn claude_runs(&self) -> usize {
        fs::read_to_string(self.records.join("marks"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("S "))
            .count()
    }

    /// The largest number of stub `claude` processes running at once.
    pub fn max_overlap(&self) -> usize {
        let marks = fs::read_to_string(self.records.join("marks")).unwrap_or_default();
        let mut events: Vec<(u128, i32)> = marks
            .lines()
            .filter_map(|l| {
                let (k, t) = l.split_once(' ')?;
                let t: u128 = t.trim().parse().ok()?;
                Some((t, if k == "S" { 1 } else { -1 }))
            })
            .collect();
        // At equal times, count an end before a start.
        events.sort();
        let (mut cur, mut max) = (0i32, 0i32);
        for (_, d) in events {
            cur += d;
            max = max.max(cur);
        }
        max as usize
    }

    /// Every file under the temporary directory, with its bytes.
    pub fn tree(&self, under: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![under.to_path_buf()];
        while let Some(p) = stack.pop() {
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.is_dir() {
                for e in fs::read_dir(&p).unwrap() {
                    stack.push(e.unwrap().path());
                }
                out.push((p, Vec::new()));
            } else {
                out.push((p.clone(), fs::read(&p).unwrap_or_default()));
            }
        }
        out.sort();
        out
    }
}
