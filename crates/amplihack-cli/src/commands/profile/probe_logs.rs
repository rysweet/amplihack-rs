//! `amplihack profile prune-probe-logs` (issue #1974): removes the session
//! logs that usage probes from older versions left in the shared `projects`
//! folder, and nothing else. Run only when an operator asks for it.
//!
//! The rule is in docs/reference/profile-command.md, "Prune probe logs".
//! Log contents are untrusted: any doubt keeps the file, and no log content
//! is ever printed.

use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Files larger than this are kept without being read.
const MAX_BYTES: u64 = 1024 * 1024;

/// Entry types a `/usage` run writes around its `user` entries.
const IGNORED_TYPES: [&str; 6] = [
    "session-owner",
    "queue-operation",
    "attachment",
    "system",
    "last-prompt",
    "cost-state",
];

const USAGE_COMMAND: &str = "<command-name>/usage</command-name>";
const USAGE_MESSAGE: &str = "<command-message>usage</command-message>";
const EMPTY_ARGS: &str = "<command-args></command-args>";

/// What to do with one log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Written by a usage probe and nothing else: may be removed.
    ProbeOnly,
    Keep,
}

/// The outcome of [`prune`].
#[derive(Debug, Default)]
pub(super) struct Pruned {
    /// Paths that match the rule, sorted.
    pub(super) matched: Vec<PathBuf>,
    /// Files deleted (0 in a dry run).
    pub(super) removed: usize,
    /// Every other `.jsonl` file, including those that could not be removed.
    pub(super) kept: usize,
}

/// Reads a log one line at a time; `None` from [`Self::line`] means keep.
#[derive(Default)]
struct Classifier {
    commands: usize,
}

impl Classifier {
    /// Takes one line; `None` when the line alone keeps the file.
    fn line(&mut self, line: &str) -> Option<()> {
        if line.trim().is_empty() {
            return Some(());
        }
        let entry: Value = serde_json::from_str(line).ok()?;
        let entry = entry.as_object()?;
        let kind = entry.get("type")?.as_str()?;
        if kind != "user" {
            let ignored = IGNORED_TYPES.contains(&kind) && holds_no_prompt(kind, entry);
            return ignored.then_some(());
        }
        let content = entry.get("message")?.get("content")?.as_str()?;
        if is_usage_command(content) {
            self.commands += 1;
            return (self.commands == 1).then_some(());
        }
        let caveat = entry.get("isMeta") == Some(&Value::Bool(true))
            && is_one_element(content, "local-command-caveat");
        let output = is_one_element(content, "local-command-stdout")
            || is_one_element(content, "local-command-stderr");
        (caveat || output).then_some(())
    }

    fn verdict(&self) -> Verdict {
        if self.commands == 1 {
            Verdict::ProbeOnly
        } else {
            Verdict::Keep
        }
    }
}

/// Whether a log holds a usage probe and nothing else. Files are read
/// line by line through the same [`Classifier`] in [`classify_file`].
#[cfg(test)]
pub(super) fn classify(text: &str) -> Verdict {
    let mut c = Classifier::default();
    for line in text.lines() {
        if c.line(line).is_none() {
            return Verdict::Keep;
        }
    }
    c.verdict()
}

/// `queue-operation` (`content`) and `last-prompt` (`lastPrompt`) entries
/// can hold text the user typed: they are ignored only when that field is
/// missing or is `/usage`. Any other value keeps the file.
fn holds_no_prompt(kind: &str, entry: &serde_json::Map<String, Value>) -> bool {
    let field = match kind {
        "queue-operation" => "content",
        "last-prompt" => "lastPrompt",
        _ => return true,
    };
    match entry.get(field) {
        None => true,
        Some(v) => v.as_str().is_some_and(|s| s.trim() == "/usage"),
    }
}

/// `<command-name>/usage</command-name>`, then optionally the message, then
/// optionally empty args, with only whitespace between and around them.
fn is_usage_command(content: &str) -> bool {
    let Some(rest) = content.trim_start().strip_prefix(USAGE_COMMAND) else {
        return false;
    };
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix(USAGE_MESSAGE)
        .map_or(rest, str::trim_start);
    let rest = rest.strip_prefix(EMPTY_ARGS).unwrap_or(rest);
    rest.trim().is_empty()
}

