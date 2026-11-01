//! `amplihack agent supervise` (R5): one supervisor per user, held by
//! `supervisor.lock` for the life of the process; a pass, then a wait of
//! `--interval` from the end of that pass, until SIGTERM or SIGINT (R5.9).

use super::SuperviseArgs;
use super::launch;
use super::pass::{self, PassCfg};
use super::store::AgentsDir;
use amplihack_utils::secure_files::{
    ExclusiveLock, lock_exclusive_timeout, open_private_nofollow, read_bounded_nofollow,
};
use anyhow::{Context, anyhow};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SLICE: Duration = Duration::from_millis(200);
const MAX_LOCK_BYTES: u64 = 64;

pub fn run_supervise(args: SuperviseArgs, home: &Path) -> anyhow::Result<()> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("amplihack agent supervise needs Linux /proc; nothing was started");
    }
    // S16: the path the panes will run is settled before any lock is taken.
    let exe = launch::amplihack_exe()?;
    let dir = AgentsDir::open(home)?;
    let _lock = take_lock(&dir)?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop))
            .with_context(|| format!("failed to register a handler for signal {sig}"))?;
    }
    let cfg = PassCfg {
        batch: usize::try_from(args.batch).unwrap_or(usize::MAX),
        gap: Duration::from_secs(args.gap),
        max_restarts: usize::try_from(args.max_restarts).unwrap_or(usize::MAX),
        exe,
    };
    let interval = Duration::from_secs(args.interval);
    run_loop(args.once, interval, &stop, &sleep_unless_stopped, || {
        pass::run_pass(&dir, home, &cfg, &stop)
    })
}

/// Takes `supervisor.lock` without waiting and writes this pid into it. The
/// file is opened without following a symlink and with `O_CLOEXEC`, so no
/// tmux server can inherit the lock (S18).
pub fn take_lock(dir: &AgentsDir) -> anyhow::Result<ExclusiveLock> {
    let path = dir.supervisor_lock();
    let shown = path.display();
    let file = open_private_nofollow(&path).map_err(|e| anyhow!("{shown}: {e}"))?;
    let lock = match lock_exclusive_timeout(file, Duration::ZERO) {
        Ok(lock) => lock,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            let held = read_bounded_nofollow(&path, MAX_LOCK_BYTES).ok().flatten();
            let pid = held.and_then(|(raw, _)| lock_pid(&raw));
            return Err(anyhow!(held_message(pid)));
        }
        Err(e) => return Err(anyhow!("{shown}: {e}")),
    };
    let mut file = lock.file();
    file.set_len(0)
        .and_then(|()| file.write_all(format!("{}\n", std::process::id()).as_bytes()))
        .map_err(|e| anyhow!("{shown}: {e}"))?;
    Ok(lock)
}

/// The pid in a lock file: a decimal u32 and an optional newline, in at most
/// 64 bytes.
pub fn lock_pid(raw: &[u8]) -> Option<u32> {
    if raw.len() as u64 > MAX_LOCK_BYTES {
        return None;
    }
    let digits = raw.strip_suffix(b"\n").unwrap_or(raw);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

pub fn held_message(pid: Option<u32>) -> String {
    let pid = pid.map_or_else(|| "unknown".to_string(), |p| p.to_string());
    format!("another amplihack agent supervisor is running (pid {pid})")
}

/// Runs `pass`, then sleeps `interval` and repeats until `stop` is set. With
/// `once`, the single pass's result is returned. Otherwise a failed pass is
/// printed and the loop goes on (A16).
pub fn run_loop(
    once: bool,
    interval: Duration,
    stop: &AtomicBool,
    sleep: &dyn Fn(Duration, &AtomicBool) -> bool,
    mut pass: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    loop {
        let result = pass();
        if once {
            return result;
        }
        if let Err(e) = result {
            eprintln!("error: {e:#}");
        }
        if stop.load(Ordering::SeqCst) || !sleep(interval, stop) {
            return Ok(());
        }
    }
}

/// Sleeps `d` in 200 ms slices; `false` as soon as `stop` is set.
pub fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(SLICE));
    }
}
