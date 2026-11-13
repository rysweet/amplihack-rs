//! #2029 CLI boundaries; policy and native I/O contracts live beside rotate.
//! No daemon escapes this process, no real CLI configuration is inherited,
//! and the synthetic process table prevents reads of host sessions.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use amplihack_utils::usage_observation::{ObservationStore, ReadOutcome};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Fixture {
    tmp: tempfile::TempDir,
    home: PathBuf,
    profiles: PathBuf,
    fake: PathBuf,
}

#[test]
fn statusline_publishes_attributed_evidence_and_preserves_original_input_and_output() {
    let f = Fixture::new();
    f.run(&["profile", "add", "work"]);
    let store = ObservationStore::open(&f.profiles).unwrap();
    let binding = store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    store
        .lock()
        .unwrap()
        .bind_session("status-session", &binding)
        .unwrap();
    let saved = f.profiles.join(".auto-switch.d/statusline");
    fs::create_dir_all(&saved).unwrap();
    fs::set_permissions(&saved, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        saved.join("work.json"),
        br#"{"type":"command","command":"/bin/cat"}"#,
    )
    .unwrap();
    fs::set_permissions(saved.join("work.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let payload = br#"{"session_id":"status-session","rate_limits":{"five_hour":{"used_percentage":85},"seven_day":{"used_percentage":40}}}"#;
    let mut child = f
        .cmd(&["profile", "statusline", "--owner", "work"])
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("work"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout, payload,
        "original command must receive and return unchanged bytes"
    );
    let ReadOutcome::Usable(record) = store.lock().unwrap().read(&binding).unwrap() else {
        panic!("status line did not publish an attributed observation")
    };
    assert_eq!(record.capture.binding, binding);
    assert_eq!(record.evidence.session_used, Some(85.0));
    assert_eq!(record.evidence.week_used, Some(40.0));
}

#[test]
fn a_forged_statusline_payload_cannot_create_an_observation_binding() {
    let f = Fixture::new();
    f.run(&["profile", "add", "work"]);
    let store = ObservationStore::open(&f.profiles).unwrap();
    let binding = store
        .lock()
        .unwrap()
        .authorised_login("work", "account-work")
        .unwrap();
    let mut child = f
        .cmd(&["profile", "statusline", "--owner", "work"])
        .env("AMPLIHACK_CLAUDE_AUTO_SWITCH", "1")
        .env("CLAUDE_CONFIG_DIR", f.profiles.join("work"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"session_id":"unregistered","profile":"work","account_id":"account-work","rate_limits":{"five_hour":{"used_percentage":99},"seven_day":{"used_percentage":99}}}"#).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert!(matches!(
        store.lock().unwrap().read(&binding).unwrap(),
        ReadOutcome::Missing
    ));
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let home = tmp.path().join("home");
        let profiles = tmp.path().join("profiles");
        fs::create_dir_all(home.join(".claude/projects")).unwrap();
        fs::create_dir(tmp.path().join("proc")).unwrap();
        fs::create_dir(tmp.path().join("work")).unwrap();
        let fake = tmp.path().join("claude");
        crate::support::exec_stub::write_executable(
            &fake,
            r#"#!/bin/sh
case "$1" in --version) echo '2.1.319 (Claude Code)'; exit 0 ;; esac
printf '%s|%s\n' "${AMPLIHACK_USAGE_PROBE-unset}" "$*" >> "$RECORD"
printf 'Current session: 7%% used\nCurrent week (all models): 9%% used\n'
"#,
            0o755,
        );
        Self {
            tmp,
            home,
            profiles,
            fake,
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
        cmd.env_clear()
            .args(args)
            .current_dir(self.tmp.path().join("work"))
            .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env(
                crate::isolated_amplihack::PROC_ROOT_ENV,
                self.tmp.path().join("proc"),
            )
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env(
                "CODEX_PROFILES_ROOT",
                self.tmp.path().join("codex-profiles"),
            )
            .env("CODEX_BIN", self.tmp.path().join("absent-codex"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &self.profiles)
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", &self.fake)
            .env("AMPLIHACK_PROFILE_USAGE_PROBE", "text")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("PATH", "/usr/bin:/bin")
            .env("RECORD", self.tmp.path().join("attempts"));
        crate::isolated_amplihack::no_git_prompt(&mut cmd);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        let out = self.cmd(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

#[test]
fn profile_usage_sets_probe_suppression_for_the_actual_claude_child() {
    let f = Fixture::new();
    f.run(&["profile", "add", "work"]);
    let out = f.run(&["profile", "usage", "--json", "work"]);
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows[0]["session"]["used_percentage"], 7.0);
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("attempts")).unwrap(),
        "1|-p /usage --no-session-persistence\n"
    );
}

#[test]
fn interactive_login_clears_inherited_probe_suppression() {
    let f = Fixture::new();
    f.run(&["profile", "add", "work"]);
    let out = f
        .cmd(&["profile", "login", "work"])
        .env("AMPLIHACK_USAGE_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.tmp.path().join("attempts")).unwrap(),
        "unset|\n"
    );
}
