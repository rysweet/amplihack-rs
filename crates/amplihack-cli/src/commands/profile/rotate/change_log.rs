//! The change log (issue #1803): every change to the rotate autostart
//! setting or the auto-switch toggle appends one `change:` line to
//! `rotate.log`, naming the process that made it and its parent, so a log
//! shows who turned rotation off. See "Rotate change log" in
//! docs/reference/profile-command.md.
//!
//! Best effort and lock-free: a failure prints one warning to stderr and the
//! command that changed the setting still succeeds. The parent's command
//! line is redacted and sanitised, so no value can carry a secret into the
//! log, start a second line or forge a field.

use super::ledger::RotateState;
use super::log::{Sink, open_or_warn};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::ffi::OsString;
use std::path::Path;

const MAX_PARENT_READ: u64 = 8 * 1024;
const MAX_PARENT_CHARS: usize = 200;
const MAX_ID_CHARS: usize = 64;
const SECRET_WORDS: [&str; 5] = ["key", "token", "secret", "password", "auth"];
const HIDDEN: &str = "***";

/// The process that made a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub pid: u32,
    pub ppid: u32,
    /// The parent's raw NUL-separated `/proc/<ppid>/cmdline`, if readable.
    pub parent: Option<Vec<u8>>,
    /// `AMPLIHACK_RECIPE_RUN_ID`, when set and non-empty.
    pub recipe_run: Option<String>,
    /// `INVOCATION_ID`, when set and non-empty.
    pub invocation: Option<String>,
}

/// `on` or `off`.
pub fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// This process, its parent from `proc_root`, and the run ids from `env`.
pub fn caller(env: &dyn Fn(&str) -> Option<OsString>, proc_root: &Path) -> Caller {
    // SAFETY: getppid has no preconditions and cannot fail.
    let ppid = unsafe { libc::getppid() } as u32;
    let var = |k: &str| {
        env(k)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string_lossy().into_owned())
    };
    Caller {
        pid: std::process::id(),
        ppid,
        parent: read_cmdline(&proc_root.join(ppid.to_string()).join("cmdline")),
        recipe_run: var(super::autostart::RECIPE_RUN_ENV),
        invocation: var("INVOCATION_ID"),
    }
}

/// The first 8 KiB of a regular file, opened without following a symlink
/// or blocking. `/proc` files report no size, so the cap is applied by
/// reading, and a longer command line is cut rather than dropped.
fn read_cmdline(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut raw = amplihack_utils::secure_files::read_bounded(file, MAX_PARENT_READ).ok()?;
    raw.truncate(MAX_PARENT_READ as usize);
    Some(raw)
}

fn secret_flag(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    SECRET_WORDS.iter().any(|w| name.contains(w))
}

fn is_env_name(n: &str) -> bool {
    let mut chars = n.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `s` with `user:password@` removed from every URL in it.
fn strip_userinfo(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        let authority = &tail[..end];
        let host = authority
            .rfind('@')
            .map_or(authority, |at| &authority[at + 1..]);
        out.push_str(host);
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// A `Name: value` header whose name holds a secret word, such as
/// `Authorization: Bearer ...` or `X-Api-Key: ...`, as `Name: ***`.
fn secret_header(a: &str) -> Option<String> {
    let (name, _) = a.split_once(": ")?;
    let lower = name.to_ascii_lowercase();
    (!name.is_empty()
        && !name.contains(char::is_whitespace)
        && SECRET_WORDS.iter().any(|w| lower.contains(w)))
    .then(|| format!("{name}: {HIDDEN}"))
}

/// The arguments with secrets replaced: `NAME=value`, the value of a flag
/// whose name holds a secret word (`--flag value` and `--flag=value`), a
/// `Name: value` header whose name holds one, and URL user information.
fn redact(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for a in args {
        if std::mem::take(&mut hide_next) {
            out.push(HIDDEN.to_string());
            continue;
        }
        if let Some(h) = secret_header(a) {
            out.push(h);
            continue;
        }
        if a.starts_with('-') {
            let (name, value) = a
                .split_once('=')
                .map_or((a.as_str(), None), |(n, v)| (n, Some(v)));
            if secret_flag(name) {
                match value {
                    Some(_) => out.push(format!("{name}={HIDDEN}")),
                    None => {
                        hide_next = true;
                        out.push(a.clone());
                    }
                }
                continue;
            }
        } else if let Some((name, _)) = a.split_once('=')
            && is_env_name(name)
        {
            out.push(format!("{name}={HIDDEN}"));
            continue;
        }
        out.push(strip_userinfo(a));
    }
    out
}

/// The `parent="..."` value: redacted, sanitised, `"` as `'`, at most 200
/// characters, or `-` when it could not be read.
fn parent_text(raw: Option<&[u8]>) -> String {
    let Some(raw) = raw else {
        return "-".into();
    };
    let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
    if raw.is_empty() {
        return "-".into();
    }
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    sanitize_display(&redact(&args).join(" "), false)
        .replace('"', "'")
        .chars()
        .take(MAX_PARENT_CHARS)
        .collect()
}

/// A run id as one field: sanitised, spaces as `_`, at most 64 characters.
fn id_text(id: &str) -> String {
    sanitize_display(id, false)
        .replace(' ', "_")
        .chars()
        .take(MAX_ID_CHARS)
        .collect()
}

/// The log message, without the time.
pub fn line(setting: &str, old: &str, new: &str, c: &Caller) -> String {
    let mut out = format!(
        "change: {setting} {old} -> {new} by pid={} ppid={} parent=\"{}\"",
        c.pid,
        c.ppid,
        parent_text(c.parent.as_deref())
    );
    if let Some(r) = &c.recipe_run {
        out.push_str(&format!(" recipe_run={}", id_text(r)));
    }
    if let Some(i) = &c.invocation {
        out.push_str(&format!(" invocation={}", id_text(i)));
    }
    out
}

/// Appends the timed `change:` line to `rotate.log` under `root`. Never
/// fails: a problem is one warning on stderr.
pub fn record(root: &Path, setting: &str, old: &str, new: &str) {
    let rs = RotateState::new(root);
    if let Err(e) = rs.ensure() {
        eprintln!("warning: the {setting} change was not logged ({e})");
        return;
    }
    let env = |k: &str| std::env::var_os(k);
    let msg = line(setting, old, new, &caller(&env, Path::new("/proc")));
    Sink::new(open_or_warn(&rs.log, &rs.log1), false, true).emit(now(), &msg);
}
