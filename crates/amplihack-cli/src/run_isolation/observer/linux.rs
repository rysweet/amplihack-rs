//! Linux ptrace supervision. Successful exec events are inspected while the
//! process is stopped, before its new executable can issue an index write.
//! This is process observation, not protection against same-user tampering.
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

/// A marker alone never authorizes observer reuse: the kernel must identify
/// that process's thread group as our active tracer.
///
/// Issue #2677: a traced process with no `AMPLIHACK_OBSERVER_PID` at all is
/// treated as having no parent observer. That cannot produce a false
/// "established" record. The direct path then asks for `PTRACE_TRACEME`:
/// if the tracer follows forks the kernel refuses it and the record says
/// "unavailable"; otherwise the child is untraced and is observed normally.
/// A variable that is present in any form (empty, non-UTF-8, padded,
/// signed, overflowing or naming another group) must still parse exactly
/// and name the tracer's thread group, or the run stops here.
pub(crate) fn parent_observer() -> Result<Option<u32>> {
    let status = fs::read_to_string("/proc/self/status")?;
    let tracer: u32 = status
        .lines()
        .find_map(|line| line.strip_prefix("TracerPid:")?.trim().parse().ok())
        .context("missing kernel tracer identity")?;
    if tracer == 0 {
        return Ok(None);
    }
    let Some(announced) = std::env::var_os("AMPLIHACK_OBSERVER_PID") else {
        return Ok(None);
    };
    let status = fs::read_to_string(format!("/proc/{tracer}/status"))?;
    let group: u32 = status
        .lines()
        .find_map(|line| line.strip_prefix("Tgid:")?.trim().parse().ok())
        .context("missing tracer thread group")?;
    if announced.to_str().and_then(|s| s.parse::<u32>().ok()) != Some(group) {
        bail!(
            "process is traced by an unrelated observer; cannot establish required recording ownership"
        );
    }
    Ok(Some(group))
}

/// The observer could not be set up for this attempt, so nothing about the
/// runner was observed: `PTRACE_TRACEME` was refused (the spawn failed with
/// exactly `EPERM` or `ENOSYS`). Issue #2677: the caller may then run the
/// attempt once unobserved and record coverage as unavailable. Every other
/// observer failure stays a plain error and fails closed.
#[derive(Debug)]
pub(crate) struct ObserverNotEstablished {
    /// The error number of the refused spawn, taken from its `io::Error`.
    pub(crate) errno: i32,
    pub(crate) error: anyhow::Error,
}

impl ObserverNotEstablished {
    pub(crate) fn errno(&self) -> i32 {
        self.errno
    }

    /// Fixed names for the only numbers `traceme_refused` accepts.
    pub(crate) fn errno_name(&self) -> &'static str {
        errno_name(self.errno)
    }
}

/// Fixed names for the only numbers `traceme_refused` accepts. No other
/// number can reach this, so it is never guessed.
pub(crate) fn errno_name(errno: i32) -> &'static str {
    match errno {
        libc::EPERM => "EPERM",
        libc::ENOSYS => "ENOSYS",
        _ => "unexpected",
    }
}

impl std::fmt::Display for ObserverNotEstablished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The inner chain is reached through `source`, so it is not
        // repeated here.
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for ObserverNotEstablished {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

/// The runner's own spawn failed for a reason other than a refused
/// `PTRACE_TRACEME` (for example a missing binary or working folder), after
/// the tracing thread returned cleanly. Nothing ran, so the caller may
/// discard this attempt's empty journal and report the spawn error alone.
/// Built only in the `Command::spawn` error arm; every other observer
/// failure stays a plain error and fails closed.
#[derive(Debug)]
pub(crate) struct RunnerNotStarted {
    pub(crate) error: anyhow::Error,
}

impl std::fmt::Display for RunnerNotStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for RunnerNotStarted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

/// Classified by error number only, never by message text. Exec errors
/// (`ENOENT`, `EACCES`, `ENOEXEC`, `E2BIG`, `ETXTBSY`, `ESRCH`) mean the
/// runner itself cannot start, so they are not an observer failure.
fn traceme_refused(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::ENOSYS))
}

