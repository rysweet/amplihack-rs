//! Issue #1877: new and newly logged-in profiles inherit the `env` block of
//! `~/.claude/settings.json` (or of `profile.settings_template`); keys the
//! profile already has win.
//!
//! Runs the real `amplihack` binary through `isolated(..)` with a temporary
//! `HOME`. `AMPLIHACK_CLAUDE_PROFILE_ROOT` is not set (isolated removes any
//! inherited value), so the root is the default one under the fake HOME:
//! `<HOME>/.amplihack/claude-profiles`. A fake `claude` on `PATH` stands in
//! for Claude Code. Nothing reads or writes the real `~/.claude`, the real
//! profile root or a tmux server.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.301 (Claude Code)'; exit 0 ;;
esac
echo "fake claude"
exit 0
"#;

/// A value that must never appear in any output of the command.
const SECRET: &str = "sk-ant-SENTINEL-1877-do-not-print";

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

struct Fixture {
    tmp: tempfile::TempDir,
    home: PathBuf,
    canon: PathBuf,
    root: PathBuf,
    work: PathBuf,
    tools: PathBuf,
}

impl Fixture {
    /// `~/.claude/settings.json` holds an `env` block with XPIA off, a
    /// secret-looking value, and keys that must not be inherited.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        chmod(tmp.path(), 0o700);
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let canon = home.join(".claude");
        let work = base.join("work");
        let tools = base.join("tools");
        for d in [&home, &canon, &work, &tools] {
            fs::create_dir_all(d).unwrap();
            chmod(d, 0o700);
        }
        crate::support::exec_stub::write_executable(&tools.join("claude"), FAKE_CLAUDE, 0o755);
        let settings = canon.join("settings.json");
        fs::write(
            &settings,
            json!({
                "env": {"XPIA_ENABLED": "false", "FOO": "1", "API_TOKEN": SECRET},
                "apiKeyHelper": "/bin/false",
                "language": "en"
            })
            .to_string(),
        )
        .unwrap();
        chmod(&settings, 0o600);
        let f = Self {
            root: home.join(".amplihack").join("claude-profiles"),
            tmp,
            home,
            canon,
            work,
            tools,
        };
        f.assert_isolated();
        f
    }

    fn assert_isolated(&self) {
        let base = self.tmp.path().canonicalize().unwrap();
        assert!(self.home.starts_with(&base), "HOME under the temp dir");
        assert!(self.root.starts_with(&base), "root under the temp dir");
        assert!(
            self.canon.starts_with(&base),
            "~/.claude under the temp dir"
        );
        if let Some(real) = std::env::var_os("HOME") {
            let real = PathBuf::from(real);
            assert_ne!(self.home, real, "never the real HOME");
            assert_ne!(self.canon, real.join(".claude"), "never the real ~/.claude");
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.assert_isolated();
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.work)
            .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display()),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.tools.join("claude"))
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = self.cmd(args);
        let mut attempts = 0;
        loop {
            match c.output() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r.unwrap(),
            }
        }
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}\nstdout: {}\nstderr: {}",
            out.status,
            text(&out.stdout),
            text(&out.stderr),
        );
        assert_no_secret(&out);
        out
    }

    fn settings_path(&self, p: &str) -> PathBuf {
        self.root.join(p).join("settings.json")
    }

    fn settings(&self, p: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.settings_path(p)).unwrap()).unwrap()
    }

    /// Removes `env` from the profile, as a profile made before #1877 has.
    fn strip_env(&self, p: &str) {
        let mut s = self.settings(p);
        s.as_object_mut().unwrap().remove("env");
        fs::write(self.settings_path(p), s.to_string()).unwrap();
        chmod(&self.settings_path(p), 0o600);
    }

    /// Writes `profile.settings_template` into the fake HOME's user config.
    fn set_template(&self, template: &Path) {
        let dir = self.home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        chmod(&dir, 0o700);
        let file = dir.join("config.toml");
        fs::write(
            &file,
            format!(
                "[profile]\nsettings_template = \"{}\"\n",
                template.display()
            ),
        )
        .unwrap();
        chmod(&file, 0o600);
    }
}

