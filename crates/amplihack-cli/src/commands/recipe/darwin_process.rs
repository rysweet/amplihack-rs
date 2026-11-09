//! Darwin process identities from the SDK's libproc BSD information API.
//! Group membership is never ownership: a launcher can share its caller's group.

use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;
use std::mem::{MaybeUninit, size_of};

#[derive(Clone, Copy, Debug)]
pub(super) struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub start: u64,
    pub uid: u32,
    pub pgid: i32,
    pub zombie: bool,
}

// sys/proc_info.h: proc_bsdinfo (PROC_PIDTBSDINFO = 3). Keep the complete
// SDK layout, including reserved fields, so short/failed reads fail closed.
#[repr(C)]
struct BsdInfo {
    flags: u32,
    status: u32,
    xstatus: u32,
    pid: u32,
    ppid: u32,
    uid: u32,
    gid: u32,
    ruid: u32,
    rgid: u32,
    svuid: u32,
    svgid: u32,
    reserved: u32,
    comm: [u8; 16],
    name: [u8; 32],
    nfiles: u32,
    pgid: u32,
    jobc: u32,
    tdev: u32,
    tpgid: u32,
    nice: i32,
    start_sec: u64,
    start_usec: u64,
}

// sys/proc_info.h: proc_bsdshortinfo (PROC_PIDT_SHORTBSDINFO = 13).
// XNU permits this ancestry-only query across UIDs. It carries no start
// identity and must never authorize a signal or admit a cleanup target.
#[repr(C)]
struct ShortBsdInfo {
    pid: u32,
    ppid: u32,
    pgid: u32,
    status: u32,
    comm: [u8; 16],
    flags: u32,
    uid: u32,
    gid: u32,
    ruid: u32,
    rgid: u32,
    svuid: u32,
    svgid: u32,
    reserved: u32,
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut libc::c_void, size: i32) -> i32;
    fn proc_listpids(kind: u32, info: u32, buffer: *mut libc::c_void, size: i32) -> i32;
}

/// Only a known zombie or ESRCH is absence; permission/ABI failures are errors.
pub(super) fn inspect(pid: i32) -> Result<Option<Proc>> {
    Ok(identity(pid)?.filter(|p| !p.zombie))
}

/// PROC_PIDTBSDINFO's arg=1 includes a waitable zombie. Used only to prove
/// ownership while the launcher still holds its unreaped Child.
pub(super) fn identity(pid: i32) -> Result<Option<Proc>> {
    if pid <= 1 {
        return Ok(None);
    }
    let mut info = MaybeUninit::<BsdInfo>::zeroed();
    errno::set_errno(errno::Errno(0));
    // SAFETY: the SDK function receives a correctly sized writable structure.
    let read = unsafe {
        proc_pidinfo(
            pid,
            3,
            1,
            info.as_mut_ptr().cast(),
            size_of::<BsdInfo>() as i32,
        )
    };
    if !complete_read::<BsdInfo>(pid, read, std::io::Error::last_os_error())? {
        return Ok(None);
    }
    // SAFETY: libproc filled the entire structure.
    let info = unsafe { info.assume_init() };
    ensure!(
        info.pid == pid as u32 && info.start_sec > 0 && info.start_usec < 1_000_000,
        "invalid Darwin identity for {pid}"
    );
    Ok(Some(Proc {
        pid,
        ppid: info.ppid as i32,
        start: info
            .start_sec
            .checked_mul(1_000_000)
            .and_then(|v| v.checked_add(info.start_usec))
            .context("Darwin start time overflow")?,
        uid: info.uid,
        pgid: info.pgid as i32,
        zombie: info.status == 5,
    }))
}

fn complete_read<T>(pid: i32, read: i32, error: std::io::Error) -> Result<bool> {
    if read == size_of::<T>() as i32 {
        return Ok(true);
    }
    if read <= 0 && error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(error).with_context(|| format!("cannot inspect Darwin process {pid} (read {read})"))
}

