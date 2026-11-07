//! One scratch world per test: fake CLIs, a stub runner, a scratch `HOME`
//! and a `.git`-bounded work directory, all under `tempfile::tempdir()`.

use super::record::Record;
use super::script;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Wall-clock limit for one top-level command.
const LIMIT: Duration = Duration::from_secs(120);
/// Each file in `logs()` is cut to this many bytes.
const LOG_CAP: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    pub fn name(self) -> &'static str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        }
    }

    pub fn other(self) -> Cli {
        match self {
            Cli::Claude => Cli::Codex,
            Cli::Codex => Cli::Claude,
        }
    }
}

/// The marker variables that identify `cli`'s session.
pub fn markers_of(cli: Cli) -> Vec<&'static str> {
    amplihack_utils::agent_binary::SESSION_MARKERS
        .iter()
        .filter(|(_, owner)| *owner == cli.name())
        .map(|(name, _)| *name)
        .collect()
}

pub struct Fixture {
    tmp: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create scratch dir");
        let fx = Self { tmp };
        for dir in ["home", "amplihack-home", "tools", "records", "tmux", "logs"] {
            fs::create_dir_all(fx.path(dir)).unwrap();
        }
        fs::create_dir_all(fx.work().join(".git")).unwrap();
        fs::write(fx.work().join("probe.yaml"), script::PROBE_RECIPE).unwrap();
        let fake = script::fake_cli(&script::marker_names().join(" "));
        for cli in ["claude", "codex"] {
            write_executable(&fx.tools().join(cli), &fake, 0o755);
        }
        write_executable(
            &fx.tools().join("recipe-runner-rs"),
            script::STUB_RUNNER,
            0o755,
        );
        fx
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    pub fn tools(&self) -> PathBuf {
        self.path("tools")
    }

    pub fn work(&self) -> PathBuf {
        self.path("work")
    }

    pub fn record_dir(&self) -> PathBuf {
        self.path("records")
    }

