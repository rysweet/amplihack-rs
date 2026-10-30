//! The registry's files (E1, S20): the private `~/.amplihack/agents/`
//! directory, `load` with its two stages and content checks (C9, B9, C1),
//! and `update`, the only writer, with its checks before writing (C2). A
//! file that cannot be loaded is never overwritten.

use super::registry::{Registry, VERSION, prune_restarts};
use super::validate;
use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, lock_exclusive_timeout, open_private_nofollow,
    owned_by_euid, read_bounded_nofollow, writable_by_others, write_private_atomic,
};
use anyhow::anyhow;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const REGISTRY: &str = "registry.json";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const KEPT: &str = "it was not changed";

/// `~/.amplihack/agents/`, checked private.
#[derive(Debug, Clone)]
pub struct AgentsDir {
    root: PathBuf,
}

impl AgentsDir {
    pub fn path(home: &Path) -> PathBuf {
        home.join(".amplihack").join("agents")
    }

    /// Checks `~/.amplihack` (creating it at 0700 when missing) and creates
    /// or checks `agents/` at exactly 0700.
    pub fn open(home: &Path) -> anyhow::Result<Self> {
        let top = home.join(".amplihack");
        match fs::symlink_metadata(&top) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::DirBuilder::new()
                .mode(0o700)
                .create(&top)
                .or_else(|e| match e.kind() {
                    io::ErrorKind::AlreadyExists => check_top(&top),
                    _ => Err(anyhow!("{}: {e}", top.display())),
                })?,
            _ => check_top(&top)?,
        }
        let root = Self::path(home);
        ensure_private_dir(&root).map_err(|e| anyhow!("{}: {e}", root.display()))?;
        Ok(Self { root })
    }

    /// The same checks as [`AgentsDir::open`], creating nothing. `None`
    /// when either directory is missing.
    pub fn existing(home: &Path) -> anyhow::Result<Option<Self>> {
        let top = home.join(".amplihack");
        if missing(&top)? {
            return Ok(None);
        }
        check_top(&top)?;
        let root = Self::path(home);
        if missing(&root)? {
            return Ok(None);
        }
        check_private_dir(&root).map_err(|e| anyhow!("{}: {e}", root.display()))?;
        Ok(Some(Self { root }))
    }

    pub fn registry(&self) -> PathBuf {
        self.root.join(REGISTRY)
    }
    pub fn registry_lock(&self) -> PathBuf {
        self.root.join("registry.lock")
    }
    pub fn supervisor_lock(&self) -> PathBuf {
        self.root.join("supervisor.lock")
    }
    pub fn log(&self) -> PathBuf {
        self.root.join("supervisor.log")
    }
    pub fn ended(&self) -> PathBuf {
        self.root.join("ended")
    }
}

fn missing(p: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(anyhow!("{}: {e}", p.display())),
    }
}

/// `~/.amplihack` may be the user's own: a real directory owned by the
/// effective user that others cannot write (a private group may).
fn check_top(top: &Path) -> anyhow::Result<()> {
    let m = fs::symlink_metadata(top).map_err(|e| anyhow!("{}: {e}", top.display()))?;
    let problem = if m.file_type().is_symlink() {
        "is a symlink"
    } else if !m.is_dir() {
        "is not a directory"
    } else if !owned_by_euid(&m) {
        "owned by another user"
    } else if writable_by_others(&m) {
        "writable by other users"
    } else {
        return Ok(());
    };
    Err(anyhow!("{}: {problem}", top.display()))
}