pub(super) fn same(pid: i32, start: u64) -> bool {
    matches(pid, start).unwrap_or(false)
}

pub(super) fn matches(pid: i32, start: u64) -> Result<bool> {
    Ok(inspect(pid)?.is_some_and(|p| p.start == start && p.uid == unsafe { libc::geteuid() }))
}

fn ancestry(pid: i32) -> Result<Option<ShortBsdInfo>> {
    let mut info = MaybeUninit::<ShortBsdInfo>::zeroed();
    errno::set_errno(errno::Errno(0));
    // SAFETY: the SDK function receives a correctly sized writable structure.
    let read = unsafe {
        proc_pidinfo(
            pid,
            13,
            0,
            info.as_mut_ptr().cast(),
            size_of::<ShortBsdInfo>() as i32,
        )
    };
    if !complete_read::<ShortBsdInfo>(pid, read, std::io::Error::last_os_error())? {
        return Ok(None);
    }
    // SAFETY: libproc filled the entire structure.
    let info = unsafe { info.assume_init() };
    ensure!(info.pid == pid as u32, "invalid Darwin ancestry for {pid}");
    Ok(Some(info))
}

pub(super) fn protected() -> Result<BTreeSet<i32>> {
    let mut found = BTreeSet::new();
    let mut pid = std::process::id() as i32;
    while pid > 1 && found.insert(pid) {
        let Some(p) = ancestry(pid)? else { break };
        pid = p.ppid as i32;
    }
    Ok(found)
}

fn children(pid: i32) -> Result<Vec<i32>> {
    list(6, pid)
}

fn list(kind: u32, pid: i32) -> Result<Vec<i32>> {
    // Leave room for children born between the size query and the read.
    // PROC_PPID_ONLY=6 returns BYTES, unlike proc_listchildpids's PID count.
    errno::set_errno(errno::Errno(0));
    let size = unsafe { proc_listpids(kind, pid as u32, std::ptr::null_mut(), 0) };
    let error = errno::errno().0;
    ensure!(
        error == 0 && (0..=4 * 1024 * 1024).contains(&size),
        "cannot enumerate children of {pid}"
    );
    let mut pids = vec![0i32; size as usize / size_of::<i32>() + 256];
    errno::set_errno(errno::Errno(0));
    let read = unsafe {
        proc_listpids(
            kind,
            pid as u32,
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<i32>()) as i32,
        )
    };
    let error = errno::errno().0;
    ensure!(
        error == 0
            && read >= 0
            && (read as usize) < pids.len() * size_of::<i32>()
            && (read as usize).is_multiple_of(size_of::<i32>()),
        "incomplete child enumeration for {pid}"
    );
    pids.truncate(read as usize / size_of::<i32>());
    ensure!(
        pids.iter().all(|pid| *pid > 1),
        "invalid child enumeration for {pid}"
    );
    Ok(pids)
}

/// Prove the Child is still owned without reaping it. Returns true when an
/// exit is pending, so the owner can capture its last children before reaping.
pub(super) fn waitable(child: &std::process::Child) -> Result<bool> {
    loop {
        let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            // SAFETY: waitid succeeded; the zeroed si_pid remains zero when
            // WNOHANG found no exit, otherwise XNU filled the exit record.
            let info = unsafe { info.assume_init() };
            ensure!(
                info.si_pid == 0 || info.si_pid == child.id() as i32,
                "unexpected waitid runner identity"
            );
            return Ok(info.si_pid != 0);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(error).context("runner is not an owned waitable child");
    }
}

