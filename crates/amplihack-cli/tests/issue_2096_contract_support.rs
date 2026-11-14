//! Rust launch-boundary fixture; this does not certify the pinned runner.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::OnceLock;
use std::thread::JoinHandle;

pub(super) fn driver() -> &'static Path {
    static DRIVER: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &DRIVER
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("issue-2096-driver");
            let result = Command::new("rustc")
                .args(["--edition=2024", "--crate-name", "issue_2096_driver"])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("test-fixtures/issue_2096/runner.rs"),
                )
                .arg("-o")
                .arg(&binary)
                .output()
                .expect("compile Rust boundary fixture");
            succeeds(&result);
            (dir, binary)
        })
        .1
}

pub(super) fn command(f: &Fixture, label: &str) -> Command {
    let mut command = f.command(label);
    command
        .env("RECIPE_RUNNER_RS_PATH", driver())
        .env("ISSUE_2096_GIT", absolute_git());
    command
}

pub(super) fn absolute_git() -> PathBuf {
    // Git's exec path is not necessarily the directory holding the public
    // binary. Resolve PATH ourselves so absolute launches bypass wrappers.
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|path| path.is_file())
        .unwrap()
        .canonicalize()
        .unwrap()
}

pub(super) fn path(f: &Fixture, label: &str, name: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = f.observed(label, name);
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

/// Both readiness and early CLI exit are events on the same socket. A
/// pre-launch rejection therefore fails with output instead of hanging.
/// Dropping a failed assertion releases the runner and joins its parent.
pub(super) struct Running {
    stream: Option<UnixStream>,
    worker: Option<JoinHandle<Output>>,
}

impl Running {
    pub(super) fn start(f: &Fixture, label: &str) -> Self {
        Self::start_command(f, label, command(f, label))
    }

    pub(super) fn start_command(f: &Fixture, label: &str, mut command: Command) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);
        // TMPDIR may already be close to sun_path's limit in recipe runs.
        let socket = f
            .root
            .join(format!("s{}", NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)));
        let listener = UnixListener::bind(&socket).unwrap();
        command.env("ISSUE_2096_RENDEZVOUS", &socket);
        let worker = std::thread::spawn(move || {
            let output = command.output().expect("run fixture CLI");
            // Wake accept even when preparation failed before runner launch.
            if let Ok(mut stream) = UnixStream::connect(&socket) {
                stream.write_all(b"exit\n").unwrap();
            }
            output
        });
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let running = Self {
            stream: Some(stream),
            worker: Some(worker),
        };
        if line != "ready\n" {
            let output = running.finish();
            panic!("runner {label} did not acknowledge readiness: {line:?}\n{output:?}");
        }
        running
    }

    pub(super) fn finish(mut self) -> Output {
        self.release();
        self.worker.take().unwrap().join().unwrap()
    }

    fn release(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            // A dead child has already released its resources; BrokenPipe is
            // expected on the early-exit path and must not mask its output.
            if let Err(error) = stream.write_all(b"go\n") {
                assert!(
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ),
                    "release runner: {error}"
                );
            }
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.release();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn run_dir(f: &Fixture) -> PathBuf {
    let paths: Vec<_> = fs::read_dir(f.root.join("work/runs"))
        .expect("read required run storage")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(paths.len(), 1, "expected one logical run: {paths:?}");
    paths[0].clone()
}

pub(super) fn records(f: &Fixture) -> Vec<serde_json::Value> {
    let file = run_dir(f).join("git-commands.jsonl");
    fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("read {}: {error}", file.display()))
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid command record"))
        .collect()
}
