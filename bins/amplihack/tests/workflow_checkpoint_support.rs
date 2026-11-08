//! Shared, hermetic harness for the `amplihack internal workflow-checkpoint`
//! tests (#1554, Rust port of the V23 checkpoint engine).
//!
//! Every command built here starts from an empty environment. HOME, TMPDIR
//! and XDG_* live under the test's tempdir, Git reads no global or system
//! configuration, the identity and dates are fixed, and the 17 repository
//! selectors that `pre-push` exports (GIT_INDEX_FILE and similar) are never
//! inherited. `AMPLIHACK_BIN` is always `CARGO_BIN_EXE_amplihack`; there is
//! no fallback to an `amplihack` on PATH.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// The binary under test. Never a PATH lookup.
pub const AMPLIHACK: &str = env!("CARGO_BIN_EXE_amplihack");

/// The engine refuses each of these; the harness clears each of these. One
/// list, exported by the engine and pinned to 918c by the static guards.
pub use amplihack_cli::workflow_checkpoint::FORBIDDEN_SELECTORS;

/// The 16 actions the wrappers call. `actions_match_the_engine_table`
/// keeps this list equal to the engine's closed table.
pub const ACTIONS: [&str; 16] = [
    "selectors",
    "capture",
    "admit",
    "check",
    "paths",
    "stage-strict",
    "before",
    "sync-fetch-mode",
    "after",
    "destination-is-head",
    "verify-pushed-ref",
    "advertised-ref",
    "sha256",
    "admission-refused-result",
    "cleanup-skip-result",
    "worktree-path",
];

pub const REFUSAL_PREFIX: &str = "checkpoint scope refusal: ";

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

pub fn tools_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/tools")
}

