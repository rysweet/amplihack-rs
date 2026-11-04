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
if [ -n "$AMPLIHACK_1877_ENV_MARKER" ]; then
  grep -q '"FOO"' "$CLAUDE_CONFIG_DIR/settings.json" || exit 87
  : > "$AMPLIHACK_1877_ENV_MARKER"
fi
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

/// File-backed capture avoids pipe deadlocks while the parent enforces a deadline.
fn bounded_output(mut command: Command, dir: &Path) -> Output {
    let stdout = tempfile::tempfile_in(dir).unwrap();
    let stderr = tempfile::tempfile_in(dir).unwrap();
    command
        .stdout(stdout.try_clone().unwrap())
        .stderr(stderr.try_clone().unwrap());
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("profile command blocked on a FIFO with no writer");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    use std::io::{Read, Seek, SeekFrom};
    fn captured(mut file: fs::File) -> Vec<u8> {
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        bytes
    }
    Output {
        status,
        stdout: captured(stdout),
        stderr: captured(stderr),
    }
}

#[test]
fn repair_refuses_a_source_fifo_without_a_writer_or_destination_changes() {
    for use_template in [false, true] {
        let f = Fixture::new();
        f.ok(&["profile", "add", "work"]);
        // Install statusLine first so its independent repair is a no-op.
        f.ok(&["profile", "auto-switch", "repair"]);
        let mut settings = f.settings("work");
        settings["env"] = json!({"XPIA_ENABLED": "true"});
        fs::write(f.settings_path("work"), settings.to_string()).unwrap();
        let source = if use_template {
            let template = f.tmp.path().canonicalize().unwrap().join("template.json");
            f.set_template(&template);
            template
        } else {
            let shared = f.canon.join("settings.json");
            fs::remove_file(&shared).unwrap();
            shared
        };
        let c_path = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: c_path is a live, NUL-terminated fixture pathname.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let before = fs::read(f.settings_path("work")).unwrap();
        let out = bounded_output(f.cmd(&["profile", "auto-switch", "repair"]), f.tmp.path());
        assert_no_secret(&out);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let stderr = text(&out.stderr);
        assert!(stderr.contains("file is not a regular file"), "{stderr}");
        assert!(stderr.contains(&source.display().to_string()), "{stderr}");
        assert_eq!(fs::read(f.settings_path("work")).unwrap(), before);
        use std::os::unix::fs::FileTypeExt;
        assert!(fs::symlink_metadata(source).unwrap().file_type().is_fifo());
    }
}

#[test]
fn add_filters_source_entries_without_printing_their_values() {
    let f = Fixture::new();
    let source = f.canon.join("settings.json");
    fs::write(&source, json!({"env": {
        "VALID": "inherited", "1BAD": SECRET, "NUMBER": 123,
        "ANTHROPIC_API_KEY": SECRET, "ANTHROPIC_AUTH_TOKEN": SECRET, "CLAUDE_CODE_OAUTH_TOKEN": SECRET
    }}).to_string()).unwrap();
    let out = f.ok(&["profile", "add", "work"]);
    assert_eq!(f.settings("work")["env"], json!({"VALID": "inherited"}));
    let stderr = text(&out.stderr);
    for key in [
        "1BAD",
        "NUMBER",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        assert!(
            stderr.contains(&format!(
                "env key {key} not inherited from {}",
                source.display()
            )),
            "{stderr}"
        );
    }
}

#[test]
fn add_copies_the_env_block_and_nothing_else_new() {
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
fn add_with_a_group_writable_source_warns_and_copies_no_env() {
    let f = Fixture::new();
    chmod(&f.canon.join("settings.json"), 0o664);
    let out = f.ok(&["profile", "add", "work"]);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("profile work: env not inherited from")
            && stderr.contains("file is writable by group or others"),
        "{stderr}"
    );
    assert!(stderr.contains(&format!(
        "chmod 600 {}",
        f.canon.join("settings.json").display()
    )));
    let s = f.settings("work");
    assert!(s.get("env").is_none(), "env copied: {s}");
    assert_eq!(s["language"], json!("en"));
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
fn repair_preserves_xpia_and_inherits_other_missing_keys() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    let body = "{\"env\":{\"XPIA_ENABLED\":\"true\"},\"language\":\"de\"}";
    fs::write(f.settings_path("work"), body).unwrap();
    chmod(&f.settings_path("work"), 0o600);

    let out = f.ok(&["profile", "auto-switch", "repair"]);
    assert!(text(&out.stdout).contains("env: work updated"));
    // Repair independently installs statusLine.
    let mut after = f.settings("work");
    after.as_object_mut().unwrap().remove("statusLine");
    assert_eq!(
        after,
        json!({"env": {"XPIA_ENABLED": "true", "FOO": "1", "API_TOKEN": SECRET}, "language": "de"})
    );
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
    let mut settings = f.settings("work");
    settings["env"] = json!({"XPIA_ENABLED": "true"});
    fs::write(f.settings_path("work"), settings.to_string()).unwrap();
    let marker = f.tmp.path().join("login-saw-env");
    let out = f
        .cmd(&["profile", "login", "work"])
        .env("AMPLIHACK_1877_ENV_MARKER", &marker)
        .output()
        .unwrap();
    assert_no_secret(&out);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(marker.exists(), "Claude did not observe env before login");
    assert_eq!(
        f.settings("work")["env"],
        json!({"XPIA_ENABLED": "true", "FOO": "1", "API_TOKEN": SECRET})
    );
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

/// A launch with a profile backfills `env` in the pre-spawn check, so a
/// profile nobody ran login, switch or repair on still gets missing keys
/// while keeping its existing `XPIA_ENABLED` override.
#[cfg(target_os = "linux")]
#[test]
fn a_profile_launch_backfills_env_before_claude_starts() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "work"]);
    f.strip_env("work");
    let mut s = f.settings("work");
    s["env"] = json!({"XPIA_ENABLED": "true"});
    s["language"] = json!("de");
    fs::write(f.settings_path("work"), s.to_string()).unwrap();
    // The launch guard (issue #1700) refuses a profile without a login file.
    let creds = f.root.join("work").join(".credentials.json");
    fs::write(&creds, r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#).unwrap();
    chmod(&creds, 0o600);

    let marker = f.tmp.path().join("launch-saw-env");
    let out = f
        .cmd(&[
            "claude",
            "--profile",
            "work",
            "--subprocess-safe",
            "--",
            "-p",
            "x",
        ])
        .env("AMPLIHACK_1877_ENV_MARKER", &marker)
        .output()
        .unwrap();
    assert_no_secret(&out);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        marker.exists(),
        "Claude did not observe env before session start"
    );
    let after = f.settings("work");
    assert_eq!(
        after["env"],
        json!({"XPIA_ENABLED": "true", "FOO": "1", "API_TOKEN": SECRET})
    );
    assert_eq!(
        after["language"],
        json!("de"),
        "the profile's own key was lost"
    );
}