/// Whether `content` is exactly one `<tag>…</tag>` element.
fn is_one_element(content: &str, tag: &str) -> bool {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    content
        .strip_prefix(&open)
        .and_then(|r| r.strip_suffix(&close))
        .is_some_and(|inner| !inner.contains(&close) && !inner.contains(&open))
}

/// Claude's name for a project folder: each char outside `[A-Za-z0-9-]`
/// becomes `-`.
pub(super) fn encode(home: &Path) -> String {
    home.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `home/.claude/projects/<encode(home)>`.
pub(super) fn default_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects").join(encode(home))
}

/// Classifies one file without following a link; any error keeps it.
fn classify_file(path: &Path) -> Verdict {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(file) = options.open(path) else {
        return Verdict::Keep;
    };
    match file.metadata() {
        Ok(m) if m.is_file() && m.len() <= MAX_BYTES => {}
        _ => return Verdict::Keep,
    }
    // The file can grow after the check; never read past the ceiling.
    let mut reader = BufReader::new(file.take(MAX_BYTES + 1));
    let mut c = Classifier::default();
    let mut line = String::new();
    let mut read = 0u64;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => read += n as u64,
            Err(_) => return Verdict::Keep,
        }
        if read > MAX_BYTES || c.line(&line).is_none() {
            return Verdict::Keep;
        }
    }
    c.verdict()
}

/// Removes (or, with `dry_run`, only lists) the probe logs directly in
/// `dir`, reporting each file that could not be removed to stderr. The
/// subcommand uses [`run`], which reports to its own writer.
#[cfg(test)]
pub(super) fn prune(dir: &Path, dry_run: bool) -> io::Result<Pruned> {
    prune_with(dir, dry_run, |path, e| {
        eprintln!(
            "warning: could not remove {:?}: {e}",
            path.display().to_string()
        );
    })
}

/// [`prune`], with `on_fail` told of each file that could not be removed.
fn prune_with(
    dir: &Path,
    dry_run: bool,
    mut on_fail: impl FnMut(&Path, &io::Error),
) -> io::Result<Pruned> {
    let mut pruned = Pruned::default();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_name().as_encoded_bytes().ends_with(b".jsonl") {
            continue;
        }
        // `DirEntry::file_type` does not follow links and, on most file
        // systems, comes from the directory listing without another call.
        let regular = entry.file_type().is_ok_and(|t| t.is_file());
        let path = entry.path();
        if regular && classify_file(&path) == Verdict::ProbeOnly {
            pruned.matched.push(path);
        } else {
            pruned.kept += 1;
        }
    }
    pruned.matched.sort();
    if !dry_run {
        for path in &pruned.matched {
            // `remove_file` removes a link itself, never its target.
            match fs::remove_file(path) {
                Ok(()) => pruned.removed += 1,
                Err(e) => {
                    on_fail(path, &e);
                    pruned.kept += 1;
                }
            }
        }
    }
    Ok(pruned)
}

/// The subcommand. `home` is `$HOME`; returns the exit status.
pub(super) fn run(
    dir: Option<&Path>,
    dry_run: bool,
    home: Option<&OsStr>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => match home.map(Path::new).filter(|h| h.is_absolute()) {
            Some(h) => default_dir(h),
            None => {
                return fail(err, "prune-probe-logs needs an absolute HOME, or --dir");
            }
        },
    };
    let shown = |p: &Path| format!("{:?}", p.display().to_string());
    let pruned = prune_with(&dir, dry_run, |path, e| {
        // stderr is the only place to report to; a failed write there
        // changes nothing for the files.
        let _ = writeln!(err, "warning: could not remove {}: {e}", shown(path));
    });
    let pruned = match pruned {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return fail(err, &format!("no such directory: {}", shown(&dir)));
        }
        Err(e) => return fail(err, &format!("could not read {}: {e}", shown(&dir))),
    };
    let written = if dry_run {
        pruned
            .matched
            .iter()
            .try_for_each(|p| writeln!(out, "{}", shown(p)))
    } else {
        writeln!(out, "removed {}, kept {}", pruned.removed, pruned.kept)
    };
    match written {
        Ok(()) => 0,
        Err(e) => fail(err, &format!("could not write output: {e}")),
    }
}

fn fail(err: &mut impl Write, message: &str) -> i32 {
    // Exit status 1 reports the failure even when stderr is closed.
    let _ = writeln!(err, "error: {message}");
    1
}