/// Why the registry could not be loaded. `Unsafe` and `Corrupt` hold the
/// whole message; [`LoadError::into_error`] adds the path to the others.
#[derive(Debug)]
pub enum LoadError {
    Unsafe(String),
    Corrupt(String),
    TooNew(u64),
    Io(io::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsafe(m) | Self::Corrupt(m) => f.write_str(m),
            Self::TooNew(n) => write!(
                f,
                "written by a newer amplihack (registry version {n}); upgrade amplihack. It was \
                 not changed"
            ),
            Self::Io(e) => write!(f, "cannot be read: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl LoadError {
    /// The error as a one-line message that always names the file.
    pub fn into_error(self, dir: &AgentsDir) -> anyhow::Error {
        match self {
            Self::Unsafe(_) | Self::Corrupt(_) => anyhow!("{self}"),
            other => anyhow!("{}: {other}", dir.registry().display()),
        }
    }
}

/// Reads the registry without the lock (whole-file reads are safe because
/// every write is an atomic rename). A missing file is an empty registry.
pub fn load(dir: &AgentsDir) -> Result<Registry, LoadError> {
    read(dir).map(|(reg, _)| reg)
}

/// [`load`], with the bytes it parsed; `None` for a missing file.
fn read(dir: &AgentsDir) -> Result<(Registry, Option<Vec<u8>>), LoadError> {
    let path = dir.registry();
    let shown = path.display();
    let unsafe_file = || {
        LoadError::Unsafe(format!(
            "{shown}: must be a regular file owned by you with mode 0600; {KEPT}"
        ))
    };
    let (bytes, meta) = match read_bounded_nofollow(&path, MAX_BYTES as u64) {
        Ok(Some(read)) => read,
        Ok(None) => return Ok((Registry::new(), None)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            let big = fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && m.len() > MAX_BYTES as u64);
            if big {
                let msg = format!("{shown}: larger than 8 MiB; {KEPT}");
                return Err(LoadError::Corrupt(msg));
            }
            return Err(unsafe_file());
        }
        Err(e) => return Err(LoadError::Io(e)),
    };
    if !owned_by_euid(&meta) || meta.mode() & 0o077 != 0 {
        return Err(unsafe_file());
    }
    let at = |what: &str, e: serde_json::Error| {
        LoadError::Corrupt(format!(
            "{shown}: {what} (line {}, column {}); {KEPT}",
            e.line(),
            e.column()
        ))
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| at("not valid JSON", e))?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(VERSION) => {}
        Some(n) if n > VERSION => return Err(LoadError::TooNew(n)),
        _ => {
            return Err(LoadError::Corrupt(format!(
                "{shown}: field version is not valid; {KEPT}"
            )));
        }
    }
    let reg: Registry =
        serde_json::from_slice(&bytes).map_err(|e| at("does not match the registry format", e))?;
    for (name, entry) in &reg.agents {
        if let Err(fe) = validate::entry(name, entry) {
            let msg = if fe.field == "name" {
                format!("{shown}: an agent name is not valid; {KEPT}")
            } else {
                format!("{shown}: agent {name}: {fe}; {KEPT}")
            };
            return Err(LoadError::Corrupt(msg));
        }
    }
    Ok((reg, Some(bytes)))
}

/// Loads the registry under `registry.lock`, applies `f`, prunes restarts
/// against `now`, checks every entry and the size, and writes the file
/// atomically. If any step fails, nothing is written. Nor is it when the
/// result is byte for byte the file already there: a result repeated every
/// pass (D13) would otherwise cost a rename and two fsyncs each minute.
pub fn update<T>(
    dir: &AgentsDir,
    now: i64,
    f: impl FnOnce(&mut Registry) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lock_path = dir.registry_lock();
    let file =
        open_private_nofollow(&lock_path).map_err(|e| anyhow!("{}: {e}", lock_path.display()))?;
    let _lock = lock_exclusive_timeout(file, LOCK_TIMEOUT).map_err(|e| {
        if e.kind() == io::ErrorKind::TimedOut {
            anyhow!(
                "{}: the registry lock was held for 10 s by another amplihack process; try again",
                lock_path.display()
            )
        } else {
            anyhow!("{}: {e}", lock_path.display())
        }
    })?;
    let (mut reg, on_disk) = read(dir).map_err(|e| e.into_error(dir))?;
    let out = f(&mut reg)?;
    let path = dir.registry();
    for (name, entry) in reg.agents.iter_mut() {
        entry.restarts = prune_restarts(&entry.restarts, now);
        if let Err(fe) = validate::entry(name, entry) {
            if fe.field == "name" {
                return Err(anyhow!(
                    "{}: refusing to write an agent with an invalid name; {KEPT}",
                    path.display()
                ));
            }
            return Err(anyhow!(
                "{}: refusing to write agent {name}: {fe}; {KEPT}",
                path.display()
            ));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&reg)
        .map_err(|e| anyhow!("{}: cannot serialise the registry: {e}", path.display()))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_BYTES {
        return Err(anyhow!(
            "{}: the registry would exceed 8 MiB; {KEPT}",
            path.display()
        ));
    }
    if on_disk.as_deref() == Some(bytes.as_slice()) {
        return Ok(out);
    }
    write_private_atomic(&dir.root, REGISTRY, &bytes)
        .map_err(|e| anyhow!("{}: {e}", path.display()))?;
    Ok(out)
}