/// `profile switch` runs the backfill before the move, whatever its outcome. These
/// use the rotator fixture (#1691): `work` is live in `~/.claude`, `spare`
/// and `extra` hold their own logins, all under a temporary HOME. Linux-only,
/// as the swap needs `renameat2(RENAME_EXCHANGE)`.
#[cfg(target_os = "linux")]
mod switch {
    use super::{chmod, text};
    use crate::support::profile_rotate::Fixture;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;

    const PROFILES: [&str; 3] = ["work", "spare", "extra"];

    fn settings_path(f: &Fixture, p: &str) -> PathBuf {
        f.root.join(p).join("settings.json")
    }

    /// The shared `env` source, and `spare` with one key of its own.
    fn setup() -> Fixture {
        let f = Fixture::new();
        let shared = f.home.join(".claude").join("settings.json");
        fs::write(
            &shared,
            json!({"env": {"XPIA_ENABLED": "false", "FOO": "1", "BAR": "2"}}).to_string(),
        )
        .unwrap();
        chmod(&shared, 0o600);
        let spare = settings_path(&f, "spare");
        fs::write(
            &spare,
            r#"{"env":{"XPIA_ENABLED":"true","FOO":"mine"},"language":"de"}"#,
        )
        .unwrap();
        chmod(&spare, 0o600);
        f
    }

    fn snapshot(f: &Fixture) -> Vec<Option<Vec<u8>>> {
        PROFILES
            .iter()
            .map(|p| {
                let path = settings_path(f, p);
                match fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => panic!("could not read {}: {error}", path.display()),
                }
            })
            .collect()
    }

    #[test]
    fn switch_backfills_the_target_and_keeps_its_own_keys() {
        let f = setup();
        f.ok(&["profile", "switch", "spare"]);
        let s: Value =
            serde_json::from_str(&fs::read_to_string(settings_path(&f, "spare")).unwrap()).unwrap();
        assert_eq!(
            s["env"],
            json!({"FOO": "mine", "XPIA_ENABLED": "true", "BAR": "2"})
        );
        assert_eq!(s["language"], json!("de"));
    }

    /// A swap refused as `DefaultOnly` still makes the target the default for
    /// new launches, which then run in the target's own folder, so the target
    /// must gain its missing `BAR` even though the switch exits 1.
    #[test]
    fn a_refused_switch_still_backfills_the_new_default() {
        let f = setup();
        // Claude holds its refresh lock in ~/.claude: the swap refuses.
        fs::create_dir(f.home.join(".claude").join(".oauth_refresh.lock")).unwrap();
        let before = snapshot(&f);
        let out = f.run(&["profile", "switch", "spare"]);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
        assert!(
            text(&out.stdout).contains("Default for new launches: spare"),
            "stdout: {}\nstderr: {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let s: Value =
            serde_json::from_str(&fs::read_to_string(settings_path(&f, "spare")).unwrap()).unwrap();
        assert_eq!(
            s["env"],
            json!({"FOO": "mine", "XPIA_ENABLED": "true", "BAR": "2"})
        );
        assert_eq!(s["language"], json!("de"));
        // Only the target's file changed.
        let after = snapshot(&f);
        for (i, p) in PROFILES.iter().enumerate() {
            if *p != "spare" {
                assert_eq!(after[i], before[i], "{p}'s settings.json changed");
            }
        }
    }

    #[test]
    fn switch_none_writes_no_profile_settings() {
        let f = setup();
        let before = snapshot(&f);
        f.ok(&["profile", "switch", "--none"]);
        assert_eq!(snapshot(&f), before, "--none wrote settings.json");
    }
}
