//! Issue #1619: the fake harness and temp-dir layout the model-tier
//! integration tests share.
//!
//! `issue_1619_tier_propagation` and `issue_1619_tier_argv_safety` both start
//! the real `amplihack` binary against a harness that records its own argv.
//! They ask different questions of the recording — what the model became, and
//! what else on the command line moved — but they need the same stage set, so
//! it is built once here rather than copied into each.
//!
//! Every path a run may read or write is under one temp dir and every
//! variable is set on the child only: no test using this touches the real
//! `~/.claude`, `~/.codex` or `~/.amplihack`.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use crate::support::amplihack_bin::amplihack_bin;

/// A fake harness: answers `--version` plausibly for whichever name it was
/// invoked under, answers `--help` with the `--effort` line Claude Code
/// 2.1.329 prints when invoked under a Claude name (issue #2916), writes every argument it was given one per line to
/// `$ARGV_LOG`, then exits 0.
///
/// The Claude version must be at least 2.1.319 or amplihack refuses the
/// launch rather than deliver its contract without
/// `--append-system-prompt-file` (issue #2030).
pub const FAKE_HARNESS: &str = r#"#!/bin/sh
case "$1" in
  --version)
    case "${0##*/}" in
      claude|rusty|rustyclawd|amplifier) echo '2.1.319 (Claude Code)' ;;
      codex*) echo 'codex-cli 0.166.0' ;;
      *) echo '0.0.400' ;;
    esac
    exit 0 ;;
  --help)
    case "${0##*/}" in
      claude|rusty|rustyclawd|amplifier)
        echo '  --effort <level>    Effort level for the current session'
        echo '                      (low, medium, high, xhigh, max)' ;;
    esac
    exit 0 ;;
esac
: > "$ARGV_LOG"
for a in "$@"; do printf '%s\n' "$a" >> "$ARGV_LOG"; done
exit 0
"#;

/// One isolated stage: a HOME, an AMPLIHACK_HOME, a working directory, a
/// `PATH` directory of fake harnesses, a run directory and the argv log they
/// write to.
pub struct Fixture {
    _tmp: tempfile::TempDir,
    pub home: PathBuf,
    /// The directory of fake harnesses on `PATH`. Public so a test can
    /// rewrite one of them — an older Claude, say — before it launches.
    pub tools: PathBuf,
    pub argv_log: PathBuf,
    pub run_dir: PathBuf,
    // Set on every child by `launch`, and read by nothing else.
    amplihack_home: PathBuf,
    work: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        let amplihack_home = root.join("amplihack-home");
        let work = root.join("work");
        let tools = root.join("tools");
        let run_dir = root.join("run");
        for d in [&home, &amplihack_home, &work, &tools, &run_dir] {
            fs::create_dir_all(d).unwrap();
        }
        // `codex-code-mode-host` must sit beside `codex`: amplihack's health
        // check refuses a codex install whose helper is missing.
        for name in ["claude", "codex", "codex-code-mode-host", "copilot"] {
            crate::support::exec_stub::write_executable(&tools.join(name), FAKE_HARNESS, 0o755);
        }
        Self {
            _tmp: tmp,
            home,
            tools,
            argv_log: root.join("argv.log"),
            run_dir,
            amplihack_home,
            work,
        }
    }

    /// Write `~/.amplihack/model-tiers.toml` in this fixture's HOME.
    pub fn write_override(&self, toml: &str) {
        self.write_home_file("model-tiers.toml", toml);
    }

    /// Write `~/.amplihack/config.toml` in this fixture's HOME.
    pub fn write_user_config(&self, toml: &str) {
        self.write_home_file("config.toml", toml);
    }

    fn write_home_file(&self, name: &str, text: &str) {
        let dir = self.home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    /// `amplihack <harness> -- <passthrough>`, isolated and not yet run, so a
    /// caller can add the variables its own question needs.
    pub fn launch(&self, harness: &str, passthrough: &[&str]) -> Command {
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.arg(harness).arg("--").args(passthrough);
        c.current_dir(&self.work)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("ARGV_LOG", &self.argv_log)
            .env(
                format!("AMPLIHACK_{}_BINARY_PATH", harness.to_uppercase()),
                self.tools.join(harness),
            )
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_GOVERNOR", "off");
        c
    }

    /// Run a command [`Fixture::launch`] built, and read what the fake
    /// harness recorded.
    ///
    /// The log is removed first, so a reading is always this run's: the fake
    /// harness truncates the file itself, but a launch amplihack refuses
    /// never reaches it and would otherwise be read as the previous argv.
    pub fn finish(&self, command: &mut Command) -> Launch {
        let _ = fs::remove_file(&self.argv_log);
        let output = command.output().expect("run amplihack");
        let argv = fs::read_to_string(&self.argv_log).unwrap_or_else(|e| {
            panic!(
                "the fake harness recorded no argv at {}: {e}. amplihack did not \
                 launch it. stderr:\n{}",
                self.argv_log.display(),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        Launch {
            argv: argv.lines().map(str::to_string).collect(),
            output,
        }
    }

    /// The common case: launch with nothing extra and read the recording.
    pub fn run(&self, harness: &str, passthrough: &[&str]) -> Launch {
        self.finish(&mut self.launch(harness, passthrough))
    }

    /// Just the recorded argv, for the tests that compare command lines.
    pub fn argv(&self, harness: &str, passthrough: &[&str]) -> Vec<String> {
        self.run(harness, passthrough).argv
    }
}

/// What one launch produced: the argv the fake harness recorded, and
/// amplihack's own output.
pub struct Launch {
    pub argv: Vec<String>,
    pub output: Output,
}

impl Launch {
    /// The value of the last `--model` on the recorded command line, or
    /// `None` when it carries none.
    pub fn model(&self) -> Option<&str> {
        let mut found = None;
        let mut iter = self.argv.iter();
        while let Some(arg) = iter.next() {
            if arg == "--model" {
                found = iter.next().map(String::as_str);
            } else if let Some(value) = arg.strip_prefix("--model=") {
                found = Some(value);
            }
        }
        found
    }

    /// The value of `-c model_reasoning_effort=…`, or `None`.
    pub fn effort(&self) -> Option<&str> {
        self.argv
            .iter()
            .filter_map(|a| a.strip_prefix("model_reasoning_effort="))
            .next_back()
    }

    /// The value of Claude Code's `--effort`, in either spelling, or `None`.
    /// Occurrence counting stays in the tests, so a duplicate is visible.
    pub fn claude_effort(&self) -> Option<&str> {
        let mut found = None;
        let mut iter = self.argv.iter();
        while let Some(arg) = iter.next() {
            if arg == "--effort" {
                found = iter.next().map(String::as_str);
            } else if let Some(value) = arg.strip_prefix("--effort=") {
                found = Some(value);
            }
        }
        found
    }

    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
}