/// Capture the launcher's private session before consuming ANY wait. The
/// caller holds the actual Child and is its sole reaper. Group provenance is
/// valid only while waitid(WNOWAIT) and the original leader identity both
/// prove the session ID is pinned; nothing is signalled by group number.
pub(super) fn unreaped_group(child: &std::process::Child, start: u64) -> Result<super::stop::Tree> {
    fn leader(child: &std::process::Child, start: u64) -> Result<()> {
        let pid = child.id() as i32;
        let p = identity(pid)?.context("unreaped runner identity unavailable")?;
        ensure!(
            p.pid == pid
                && p.pgid == pid
                && p.start == start
                && p.ppid == std::process::id() as i32
                && p.uid == unsafe { libc::geteuid() },
            "unreaped private runner session identity changed"
        );
        Ok(())
    }
    waitable(child)?;
    leader(child, start)?;
    let mut members = super::stop::Tree::new();
    for pid in list(2, child.id() as i32)? {
        if let Some(p) = inspect(pid)?
            && p.pgid == child.id() as i32
            && p.uid == unsafe { libc::geteuid() }
            && p.start >= start
        {
            members.insert(p.pid, p.start);
        }
    }
    let staged = tree_bound(&members)?;
    waitable(child)?;
    leader(child, start)?;
    Ok(staged)
}

pub(super) fn require_waitable_children() -> Result<()> {
    let mut action = MaybeUninit::<libc::sigaction>::zeroed();
    ensure!(
        unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()) } == 0,
        "cannot inspect SIGCHLD disposition"
    );
    let action = unsafe { action.assume_init() };
    ensure!(
        action.sa_sigaction == libc::SIG_DFL && action.sa_flags & libc::SA_NOCLDWAIT == 0,
        "recipe run: SIGCHLD must use its default disposition and retain waitable children for safe Darwin teardown"
    );
    Ok(())
}

pub(super) fn tree(roots: &BTreeSet<i32>) -> Result<super::stop::Tree> {
    let mut bound = super::stop::Tree::new();
    for pid in roots {
        if let Some(root) = inspect(*pid)? {
            bound.insert(root.pid, root.start);
        }
    }
    tree_bound(&bound)
}

