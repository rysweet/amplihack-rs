//! Claude Code's own records, read only (R4.1(a), D12, S10): the config
//! directories (`~/.claude` and every profile directory), their
//! `sessions/<pid>.json` files and the transcript path. Every directory must
//! be owned by the user and closed to other users' writes; otherwise the
//! answer is an error, which makes liveness unknown and starts nothing.

use super::validate;
use amplihack_utils::rotator_status::{RootError, locate_root};
use amplihack_utils::secure_files::{owned_by_euid, read_bounded_nofollow, writable_by_others};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SESSION_LIMIT: u64 = 64 * 1024;
/// The liveness-unknown reason for any S10 failure.
pub const NOT_PRIVATE: &str = "Claude config directory is not private";
const PRIVATE_RULE: &str = "it must be owned by you and not writable by other users";
const PROFILE_MISSING: &str = "--profile: the profile's directory must exist under the profile \
                               root and must not be a symlink";

/// Why the config directories could not be listed.
enum DirsError {
    /// An S10 failure, naming the directory by `~/.claude` or profile name.
    Private(String),
    Other(String),
}

/// [`config_dirs_from`] for this process's environment. An S10 failure is
/// reported as [`NOT_PRIVATE`], the reason the supervisor prints.
pub fn config_dirs(home: &Path) -> Result<Vec<PathBuf>, String> {
    collect(home, |k| std::env::var_os(k)).map_err(|e| match e {
        DirsError::Private(_) => NOT_PRIVATE.to_string(),
        DirsError::Other(m) => m,
    })
}

/// `home/.claude`, then each real directory directly inside the profile
/// root (symlinks, files and names that are not profile names are skipped).
/// A missing root gives no profiles. A profile-root error, an unlistable
/// root or an S10 failure is an error that names the directory without its
/// path.
pub fn config_dirs_from(
    home: &Path,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<Vec<PathBuf>, String> {
    collect(home, env).map_err(|e| match e {
        DirsError::Private(m) | DirsError::Other(m) => m,
    })
}

fn collect(home: &Path, env: impl Fn(&str) -> Option<OsString>) -> Result<Vec<PathBuf>, DirsError> {
    let claude = home.join(".claude");
    match fs::metadata(&claude) {
        Ok(m) if !m.is_dir() => {
            return Err(DirsError::Other("~/.claude is not a directory".into()));
        }
        Ok(m) if !private(&m) => {
            return Err(DirsError::Private(format!(
                "~/.claude is not private: {PRIVATE_RULE}"
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(DirsError::Other("~/.claude cannot be read".into())),
    }
    let listing = |why: &str| DirsError::Other(format!("cannot list profile directories: {why}"));
    let (root, _) = locate_root(env, Some(home)).map_err(|e| listing(&root_error(&e)))?;
    let mut dirs = vec![claude];
    match fs::metadata(&root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(dirs),
        Err(_) => return Err(listing("the profile root cannot be read")),
        Ok(m) if !m.is_dir() => return Err(listing("the profile root is not a directory")),
        Ok(m) if !private(&m) => {
            let msg = format!("the profile root is not private: {PRIVATE_RULE}");
            return Err(DirsError::Private(msg));
        }
        Ok(_) => {}
    }
    let entries = fs::read_dir(&root).map_err(|_| listing("the profile root cannot be read"))?;
    let mut profiles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| listing("the profile root cannot be read"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if validate::profile_name(&name).is_err() {
            continue;
        }
        let path = root.join(&name);
        let Ok(m) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !m.is_dir() {
            continue;
        }
        if !private(&m) {
            let msg = format!("the directory of profile {name} is not private: {PRIVATE_RULE}");
            return Err(DirsError::Private(msg));
        }
        profiles.push(path);
    }
    profiles.sort();
    dirs.extend(profiles);
    Ok(dirs)
}

/// The B6 text for a profile-root error; the variable's value is never
/// repeated.
fn root_error(e: &RootError) -> String {
    match e {
        RootError::Env(_) => "AMPLIHACK_CLAUDE_PROFILE_ROOT is not a valid absolute path".into(),
        RootError::NoHome => "HOME is not set".into(),
        RootError::Saved { file, reason } => format!("{}: {reason}", file.display()),
    }
}

/// S10: owned by the effective user, not writable by others (a private
/// group may write).
fn private(m: &fs::Metadata) -> bool {
    owned_by_euid(m) && !writable_by_others(m)
}

/// [`profile_config_dir_from`] for this process's environment.
pub fn profile_config_dir(home: &Path, profile: Option<&str>) -> Result<PathBuf, String> {
    profile_config_dir_from(home, profile, |k| std::env::var_os(k))
}

/// The config directory a resume with `profile` uses (D12): `~/.claude`
/// without a profile, otherwise `root/<profile>`, which must be a real
/// directory. Errors are whole messages for the `--profile` flag.
pub fn profile_config_dir_from(
    home: &Path,
    profile: Option<&str>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, String> {
    let Some(p) = profile else {
        return Ok(home.join(".claude"));
    };
    validate::profile_name(p).map_err(|rule| format!("--profile: {rule}"))?;
    let (root, _) = locate_root(env, Some(home))
        .map_err(|e| format!("cannot list profile directories: {}", root_error(&e)))?;
    let dir = root.join(p);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Ok(dir),
        _ => Err(PROFILE_MISSING.to_string()),
    }
}

/// The fields of Claude's `sessions/<pid>.json` that are used.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFile {
    pub pid: u32,
    pub session_id: String,
    pub proc_start: u64,
    pub cwd: Option<PathBuf>,
}

/// `pid` (integer), `sessionId` (a valid id, S7) and `procStart` (string or
/// integer) are required; `cwd` is optional; other keys are ignored.
pub fn parse_session_file(raw: &[u8]) -> Option<SessionFile> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    let session_id = v.get("sessionId")?.as_str()?;
    validate::conversation_id(session_id).ok()?;
    let proc_start = match v.get("procStart")? {
        Value::String(s) => s.parse().ok()?,
        Value::Number(n) => n.as_u64()?,
        _ => return None,
    };
    let cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from);
    Some(SessionFile {
        pid,
        session_id: session_id.to_string(),
        proc_start,
        cwd,
    })
}

/// Live-process candidates by conversation id, from every sessions file.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub by_id: BTreeMap<String, Vec<(u32, u64)>>,
}

/// Reads `<dir>/sessions/*.json` in each directory (64 KiB each, symlinks
/// not followed, unparsable files skipped). A missing `sessions/` is empty;
/// one that cannot be listed or fails S10 is an error.
pub fn index(dirs: &[PathBuf]) -> Result<Index, String> {
    let mut idx = Index::default();
    for dir in dirs {
        let sessions = dir.join("sessions");
        match fs::symlink_metadata(&sessions) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err("a Claude sessions directory cannot be read".into()),
            Ok(m) if !m.is_dir() || !private(&m) => return Err(NOT_PRIVATE.into()),
            Ok(_) => {}
        }
        let entries = fs::read_dir(&sessions)
            .map_err(|_| "a Claude sessions directory cannot be listed".to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            if let Some(s) = read_session(&path) {
                idx.by_id
                    .entry(s.session_id)
                    .or_default()
                    .push((s.pid, s.proc_start));
            }
        }
    }
    Ok(idx)
}

