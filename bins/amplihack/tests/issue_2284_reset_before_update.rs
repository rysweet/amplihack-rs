//! Real interactive updater ordering; release lookup uses its existing fake.
#![cfg(target_os = "linux")]
use rexpect::session::spawn_command;
use std::{
    fs,
    process::{Command, Stdio},
};

fn command(home: &std::path::Path, codex: &std::path::Path, reset: bool) -> Command {
    let mut cmd = crate::isolated_amplihack::isolated(env!("CARGO_BIN_EXE_amplihack"));
    cmd.env_clear()
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("HOME", home)
        .env("CODEX_HOME", codex)
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/bin:/bin")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_TEST_FAKE_LATEST_VERSION", "99.99.99")
        .current_dir(home)
        .stdin(Stdio::inherit())
        .arg("codex");
    if reset {
        cmd.arg("--reset-recovery");
    }
    // Append avoids starting a native client after declining the update.
    cmd.args(["--append", "update-order-fixture"]);
    cmd
}

fn codex_target() -> tempfile::TempDir {
    let codex = tempfile::tempdir().unwrap();
    fs::write(codex.path().join("config.toml"), b"# native config\n").unwrap();
    fs::write(codex.path().join("hooks.json"), b"{\"native\":true}\n").unwrap();
    codex
}

fn assert_targets_untouched(codex: &std::path::Path) {
    assert_eq!(
        fs::read(codex.join("config.toml")).unwrap(),
        b"# native config\n"
    );
    assert_eq!(
        fs::read(codex.join("hooks.json")).unwrap(),
        b"{\"native\":true}\n"
    );
}

#[test]
fn reset_is_durable_before_real_update_prompt_and_acceptance_or_decline() {
    for accept in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let codex = codex_target();
        let root = home.path().join(".amplihack/codex");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("pending.json"), b"malformed journal").unwrap();
        let mut session =
            spawn_command(command(home.path(), codex.path(), true), Some(15_000)).unwrap();
        let output = session.exp_string("Update now?").unwrap();
        assert!(
            output.contains("preserved Codex recovery journal:"),
            "{output}"
        );
        assert!(!root.join("pending.json").exists());
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("recovery-archive.json")).unwrap()).unwrap();
        assert_eq!(receipt["complete"], true);
        let archive = root.join(receipt["destination"].as_str().unwrap());
        assert_eq!(fs::read(&archive).unwrap(), b"malformed journal");
        session.send_line(if accept { "y" } else { "n" }).unwrap();
        if accept {
            session.exp_string("amplihack update (current:").unwrap();
            // The fake release has no download URL, so this real updater
            // cannot replace binaries. Injected startup unit tests separately
            // exercise a successful post-update installation and early exit.
        }
        let _ = session.exp_eof().unwrap();
        assert_eq!(fs::read(archive).unwrap(), b"malformed journal");
        assert_targets_untouched(codex.path());
    }
}

#[test]
fn failed_reset_never_reaches_enabled_update_checks() {
    let home = tempfile::tempdir().unwrap();
    let codex = codex_target();
    let root = home.path().join(".amplihack/codex");
    fs::create_dir_all(root.join("pending.json")).unwrap();
    let mut session =
        spawn_command(command(home.path(), codex.path(), true), Some(15_000)).unwrap();
    let output = session.exp_eof().unwrap();
    assert!(output.contains("startup recovery failed"), "{output}");
    assert!(!output.contains("Update now?"), "{output}");
    assert!(!home.path().join(".amplihack/last_update_check").exists());
    assert!(root.join("pending.json").is_dir());
    assert_targets_untouched(codex.path());
}

#[test]
fn ordinary_startup_checks_updates_without_archiving_pending_metadata() {
    let home = tempfile::tempdir().unwrap();
    let codex = codex_target();
    let root = home.path().join(".amplihack/codex");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("pending.json"), b"current journal").unwrap();
    let mut session =
        spawn_command(command(home.path(), codex.path(), false), Some(15_000)).unwrap();
    session.exp_string("Update now?").unwrap();
    assert_eq!(
        fs::read(root.join("pending.json")).unwrap(),
        b"current journal"
    );
    assert!(!root.join("recovery-archive.json").exists());
    session.send_line("n").unwrap();
    session.exp_eof().unwrap();
    assert_targets_untouched(codex.path());
}

#[test]
fn help_parses_and_exits_before_enabled_update_checks() {
    let home = tempfile::tempdir().unwrap();
    let codex = codex_target();
    let mut cmd = command(home.path(), codex.path(), true);
    cmd.arg("--help");
    let mut session = spawn_command(cmd, Some(15_000)).unwrap();
    let output = session.exp_eof().unwrap();
    assert!(output.contains("--reset-recovery"), "{output}");
    assert!(!output.contains("Update now?"), "{output}");
    assert!(!home.path().join(".amplihack").exists());
    assert_targets_untouched(codex.path());
}