pub(super) fn tree_bound(roots: &super::stop::Tree) -> Result<super::stop::Tree> {
    let protected = protected()?;
    let uid = unsafe { libc::geteuid() };
    let mut found = super::stop::Tree::new();
    let mut pending = Vec::new();
    for (pid, start) in roots {
        if let Some(root) = inspect(*pid)?
            && root.start == *start
        {
            pending.push(root)
        }
    }
    while let Some(parent) = pending.pop() {
        ensure!(found.len() < 16384, "run process identity limit exceeded");
        if parent.uid != uid
            || !matches(parent.pid, parent.start)?
            || found.contains_key(&parent.pid)
        {
            continue;
        }
        // Protect caller/ancestors without following their other children.
        if protected.contains(&parent.pid) {
            continue;
        }
        found.insert(parent.pid, parent.start);
        for pid in children(parent.pid)? {
            if let Some(child) = inspect(pid)?
                && child.ppid == parent.pid
                && child.uid == uid
                && child.start >= parent.start
                && matches(parent.pid, parent.start)?
            {
                pending.push(child);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_identity_is_start_bound_and_protects_caller() {
        assert_eq!(size_of::<BsdInfo>(), 136);
        assert_eq!(size_of::<ShortBsdInfo>(), 64);
        let pid = std::process::id() as i32;
        let me = inspect(pid).unwrap().unwrap();
        assert!(matches(pid, me.start).unwrap());
        assert!(!matches(pid, me.start + 1).unwrap());
        assert!(
            tree_bound(&super::super::stop::Tree::from([(pid, me.start)]))
                .unwrap()
                .is_empty()
        );
        assert!(protected().unwrap().contains(&pid));
    }

    #[test]
    fn short_ancestry_query_can_read_root_launchd() {
        let launchd = ancestry(1).unwrap().unwrap();
        assert_eq!(launchd.pid, 1);
        assert_eq!(launchd.uid, 0);
        // The full identity query remains strict even though ancestry can
        // cross this boundary. PID 1 is normally the first foreign UID a
        // developer without privileges can reliably exercise.
        if unsafe { libc::geteuid() } != 0 {
            let mut info = MaybeUninit::<BsdInfo>::zeroed();
            errno::set_errno(errno::Errno(0));
            let read = unsafe {
                proc_pidinfo(
                    1,
                    3,
                    0,
                    info.as_mut_ptr().cast(),
                    size_of::<BsdInfo>() as i32,
                )
            };
            assert_eq!(read, 0);
            assert_eq!(errno::errno().0, libc::EPERM);
        }
    }

    /// Run the native unit-test binary as root with
    /// `--ignored cross_uid_ancestor_does_not_block_private_group_capture`.
    #[test]
    #[ignore = "requires root to create a nonroot launcher beneath a root-owned supervisor"]
    fn cross_uid_ancestor_does_not_block_private_group_capture() {
        use std::os::unix::process::CommandExt;
        const SUPERVISOR: &str = "AMPLIHACK_TEST_ROOT_SUPERVISOR";
        if let Ok(supervisor) = std::env::var(SUPERVISOR) {
            let supervisor: i32 = supervisor.parse().unwrap();
            assert_ne!(unsafe { libc::geteuid() }, 0);
            assert_eq!(unsafe { libc::getppid() }, supervisor);
            assert_eq!(ancestry(supervisor).unwrap().unwrap().uid, 0);
            let error = inspect(supervisor).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(libc::EPERM)
            );
            assert!(protected().unwrap().contains(&supervisor));
            // Exercise the startup capture, including tree_bound's ancestor
            // protection, from beneath the foreign-UID parent.
            private_group_capture_requires_original_waitable_leader();
            return;
        }
        assert_eq!(unsafe { libc::geteuid() }, 0, "run this fixture as root");
        let test_name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--ignored", "--nocapture"])
            .env(SUPERVISOR, std::process::id().to_string())
            .gid(65534)
            .uid(65534)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "nonroot fixture failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }

    #[test]
    fn unreadable_is_never_mistaken_for_terminal() {
        for (read, error) in [
            (0, libc::EPERM),
            (0, libc::EACCES),
            (8, libc::ESRCH),
            (0, 0),
        ] {
            assert!(
                complete_read::<BsdInfo>(42, read, std::io::Error::from_raw_os_error(error))
                    .is_err()
            );
            assert!(
                complete_read::<ShortBsdInfo>(42, read, std::io::Error::from_raw_os_error(error))
                    .is_err()
            );
        }
        assert!(
            !complete_read::<BsdInfo>(42, 0, std::io::Error::from_raw_os_error(libc::ESRCH))
                .unwrap()
        );
        assert!(complete_read::<BsdInfo>(42, 136, std::io::Error::from_raw_os_error(0)).unwrap());
        assert!(
            complete_read::<ShortBsdInfo>(42, 64, std::io::Error::from_raw_os_error(0)).unwrap()
        );
    }

    #[test]
    fn private_group_capture_requires_original_waitable_leader() {
        use std::os::unix::process::CommandExt;
        struct OwnedChild(std::process::Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("300");
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = OwnedChild(command.spawn().unwrap());
        let root = identity(child.0.id() as i32).unwrap().unwrap();
        assert!(!waitable(&child.0).unwrap());
        assert!(unreaped_group(&child.0, root.start + 1).is_err());
        assert!(child.0.try_wait().unwrap().is_none());
        let found = unreaped_group(&child.0, root.start).unwrap();
        assert_eq!(found.get(&root.pid), Some(&root.start));
        child.0.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !waitable(&child.0).unwrap() {
            assert!(std::time::Instant::now() < deadline, "runner did not exit");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            waitable(&child.0).unwrap(),
            "exit observation must not reap"
        );
        assert!(identity(root.pid).unwrap().unwrap().zombie);
        unreaped_group(&child.0, root.start).unwrap();
        child.0.wait().unwrap();
        assert!(
            unreaped_group(&child.0, root.start).is_err(),
            "reaped PID is never group authority"
        );
    }
}
