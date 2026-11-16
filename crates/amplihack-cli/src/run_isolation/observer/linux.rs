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
/// negative, overflowing or naming another group) must still parse as a
/// `u32` and name the tracer's thread group, or the run stops here.
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
    errno: i32,
    error: anyhow::Error,
}

impl ObserverNotEstablished {
    pub(crate) fn errno(&self) -> i32 {
        self.errno
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
    error: anyhow::Error,
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

/// Issue #2716: a ptrace request in the observation loop failed because its
/// task has gone. `ESRCH` on a stopped tracee is the documented result when
/// that tracee died between the stop and the request — a thread of a group
/// whose leader called `exit_group`, a tracee killed with SIGKILL, or a tid
/// retired by a sibling's exec — which is ordinary in a traced tree of agent,
/// node, cargo and shell processes, not a broken observation.
///
/// This is the one deliberate fail-open case in a fail-closed design. It is
/// safe only because no site that classifies a task as gone resumes it, so a
/// task misclassified while alive stays in its ptrace-stop and executes
/// nothing, and because no exit status is ever synthesized: the runner's
/// comes from a real wait status, with `observer lost runner exit status` as
/// the backstop. A change that resumes a task on a classifying path, or
/// registers one before the site that may classify it, invalidates that.
///
/// Classified by error number only, never message text: a tracee can
/// influence strings that surface in an error chain. The number read is the
/// one [`PtraceRequestFailed`] carries, which only a ptrace request in this
/// module can produce, so an `ESRCH` from anything else — notably the `/proc`
/// reads in [`Scopes::exec`] and [`record_exec`] — cannot be mistaken for
/// this however it is wrapped. The full rationale and its limits are in
/// `docs/reference/run-isolation-vanished-observed-tasks.md`.
fn vanished_task(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<PtraceRequestFailed>()
        .and_then(|failed| failed.error.raw_os_error())
        == Some(libc::ESRCH)
}

/// Issue #2716: a failed ptrace request, carrying its own `errno` in the type
/// so [`vanished_task`] never has to walk an error chain to find one. Walking
/// a chain is what would let an `ESRCH` raised by something other than the
/// request being classified — a `/proc` read at an exec stop, say — open the
/// one fail-open path in this module. Only [`ptrace`] and [`event_message`]
/// construct this, so nothing else can.
///
/// Displays as the request's own context text, with the `io::Error` as its
/// source, so `{:#}` and [`anyhow::Error::root_cause`] read exactly as the
/// `.context(…)` chain this replaced.
#[derive(Debug)]
struct PtraceRequestFailed {
    context: &'static str,
    error: std::io::Error,
}

impl std::fmt::Display for PtraceRequestFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.context)
    }
}

impl std::error::Error for PtraceRequestFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A `/proc/<tid>` read failed because that task has gone: `ENOENT` for a
/// retired entry, `ESRCH` for a task that died with the read in flight.
///
/// Kept separate from [`vanished_task`] so `ENOENT` can never be accepted
/// from a ptrace request. Used at one site only — the newborn thread-group
/// read, whose subject is still at its initial `SIGSTOP` and has therefore
/// executed nothing that is not already recorded. The `environ` read in
/// [`Scopes::exec`] and the `exe` read-link in [`record_exec`] sit at an exec
/// stop, where that argument does not hold, and stay fatal.
fn vanished_proc_entry(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::ENOENT))
}

