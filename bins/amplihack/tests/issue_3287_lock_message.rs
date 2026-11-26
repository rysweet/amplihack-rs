//! CLI-to-Stop regression scenarios. Each child has its own project and state.

use amplihack_hooks::{protocol::Hook, stop::StopHook};
use amplihack_types::{HookInput, ProjectDirs, paths::MAX_LOCK_MESSAGE_BYTES};
use serde_json::{Value, json};
use std::fs;
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
    dirs: ProjectDirs,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(root.path().join("project"));
        fs::create_dir_all(&dirs.root).unwrap();
        fs::create_dir(root.path().join("home")).unwrap();
        Self { root, dirs }
    }

    fn command(&self, binary: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = crate::isolated_amplihack::isolated(binary);
        command
            .current_dir(&self.dirs.root)
            .env("HOME", self.root.path().join("home"))
            .env("AMPLIHACK_HOME", self.root.path().join("state"))
            .env("CLAUDE_PROJECT_DIR", &self.dirs.root)
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_ENABLE_REFLECTION", "0");
        command
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_amplihack"))
            .args(args)
            .output()
            .unwrap()
    }

    fn lock(&self, message: Option<&str>) {
        let args = match message {
            Some(text) => vec!["lock", "--message", text],
            None => vec!["lock"],
        };
        let output = self.cli(&args);
        assert!(output.status.success(), "{output:?}");
    }

    fn stop(&self) -> Value {
        let result = tempfile::NamedTempFile::new_in(self.root.path()).unwrap();
        let output = self
            .command(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "issue_3287_lock_message::stop_child",
            ])
            .env("LOCK_TEST_OUTPUT", result.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&fs::read(result.path()).unwrap()).unwrap()
    }
}

// Run the real Stop hook in a child so its cwd and environment are isolated
// from the other modules in this test binary.
#[test]
#[ignore = "invoked by the lock scenarios as an isolated subprocess"]
fn stop_child() {
    let input: HookInput = serde_json::from_value(json!({
        "hook_event_name": "Stop",
        "session_id": "lock-message-test",
        "stop_hook_active": false
    }))
    .unwrap();
    let decision = StopHook.process(input).unwrap();
    fs::write(
        std::env::var_os("LOCK_TEST_OUTPUT").unwrap(),
        serde_json::to_vec(&decision).unwrap(),
    )
    .unwrap();
}

#[test]
fn cli_message_wins_and_unlock_preserves_handwritten_prompt() {
    let f = Fixture::new();
    f.lock(Some("  Review the copper fixture\n"));
    fs::write(
        f.dirs.continuation_prompt_file(),
        "Review the silver fixture",
    )
    .unwrap();
    let result = f.stop();
    assert_eq!(result["decision"], "block");
    assert_eq!(result["reason"], "Review the copper fixture");

    let output = f.cli(&["unlock"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!f.dirs.lock_active_file().exists());
    assert!(!f.dirs.lock_message_file().exists());
    assert_eq!(
        fs::read_to_string(f.dirs.continuation_prompt_file()).unwrap(),
        "Review the silver fixture"
    );
    assert_eq!(f.stop()["decision"], "approve");
}

#[test]
fn unusable_files_fall_through_to_handwritten_text_then_default() {
    let f = Fixture::new();
    f.lock(None);
    let default = f.stop();
    assert_eq!(default["decision"], "block");
    assert!(!default["reason"].as_str().unwrap().is_empty());

    // Missing, blank, over-limit, invalid UTF-8 and unreadable sources all
    // receive the same treatment, independently at both precedence levels.
    for invalid in [
        None,
        Some(b" \n\t".to_vec()),
        Some(vec![b'x'; MAX_LOCK_MESSAGE_BYTES + 1]),
        Some(format!("{}x", "é".repeat(MAX_LOCK_MESSAGE_BYTES / 2)).into_bytes()),
        Some(vec![0xff]),
        Some(Vec::new()),
    ] {
        for path in [
            f.dirs.lock_message_file(),
            f.dirs.continuation_prompt_file(),
        ] {
            match &invalid {
                None => {}
                Some(bytes) if bytes.is_empty() => fs::create_dir(&path).unwrap(),
                Some(bytes) => fs::write(&path, bytes).unwrap(),
            }
        }
        assert_eq!(f.stop(), default);
        let fallback = f.dirs.continuation_prompt_file();
        if fallback.is_dir() {
            fs::remove_dir(&fallback).unwrap();
        }
        fs::write(&fallback, " \nContinue the amber fixture\t ").unwrap();
        assert_eq!(f.stop()["reason"], "Continue the amber fixture");
        for path in [f.dirs.lock_message_file(), fallback] {
            if path.is_dir() {
                fs::remove_dir(path).unwrap();
            } else if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
    }
}

#[test]
fn invalid_cli_messages_leave_fresh_and_existing_state_unchanged() {
    for text in [
        String::new(),
        " \n\t".into(),
        "x".repeat(MAX_LOCK_MESSAGE_BYTES + 1),
        format!("{}x", "é".repeat(MAX_LOCK_MESSAGE_BYTES / 2)),
    ] {
        for existing in [false, true] {
            let f = Fixture::new();
            let before = if existing {
                f.lock(Some("Keep the original fixture"));
                fs::write(f.dirs.continuation_prompt_file(), "Handwritten fixture").unwrap();
                Some(fs::read(f.dirs.lock_active_file()).unwrap())
            } else {
                None
            };
            let output = f.cli(&["lock", "--message", &text]);
            assert!(!output.status.success(), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains(&MAX_LOCK_MESSAGE_BYTES.to_string())
            );
            if let Some(before) = before {
                assert_eq!(fs::read(f.dirs.lock_active_file()).unwrap(), before);
                assert_eq!(
                    fs::read_to_string(f.dirs.lock_message_file()).unwrap(),
                    "Keep the original fixture"
                );
                assert_eq!(
                    fs::read_to_string(f.dirs.continuation_prompt_file()).unwrap(),
                    "Handwritten fixture"
                );
            } else {
                assert!(!f.dirs.locks.exists());
            }
        }
    }
}

#[test]
fn exact_byte_limit_accepts_ascii_and_multibyte_text_after_trimming() {
    for text in [
        "x".repeat(MAX_LOCK_MESSAGE_BYTES),
        "é".repeat(MAX_LOCK_MESSAGE_BYTES / 2),
    ] {
        let f = Fixture::new();
        let supplied = format!(" \n{text}\t ");
        f.lock(Some(&supplied));
        assert_eq!(
            fs::read_to_string(f.dirs.lock_message_file()).unwrap(),
            supplied
        );
        let result = f.stop();
        assert_eq!(result["decision"], "block");
        assert_eq!(result["reason"], text);
        fs::remove_file(f.dirs.lock_message_file()).unwrap();
        fs::write(f.dirs.continuation_prompt_file(), &supplied).unwrap();
        assert_eq!(f.stop()["reason"], text);
    }
}