pub(crate) struct Observer {
    result: mpsc::Receiver<Result<ExitStatus>>,
    thread: Option<JoinHandle<()>>,
    pids: Arc<Mutex<BTreeSet<i32>>>,
    finished_scopes: mpsc::Receiver<Vec<i32>>,
}

impl Observer {
    pub(crate) fn spawn(
        mut command: Command,
        path: PathBuf,
        run_id: String,
        attempt_id: String,
    ) -> Result<(Child, Self)> {
        if parent_observer()?.is_some() {
            return Self::spawn_relay(command, path, run_id, attempt_id);
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .context("create required observer journal")?;
        let identity = file.metadata()?;
        let identity = (identity.dev(), identity.ino());
        let (child_tx, child_rx) = mpsc::sync_channel(1);
        let (tx, result) = mpsc::sync_channel(1);
        let (scope_tx, finished_scopes) = mpsc::channel();
        let pids = Arc::new(Mutex::new(BTreeSet::new()));
        let observed_pids = Arc::clone(&pids);
        // ptrace ownership belongs to a thread. The same dedicated thread
        // must spawn, trace and reap; no unrelated Child::wait may reap it.
        let thread = std::thread::spawn(move || {
            // SAFETY: only the async-signal-safe ptrace syscall is used after
            // fork. A denied TRACEME becomes a spawn failure before exec.
            //
            // Hooks registered earlier by the caller (setsid, prctl
            // PR_SET_PDEATHSIG, getppid) run before this one. An EPERM from
            // them would be read below as a refused TRACEME. setsid cannot
            // fail in a freshly forked non-leader and PR_SET_PDEATHSIG does
            // not return EPERM, so no further check is made.
            unsafe {
                command.pre_exec(|| {
                    if libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let error = match error.raw_os_error() {
                        Some(errno) if traceme_refused(&error) => {
                            // The errno is the one this spawn returned; it is never
                            // read again or parsed from text.
                            let mut refused = ObserverNotEstablished {
                                errno,
                                error: anyhow::Error::new(error),
                            };
                            let context = format!(
                                "required Linux observer unavailable: PTRACE_TRACEME refused (errno {errno}, {})",
                                refused.errno_name()
                            );
                            refused.error = refused.error.context(context);
                            anyhow::Error::new(refused)
                        }
                        _ => anyhow::Error::new(RunnerNotStarted {
                            error: anyhow::Error::new(error)
                                .context("failed to spawn recipe-runner-rs"),
                        }),
                    };
                    let _ = child_tx.send(Err(error));
                    return;
                }
            };
            let root = child.id() as i32;
            observed_pids.lock().expect("observer pids").insert(root);
            if child_tx.send(Ok(child)).is_err() {
                // SAFETY: root is the stopped child just spawned here.
                unsafe {
                    libc::kill(root, libc::SIGKILL);
                }
            }
            let observed = observe(
                root,
                Journal {
                    file,
                    path,
                    identity,
                    run_id,
                    attempt_id,
                    acknowledgement: None,
                    complete: false,
                    root,
                    root_closed: false,
                },
                &observed_pids,
                &scope_tx,
            );
            if observed.is_err() {
                kill_all(&observed_pids);
                // Reap all traced tasks before reporting collector failure.
                reap_all(&observed_pids);
            }
            let _ = tx.send(observed);
        });
        let child = match child_rx.recv() {
            Ok(Ok(child)) => child,
            Ok(Err(error)) => {
                // The thread returns right after reporting a failed spawn.
                // Joined first, so nothing still holds the journal open.
                join_tracing_thread(thread)?;
                return Err(error);
            }
            Err(_) => {
                // The thread sends on every path except a panic, so a
                // dropped sender means it panicked, which the join reports.
                // A panicked thread may have spawned the runner before it
                // died, so this stays a plain error and fails closed. It is
                // deliberately not a RunnerNotStarted, so the caller never
                // discards this attempt's journal.
                join_tracing_thread(thread)?;
                return Err(anyhow::anyhow!(
                    "required observer exited before runner handoff"
                ));
            }
        };
        Ok((
            child,
            Self {
                result,
                thread: Some(thread),
                pids,
                finished_scopes,
            },
        ))
    }
    fn spawn_relay(
        mut command: Command,
        path: PathBuf,
        run_id: String,
        attempt_id: String,
    ) -> Result<(Child, Self)> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let mut descriptors = [0; 2];
        // SAFETY: descriptors is valid output storage. Both ends are owned
        // immediately and are closed on every error path.
        if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut read = unsafe { File::from_raw_fd(descriptors[0]) };
        let write = unsafe { File::from_raw_fd(descriptors[1]) };
        let fd = write.as_raw_fd();
        command
            .env("AMPLIHACK_OBSERVER_JOURNAL", &path)
            .env("AMPLIHACK_OBSERVER_RUN", run_id)
            .env("AMPLIHACK_OBSERVER_ATTEMPT", attempt_id)
            .env("AMPLIHACK_OBSERVER_ACK_FD", fd.to_string());
        // The ancestor tracer opens this pipe at the successful exec stop.
        // Its acknowledgement arrives only after every scoped task is reaped
        // and the scope's journal and receipt have been synchronized.
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .context("spawn independently owned run under verified ancestor observation")?;
        drop(command);
        drop(write);
        let root = child.id() as i32;
        let (tx, result) = mpsc::sync_channel(1);
        let pids = Arc::new(Mutex::new(BTreeSet::from([root])));
        let set = Arc::clone(&pids);
        let thread = std::thread::spawn(move || {
            let result = (|| -> Result<ExitStatus> {
                let mut status = 0;
                loop {
                    // This thread is the normal parent, not the tracer. Only
                    // final child status is consumed, never ptrace events.
                    if unsafe { libc::waitpid(root, &mut status, 0) } >= 0 {
                        break;
                    }
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(error.into());
                    }
                }
                set.lock().expect("relay pids").remove(&root);
                let mut ack = [0];
                read.read_exact(&mut ack)
                    .context("ancestor observer lost before scoped finalization")?;
                if ack != [1] {
                    bail!("ancestor observer rejected recording finalization");
                }
                Ok(ExitStatus::from_raw(status))
            })();
            let _ = tx.send(result);
        });
        let (_scope_tx, finished_scopes) = mpsc::channel();
        Ok((
            child,
            Self {
                result,
                thread: Some(thread),
                pids,
                finished_scopes,
            },
        ))
    }
    pub(crate) fn try_wait(&self) -> Result<Option<ExitStatus>> {
        match self.result.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("required observer was lost before finalization")
            }
        }
    }
    /// The tracing thread keeps consuming events while the existing CLI
    /// teardown policy gracefully stops a completed scope's remaining tasks.
    pub(crate) fn finished_scope(&self) -> Option<Vec<i32>> {
        self.finished_scopes.try_recv().ok()
    }
    pub(crate) fn terminate(&mut self) -> Result<()> {
        kill_all(&self.pids);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("required observer panicked during teardown"))?;
        }
        Ok(())
    }
}
/// Waits for a tracing thread that never handed a runner over.
fn join_tracing_thread(thread: JoinHandle<()>) -> Result<()> {
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("required observer panicked before runner handoff"))
}