/// Builds the line reporting one task that had gone: fixed text, the task,
/// and a closed-vocabulary request name. No error chain, path, command line,
/// working directory or environment value reaches it — the observer reads
/// `environ` and `cmdline`, which hold tokens and private paths, and run
/// stderr reaches CI logs and retained evidence folders. One line names one
/// call site, which is what makes it diagnostic alone.
fn vanished_warning(task: i32, request: &'static str) -> String {
    format!("amplihack: warning: observed task {task} was gone at {request}")
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
                            let context = format!(
                                "required Linux observer unavailable: PTRACE_TRACEME refused (errno {errno}, {})",
                                errno_name(errno)
                            );
                            anyhow::Error::new(ObserverNotEstablished {
                                errno,
                                error: anyhow::Error::new(error).context(context),
                            })
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
                    vanished: 0,
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
                // Joined first, so nothing still holds the journal open. A
                // join that fails is reported, never returned: this attempt's
                // own cause is what the run must name.
                return Err(spawn_error_outranks_join_failure(
                    error,
                    join_tracing_thread(thread),
                ));
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
    /// The tasks this observer still holds unreaped: the set `terminate`
    /// kills. The thread that reaps a task removes it from the set as soon as
    /// its wait returns, so a task listed here has not been free for reuse
    /// any longer than that. A runner's own pid, by contrast, stays in its
    /// `Child` after the runner has been reaped.
    pub(crate) fn traced_tasks(&self) -> BTreeSet<i32> {
        self.pids.lock().expect("observer pids").clone()
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

/// Builds the line reporting a tracing thread that did not exit cleanly after
/// the runner failed to start. Fixed text plus the join's own chain: no path,
/// command line or environment value reaches it, and it carries neither the
/// spawn context nor the word `nested`, so it is never mistaken for the cause
/// the caller returns.
fn join_failure_warning(failure: &anyhow::Error) -> String {
    format!(
        "amplihack: warning: the observer thread did not exit cleanly after the runner \
         failed to start: {failure:#}"
    )
}

/// Issue #2677 A6: the runner's own spawn failure outranks a failed join of
/// the tracing thread. The thread is joined so nothing still holds the
/// journal open, but replacing `spawn` with the join's error would lose the
/// attempt's errno and its [`RunnerNotStarted`] marker, and the run would
/// again name something other than its own cause. The join failure is
/// reported on stderr; `spawn` is returned exactly as it stands, never
/// re-contextualised.
fn spawn_error_outranks_join_failure(spawn: anyhow::Error, join: Result<()>) -> anyhow::Error {
    if let Err(failure) = join {
        eprintln!("{}", join_failure_warning(&failure));
    }
    spawn
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
    /// Issue #2716: how many tasks this scope found gone. A count, never a
    /// list of tasks: the receipt is read back under a 4096-byte cap that
    /// fails above it, so a list a traced tree can inflate would hand that
    /// tree a new way to fail the run.
    vanished: u64,
}
impl Journal {
    fn finish(&mut self) -> Result<()> {
        if self.complete {
            return Ok(());
        }
        verify_journal(&self.path, self.identity)?;
        self.file.sync_all()?;
        // The seven keys the recording reader checks are unchanged. It
        // compares named keys and ignores any other, so the count rides
        // along here rather than in `observer-*.jsonl`, where every line is
        // validated as a Git command and a new shape would make a lost task
        // a run failure every time.
        let receipt = json!({"schema_version":1,"coverage":"established","complete":true,"run_id":self.run_id,"attempt_id":self.attempt_id,"scope":"runner-descendants","backend":"linux-ptrace-exec","vanished":self.vanished});
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
                    vanished: 0,
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
    /// Issue #2716: records one task the observation loop found gone.
    /// Infallible on purpose — this runs on a path that must continue, so it
    /// has no failure to report and nothing here may end the run.
    ///
    /// The count goes on every journal in `owner`'s chain. Every chain holds
    /// index 0, so `journals[0].vanished` is the run's exact total and a relay
    /// scope counts only the tasks it owned. An owner whose chain is already
    /// gone still counts on the run, so no task is lost from the evidence.
    ///
    /// The warning is capped while the count is not: a traced tree can create
    /// unbounded tasks, so an uncapped per-task line is log amplification and
    /// a way to fill a disk. The cap bounds the lines such a tree can make us
    /// write, never the evidence.
    fn vanished(&mut self, task: i32, owner: i32, request: &'static str) {
        /// Warning lines one run may write about tasks that had gone.
        const WARNINGS: u64 = 16;
        let chain = self
            .tasks
            .get(&owner)
            .cloned()
            .unwrap_or_else(|| vec![0usize]);
        for index in chain {
            if let Some(journal) = self.journals.get_mut(index) {
                journal.vanished = journal.vanished.saturating_add(1);
            }
        }
        if let Some(run) = self.journals.first()
            && run.vanished <= WARNINGS
        {
            eprintln!("{}", vanished_warning(task, request));
        }
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
            // Issue #2716: a stop this task never had resumed cannot outlive
            // it. Left behind, a stale entry would resume a reused pid whose
            // stop was never consumed, and the task that reused the number
            // would be the one counted as gone.
            pending_stops.remove(&pid);
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
            if let Err(error) = ptrace(
                libc::PTRACE_SETOPTIONS,
                pid,
                (libc::PTRACE_O_TRACEFORK
                    | libc::PTRACE_O_TRACEVFORK
                    | libc::PTRACE_O_TRACECLONE
                    | libc::PTRACE_O_TRACEEXEC
                    | libc::PTRACE_O_EXITKILL) as usize,
            ) {
                // Issue #2716: the runner itself has gone before its own
                // options were set. It is left unresumed and unregistered
                // for observation, so it executes nothing further; its own
                // wait status still arrives and is still what the run
                // reports. Never an early return: taking a status from here
                // would be inventing one.
                if !vanished_task(&error) {
                    return Err(error);
                }
                scopes.vanished(pid, pid, "PTRACE_SETOPTIONS");
                continue;
            }
            initialized = true;
            scopes.exec(pid)?;
        } else if [
            libc::PTRACE_EVENT_FORK,
            libc::PTRACE_EVENT_VFORK,
            libc::PTRACE_EVENT_CLONE,
        ]
        .contains(&event)
        {
            let child = match event_message(pid) {
                Ok(message) => message as i32,
                Err(error) => {
                    // Issue #2716: the parent has gone before it could say
                    // which task it had just created. The newborn is left at
                    // its initial SIGSTOP, unregistered, so it executes
                    // nothing; its own stop is held in `pending_stops` and
                    // released when it dies with its parent's group.
                    if !vanished_task(&error) {
                        return Err(error);
                    }
                    scopes.vanished(pid, pid, "PTRACE_GETEVENTMSG (fork)");
                    continue;
                }
            };
            // The newborn's identity is a kernel word cast to a tid and
            // interpolated into a `/proc` path. An implausible value used to
            // fail loudly on ENOENT; that answer is now read as "gone", so
            // it is checked here instead and still fails the run.
            if child <= 0 {
                bail!("implausible observed newborn identity");
            }
            if !exited_before_registration.remove(&child) {
                // Issue #2716, registration order: the newborn's identity is
                // read before it is registered anywhere. A newborn abandoned
                // below therefore leaves no registration behind, so if it is
                // in fact alive its initial SIGSTOP still lands in the
                // unregistered-stop branch above and it is never resumed.
                // Registering first would instead leave it in `pids` with no
                // ownership chain, skip that branch, and let the resume at
                // the foot of this loop run it unjournaled.
                'newborn: {
                    let group = match fs::read_to_string(format!("/proc/{child}/status")) {
                        Ok(status) => status
                            .lines()
                            .find_map(|line| line.strip_prefix("Tgid:")?.trim().parse::<i32>().ok())
                            .context("missing observed thread group")?,
                        Err(error) => {
                            if !vanished_proc_entry(&error) {
                                return Err(anyhow::Error::new(error))
                                    .context("read observed newborn thread group");
                            }
                            // Still at its initial SIGSTOP, so it has run no
                            // code at all and nothing can be missing from
                            // the evidence. The parent is resumed below.
                            scopes.vanished(child, pid, "thread group identity");
                            break 'newborn;
                        }
                    };
                    pids.lock().expect("observer pids").insert(child);
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
                        if let Err(error) = ptrace(libc::PTRACE_CONT, child, 0) {
                            // Issue #2716: the newborn has gone. Its
                            // registration stays, so its own wait
                            // notification reaps it and removes it through
                            // the one existing exit path. What must not
                            // happen is losing the parent's resume below:
                            // this arm is handling the parent's stop, and
                            // continuing the loop here would leave the
                            // parent stopped for good. Falling out of the
                            // block does exactly that.
                            if !vanished_task(&error) {
                                return Err(error);
                            }
                            scopes.vanished(child, pid, "PTRACE_CONT (newborn)");
                        }
                    }
                }
            }
        } else if event == libc::PTRACE_EVENT_EXEC {
            // exec from a nonleader thread changes its tid to the leader pid.
            let old = match event_message(pid) {
                Ok(message) => message as i32,
                Err(error) => {
                    if !vanished_task(&error) {
                        return Err(error);
                    }
                    // Issue #2716: the task that execed has gone, so its
                    // previous tid can no longer be asked for. Its thread
                    // group was already destroyed by that exec and those
                    // tids produce no further wait, so they are retired
                    // here: left in `pids`, they would keep this loop
                    // waiting for tasks the kernel will never report.
                    let mut set = pids.lock().expect("observer pids");
                    retire_thread_group(
                        pid,
                        &mut set,
                        &mut groups,
                        &mut scopes,
                        &mut pending_stops,
                    );
                    drop(set);
                    scopes.vanished(pid, pid, "PTRACE_GETEVENTMSG (exec)");
                    continue;
                }
            };
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
            retire_thread_group(pid, &mut set, &mut groups, &mut scopes, &mut pending_stops);
            groups.insert(pid, pid);
            drop(set);
            if old != pid {
                scopes.tasks.remove(&old);
            }
            scopes.exec(pid)?;
        } else if signal != libc::SIGSTOP && signal != libc::SIGTRAP {
            deliver = signal;
        }
        if let Err(error) = ptrace(libc::PTRACE_CONT, pid, deliver as usize) {
            // Issue #2716: the task whose stop this was has gone. It is left
            // unresumed, which it already is, so nothing it had not already
            // recorded can run. Its own wait notification still arrives and
            // removes it from the observed set through the one existing exit
            // path, and if it was the runner its status comes from there.
            if !vanished_task(&error) {
                return Err(error);
            }
            scopes.vanished(pid, pid, "PTRACE_CONT");
        }
    }
    for journal in &mut scopes.journals {
        journal.finish()?;
    }
    root_status.context("observer lost runner exit status")
}
/// Drops every nonleader tid of `leader`'s thread group from all four places
/// the observation loop tracks a task. A nonleader exec destroys the rest of
/// its thread group before the exec event arrives, and those tids need not
/// each produce a separate wait, so a tid left behind would keep the loop
/// waiting for a task the kernel will never report. The leader itself always
/// stays.
fn retire_thread_group(
    leader: i32,
    pids: &mut BTreeSet<i32>,
    groups: &mut BTreeMap<i32, i32>,
    scopes: &mut Scopes,
    pending_stops: &mut BTreeMap<i32, i32>,
) {
    let retired: Vec<_> = groups
        .iter()
        .filter_map(|(task, group)| (*group == leader && *task != leader).then_some(*task))
        .collect();
    for task in retired {
        pids.remove(&task);
        groups.remove(&task);
        scopes.tasks.remove(&task);
        pending_stops.remove(&task);
    }
}
fn ptrace(request: libc::c_uint, pid: i32, data: usize) -> Result<()> {
    // SAFETY: request uses no pointer output; pid is a stopped tracee.
    if unsafe { libc::ptrace(request, pid, 0, data) } == -1 {
        return Err(anyhow::Error::new(PtraceRequestFailed {
            context: "required process observation failed",
            error: std::io::Error::last_os_error(),
        }));
    }
    Ok(())
}
fn event_message(pid: i32) -> Result<usize> {
    let mut message = 0usize;
    // SAFETY: message points to initialized writable storage of the ABI size.
    if unsafe { libc::ptrace(libc::PTRACE_GETEVENTMSG, pid, 0, &mut message) } == -1 {
        return Err(anyhow::Error::new(PtraceRequestFailed {
            context: "read observed process relationship",
            error: std::io::Error::last_os_error(),
        }));
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
        assert_eq!(errno_name(marker.errno()), "EPERM");
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
        assert_eq!(errno_name(libc::EPERM), "EPERM");
        assert_eq!(errno_name(libc::ENOSYS), "ENOSYS");
        // Unreachable through traceme_refused; never a guessed name.
        assert_eq!(errno_name(libc::ENOENT), "unexpected");
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

    // -----------------------------------------------------------------
    // Issue #2677 A6: when the tracing thread has already reported a failed
    // runner spawn, `Observer::spawn` joins it so nothing still holds the
    // journal open. A join that fails must not become the error the caller
    // sees: `join_tracing_thread(thread)?` would replace the spawn error,
    // losing its errno and its `RunnerNotStarted` marker, and the run would
    // again name something other than its own cause. The join failure is
    // reported separately; the spawn error is returned as it stands.
    //
    // `spawn_error_outranks_join_failure(spawn, join)` is that rule: it
    // reports a failed `join` and returns `spawn` unchanged.
    // -----------------------------------------------------------------

    fn unstarted_spawn_error() -> anyhow::Error {
        anyhow::Error::new(RunnerNotStarted {
            error: anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT))
                .context("failed to spawn recipe-runner-rs"),
        })
    }

    #[test]
    fn a_failed_join_never_displaces_the_spawn_error() {
        let error = spawn_error_outranks_join_failure(
            unstarted_spawn_error(),
            Err(anyhow::anyhow!(
                "required observer panicked before runner handoff"
            )),
        );
        let text = format!("{error:#}");
        assert!(
            error.downcast_ref::<RunnerNotStarted>().is_some(),
            "the attempt still never started, so its journal is still released: {text}"
        );
        assert_eq!(
            error.chain().next().map(ToString::to_string).as_deref(),
            Some("failed to spawn recipe-runner-rs"),
            "{text}"
        );
        assert_eq!(
            text.matches("failed to spawn recipe-runner-rs").count(),
            1,
            "{text}"
        );
        assert_eq!(
            error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ENOENT),
            "the spawn's errno survives the join failure: {text}"
        );
        for word in ["panicked", "did not finalize", "PTRACE", "tracing"] {
            assert!(!text.contains(word), "{word}: {text}");
        }
    }

    #[test]
    fn a_successful_join_leaves_the_spawn_error_alone() {
        let error = spawn_error_outranks_join_failure(unstarted_spawn_error(), Ok(()));
        let text = format!("{error:#}");
        assert!(error.downcast_ref::<RunnerNotStarted>().is_some(), "{text}");
        assert_eq!(
            error.chain().next().map(ToString::to_string).as_deref(),
            Some("failed to spawn recipe-runner-rs"),
            "{text}"
        );
        assert_eq!(
            text.matches("failed to spawn recipe-runner-rs").count(),
            1,
            "{text}"
        );
    }

    #[test]
    fn a_join_failure_warning_carries_its_fixed_prefix_and_no_path() {
        // The join failure is reported, never returned, so what it carries is
        // asserted against the built line rather than captured from stderr.
        const PREFIX: &str = "amplihack: warning: the observer thread did not exit cleanly after the runner \
             failed to start: ";
        let failure = anyhow::anyhow!("required observer panicked before runner handoff");
        let line = join_failure_warning(&failure);
        assert!(line.starts_with(PREFIX), "{line}");
        assert!(
            line.contains("required observer panicked before runner handoff"),
            "the join's own reason is what the line is for: {line}"
        );
        assert!(
            !line.contains('/'),
            "no path reaches the line, so it can be quoted unread: {line}"
        );
        assert!(!line.contains('\n'), "one warning is one line: {line:?}");
        for word in ["failed to spawn recipe-runner-rs", "nested"] {
            assert!(
                !line.contains(word),
                "{word} belongs to the returned error, not this warning: {line}"
            );
        }
    }
}