/// A private sandbox: HOME, TMPDIR (0700), XDG_* and a shim directory.
pub struct Sandbox {
    pub dir: TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("wf-checkpoint-")
            .tempdir()
            .expect("tempdir");
        for sub in [
            "home",
            "tmp",
            "xdg/config",
            "xdg/cache",
            "xdg/data",
            "xdg/state",
            "shims",
        ] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        fs::set_permissions(dir.path().join("tmp"), fs::Permissions::from_mode(0o700)).unwrap();
        Sandbox { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
    pub fn home(&self) -> PathBuf {
        self.path().join("home")
    }
    pub fn tmp(&self) -> PathBuf {
        self.path().join("tmp")
    }
    pub fn shims(&self) -> PathBuf {
        self.path().join("shims")
    }

    /// PATH with the shim directory first, then the host's system directories.
    pub fn path_env(&self) -> String {
        let host = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        format!("{}:{}", self.shims().display(), host)
    }

    /// An isolated command with the full hermetic environment applied.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env("PATH", self.path_env())
            .env("HOME", self.home())
            .env("TMPDIR", self.tmp())
            .env("XDG_CONFIG_HOME", self.path().join("xdg/config"))
            .env("XDG_CACHE_HOME", self.path().join("xdg/cache"))
            .env("XDG_DATA_HOME", self.path().join("xdg/data"))
            .env("XDG_STATE_HOME", self.path().join("xdg/state"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Checkpoint Test")
            .env("GIT_AUTHOR_EMAIL", "checkpoint@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Checkpoint Test")
            .env("GIT_COMMITTER_EMAIL", "checkpoint@example.invalid")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .env("AMPLIHACK_BIN", AMPLIHACK)
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("LC_ALL", "C")
            .stdin(Stdio::null());
        for name in FORBIDDEN_SELECTORS {
            c.env_remove(name);
        }
        c
    }

    /// `amplihack internal workflow-checkpoint <action> <args...>`.
    pub fn checkpoint<I, S>(&self, action: impl AsRef<OsStr>, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut c = self.command(AMPLIHACK);
        c.arg("internal")
            .arg("workflow-checkpoint")
            .arg(action)
            .args(args);
        c
    }

    /// Install a `python3` shim that records any call and exits 127.
    pub fn python3_marker_shim(&self) -> PathBuf {
        let marker = self.path().join("python3-was-called");
        write_exec(
            &self.shims().join("python3"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 127\n",
                marker.display()
            ),
        );
        marker
    }

    /// Install a `git` shim. `ls-remote` prints `stdout` (raw bytes, from a
    /// file) and exits `status`; every other subcommand runs the real git.
    /// Each call is appended to a log, which records whether
    /// GIT_CONFIG_VALUE_0 was set and its length, never its value.
    pub fn git_shim(&self, ls_remote_stdout: &[u8], status: i32) -> GitShim {
        let real = which("git");
        let body = self.path().join("ls-remote.out");
        fs::write(&body, ls_remote_stdout).unwrap();
        let log = self.path().join("git-shim.log");
        write_exec(
            &self.shims().join("git"),
            &format!(
                r#"#!/bin/bash
v="${{GIT_CONFIG_VALUE_0-}}"
printf 'argv=%s value0_set=%s value0_len=%s\n' "$*" "${{GIT_CONFIG_VALUE_0+yes}}" "${{#v}}" >> '{log}'
for a in "$@"; do
  if [ "$a" = ls-remote ]; then cat '{body}'; exit {status}; fi
done
exec '{real}' "$@"
"#,
                log = log.display(),
                body = body.display(),
                status = status,
                real = real.display(),
            ),
        );
        GitShim { log }
    }
}

pub struct GitShim {
    pub log: PathBuf,
}

impl GitShim {
    pub fn calls(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
    pub fn ls_remote_calls(&self) -> usize {
        self.calls()
            .lines()
            .filter(|l| l.contains("ls-remote"))
            .count()
    }
}

pub fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn which(tool: &str) -> PathBuf {
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let p = Path::new(dir).join(tool);
        if p.is_file() {
            return p;
        }
    }
    panic!("{tool} not found on PATH");
}

/// Plain git in `dir`, with the hermetic environment.
pub fn git(sb: &Sandbox, dir: &Path, args: &[&str]) -> Output {
    let out = sb
        .command(which("git"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A small repository with one commit on `feat/x` and a local bare `origin`
/// reached by a file:// URL. Returns (worktree, bare).
pub fn repo_with_origin(sb: &Sandbox) -> (PathBuf, PathBuf) {
    let bare = sb.path().join("origin.git");
    let work = sb.path().join("work");
    fs::create_dir_all(&work).unwrap();
    git(
        sb,
        sb.path(),
        &["init", "--bare", "-q", "-b", "main", bare.to_str().unwrap()],
    );
    git(sb, &work, &["init", "-q", "-b", "main"]);
    fs::write(work.join("README"), "seed\n").unwrap();
    git(sb, &work, &["add", "README"]);
    git(sb, &work, &["commit", "-q", "-m", "seed"]);
    let url = format!("file://{}", bare.display());
    git(sb, &work, &["remote", "add", "origin", &url]);
    git(sb, &work, &["push", "-q", "origin", "main"]);
    git(sb, &work, &["switch", "-q", "-c", "feat/x"]);
    (work, bare)
}

/// The observable state of a bare destination: every ref and the object count.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub refs: String,
    pub objects: String,
    pub packed_refs: Option<Vec<u8>>,
}

pub fn destination_snapshot(sb: &Sandbox, bare: &Path) -> Snapshot {
    let refs = git(
        sb,
        bare,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let objects = git(sb, bare, &["count-objects", "-v"]);
    Snapshot {
        refs: String::from_utf8(refs.stdout).unwrap(),
        objects: String::from_utf8(objects.stdout).unwrap(),
        packed_refs: fs::read(bare.join("packed-refs")).ok(),
    }
}

/// Every file under `dir`, with its bytes, for a before/after comparison.
pub fn tree_snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_path_buf();
            let meta = fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                out.insert(rel, b"<dir>".to_vec());
                walk(base, &p, out);
            } else {
                out.insert(rel, fs::read(&p).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

pub fn status(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A SHA-1 object ID made of one repeated hex digit.
pub fn oid1(c: char) -> String {
    std::iter::repeat_n(c, 40).collect()
}

/// A SHA-256 object ID made of one repeated hex digit.
pub fn oid256(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}