impl Drop for Observer {
    fn drop(&mut self) {
        if let Err(error) = self.terminate() {
            eprintln!("amplihack: observer teardown failed: {error:#}");
        }
    }
}

struct Journal {
    file: File,
    path: PathBuf,
    identity: (u64, u64),
    run_id: String,
    attempt_id: String,
    acknowledgement: Option<File>,
    complete: bool,
    root: i32,
    root_closed: bool,
}
impl Journal {
    fn finish(&mut self) -> Result<()> {
        if self.complete {
            return Ok(());
        }
        verify_journal(&self.path, self.identity)?;
        self.file.sync_all()?;
        let receipt = json!({"schema_version":1,"coverage":"established","complete":true,"run_id":self.run_id,"attempt_id":self.attempt_id,"scope":"runner-descendants","backend":"linux-ptrace-exec"});
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path.with_extension("receipt"))?;
        serde_json::to_writer(&mut file, &receipt)?;
        file.sync_all()?;
        fs::File::open(self.path.parent().context("observer receipt parent")?)?.sync_all()?;
        if let Some(mut pipe) = self.acknowledgement.take() {
            pipe.write_all(&[1])?;
        }
        self.complete = true;
        Ok(())
    }
}
struct Scopes {
    journals: Vec<Journal>,
    tasks: BTreeMap<i32, Vec<usize>>,
}
impl Scopes {
    fn exec(&mut self, pid: i32) -> Result<()> {
        let bytes = capped_read(&PathBuf::from(format!("/proc/{pid}/environ")))?;
        let environment: BTreeMap<_, _> = bytes
            .split(|b| *b == 0)
            .filter_map(|entry| {
                let at = entry.iter().position(|b| *b == b'=')?;
                Some((
                    String::from_utf8_lossy(&entry[..at]).into_owned(),
                    String::from_utf8_lossy(&entry[at + 1..]).into_owned(),
                ))
            })
            .collect();
        if let Some(path) = environment.get("AMPLIHACK_OBSERVER_JOURNAL") {
            let path = PathBuf::from(path);
            let run_id = environment
                .get("AMPLIHACK_OBSERVER_RUN")
                .context("relay run identity")?;
            let attempt_id = environment
                .get("AMPLIHACK_OBSERVER_ATTEMPT")
                .context("relay attempt identity")?;
            let chain = self.tasks.get_mut(&pid).context("unowned observed task")?;
            if !chain.iter().any(|i| self.journals[*i].path == path) {
                // Registration is possible only from a stopped descendant,
                // and authorizes a new journal, never checkout reuse/cleanup.
                let parent = path.parent().context("relay journal parent")?;
                if !path.is_absolute()
                    || parent.file_name().is_none_or(|n| n != "git-trace2")
                    || path
                        .file_name()
                        .is_none_or(|n| n != format!("observer-{attempt_id}.jsonl").as_str())
                {
                    bail!("invalid independent observer journal destination");
                }
                let report_path = parent
                    .parent()
                    .context("relay run storage")?
                    .join("run.json");
                let report: serde_json::Value = serde_json::from_slice(
                    &crate::commands::cleanup::run_dir::read_capped_nofollow(&report_path, 65536)?,
                )?;
                if report["run_id"] != *run_id
                    || !report["attempts"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|a| a["correlation_id"] == *attempt_id))
                {
                    bail!("independent observer scope does not match its run/attempt record");
                }
                let fd: i32 = environment
                    .get("AMPLIHACK_OBSERVER_ACK_FD")
                    .context("relay acknowledgement descriptor")?
                    .parse()?;
                let pipe = fs::OpenOptions::new()
                    .write(true)
                    .open(format!("/proc/{pid}/fd/{fd}"))?;
                use std::os::unix::fs::FileTypeExt;
                if !pipe.metadata()?.file_type().is_fifo() {
                    bail!("relay acknowledgement is not a pipe");
                }
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                let metadata = file.metadata()?;
                chain.push(self.journals.len());
                self.journals.push(Journal {
                    file,
                    path,
                    identity: (metadata.dev(), metadata.ino()),
                    run_id: run_id.clone(),
                    attempt_id: attempt_id.clone(),
                    acknowledgement: Some(pipe),
                    complete: false,
                    root: pid,
                    root_closed: false,
                });
            }
        }
        for index in self
            .tasks
            .get(&pid)
            .context("missing observed task ownership")?
            .clone()
        {
            let journal = &mut self.journals[index];
            record_exec(
                pid,
                &mut journal.file,
                &journal.path,
                journal.identity,
                &journal.run_id,
                &journal.attempt_id,
            )?;
        }
        Ok(())
    }
    fn exited(&mut self, pid: i32, teardown: &mpsc::Sender<Vec<i32>>) -> Result<()> {
        self.tasks.remove(&pid);
        for (index, journal) in self.journals.iter_mut().enumerate() {
            if journal.root == pid {
                journal.root_closed = true;
                let remaining: Vec<_> = self
                    .tasks
                    .iter()
                    .filter_map(|(pid, chain)| chain.contains(&index).then_some(*pid))
                    .collect();
                if !remaining.is_empty() {
                    teardown
                        .send(remaining)
                        .context("observer teardown controller was lost")?;
                }
            }
            if index == 0 {
                continue;
            }
            if !self.tasks.values().any(|chain| chain.contains(&index)) {
                journal.finish()?;
            }
        }
        Ok(())
    }
}

