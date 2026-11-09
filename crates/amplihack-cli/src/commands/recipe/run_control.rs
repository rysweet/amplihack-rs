//! A live, identity-bound stop handle for ordinary Darwin recipe launches.
//! A record alone never authorizes a signal: the client must authenticate the
//! live Unix peer and the launcher's start identity before requesting its tree.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::{darwin_process, stop::Tree};

const LIMIT: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Record {
    version: u8,
    run_id: String,
    pid: i32,
    start: u64,
    uid: u32,
    socket: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct TerminalInventory {
    tree: Tree,
    error: Option<String>,
}

fn records() -> Result<PathBuf> {
    Ok(super::home_dir()?.join(".amplihack/recipe-runs"))
}

fn record_path(run_id: &str) -> Result<PathBuf> {
    // Run IDs are generated UUIDs, never user-controlled paths.
    let id = uuid::Uuid::parse_str(run_id).context("recipe stop: expected a run UUID")?;
    ensure!(
        id.to_string() == run_id,
        "recipe stop: expected a canonical run UUID"
    );
    Ok(records()?.join(format!("{run_id}.json")))
}

fn owned_private(path: &std::path::Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    private_metadata(&meta, path, directory)
}

fn private_metadata(meta: &fs::Metadata, path: &std::path::Path, directory: bool) -> Result<()> {
    ensure!(
        meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o077 == 0
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            },
        "recipe stop: insecure ownership or permissions on {}",
        path.display()
    );
    Ok(())
}

/// Created before workload starts; dropped on every completion/failure path.
pub(super) struct LaunchControl {
    record: PathBuf,
    socket_dir: tempfile::TempDir,
    listener: Option<UnixListener>,
    done: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    runner: Option<darwin_process::Proc>,
    known: Arc<Mutex<Tree>>,
    error: Arc<Mutex<Option<String>>>,
}

