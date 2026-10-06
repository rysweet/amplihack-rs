//! Issue #1525: `AMPLIHACK_AGENT_BINARY` names which agent CLI to run. It used
//! to also switch `amplihack copilot` into subprocess-safe mode and skip the
//! startup update check. Anyone who exported it to choose a CLI then had every
//! interactive copilot launch degraded: no reflection, no launcher staging, no
//! power-steering prompt, `--allow-all-tools --allow-all-paths` injected, and
//! no update prompt.
//!
//! These tests run the real binary at a pseudo-terminal, which is the only
//! place the difference shows. Under `cargo test` stdio is piped, and a
//! non-TTY stream makes a launch subprocess-safe whatever the variable says.
//! Testing `resolve_subprocess_safe` alone would also miss the dispatcher in
//! `commands/mod.rs` reading the variable again and passing it in another way.
//!
//! The copilot launches use `--docker` with a stand-in `docker` on PATH. That
//! branch runs straight after the dispatcher has decided and receives the
//! decision as `--subprocess-safe` / `--no-reflection` launcher flags. It
//! needs no network, no install and no real Copilot.

#![cfg(target_os = "linux")]

use rexpect::session::spawn_command;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TIMEOUT_MS: u64 = 60_000;
const SKIP_LINE: &str = "amplihack: skipping update check (subprocess-safe / no TTY)";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("create bin");
        fs::create_dir_all(dir.path().join("home")).expect("create home");
        fs::create_dir_all(dir.path().join("work").join(".git")).expect("create work");
        // A docker that is running, already has the image, and records the
        // arguments of `docker run`, one per line.
        fs::write(
            bin.join("docker"),
            format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                   info) exit 0 ;;\n\
                   images) echo 0123456789ab; exit 0 ;;\n\
                   run) shift; for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'; exit 0 ;;\n\
                 esac\n\
                 exit 0\n",
                Self::docker_log_in(dir.path()).display()
            ),
        )
        .expect("write docker stub");
        fs::set_permissions(bin.join("docker"), fs::Permissions::from_mode(0o755))
            .expect("chmod docker stub");
        Self { dir }
    }

    fn docker_log_in(root: &Path) -> PathBuf {
        root.join("docker-run.args")
    }

    /// `amplihack <args>` under a cleared environment plus `extra`.
    fn command(&self, args: &[&str], extra: &[(&str, &str)]) -> Command {
        let path = std::env::join_paths(std::iter::once(self.dir.path().join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("join PATH");
        let mut command = Command::new(env!("CARGO_BIN_EXE_amplihack"));
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path().join("home"))
            .env("TMPDIR", std::env::temp_dir())
            .env("TERM", "dumb")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .current_dir(self.dir.path().join("work"))
            .args(args);
        for (key, value) in extra {
            command.env(key, value);
        }
        command
    }

    /// Run `amplihack copilot --docker` at a pseudo-terminal and return the
    /// arguments the stand-in `docker run` was given.
    fn copilot_launch_at_a_tty(&self, extra: &[(&str, &str)]) -> Vec<String> {
        let mut env = vec![("AMPLIHACK_NO_UPDATE_CHECK", "1")];
        env.extend_from_slice(extra);
        let mut session = spawn_command(
            self.command(&["copilot", "--docker"], &env),
            Some(TIMEOUT_MS),
        )
        .expect("spawn amplihack copilot at a pty");
        let output = session.exp_eof().unwrap_or_default();
        let log = Self::docker_log_in(self.dir.path());
        let args = fs::read_to_string(&log)
            .unwrap_or_else(|_| panic!("the launch never reached `docker run`; output:\n{output}"));
        args.lines().map(str::to_owned).collect()
    }
}

/// The issue itself: at a terminal, setting the variable to choose a CLI must
/// leave the launch interactive.
#[test]
fn agent_binary_does_not_make_an_interactive_copilot_launch_subprocess_safe() {
    let fx = Fixture::new();
    let args = fx.copilot_launch_at_a_tty(&[("AMPLIHACK_AGENT_BINARY", "copilot")]);
    assert!(
        args.iter().any(|a| a == "--tty"),
        "the harness must give amplihack a real terminal: {args:?}"
    );
    assert!(
        !args
            .iter()
            .any(|a| a == "--subprocess-safe" || a == "--no-reflection"),
        "AMPLIHACK_AGENT_BINARY alone made an interactive launch subprocess-safe: {args:?}"
    );
}

/// Control: the same harness does see subprocess-safe when a real signal is
/// present. Delegated runs (`recipe run`) carry exactly this one.
#[test]
fn amplihack_noninteractive_still_makes_the_launch_subprocess_safe() {
    let fx = Fixture::new();
    let args = fx.copilot_launch_at_a_tty(&[
        ("AMPLIHACK_AGENT_BINARY", "copilot"),
        ("AMPLIHACK_NONINTERACTIVE", "1"),
    ]);
    assert!(
        args.iter().any(|a| a == "--subprocess-safe")
            && args.iter().any(|a| a == "--no-reflection"),
        "AMPLIHACK_NONINTERACTIVE=1 must still make the launch subprocess-safe: {args:?}"
    );
}

/// The second coupling site: the startup update check skipped itself, as
/// "subprocess-safe", whenever the variable was set. At a terminal the prompt
/// must now appear.
#[test]
fn agent_binary_does_not_skip_the_startup_update_check() {
    let fx = Fixture::new();
    let mut session = spawn_command(
        fx.command(
            &["copilot", "--help"],
            &[
                ("AMPLIHACK_AGENT_BINARY", "copilot"),
                ("AMPLIHACK_TEST_FAKE_LATEST_VERSION", "99.99.99"),
            ],
        ),
        Some(TIMEOUT_MS),
    )
    .expect("spawn amplihack copilot --help at a pty");
    let before = session
        .exp_string("Update now?")
        .expect("the update prompt must be offered at a terminal");
    session.send_line("n").expect("decline the update");
    let after = session.exp_eof().unwrap_or_default();
    assert!(
        !before.contains(SKIP_LINE) && !after.contains(SKIP_LINE),
        "the update check must not report itself skipped:\n{before}{after}"
    );
}