#[cfg(test)]
mod tests_vanished_task {
    //! Issue #2716: a ptrace request in the observation loop that fails with
    //! `ESRCH` means that task has gone, not that the observation broke. The
    //! eight field failures on 1.2.167 all ended with
    //! "required process observation failed: No such process (os error 3)".
    //!
    //! These tests pin the classification, the recording and the two helpers
    //! the per-site control flow depends on. The loop's own behaviour is
    //! covered by `tests/issue_2716_vanished_observed_task.rs`.
    use super::*;

    /// Every name `Scopes::vanished` may be given, one per call site. One
    /// warning line names one site, so a stderr line is diagnostic alone.
    const SITES: [&str; 6] = [
        "PTRACE_SETOPTIONS",
        "PTRACE_GETEVENTMSG (fork)",
        "PTRACE_GETEVENTMSG (exec)",
        "thread group identity",
        "PTRACE_CONT (newborn)",
        "PTRACE_CONT",
    ];

    /// What a failed request in the loop hands to the classifier: exactly
    /// what the `ptrace` helper returns.
    fn ptrace_failure(errno: i32) -> anyhow::Error {
        anyhow::Error::new(PtraceRequestFailed {
            context: "required process observation failed",
            error: std::io::Error::from_raw_os_error(errno),
        })
    }

