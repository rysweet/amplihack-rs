//! Issue #1733: with `$TMUX` set and no `-S` or `-L`, tmux uses the socket in
//! `$TMUX` and ignores `TMUX_TMPDIR`. This is why
//! `TMUX_TMPDIR=$SB/tmuxsock tmux kill-server` run from a tmux pane kills the
//! user's own server, and why the PreToolUse tmux guard exists.
//!
//! Safety: every tmux call goes through [`tmux`], which clears the environment
//! (so the caller's `$TMUX` never leaks in), sets `HOME` to the temporary
//! directory and passes `-f /dev/null`. Servers are started and killed only
//! through `-S` on sockets inside the temporary directory. The two probes
//! without `-S` always set both `TMUX` and `TMUX_TMPDIR` to paths inside it.
//! No signals, no `pkill`.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One tmux command with a cleared environment: only `PATH` and `HOME`.
fn tmux(dir: &Path) -> Command {
    let mut command = Command::new("tmux");
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("HOME", dir).args(["-f", "/dev/null"]);
    command
}

/// Two private servers and the temporary directory that holds their sockets.
struct Sandbox {
    dir: Option<tempfile::TempDir>,
    sockets: Vec<PathBuf>,
}

impl Sandbox {
    fn path(&self) -> &Path {
        self.dir.as_ref().expect("live sandbox").path()
    }

    fn assert_inside(&self, socket: &Path) {
        assert!(
            socket.starts_with(self.path()),
            "{} is outside the sandbox {}",
            socket.display(),
            self.path().display()
        );
    }

    /// Starts a detached server on `socket` with one session named `name`.
    fn start(&mut self, socket: &Path, name: &str) -> bool {
        self.assert_inside(socket);
        let status = tmux(self.path())
            .arg("-S")
            .arg(socket)
            .args(["new-session", "-d", "-s", name, "sleep 300"])
            .status();
        if status.is_ok_and(|s| s.success()) {
            self.sockets.push(socket.to_path_buf());
            true
        } else {
            false
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let mut leaked = false;
        for socket in std::mem::take(&mut self.sockets) {
            if !socket.starts_with(self.path()) {
                leaked = true;
                continue;
            }
            let killed = tmux(self.path())
                .arg("-S")
                .arg(&socket)
                .arg("kill-server")
                .status()
                .is_ok_and(|s| s.success());
            leaked |= !killed;
        }
        if leaked && let Some(dir) = self.dir.take() {
            // Keep the directory, so a server that would not die keeps its
            // socket; never fall back to a signal.
            let kept = dir.keep();
            eprintln!("tmux sandbox left running at {}", kept.display());
        }
    }
}

fn stdout(output: Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn tmux_env_takes_precedence_over_tmux_tmpdir() {
    if !tmux(Path::new("/nonexistent"))
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("skipped: tmux is not installed");
        return;
    }

    let dir = tempfile::Builder::new()
        .prefix("tx")
        .tempdir_in("/tmp")
        .expect("temporary directory");
    let mut sandbox = Sandbox {
        dir: Some(dir),
        sockets: Vec::new(),
    };
    let a_sock = sandbox.path().join("a.sock");
    // The socket TMUX_TMPDIR=<dir>/b resolves to, created as tmux would.
    let b_tmpdir = sandbox.path().join("b");
    let uid = unsafe { libc::getuid() };
    let b_dir = b_tmpdir.join(format!("tmux-{uid}"));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&b_dir)
            .expect("socket directory");
    }
    let b_sock = b_dir.join("default");
    for socket in [&a_sock, &b_sock] {
        assert!(
            socket.as_os_str().len() < 100,
            "socket path too long: {}",
            socket.display()
        );
    }

    if !sandbox.start(&a_sock, "alpha") || !sandbox.start(&b_sock, "beta") {
        eprintln!("skipped: a private tmux server could not start");
        return;
    }

    // Control: with TMUX unset, TMUX_TMPDIR alone reaches server B.
    sandbox.assert_inside(&b_tmpdir);
    let via_tmpdir = stdout(
        tmux(sandbox.path())
            .env("TMUX_TMPDIR", &b_tmpdir)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .expect("list-sessions via TMUX_TMPDIR"),
    );
    assert_eq!(via_tmpdir.trim(), "beta", "TMUX_TMPDIR control failed");

    // The incident: TMUX names server A, TMUX_TMPDIR names server B, no -S.
    sandbox.assert_inside(&a_sock);
    let via_tmux = stdout(
        tmux(sandbox.path())
            .env("TMUX", format!("{},0,0", a_sock.display()))
            .env("TMUX_TMPDIR", &b_tmpdir)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .expect("list-sessions with TMUX set"),
    );
    assert_eq!(
        via_tmux.trim(),
        "alpha",
        "with $TMUX set, tmux must use its socket and ignore TMUX_TMPDIR"
    );

    // An explicit -S wins over both.
    let via_s = stdout(
        tmux(sandbox.path())
            .env("TMUX", format!("{},0,0", a_sock.display()))
            .env("TMUX_TMPDIR", &b_tmpdir)
            .arg("-S")
            .arg(&b_sock)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .expect("list-sessions with -S"),
    );
    assert_eq!(via_s.trim(), "beta", "-S must override $TMUX");
}