fn assert_no_secret(out: &Output) {
    for (stream, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        assert!(
            !text(bytes).contains(SECRET),
            "an env value was printed on {stream}"
        );
    }
}

fn expected_env() -> Value {
    json!({"XPIA_ENABLED": "false", "FOO": "1", "API_TOKEN": SECRET})
}

#[test]
fn add_seeds_the_env_block_and_nothing_else_new() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    let s = f.settings("work");
    assert_eq!(s["env"], expected_env());
    assert_eq!(s["language"], json!("en"));
    assert!(s.get("apiKeyHelper").is_none(), "apiKeyHelper copied: {s}");
    let mode = fs::metadata(f.settings_path("work"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!f.root.join("work").join(".credentials.json").exists());
    assert!(!f.root.join("work").join(".claude.json").exists());
}

#[test]
fn add_with_a_template_takes_the_template_env() {
    let f = Fixture::new();
    let template = f
        .tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("team-settings.json");
    fs::write(
        &template,
        json!({"env": {"XPIA_ENABLED": "true", "TEAM": "x"}, "language": "fr"}).to_string(),
    )
    .unwrap();
    chmod(&template, 0o600);
    f.set_template(&template);

    f.ok(&["profile", "add", "work"]);
    let s = f.settings("work");
    assert_eq!(s["env"], json!({"XPIA_ENABLED": "true", "TEAM": "x"}));
    // Only `env` comes from the template; other keys still come from ~/.claude.
    assert_eq!(s["language"], json!("en"));
}

#[test]
fn repair_backfills_a_profile_missing_env_and_keeps_its_own_keys() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    f.ok(&["profile", "add", "spare"]);
    f.strip_env("work");
    // `spare` has its own FOO but no XPIA_ENABLED: its FOO wins.
    let mut spare = f.settings("spare");
    spare["env"] = json!({"FOO": "mine"});
    fs::write(f.settings_path("spare"), spare.to_string()).unwrap();

    let out = f.ok(&["profile", "auto-switch", "repair"]);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("env: work updated"), "{stdout}");
    assert!(stdout.contains("env: spare updated"), "{stdout}");
    assert_eq!(f.settings("work")["env"], expected_env());
    assert_eq!(
        f.settings("spare")["env"],
        json!({"FOO": "mine", "XPIA_ENABLED": "false", "API_TOKEN": SECRET})
    );
}

#[test]
fn repair_leaves_a_profile_with_xpia_byte_identical() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    let body = "{\"env\":{\"XPIA_ENABLED\":\"true\"},\"language\":\"de\"}";
    fs::write(f.settings_path("work"), body).unwrap();
    chmod(&f.settings_path("work"), 0o600);

    let out = f.ok(&["profile", "auto-switch", "repair"]);
    assert!(!text(&out.stdout).contains("env: work updated"));
    // statusline::install may touch statusLine; the env block must not move.
    let after = f.settings("work");
    assert_eq!(after["env"], json!({"XPIA_ENABLED": "true"}));
}

#[test]
fn repair_with_a_group_writable_source_warns_and_writes_nothing() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    f.strip_env("work");
    chmod(&f.canon.join("settings.json"), 0o664);

    let out = f.ok(&["profile", "auto-switch", "repair"]);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("profile work: env not inherited from")
            && stderr.contains("file is writable by group or others"),
        "{stderr}"
    );
    assert!(f.settings("work").get("env").is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn login_backfills_env_before_claude_starts() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    f.strip_env("work");
    f.ok(&["profile", "login", "work"]);
    assert_eq!(f.settings("work")["env"], expected_env());
}

#[cfg(target_os = "linux")]
#[test]
fn login_creates_a_missing_settings_file_with_only_env() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    fs::remove_file(f.settings_path("work")).unwrap();
    f.ok(&["profile", "login", "work"]);
    assert_eq!(f.settings("work"), json!({"env": expected_env()}));
    let mode = fs::metadata(f.settings_path("work"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