    fn journal(dir: &Path, attempt_id: &str, root: i32) -> Journal {
        let path = dir.join(format!("observer-{attempt_id}.jsonl"));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .unwrap();
        let identity = file.metadata().unwrap();
        Journal {
            file,
            path,
            identity: (identity.dev(), identity.ino()),
            run_id: "run-2716".to_owned(),
            attempt_id: attempt_id.to_owned(),
            acknowledgement: None,
            complete: false,
            root,
            root_closed: false,
            vanished: 0,
        }
    }

    #[test]
    fn only_esrch_means_a_ptrace_request_found_the_task_gone() {
        assert!(vanished_task(&ptrace_failure(libc::ESRCH)));
        for errno in [
            libc::EPERM,
            libc::EIO,
            libc::EINVAL,
            libc::ENOENT,
            libc::EBUSY,
            libc::EFAULT,
        ] {
            assert!(
                !vanished_task(&ptrace_failure(errno)),
                "errno {errno} stays fatal; this change opens exactly one hole"
            );
        }
        assert!(
            !vanished_task(&anyhow::anyhow!("no errno")),
            "an error with no error number is not a task that has gone"
        );
        // Classified by error number only. A tracee can influence strings
        // that surface in an error chain, so no tracee-reachable text may
        // steer a control decision.
        assert!(
            !vanished_task(
                &anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EPERM))
                    .context("No such process (os error 3)")
            ),
            "the message text must never decide this"
        );
    }

    #[test]
    fn only_a_ptrace_request_of_this_module_can_report_a_task_gone() {
        // The errno read is the one the failed request carries, so an ESRCH
        // raised by anything else cannot open the fail-open path however it
        // is wrapped. `Scopes::exec` reads `/proc/<tid>/environ` at an exec
        // stop and can itself see ESRCH; that read must stay fatal, because
        // at an exec stop the new image is loaded and a task resumed there
        // would run unjournaled.
        let proc_read = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ESRCH))
            .context("read observed process environment");
        assert!(
            !vanished_task(&proc_read),
            "an ESRCH that is not a ptrace request's own must stay fatal"
        );
        assert!(
            !vanished_task(&proc_read.context("required process observation failed")),
            "nor may wrapping it in a request's context text make it one"
        );
        // And the shape the helpers really return still reads as the
        // `.context(…)` chain it replaced, so the field message and the
        // `root_cause` errno are unchanged.
        let failure = ptrace_failure(libc::ESRCH);
        assert_eq!(
            format!("{failure:#}"),
            "required process observation failed: No such process (os error 3)"
        );
        assert_eq!(
            failure
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn a_proc_entry_is_gone_only_for_esrch_or_enoent() {
        for errno in [libc::ESRCH, libc::ENOENT] {
            assert!(vanished_proc_entry(&std::io::Error::from_raw_os_error(
                errno
            )));
        }
        for errno in [
            libc::EPERM,
            libc::EACCES,
            libc::EIO,
            libc::EINVAL,
            libc::EMFILE,
        ] {
            assert!(
                !vanished_proc_entry(&std::io::Error::from_raw_os_error(errno)),
                "errno {errno} is not a task that has gone"
            );
        }
        assert!(!vanished_proc_entry(&std::io::Error::other("no errno")));
        // The two predicates stay separate for exactly this reason: a
        // ptrace request never means "gone" on ENOENT.
        assert!(!vanished_task(&ptrace_failure(libc::ENOENT)));
    }

    #[test]
    fn the_receipt_counts_the_tasks_that_were_gone() {
        for (gone, attempt) in [(3u64, "attempt-a"), (0, "attempt-b")] {
            let dir = tempfile::tempdir().unwrap();
            let mut journal = journal(dir.path(), attempt, 11);
            journal.vanished = gone;
            journal.finish().unwrap();
            let receipt: serde_json::Value = serde_json::from_slice(
                &fs::read(dir.path().join(format!("observer-{attempt}.receipt"))).unwrap(),
            )
            .unwrap();
            // The seven keys the recording reader checks, unchanged: a new
            // shape here would make a lost task a run failure every time.
            assert_eq!(receipt["schema_version"], 1, "{receipt}");
            assert_eq!(receipt["coverage"], "established", "{receipt}");
            assert_eq!(receipt["complete"], true, "{receipt}");
            assert_eq!(receipt["run_id"], "run-2716", "{receipt}");
            assert_eq!(receipt["attempt_id"], attempt, "{receipt}");
            assert_eq!(receipt["scope"], "runner-descendants", "{receipt}");
            assert_eq!(receipt["backend"], "linux-ptrace-exec", "{receipt}");
            // A count, never a list of tasks: the receipt is read under a
            // 4096-byte cap that fails above it, so a list a traced tree can
            // inflate would hand that tree a new way to fail the run.
            assert_eq!(receipt["vanished"], serde_json::json!(gone), "{receipt}");
            assert!(receipt["vanished"].is_u64(), "{receipt}");
            assert_eq!(
                receipt.as_object().unwrap().len(),
                8,
                "the receipt gained a key no reader was told about: {receipt}"
            );
        }
    }

    #[test]
    fn every_task_that_was_gone_is_counted_on_the_run_journal() {
        let dir = tempfile::tempdir().unwrap();
        let mut scopes = Scopes {
            journals: vec![
                journal(dir.path(), "run", 11),
                journal(dir.path(), "relay", 21),
            ],
            tasks: BTreeMap::from([(11, vec![0]), (21, vec![0, 1]), (22, vec![0, 1])]),
        };
        scopes.vanished(12, 11, "PTRACE_CONT");
        scopes.vanished(22, 21, "PTRACE_CONT (newborn)");
        // Every chain holds index 0, so journal 0 carries the run's total.
        assert_eq!(scopes.journals[0].vanished, 2);
        assert_eq!(
            scopes.journals[1].vanished, 1,
            "a relay scope counts only the tasks it owned"
        );
        // An owner with no chain left still counts on the run, so a task
        // that goes after its own group was retired is never lost.
        scopes.vanished(99, 99, "PTRACE_SETOPTIONS");
        assert_eq!(scopes.journals[0].vanished, 3);
        assert_eq!(scopes.journals[1].vanished, 1);
        // The count stays exact past the warning cap: the cap bounds the
        // lines a traced tree can make us write, not the evidence.
        for _ in 0..40 {
            scopes.vanished(31, 11, "PTRACE_CONT");
        }
        assert_eq!(scopes.journals[0].vanished, 43);
    }

    #[test]
    fn retiring_a_thread_group_drops_every_nonleader_tid_everywhere() {
        // A nonleader exec destroys the rest of its thread group before the
        // event arrives, and those tids produce no further wait. Leaving any
        // of them behind would keep the loop waiting for a task the kernel
        // will never report, so the vanished path at the exec stop retires
        // them through this one helper before it continues.
        let dir = tempfile::tempdir().unwrap();
        let mut pids = BTreeSet::from([11, 12, 13, 21]);
        let mut groups = BTreeMap::from([(11, 11), (12, 11), (13, 11), (21, 21)]);
        let mut scopes = Scopes {
            journals: vec![journal(dir.path(), "run", 11)],
            tasks: BTreeMap::from([(11, vec![0]), (12, vec![0]), (13, vec![0]), (21, vec![0])]),
        };
        let mut pending_stops = BTreeMap::from([(12, 0x57f), (21, 0x57f)]);
        retire_thread_group(11, &mut pids, &mut groups, &mut scopes, &mut pending_stops);
        assert_eq!(pids, BTreeSet::from([11, 21]), "the leader itself stays");
        assert_eq!(groups, BTreeMap::from([(11, 11), (21, 21)]));
        assert_eq!(
            scopes.tasks.keys().copied().collect::<Vec<_>>(),
            vec![11, 21]
        );
        assert_eq!(pending_stops, BTreeMap::from([(21, 0x57f)]));
    }

    #[test]
    fn a_vanished_warning_names_one_site_and_carries_no_path_or_argv() {
        const PREFIX: &str = "amplihack: warning: observed task ";
        let mut seen = BTreeSet::new();
        for site in SITES {
            let line = vanished_warning(4242, site);
            assert!(line.starts_with(PREFIX), "{line}");
            assert!(
                line.contains("4242"),
                "the task is what the line identifies: {line}"
            );
            assert!(line.contains(&format!("was gone at {site}")), "{line}");
            assert!(!line.contains('\n'), "one warning is one line: {line:?}");
            // The observer reads environ and cmdline, which hold tokens and
            // private paths, and run stderr reaches CI logs and retained
            // evidence folders. Nothing but the task and the site may pass.
            assert!(
                !line.contains('/'),
                "no path reaches the line, so it can be quoted unread: {line}"
            );
            assert!(
                seen.insert(line.clone()),
                "two sites share a warning line, so a line names no site: {site}"
            );
        }
    }
}