    /// The `amplihack` binary under test with `args`, under a cleared
    /// environment holding only the allow-list in the suite's docs.
    pub fn cmd<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let bin = amplihack_bin();
        // `isolated(..)` is required by tests/integration/isolated_amplihack_scan.rs.
        // `env_clear()` then narrows its environment to this suite's allow-list.
        let mut c = crate::isolated_amplihack::isolated(&bin);
        c.env_clear();
        crate::isolated_amplihack::no_git_prompt(&mut c);
        c.args(args)
            .current_dir(self.work())
            .stdin(Stdio::null())
            .env("HOME", self.path("home"))
            .env("TMPDIR", std::env::temp_dir())
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools().display()),
            )
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_HOME", self.path("amplihack-home"))
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools().join("claude"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools().join("codex"))
            .env(
                "RECIPE_RUNNER_RS_PATH",
                self.tools().join("recipe-runner-rs"),
            )
            .env("AMPLIHACK_LAUNCHER_BINARY", &bin)
            .env("AMPLIHACK_UNDER_TEST", &bin)
            .env("RECORD_DIR", self.record_dir())
            .env("WORK_DIR", self.work())
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("IS_SANDBOX", "1")
            .env("TMUX_TMPDIR", self.path("tmux"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0");
        c
    }

    /// The environment a real `amplihack <cli>` exports to its agent.
    pub fn session(&self, c: &mut Command, cli: Cli) {
        c.env("AMPLIHACK_AGENT_BINARY", cli.name())
            .env(
                "AMPLIHACK_AGENT_BINARY_SOURCE",
                format!("session:{}", cli.name()),
            )
            .env("AMPLIHACK_SESSION_NONCE", "issue-1983-parent-nonce");
        if cli == Cli::Claude {
            c.env("CLAUDECODE", "1")
                .env("CLAUDE_CODE_ENTRYPOINT", "cli");
        }
    }

    /// Every Claude marker in `SESSION_MARKERS`, as a Claude Code agent's
    /// shell would hold them.
    pub fn all_claude_markers(&self, c: &mut Command) {
        for name in markers_of(Cli::Claude) {
            let value = match name {
                "CLAUDE_PROJECT_DIR" => self.work().display().to_string(),
                "CLAUDE_CODE_ENTRYPOINT" => "cli".to_string(),
                "CLAUDE_CODE_SESSION_ID" => "issue-1983-session".to_string(),
                _ => "1".to_string(),
            };
            c.env(name, value);
        }
    }

    /// Run `c` to completion within [`LIMIT`], killing its process group on
    /// expiry. Panics with every log unless it exits 0.
    pub fn run_ok(&self, mut c: Command) {
        let n = fs::read_dir(self.path("logs")).unwrap().count();
        let out = self.path("logs").join(format!("cmd.{n}.stdout"));
        let err = self.path("logs").join(format!("cmd.{n}.stderr"));
        c.stdout(fs::File::create(&out).unwrap())
            .stderr(fs::File::create(&err).unwrap())
            .process_group(0);
        let mut child = c.spawn().expect("spawn amplihack");
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > LIMIT {
                let pgid = child.id() as libc::pid_t;
                // SAFETY: signalling our own child's process group.
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
                let _ = child.wait();
                panic!("command ran past {LIMIT:?} and was killed\n{}", self.logs());
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            status.success(),
            "command failed: {status}\n{}",
            self.logs()
        );
    }

    /// Every record, ordered by `seq`.
    pub fn records(&self) -> Vec<Record> {
        let mut records: Vec<Record> = fs::read_dir(self.record_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("rec."))
            })
            .map(|p| Record::parse(&fs::read_to_string(&p).unwrap(), self))
            .collect();
        records.sort_by_key(|r| r.get("seq").parse::<u32>().unwrap());
        records
    }

    /// The records written by agent steps the stub runner started.
    pub fn step_records(&self) -> Vec<Record> {
        self.records()
            .into_iter()
            .filter(|r| r.get("role").starts_with("step:"))
            .collect()
    }

    /// What the stub runner was handed, as (binary, source).
    pub fn runner_env(&self) -> (String, String) {
        let text = fs::read_to_string(self.record_dir().join("runner.env"))
            .unwrap_or_else(|e| panic!("stub runner never ran ({e})\n{}", self.logs()));
        let get = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("runner.env has no {key}: {text}"))
                .to_string()
        };
        (get("binary"), get("source"))
    }

    /// Point `recipe run` at a real `recipe-runner-rs` instead of the stub,
    /// and write the level 2 recipes into the work directory.
    pub fn use_real_runner(&self, c: &mut Command, runner: &std::path::Path) {
        for (name, text) in script::NESTED_RECIPES {
            fs::write(self.work().join(name), text).unwrap();
        }
        c.env("RECIPE_RUNNER_RS_PATH", runner);
    }

    pub fn write_config_binary(&self, cli: Cli) {
        let dir = self.path("home").join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        let text = format!("[agent]\nbinary = \"{}\"\n", cli.name());
        let file = dir.join("config.toml");
        fs::write(&file, text).unwrap();
        // The loader ignores `agent.binary` in a group- or world-writable file.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// Every record and log file, each cut to [`LOG_CAP`] bytes.
    pub fn logs(&self) -> String {
        // `tmp/amplihack-workstreams` holds the `multitask` workstream logs,
        // when a test made it.
        let mut files: Vec<PathBuf> = ["records", "logs", "tmp/amplihack-workstreams"]
            .iter()
            .filter_map(|d| fs::read_dir(self.path(d)).ok())
            .flatten()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        let mut text = String::new();
        for f in files {
            let body = fs::read(&f).unwrap_or_else(|e| format!("<unreadable: {e}>").into_bytes());
            let cut = &body[body.len().saturating_sub(LOG_CAP)..];
            text.push_str(&format!(
                "--- {} ---\n{}\n",
                f.display(),
                String::from_utf8_lossy(cut)
            ));
        }
        text
    }
}