fn observe(
    root: i32,
    journal: Journal,
    pids: &Arc<Mutex<BTreeSet<i32>>>,
    teardown: &mpsc::Sender<Vec<i32>>,
) -> Result<ExitStatus> {
    let mut root_status = None;
    let mut initialized = false;
    let mut pending_stops = BTreeMap::new();
    let mut exited_before_registration = BTreeSet::new();
    let mut groups = BTreeMap::from([(root, root)]);
    let mut scopes = Scopes {
        journals: vec![journal],
        tasks: BTreeMap::from([(root, vec![0])]),
    };
    while !pids.lock().expect("observer pids").is_empty() {
        let (pid, status) = wait_event().with_context(|| {
            format!(
                "observer task ownership: {:?}",
                pids.lock().expect("observer pids")
            )
        })?;
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            if !pids.lock().expect("observer pids").remove(&pid) {
                exited_before_registration.insert(pid);
            }
            groups.remove(&pid);
            scopes.exited(pid, teardown)?;
            if pid == root {
                root_status = Some(ExitStatus::from_raw(status));
            }
            continue;
        }
        if !libc::WIFSTOPPED(status) {
            bail!("unexpected observer wait status");
        }
        // Kernel wait notifications may report a newborn's initial stop
        // before its parent's fork/clone event. Do not let that task execute
        // (or exit) until its ownership chain has been registered.
        if !pids.lock().expect("observer pids").contains(&pid) {
            pending_stops.insert(pid, status);
            continue;
        }
        let signal = libc::WSTOPSIG(status);
        let event = status >> 16;
        let mut deliver = 0;
        if pid == root && !initialized {
            ptrace(
                libc::PTRACE_SETOPTIONS,
                pid,
                (libc::PTRACE_O_TRACEFORK
                    | libc::PTRACE_O_TRACEVFORK
                    | libc::PTRACE_O_TRACECLONE
                    | libc::PTRACE_O_TRACEEXEC
                    | libc::PTRACE_O_EXITKILL) as usize,
            )?;
            initialized = true;
            scopes.exec(pid)?;
        } else if [
            libc::PTRACE_EVENT_FORK,
            libc::PTRACE_EVENT_VFORK,
            libc::PTRACE_EVENT_CLONE,
        ]
        .contains(&event)
        {
            let child = event_message(pid)? as i32;
            if !exited_before_registration.remove(&child) {
                pids.lock().expect("observer pids").insert(child);
                let status = fs::read_to_string(format!("/proc/{child}/status"))?;
                let group = status
                    .lines()
                    .find_map(|line| line.strip_prefix("Tgid:")?.trim().parse::<i32>().ok())
                    .context("missing observed thread group")?;
                groups.insert(child, group);
                scopes.tasks.insert(
                    child,
                    scopes
                        .tasks
                        .get(&pid)
                        .context("missing fork observation ownership")?
                        .clone(),
                );
                if scopes.tasks[&child]
                    .iter()
                    .any(|index| scopes.journals[*index].root_closed)
                {
                    teardown
                        .send(vec![child])
                        .context("observer teardown controller was lost")?;
                }
                if let Some(status) = pending_stops.remove(&child) {
                    if libc::WSTOPSIG(status) != libc::SIGSTOP {
                        bail!("unexpected newborn observation stop");
                    }
                    ptrace(libc::PTRACE_CONT, child, 0)?;
                }
            }
        } else if event == libc::PTRACE_EVENT_EXEC {
            // exec from a nonleader thread changes its tid to the leader pid.
            let old = event_message(pid)? as i32;
            // A nonleader exec destroys the rest of its thread group before
            // this event. Those tids need not all produce separate waits.
            let retired: Vec<_> = groups
                .iter()
                .filter_map(|(task, group)| (*group == pid && *task != pid).then_some(*task))
                .collect();
            let chain = scopes
                .tasks
                .get(&old)
                .or_else(|| scopes.tasks.get(&pid))
                .cloned()
                .context("missing exec observation ownership")?;
            scopes.tasks.insert(pid, chain);
            let mut set = pids.lock().expect("observer pids");
            set.remove(&old);
            set.insert(pid);
            for task in retired {
                set.remove(&task);
                groups.remove(&task);
                scopes.tasks.remove(&task);
                pending_stops.remove(&task);
            }
            groups.insert(pid, pid);
            drop(set);
            if old != pid {
                scopes.tasks.remove(&old);
            }
            scopes.exec(pid)?;
        } else if signal != libc::SIGSTOP && signal != libc::SIGTRAP {
            deliver = signal;
        }
        ptrace(libc::PTRACE_CONT, pid, deliver as usize)?;
    }
    for journal in &mut scopes.journals {
        journal.finish()?;
    }
    root_status.context("observer lost runner exit status")
}
fn ptrace(request: libc::c_uint, pid: i32, data: usize) -> Result<()> {
    // SAFETY: request uses no pointer output; pid is a stopped tracee.
    if unsafe { libc::ptrace(request, pid, 0, data) } == -1 {
        return Err(std::io::Error::last_os_error()).context("required process observation failed");
    }
    Ok(())
}
fn event_message(pid: i32) -> Result<usize> {
    let mut message = 0usize;
    // SAFETY: message points to initialized writable storage of the ABI size.
    if unsafe { libc::ptrace(libc::PTRACE_GETEVENTMSG, pid, 0, &mut message) } == -1 {
        return Err(std::io::Error::last_os_error()).context("read observed process relationship");
    }
    Ok(message)
}
fn wait_event() -> Result<(i32, i32)> {
    loop {
        let mut status = 0;
        // SAFETY: status is writable. __WNOTHREAD excludes other CLI threads'
        // subprocesses; __WALL includes all traced clone children.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::__WALL | libc::__WNOTHREAD) };
        if pid >= 0 {
            return Ok((pid, status));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).context("wait for required process observation");
        }
    }
}
fn kill_all(pids: &Arc<Mutex<BTreeSet<i32>>>) {
    for pid in pids.lock().expect("observer pids").iter() {
        // SAFETY: the set contains unreaped tracees owned by this tracer.
        unsafe {
            libc::kill(*pid, libc::SIGKILL);
        }
    }
}
fn reap_all(pids: &Arc<Mutex<BTreeSet<i32>>>) {
    while !pids.lock().expect("observer pids").is_empty() {
        match wait_event() {
            Ok((pid, status)) if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) => {
                pids.lock().expect("observer pids").remove(&pid);
            }
            Ok((pid, _)) => {
                // SAFETY: resume a stopped, terminating tracee with SIGKILL.
                unsafe {
                    libc::ptrace(libc::PTRACE_CONT, pid, 0, libc::SIGKILL);
                }
            }
            Err(error) => {
                eprintln!("amplihack: observer could not reap all descendants: {error:#}");
                break;
            }
        }
    }
}
fn verify_journal(path: &Path, identity: (u64, u64)) -> Result<()> {
    let meta = fs::symlink_metadata(path).context("required observer journal disappeared")?;
    if !meta.is_file() || (meta.dev(), meta.ino()) != identity {
        bail!("required observer journal identity changed; partial evidence retained");
    }
    Ok(())
}
fn capped_read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("observed process metadata exceeds 1 MiB bound");
    }
    Ok(bytes)
}
fn record_exec(
    pid: i32,
    journal: &mut File,
    path: &Path,
    identity: (u64, u64),
    run_id: &str,
    attempt_id: &str,
) -> Result<()> {
    verify_journal(path, identity)?;
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let executable =
        fs::read_link(proc.join("exe")).context("inspect successful executable identity")?;
    let name = executable
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name != "git" && !name.starts_with("git-") {
        return Ok(());
    }
    let raw = capped_read(&proc.join("cmdline"))?;
    let argv: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    // Unknown commands (including aliases and shell aliases) are retained.
    let mut i = 1;
    let mut options = Vec::new();
    while i < argv.len() && argv[i].starts_with('-') {
        options.push(argv[i].clone());
        if [
            "-C",
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--config-env",
        ]
        .contains(&argv[i].as_str())
        {
            i += 1;
            options.push(
                argv.get(i)
                    .context("incomplete observed Git global option")?
                    .clone(),
            );
        }
        i += 1;
    }
    let sub = argv.get(i).map(String::as_str).unwrap_or("");
    if [
        "rev-parse",
        "symbolic-ref",
        "ls-files",
        "show",
        "log",
        "cat-file",
        "version",
        "--version",
        "config",
    ]
    .contains(&sub)
    {
        return Ok(());
    }
    let cwd = fs::read_link(proc.join("cwd"))?;
    let environment = capped_read(&proc.join("environ"))?;
    let env: BTreeMap<String, String> = environment
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let at = entry.iter().position(|b| *b == b'=')?;
            Some((
                String::from_utf8_lossy(&entry[..at]).into_owned(),
                String::from_utf8_lossy(&entry[at + 1..]).into_owned(),
            ))
        })
        .collect();
    let mut routing = BTreeMap::new();
    let mut probe = amplihack_git::command_in(&cwd);
    probe.args(&options);
    // Explicit launch routing wins over the scrub, exactly as it did for
    // the observed executable. No values other than routing are persisted.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        routing.insert(key, env.get(key).cloned());
        if let Some(value) = env.get(key) {
            probe.env(key, value);
        }
    }
    let output = probe
        .args([
            "-c",
            "core.fsmonitor=false",
            "rev-parse",
            "--path-format=absolute",
            "--absolute-git-dir",
            "--git-path",
            "index",
        ])
        .output()?;
    if !output.status.success() {
        bail!("cannot attribute an observed Git command to its repository/index");
    }
    let paths = String::from_utf8(output.stdout)?;
    let mut paths = paths.lines();
    let git_dir = paths.next().context("missing observed git directory")?;
    let index_file = paths.next().context("missing observed index path")?;
    let time = chrono::Utc::now().to_rfc3339();
    let redacted = super::redact(&argv);
    let entry = json!({"schema_version":1,"run_id":run_id,"attempt_id":attempt_id,"executable":executable,"cwd":cwd,"git_dir":git_dir,"index_file":index_file,"index_path":index_file,"git_environment":routing,"argv":redacted,"command":redacted,"time":time,"timestamp":time,"sid":format!("{attempt_id}/{pid}"),"pid":pid,"source":"linux-ptrace-exec"});
    let mut bytes = serde_json::to_vec(&entry)?;
    bytes.push(b'\n');
    if bytes.len() > 1024 * 1024
        || journal.metadata()?.len().saturating_add(bytes.len() as u64) > 64 * 1024 * 1024
    {
        bail!("required observer journal exceeded its storage bound before command execution");
    }
    journal.write_all(&bytes)?;
    journal
        .sync_data()
        .context("persist required observed Git command before resuming execution")?;
    Ok(())
}