fn read_session(path: &Path) -> Option<SessionFile> {
    let (bytes, _) = read_bounded_nofollow(path, SESSION_LIMIT).ok()??;
    parse_session_file(&bytes)
}

/// The sessions file of one process: only `<dir>/sessions/<pid>.json` is
/// read in each directory, and the first whose `pid` and `procStart` match.
pub fn for_process(dirs: &[PathBuf], pid: u32, start: u64) -> Option<SessionFile> {
    dirs.iter()
        .filter_map(|d| read_session(&d.join("sessions").join(format!("{pid}.json"))))
        .find(|s| s.pid == pid && s.proc_start == start)
}

/// The `projects/` directory names Claude may use for `launch_dir` (A4):
/// `/` and `.` as `-`, then every character other than an ASCII letter or
/// digit as `-`; duplicates removed.
pub fn project_dir_names(launch_dir: &Path) -> Vec<String> {
    let s = launch_dir.to_string_lossy();
    let first: String = s
        .chars()
        .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
        .collect();
    let second: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut names = vec![first];
    if names[0] != second {
        names.push(second);
    }
    names
}

/// Whether `<config_dir>/projects/<enc>/<id>.jsonl` exists for either
/// encoding of `launch_dir`.
pub fn transcript_exists(config_dir: &Path, launch_dir: &Path, id: &str) -> bool {
    if validate::conversation_id(id).is_err() {
        return false;
    }
    let projects = config_dir.join("projects");
    project_dir_names(launch_dir)
        .iter()
        .any(|enc| projects.join(enc).join(format!("{id}.jsonl")).is_file())
}