impl LaunchControl {
    pub(super) fn prepare(run_id: &str) -> Result<Self> {
        darwin_process::require_waitable_children()?;
        let parent = records()?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&parent)?;
        owned_private(&parent, true)?;
        let socket_dir = tempfile::Builder::new().prefix("as-").tempdir()?;
        let socket = socket_dir.path().join("s");
        ensure!(
            socket.as_os_str().as_encoded_bytes().len() < 104,
            "recipe run: TMPDIR is too long for a Darwin stop socket; choose a shorter TMPDIR"
        );
        let listener =
            UnixListener::bind(&socket).context("recipe run: cannot bind stop socket")?;
        listener.set_nonblocking(true)?;
        let me = darwin_process::inspect(std::process::id() as i32)?
            .context("recipe run: cannot verify Darwin launch identity")?;
        let record = Record {
            version: 1,
            run_id: run_id.into(),
            pid: me.pid,
            start: me.start,
            uid: me.uid,
            socket,
        };
        let path = record_path(run_id)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.sync_all()?;
        Ok(Self {
            record: path,
            socket_dir,
            listener: Some(listener),
            done: Arc::new(AtomicBool::new(false)),
            thread: None,
            runner: None,
            known: Arc::new(Mutex::new(Tree::new())),
            error: Arc::new(Mutex::new(None)),
        })
    }

    pub(super) fn start(&mut self, run_id: &str, child: &std::process::Child) -> Result<()> {
        // A valid micro-recipe can finish before this inspection. Its normal
        // wait/parse path still succeeds; there is no live workload to stop.
        let Some(runner) = darwin_process::identity(child.id() as i32)? else {
            return Ok(());
        };
        self.runner = Some(runner);
        self.refresh_unreaped(child)?;
        let listener = self.listener.take().expect("start control once");
        let done = Arc::clone(&self.done);
        let run_id = run_id.to_owned();
        let known = Arc::clone(&self.known);
        let error = Arc::clone(&self.error);
        self.thread = Some(thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let result = (|| -> Result<()> {
                            configure_connection(&stream)?;
                            ensure!(
                                peer_uid(&stream)? == unsafe { libc::geteuid() },
                                "foreign stop peer"
                            );
                            let request = read_line(&stream)?;
                            ensure!(request == run_id, "wrong run ID");
                            let tree = refresh_known(runner, &known)?;
                            serde_json::to_writer(&mut stream, &tree)?;
                            stream.write_all(b"\n")?;
                            // The client first receives the attributed tree, then authorizes
                            // cancellation. A dropped connection does not cancel a recipe.
                            ensure!(
                                read_line(&stream)? == "stop",
                                "missing stop acknowledgement"
                            );
                            ensure!(!done.load(Ordering::SeqCst), "attempt already completed");
                            super::run::stop_signal::request();
                            stream.write_all(b"stopping\n")?;
                            // Keep the authenticated connection until the owner
                            // finishes teardown. Include identities observed after
                            // the first snapshot, before the client reports success.
                            let deadline = std::time::Instant::now() + Duration::from_secs(60);
                            while !done.load(Ordering::SeqCst) {
                                ensure!(
                                    std::time::Instant::now() < deadline,
                                    "owner teardown timed out"
                                );
                                thread::sleep(Duration::from_millis(20));
                            }
                            let final_tree = known
                                .lock()
                                .map_err(|_| anyhow::anyhow!("poisoned run identity tracker"))?
                                .clone();
                            let error = error
                                .lock()
                                .map_err(|_| anyhow::anyhow!("poisoned inventory error"))?
                                .clone();
                            serde_json::to_writer(
                                &mut stream,
                                &TerminalInventory {
                                    tree: final_tree,
                                    error,
                                },
                            )?;
                            stream.write_all(b"\n")?;
                            Ok(())
                        })();
                        if let Err(error) = result {
                            tracing::debug!(%error, "Darwin stop request refused");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        }));
        Ok(())
    }

    pub(super) fn refresh(&self) -> Result<()> {
        if let Some(runner) = self.runner
            && let Err(error) = refresh_known(runner, &self.known)
        {
            self.mark_error(&error);
            return Err(error);
        }
        Ok(())
    }

    /// Owner-thread only, including after WNOWAIT observes the leader's exit
    /// and before Child::wait reaps it. This pins private-group authority.
    /// The control-server thread never expands a process group.
    pub(super) fn refresh_unreaped(&self, child: &std::process::Child) -> Result<()> {
        if let Some(runner) = self.runner {
            let capture = darwin_process::unreaped_group(child, runner.start);
            match capture {
                Ok(staged) => {
                    let mut known = self
                        .known
                        .lock()
                        .map_err(|_| anyhow::anyhow!("poisoned run identity tracker"))?;
                    known.extend(staged);
                }
                Err(error) => {
                    self.mark_error(&error);
                    return Err(error);
                }
            }
        }
        self.refresh()
    }

    pub(super) fn mark_error(&self, error: &anyhow::Error) {
        if let Ok(mut recorded) = self.error.lock() {
            recorded.get_or_insert_with(|| format!("{error:#}"));
        }
    }

    /// Retained identities survive reparenting and a reaped runner; a new
    /// process assigned its PID is never admitted by group number.
    pub(super) fn teardown(&self, grace: Duration) -> Result<()> {
        // Failed attribution must not prevent cleanup of identities captured
        // earlier while their ownership was verifiable.
        let refresh = self.refresh();
        let tree = self
            .known
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned run identity tracker"))?
            .clone();
        super::stop::terminate(&tree);
        super::stop::sweep(&tree, grace);
        let terminal = super::stop::verify_terminal(&tree, Duration::from_secs(1));
        if let Err(error) = &terminal {
            self.mark_error(error);
        }
        refresh.and(terminal)
    }
}

