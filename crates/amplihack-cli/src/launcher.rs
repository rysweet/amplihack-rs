//! Process launcher with managed child and graceful shutdown.
//!
//! `ManagedChild` wraps `std::process::Child` with a bounded `Drop`
//! implementation that sends SIGTERM, waits up to 3 seconds, then
//! sends SIGKILL.

use anyhow::{Context, Result};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// A child process wrapper with graceful shutdown on drop.
///
/// On drop:
/// 1. If the child already exited, do nothing.
/// 2. Send SIGTERM (Unix) or kill (Windows).
/// 3. Wait up to 3 seconds for graceful exit.
/// 4. If still alive, SIGKILL + wait.
pub struct ManagedChild {
    child: Child,
}

impl ManagedChild {
    /// Spawn a command while preserving the caller's foreground TTY.
    pub fn spawn(mut cmd: Command) -> Result<Self> {
        let child = cmd.spawn().context("failed to spawn child process")?;
        tracing::debug!(pid = child.id(), "spawned managed child");
        Ok(Self { child })
    }

    /// Spawn the agent of a headless recipe step, bound to this launcher.
    ///
    /// Issue #2018. On Linux the child sets SIGTERM as its parent-death
    /// signal between `fork` and `exec`, then checks that this process is
    /// still its parent, so the agent ends when the launcher does, even if
    /// the launcher is killed with SIGKILL. It does not call `setsid` or
    /// `setpgid`: the agent stays in the launcher's process group and
    /// session, as an interactive agent does.
    ///
    /// The parent-death signal follows the thread that forks the child, so
    /// call this from a thread that lives as long as the agent (the main
    /// thread in `run_launch_core`).
    ///
    /// # Errors
    ///
    /// If the kernel refuses `PR_SET_PDEATHSIG`, or the launcher is already
    /// gone (`ESRCH`), the spawn fails and the agent never runs: a headless
    /// step never runs an unbound agent. The raw OS error stays reachable
    /// for `enrich_spawn_error`, as with [`Self::spawn`].
    ///
    /// Off Linux this is [`Self::spawn`].
    pub fn spawn_bound_to_parent(mut cmd: Command) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // Captured before the fork; the check after prctl catches a
            // launcher that died in between.
            let parent = std::process::id() as libc::pid_t;
            // SAFETY: `pre_exec` runs after fork and before exec. The closure
            // captures only a `pid_t` and calls only `prctl`, `getppid`,
            // `Error::last_os_error` and `Error::from_raw_os_error`, which do
            // not allocate, lock or touch the environment.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                    }
                    Ok(())
                });
            }
        }
        Self::spawn(cmd)
    }

    /// Non-blocking check: has the child exited?
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to check child status")
    }

    /// Blocking wait until child exits.
    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child.wait().context("failed to wait for child")
    }

    /// Get the child's PID.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Explicitly terminate the child (SIGTERM → wait → SIGKILL).
    pub fn terminate(&mut self) {
        self.graceful_shutdown();
    }

    fn graceful_shutdown(&mut self) {
        // Already exited?
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }

        // Send SIGTERM
        #[cfg(unix)]
        {
            // SAFETY: We're sending a standard signal to a process we own.
            // The PID is valid because try_wait() above confirmed the child is still running.
            unsafe {
                libc::kill(i32::try_from(self.child.id()).unwrap_or(0), libc::SIGTERM);
            }
        }

        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        // Wait up to 3 seconds for graceful exit
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        // Force kill
        tracing::warn!(
            pid = self.child.id(),
            "child did not exit gracefully, sending SIGKILL"
        );
        if let Err(e) = self.child.kill() {
            tracing::warn!("failed to kill child process: {e}");
        }
        if let Err(e) = self.child.wait() {
            tracing::warn!("failed to wait for child process: {e}");
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.graceful_shutdown();
    }
}