#[cfg(test)]
mod tests_not_established {
    use super::*;

    #[test]
    fn only_eperm_and_enosys_mean_traceme_was_refused() {
        for errno in [libc::EPERM, libc::ENOSYS] {
            assert!(traceme_refused(&std::io::Error::from_raw_os_error(errno)));
        }
        for errno in [
            libc::ENOENT,
            libc::EACCES,
            libc::ENOEXEC,
            libc::E2BIG,
            libc::ETXTBSY,
            libc::ESRCH,
        ] {
            assert!(!traceme_refused(&std::io::Error::from_raw_os_error(errno)));
        }
        assert!(!traceme_refused(&std::io::Error::other("no errno")));
    }

    #[test]
    fn not_established_keeps_the_cause_chain_once() {
        // M2: the text names the refused call and its errno, and states no
        // cause the code cannot know.
        let inner = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EPERM)).context(
            "required Linux observer unavailable: PTRACE_TRACEME refused (errno 1, EPERM)",
        );
        let error = anyhow::Error::new(ObserverNotEstablished {
            errno: libc::EPERM,
            error: inner,
        });
        let text = format!("{error:#}");
        assert_eq!(
            text.matches("PTRACE_TRACEME refused (errno 1, EPERM)")
                .count(),
            1,
            "{text}"
        );
        assert!(!text.contains("nested tracing"), "{text}");
        let marker = error
            .downcast_ref::<ObserverNotEstablished>()
            .expect("the marker is the outermost error");
        assert_eq!(marker.errno(), libc::EPERM);
        assert_eq!(marker.errno_name(), "EPERM");
        assert!(
            error
                .chain()
                .filter_map(|e| e.downcast_ref::<std::io::Error>())
                .any(|e| e.raw_os_error() == Some(libc::EPERM)),
            "{text}"
        );
    }

    #[test]
    fn errno_names_cover_only_the_refused_traceme_numbers() {
        let named = |errno| {
            ObserverNotEstablished {
                errno,
                error: anyhow::anyhow!("refused"),
            }
            .errno_name()
        };
        assert_eq!(named(libc::EPERM), "EPERM");
        assert_eq!(named(libc::ENOSYS), "ENOSYS");
        // Unreachable through traceme_refused; never a guessed name.
        assert_eq!(named(libc::ENOENT), "unexpected");
    }

    #[test]
    fn runner_not_started_keeps_the_spawn_errno_and_says_nothing_of_ptrace() {
        // B1d: a runner that cannot start reports its own error, with the
        // io::Error as the root cause, so its errno is not lost.
        let inner = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT))
            .context("failed to spawn recipe-runner-rs");
        let error = anyhow::Error::new(RunnerNotStarted { error: inner });
        let text = format!("{error:#}");
        assert_eq!(
            text.matches("failed to spawn recipe-runner-rs").count(),
            1,
            "{text}"
        );
        for word in ["PTRACE", "ptrace", "tracing", "observer"] {
            assert!(!text.contains(word), "{word}: {text}");
        }
        assert_eq!(
            error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ENOENT),
            "{text}"
        );
        assert!(error.downcast_ref::<ObserverNotEstablished>().is_none());
    }
}