fn refresh_known(runner: darwin_process::Proc, known: &Mutex<Tree>) -> Result<Tree> {
    let mut known = known
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned run identity tracker"))?;
    let mut roots = live_identities(&known, darwin_process::matches)?;
    if darwin_process::matches(runner.pid, runner.start)? {
        roots.insert(runner.pid, runner.start);
    }
    let found = darwin_process::tree_bound(&roots)?;
    for (pid, start) in found {
        known.insert(pid, start);
    }
    ensure!(known.len() <= 16384, "run process identity limit exceeded");
    Ok(known.clone())
}

impl Drop for LaunchControl {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.record);
        // The directory (and socket) is removed by TempDir.
        let _ = &self.socket_dir;
    }
}

fn read_line(stream: &UnixStream) -> Result<String> {
    // Read exactly one message: discarding a temporary BufReader could lose
    // a second response already buffered with the acknowledgement.
    let deadline =
        std::time::Instant::now() + stream.read_timeout()?.unwrap_or(Duration::from_secs(3));
    let mut input = stream;
    let mut line = Vec::new();
    while line.len() < LIMIT as usize {
        ensure!(
            std::time::Instant::now() < deadline,
            "stop protocol message timed out"
        );
        let mut byte = [0];
        input.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            return String::from_utf8(line).context("invalid stop protocol encoding");
        }
        line.push(byte[0]);
    }
    bail!("stop protocol message exceeds size limit")
}

fn configure_connection(stream: &UnixStream) -> Result<()> {
    // BSD accept inherits the listener's O_NONBLOCK; protocol I/O must use
    // bounded blocking timeouts even when the request has not arrived yet.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(())
}

fn peer_pid(stream: &UnixStream) -> Result<i32> {
    let mut pid: i32 = 0;
    let mut size = std::mem::size_of_val(&pid) as libc::socklen_t;
    // sys/un.h: SOL_LOCAL=0, LOCAL_PEERPID=2. This is the kernel's Unix
    // connection peer identity, independent of the editable registry record.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            0,
            2,
            (&mut pid as *mut i32).cast(),
            &mut size,
        )
    };
    ensure!(
        result == 0 && size as usize == std::mem::size_of_val(&pid) && pid > 1,
        "recipe stop: cannot authenticate Unix peer PID"
    );
    Ok(pid)
}

fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    ensure!(
        unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0,
        "recipe stop: cannot authenticate Unix peer UID"
    );
    Ok(uid)
}

pub(super) fn request_stop(run_id: &str) -> Result<(Tree, (i32, u64), UnixStream)> {
    let parent = records()?;
    owned_private(&parent, true).context("recipe stop: no private run registry")?;
    let path = record_path(run_id)?;
    owned_private(&path, false).context("recipe stop: no recorded live run")?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let meta = file.metadata()?;
    private_metadata(&meta, &path, false)?;
    ensure!(meta.len() <= LIMIT, "recipe stop: oversized launch record");
    let record: Record = serde_json::from_reader(file.take(LIMIT))?;
    ensure!(
        record.version == 1
            && record.run_id == run_id
            && record.uid == unsafe { libc::geteuid() }
            && darwin_process::matches(record.pid, record.start)?,
        "recipe stop: stale or unverifiable launch identity for {run_id}"
    );
    ensure!(
        !darwin_process::protected()?.contains(&record.pid),
        "recipe stop: refusing to stop this process or an ancestor"
    );
    let mut stream = UnixStream::connect(&record.socket)
        .context("recipe stop: live owner endpoint unavailable")?;
    // Darwin rejects timeout mutation after peer close, even while its final
    // inventory remains buffered. Configure all I/O before any request.
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    ensure!(
        peer_pid(&stream)? == record.pid
            && peer_uid(&stream)? == record.uid
            && darwin_process::matches(record.pid, record.start)?,
        "recipe stop: endpoint does not match recorded launch identity"
    );
    writeln!(stream, "{run_id}")?;
    let tree = read_tree(&stream)?;
    if tree.is_empty() {
        bail!("recipe stop: no verifiable running recipe processes")
    }
    stream.write_all(b"stop\n")?;
    ensure!(
        read_line(&stream)? == "stopping",
        "recipe stop: cancellation was not acknowledged"
    );
    Ok((tree, (record.pid, record.start), stream))
}