/// Translate a raw spawn failure into a message that names the real cause.
///
/// `ManagedChild::spawn` stays generic; the launch call site wraps its error
/// through this pure function. `report` is
/// [`amplihack_utils::launch_target::Resolution::rejection_report`] output for
/// the same tool, so the message describes what was actually tried.
///
/// The failure this exists to replace, observed on the dev VM 2026-08-21:
///
/// ```text
/// error: failed to spawn child process: Exec format error (os error 8)
/// ```
///
/// That names nothing real and sends the user hunting for a CPU-architecture
/// problem that does not exist. The actual cause was a 500-byte shell
/// placeholder being exec'd as if it were a native binary.
///
/// Carries paths, rejection reasons, and the remedy — never the environment,
/// never the full argv.
///
/// `package` is a parameter for the same reason `rejection_report` takes one:
/// this is the spawn-failure path for **every** tool, and its ENOEXEC prose
/// used to name `@anthropic-ai/claude-code` even when the thing that failed to
/// exec was copilot.
pub(crate) fn enrich_spawn_error(
    raw_os_error: Option<i32>,
    path: &std::path::Path,
    package: &str,
    report: &str,
) -> String {
    /// ENOEXEC. The kernel's answer when a small ASCII file with no shebang is
    /// handed to `execve` as if it were a native binary — i.e. the placeholder.
    const ENOEXEC: i32 = 8;

    // Issue #1424. EAGAIN/ENOMEM from `fork`/`clone` is the host saying "not
    // right now": the per-user process limit (`ulimit -u`), a cgroup's
    // `pids.max`, `threads-max`, or memory. amplihack had already resolved this
    // binary and verified its `--version`, so it is present, executable and
    // working — and appending the resolution report here would end the message
    // with `npm install -g`, an instruction to reinstall something that is not
    // broken. That is the misdiagnosis this issue is about, so this arm returns
    // early and the report is deliberately NOT included.
    if matches!(raw_os_error, Some(libc::EAGAIN) | Some(libc::ENOMEM)) {
        return format!(
            "Could not start {path}.\n\n\
             The host would not create a new process for it (errno {errno}). \
             The binary is present and amplihack verified it a moment ago, so \
             this is resource exhaustion on this machine, NOT a missing or \
             broken install — reinstalling {package} will not change it.\n\n\
             Check the machine:\n  \
             ulimit -u                        # per-user process/thread limit\n  \
             cat /proc/sys/kernel/threads-max # system-wide thread limit\n  \
             cat /sys/fs/cgroup/pids.max      # cgroup/container process limit\n  \
             uptime; free -m                  # load, and memory still available\n\
             Then re-run amplihack once the host has room.",
            path = amplihack_utils::launch_target::display_untrusted_path(path),
            errno = raw_os_error.unwrap_or_default(),
        );
    }

    let cause = match raw_os_error {
        Some(ENOEXEC) => format!(
            "The file is not a runnable program. This is the placeholder that \
             {package} ships when its install is incomplete and the real binary \
             was never put in place."
        ),
        Some(libc::ENOENT) => "The file is gone. It was there when amplihack checked and had \
             disappeared by the time it tried to run it."
            .to_string(),
        Some(libc::EACCES) => "The file is not executable by you.".to_string(),
        _ => "amplihack could not start it.".to_string(),
    };

    // SEC-WS2-02: the launch path is attacker-influenced ($PATH, $HOME,
    // CLAUDE_BINARY_PATH, or a planted filename), and this is the failure path
    // — the moment the user is being told what went wrong and what to run.
    // Rendering it raw lets a newline split the sections below and write the
    // sentence the user reads as amplihack's diagnosis. Same sanitiser as
    // `Resolution::rejection_report`, deliberately.
    format!(
        "Could not launch {path}.\n\n{cause}\n\n{report}",
        path = amplihack_utils::launch_target::display_untrusted_path(path),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::home_env_lock;

    // These tests spawn system binaries (`echo`, `sleep`, `true`, `false`, `sh`).
    // They hold `home_env_lock()` to prevent concurrent fleet/install tests from
    // narrowing PATH (to a temp stub directory) while these tests are running,
    // which would cause the system-binary lookup to fail.

    #[test]
    fn spawn_and_wait_for_exit() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cmd = Command::new("echo");
        let mut child = ManagedChild::spawn(cmd).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
    }

    #[test]
    fn try_wait_returns_none_while_running() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let mut child = ManagedChild::spawn(cmd).unwrap();

        // Should not have exited yet
        let result = child.try_wait().unwrap();
        assert!(result.is_none());

        // Drop will clean up (SIGTERM → SIGKILL)
    }

    #[test]
    fn drop_terminates_running_process() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let child = ManagedChild::spawn(cmd).unwrap();
        let pid = child.id();

        // Drop the child — should terminate it
        drop(child);

        // Verify process is gone (on Unix)
        #[cfg(unix)]
        {
            // SAFETY: Sending signal 0 to check if a process exists is a standard
            // POSIX pattern and is safe for any PID value.
            let result = unsafe { libc::kill(i32::try_from(pid).unwrap_or(0), 0) };
            assert_eq!(result, -1, "process should be dead after drop");
        }
    }

    #[test]
    fn managed_child_id() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cmd = Command::new("sleep");
        let mut cmd = cmd;
        cmd.arg("0.1");
        let child = ManagedChild::spawn(cmd).unwrap();
        assert!(child.id() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_keeps_child_in_foreground_process_group() {
        // Tests that rewrite PATH hold this lock; without it `sleep` can
        // fail to resolve while one of them runs.
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let child = ManagedChild::spawn(cmd).unwrap();

        // SAFETY: getpgrp/getpgid only query kernel process-group state.
        let parent_pgid = unsafe { libc::getpgrp() };
        // SAFETY: child.id() is a live child PID here.
        let child_pgid = unsafe { libc::getpgid(i32::try_from(child.id()).unwrap_or(0)) };

        assert_eq!(child_pgid, parent_pgid);
        drop(child);
    }

    // ------------------------------------------------------------------
    // Issue #2018: a headless step's agent is bound to its launcher.
    // ------------------------------------------------------------------

    /// Alive and not a zombie, from `/proc`.
    #[cfg(target_os = "linux")]
    fn running(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z" && state != "X")
        })
    }

    /// The bound spawn starts neither a new session nor a new process group:
    /// the agent stays in the launcher's group, as an interactive one does.
    #[cfg(unix)]
    #[test]
    fn spawn_bound_to_parent_keeps_child_in_the_launchers_process_group() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("60");
        let child = ManagedChild::spawn_bound_to_parent(cmd).unwrap();
        let pid = i32::try_from(child.id()).unwrap();

        // SAFETY: getpgrp/getpgid/getsid only query kernel process state.
        let (parent_pgid, child_pgid, parent_sid, child_sid) = unsafe {
            (
                libc::getpgrp(),
                libc::getpgid(pid),
                libc::getsid(0),
                libc::getsid(pid),
            )
        };
        assert_eq!(child_pgid, parent_pgid, "same process group");
        assert_eq!(child_sid, parent_sid, "same session");
        drop(child);
    }

    /// The parent-death signal follows the thread that forked the child.
    /// Forking from a thread that then exits is therefore an observable test
    /// of the binding: the bound child receives SIGTERM.
    #[cfg(target_os = "linux")]
    #[test]
    fn spawn_bound_to_parent_delivers_sigterm_when_the_forking_thread_exits() {
        use std::os::unix::process::ExitStatusExt;

        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut child = std::thread::spawn(|| {
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg("60");
            ManagedChild::spawn_bound_to_parent(cmd).unwrap()
        })
        .join()
        .unwrap();

        // Blocks until the kernel ends the child, which it does by sending
        // SIGTERM when the forking thread exits.
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM), "status: {status:?}");
    }

    /// The interactive path is unchanged: `spawn` sets no parent-death
    /// signal, so the exit of the forking thread cannot end the child.
    ///
    /// The kernel sends the signal while the forking thread exits, which may
    /// be after `join` returns, so waiting and then looking would prove
    /// nothing. The test instead has the child report its own parent-death
    /// signal (`PR_GET_PDEATHSIG`) through a pipe, from a hook that runs in
    /// the child just before `exec`, after `spawn` has done all it does. The
    /// signal is read from the child itself, so there is nothing to race.
    #[cfg(target_os = "linux")]
    #[test]
    fn plain_spawn_survives_the_exit_of_its_forking_thread() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut reader, writer) = std::io::pipe().unwrap();
        let report_fd = writer.as_raw_fd();
        let mut child = std::thread::spawn(move || {
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg("60");
            // SAFETY: the hook runs after fork and before exec and calls only
            // `prctl` and `write`, both async-signal-safe, on a descriptor
            // that stays open until exec closes it.
            unsafe {
                cmd.pre_exec(move || {
                    let mut sig: libc::c_int = -1;
                    if libc::prctl(libc::PR_GET_PDEATHSIG, &mut sig as *mut libc::c_int) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let bytes = sig.to_ne_bytes();
                    if libc::write(report_fd, bytes.as_ptr().cast(), bytes.len()) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            ManagedChild::spawn(cmd).unwrap()
        })
        .join()
        .unwrap();
        let pid = i32::try_from(child.id()).unwrap();

        // With this process's write end closed, and the child's copy closed
        // at exec, the read ends at the child's report.
        drop(writer);
        let mut report = Vec::new();
        reader.read_to_end(&mut report).unwrap();
        let sig = i32::from_ne_bytes(report.as_slice().try_into().unwrap());
        assert_eq!(sig, 0, "an unbound child must have no parent-death signal");
        assert!(
            child.try_wait().unwrap().is_none(),
            "an unbound child must survive its forking thread"
        );
        assert!(running(pid));
        drop(child);
    }

    /// A failed bound spawn keeps the raw OS error reachable, so
    /// `run_launch_core` can pass it to `enrich_spawn_error` exactly as it
    /// does for `spawn`.
    #[cfg(unix)]
    #[test]
    fn spawn_bound_to_parent_failure_keeps_the_os_error() {
        let cmd = Command::new("/nonexistent/issue-2018/agent");
        let err = ManagedChild::spawn_bound_to_parent(cmd)
            .err()
            .expect("a missing program cannot be spawned");
        let raw = err
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error);
        assert_eq!(raw, Some(libc::ENOENT), "{err:#}");
    }

    /// A bound child that exits on its own reports its own status: the
    /// binding changes nothing about a normal run.
    #[cfg(unix)]
    #[test]
    fn spawn_bound_to_parent_reports_the_childs_exit_status() {
        let _guard = home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "exit 7"]);
        let mut child = ManagedChild::spawn_bound_to_parent(cmd).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(7));
    }

    // ------------------------------------------------------------------
    // Defect 3 (issue #1266): the spawn failure must name the real cause.
    // ------------------------------------------------------------------

    /// A rejection report as `launch_target::resolve` would produce it after
    /// the stub was the only candidate.
    const STUB_REPORT: &str = "no usable claude binary found\n\n  \
        /home/you/.npm-global/bin/claude   incomplete install — 500-byte \
        placeholder, the native binary was never materialized\n\n  \
        Remedy: npm install -g @anthropic-ai/claude-code";

    fn enriched() -> String {
        // ENOEXEC — the kernel's answer when a 500-byte ASCII file with no
        // shebang is exec'd as a native binary.
        enrich_spawn_error(
            Some(8),
            std::path::Path::new("/home/you/.npm-global/bin/claude"),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        )
    }

    #[test]
    fn spawn_error_names_the_incomplete_install() {
        let msg = enriched().to_lowercase();
        assert!(
            msg.contains("install")
                && (msg.contains("incomplete")
                    || msg.contains("placeholder")
                    || msg.contains("stub")),
            "must name the real cause, got:\n{msg}"
        );
    }

    #[test]
    fn spawn_error_states_a_remedy() {
        let msg = enriched();
        assert!(
            msg.contains("npm install") && msg.contains("@anthropic-ai/claude-code"),
            "must state a remedy, got:\n{msg}"
        );
    }

    #[test]
    fn spawn_error_names_the_binary_it_tried_to_run() {
        assert!(enriched().contains("/home/you/.npm-global/bin/claude"));
    }

    // ------------------------------------------------------------------
    // Issue #1424: EAGAIN is the host, not the install.
    // ------------------------------------------------------------------

    /// The spawn that fails because the host is at its process/thread limit.
    fn eagain() -> String {
        enrich_spawn_error(
            Some(libc::EAGAIN),
            std::path::Path::new("/home/you/.npm-global/bin/copilot"),
            "@github/copilot",
            STUB_REPORT,
        )
    }

    #[test]
    fn eagain_does_not_tell_the_user_to_reinstall() {
        // amplihack had already resolved and version-checked this binary. The
        // kernel said "not right now"; answering that with `npm install -g` is
        // the misdiagnosis issue #1424 is about.
        let msg = eagain();
        assert!(
            !msg.contains("npm install"),
            "EAGAIN must not carry an install remedy, got:\n{msg}"
        );
        assert!(
            !msg.to_lowercase().contains("incomplete install"),
            "EAGAIN must not call the install incomplete, got:\n{msg}"
        );
    }

    #[test]
    fn eagain_names_resource_exhaustion_and_what_to_check() {
        let msg = eagain();
        assert!(
            msg.to_lowercase().contains("resource exhaustion"),
            "must name the real cause, got:\n{msg}"
        );
        for hint in ["ulimit -u", "pids.max", "free -m"] {
            assert!(msg.contains(hint), "must name {hint:?}, got:\n{msg}");
        }
        assert!(msg.contains("/home/you/.npm-global/bin/copilot"));
    }

    #[test]
    fn enoexec_still_names_the_incomplete_install() {
        // The genuine fallback must survive: a placeholder handed to execve is
        // still an install problem and still gets the install remedy.
        let msg = enriched();
        assert!(msg.contains("npm install") && msg.to_lowercase().contains("placeholder"));
    }

    #[test]
    fn spawn_error_does_not_send_the_user_after_an_arch_problem() {
        let msg = enriched().to_lowercase();
        for forbidden in [
            "exec format error",
            "os error 8",
            "architecture",
            "cpu",
            "platform mismatch",
        ] {
            assert!(
                !msg.contains(forbidden),
                "must not contain {forbidden:?}, got:\n{msg}"
            );
        }
    }

    #[test]
    fn spawn_error_names_the_package_it_was_given_not_claudes() {
        // A copilot user was being told the file was "the placeholder that
        // @anthropic-ai/claude-code ships".
        let msg = enrich_spawn_error(
            Some(8),
            std::path::Path::new("/home/you/.npm-global/bin/copilot"),
            "@github/copilot",
            "no usable copilot binary found",
        );
        assert!(msg.contains("@github/copilot"), "got:\n{msg}");
        assert!(!msg.contains("@anthropic-ai"), "got:\n{msg}");
    }

    #[test]
    fn spawn_error_leaks_no_environment() {
        let msg = enriched();
        for leak in ["PATH=", "HOME=", "NODE_OPTIONS", "AMPLIHACK_"] {
            assert!(!msg.contains(leak), "must not leak {leak:?}, got:\n{msg}");
        }
    }

    #[test]
    fn a_non_enoexec_spawn_failure_still_produces_something_useful() {
        // ENOENT (2): the binary vanished between resolution and exec. Still no
        // architecture talk, still a remedy.
        let msg = enrich_spawn_error(
            Some(2),
            std::path::Path::new("/home/you/.local/bin/claude"),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        );
        assert!(!msg.is_empty());
        assert!(!msg.to_lowercase().contains("architecture"));
        assert!(msg.contains("/home/you/.local/bin/claude"));
    }

    // ------------------------------------------------------------------
    // F-S3 / SEC-WS2-02 — the failure path renders an attacker-influenced
    // path into the user's terminal
    //
    // `enrich_spawn_error` formats the launch path with `Display` and no
    // sanitising. That path can come from $PATH, $HOME, `CLAUDE_BINARY_PATH`,
    // or a filename someone planted in a directory already on $PATH — exactly
    // the provenance `Resolution::rejection_report` strips at its own render
    // site, with the comment "a planted filename can carry ESC, and a newline
    // in it would forge extra rows".
    //
    // The unqualified rule is the same here, and the timing is worse: this is
    // the failure path, rendered at the exact moment the user is being told
    // what went wrong and what to run. A newline forges a `cause:` line; OSC 52
    // writes the clipboard.
    //
    // Which sanitiser, and why: `amplihack_utils::launch_target::display_untrusted_path`,
    // the same one `rejection_report` renders candidate paths with. It
    // TRUNCATES at the first control character rather than stripping escape
    // sequences out of the middle.
    //
    // That is the stricter choice and it is deliberate. Stripping keeps the
    // tail, and the tail is the payload: a path ending
    // `claude\n\nThe install is fine; run the binary directly.` survives
    // ANSI-stripping as a sentence on amplihack's own diagnosis line, in
    // amplihack's voice, at the exact moment the user is deciding what to do.
    // Truncating costs a planted path its suffix and costs a real path nothing.
    //
    // Neither `crate::util::strip_ansi` (CSI only — it leaves OSC 52, the
    // clipboard write) nor `binary_finder::strip_ansi` (CSI+OSC, but keeps the
    // tail) is the right tool at THIS site, and the repo now has all three.
    // One class of untrusted data, one sanitiser: the test below is what keeps
    // this site and `rejection_report` from drifting onto different ones.
    // ------------------------------------------------------------------

    #[test]
    fn spawn_error_strips_csi_from_the_launch_path() {
        let msg = enrich_spawn_error(
            Some(8),
            std::path::Path::new("/tmp/\x1b[2J\x1b[Hclaude"),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        );
        assert!(
            !msg.contains('\x1b'),
            "no ESC may reach the TTY, got: {msg:?}"
        );
    }

    #[test]
    fn spawn_error_strips_osc_from_the_launch_path() {
        // OSC 52 writes the user's clipboard from a rendered error message.
        let msg = enrich_spawn_error(
            Some(8),
            std::path::Path::new("/tmp/\x1b]52;c;ZXZpbA==\x07claude"),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        );
        assert!(
            !msg.contains('\x1b') && !msg.contains('\x07'),
            "OSC must not survive into the terminal, got: {msg:?}"
        );
    }

    #[test]
    fn a_newline_in_the_launch_path_cannot_forge_a_cause_line() {
        // The message renders as "Could not launch {path}.\n\n{cause}\n\n{report}".
        // A path carrying "\n\n" splits that first section and lets the
        // attacker write the sentence the user reads as amplihack's diagnosis.
        let msg = enrich_spawn_error(
            Some(8),
            std::path::Path::new(
                "/tmp/claude\n\nThe install is fine; run the binary directly.\n\n",
            ),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        );
        let headline = msg.split("\n\n").next().unwrap_or_default();
        assert!(
            !headline.contains('\n'),
            "the path must render on one line; got headline: {headline:?}"
        );
        assert!(
            !msg.contains("The install is fine"),
            "a forged cause line survived into the message:\n{msg}"
        );
    }

    #[test]
    fn spawn_error_still_names_the_path_after_sanitising() {
        // Sanitising must not cost the user the one piece of information they
        // need to act on. The printable part of the path stays.
        let msg = enrich_spawn_error(
            Some(8),
            std::path::Path::new("/home/you/.npm-global/bin/claude"),
            "@anthropic-ai/claude-code",
            STUB_REPORT,
        );
        assert!(
            msg.contains("/home/you/.npm-global/bin/claude"),
            "the launch path must still be named, got:\n{msg}"
        );
    }
}