fn read_tree(stream: &UnixStream) -> Result<Tree> {
    let tree: Tree = serde_json::from_str(&read_line(stream)?)?;
    let protected = darwin_process::protected()?;
    let mut verified = live_identities(&tree, darwin_process::matches)?;
    verified.retain(|pid, _| !protected.contains(pid));
    Ok(verified)
}

fn live_identities(tree: &Tree, mut inspect: impl FnMut(i32, u64) -> Result<bool>) -> Result<Tree> {
    let mut live = Tree::new();
    for (pid, start) in tree {
        if inspect(*pid, *start)? {
            live.insert(*pid, *start);
        }
    }
    Ok(live)
}

pub(super) fn final_tree(stream: &UnixStream) -> Result<Tree> {
    let inventory: TerminalInventory = serde_json::from_str(&read_line(stream)?)
        .context("recipe stop: owner did not provide a terminal process inventory")?;
    ensure!(
        inventory.error.is_none(),
        "recipe stop: owner could not verify complete process inventory: {}",
        inventory.error.as_deref().unwrap_or_default()
    );
    let protected = darwin_process::protected()?;
    let mut tree = live_identities(&inventory.tree, darwin_process::matches)?;
    tree.retain(|pid, _| !protected.contains(pid));
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_error_does_not_erase_attributed_identity() {
        let known = Tree::from([(42, 100), (43, 200)]);
        let result = live_identities(&known, |pid, _| {
            if pid == 43 {
                Err(std::io::Error::from_raw_os_error(libc::EPERM).into())
            } else {
                Ok(true)
            }
        });
        assert!(
            result.is_err(),
            "unverifiable must fail the entire inventory"
        );
        assert_eq!(known, Tree::from([(42, 100), (43, 200)]));
        assert_eq!(
            live_identities(&known, |pid, _| Ok(pid == 42)).unwrap(),
            Tree::from([(42, 100)])
        );
    }

    #[test]
    fn adjacent_protocol_responses_are_not_discarded() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        writer.write_all(b"stopping\n{}\n").unwrap();
        assert_eq!(read_line(&reader).unwrap(), "stopping");
        assert_eq!(read_line(&reader).unwrap(), "{}");
    }

    #[test]
    fn accepted_darwin_socket_blocks_until_delayed_request() {
        let dir = tempfile::Builder::new().prefix("ac-").tempdir().unwrap();
        let path = dir.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut client = UnixStream::connect(path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        configure_connection(&accepted).unwrap();
        let flags = unsafe { libc::fcntl(accepted.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(flags & libc::O_NONBLOCK, 0);
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            client.write_all(b"request\n").unwrap();
        });
        assert_eq!(read_line(&accepted).unwrap(), "request");
        writer.join().unwrap();
    }

    #[test]
    fn incomplete_owner_inventory_cannot_report_stop_success() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        serde_json::to_writer(
            &mut writer,
            &TerminalInventory {
                tree: Tree::new(),
                error: Some("native process inspection failed".into()),
            },
        )
        .unwrap();
        writer.write_all(b"\n").unwrap();
        let error = final_tree(&reader).unwrap_err();
        assert!(format!("{error:#}").contains("could not verify complete process inventory"));
    }

    #[test]
    fn closed_owner_buffered_inventory_is_read_without_timeout_mutation() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        serde_json::to_writer(
            &mut writer,
            &TerminalInventory {
                tree: Tree::new(),
                error: None,
            },
        )
        .unwrap();
        writer.write_all(b"\n").unwrap();
        drop(writer);
        assert!(final_tree(&reader).unwrap().is_empty());
    }
}
