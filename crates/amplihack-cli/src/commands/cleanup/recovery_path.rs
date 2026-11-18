//! The path to the work root, checked before a run opens its recovery store
//! (issue #2346).
//!
//! Since #2334 a run opens its recovery store through the pinned runner's
//! `recipe_runner_rs::recovery::private_fs::Directory::open`. That function
//! stops a work root with a folder above it that is group- or world-writable
//! without the sticky bit ("unsafe recovery ancestor"), and a work root that
//! is not owned and `0700` ("recovery directory must be owned and mode
//! 0700"). Neither message names the folder.
//!
//! [`prepare_work_root`] applies the same rules first, by path, and names
//! every folder at fault with its mode and the command that fixes it. It is
//! advisory: the runner remains the only enforcing check and runs after it
//! whatever this module decides. The one change made here is to set an owned
//! work root that has no group or other write bit (`0755`, `0750`, ...) to
//! `0700`, through a descriptor, after every check has passed.
//!
//! The specification is `docs/reference/recipe-command.md#work-root-permissions`.

use crate::commands::install::paths::sanitize_path;
use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions, Permissions};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

/// One reason the runner would stop a work root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    /// A folder above the work root with a group or other write bit and no
    /// sticky bit.
    Writable { path: PathBuf, mode: u32 },
    /// The work root with a group or other write bit, or `runs/` that is not
    /// exactly `0700`.
    NotPrivate { path: PathBuf, mode: u32 },
    /// The work root or `runs/`, owned by another user.
    NotOwned { path: PathBuf, uid: u32 },
    /// A symbolic link where the runner needs a folder. `target` is what the
    /// link holds, when it can be read.
    Link {
        path: PathBuf,
        target: Option<PathBuf>,
    },
    /// A file, socket or other entry where the runner needs a folder.
    NotFolder { path: PathBuf },
    /// An entry that could not be inspected.
    Inspect { path: PathBuf, error: String },
    /// A relative path, or one with `.` or `..` components.
    NonNormal { path: PathBuf },
}

impl Fault {
    fn path(&self) -> &Path {
        match self {
            Self::Writable { path, .. }
            | Self::NotPrivate { path, .. }
            | Self::NotOwned { path, .. }
            | Self::Link { path, .. }
            | Self::NotFolder { path }
            | Self::Inspect { path, .. }
            | Self::NonNormal { path } => path,
        }
    }

    /// A fault after which the walk cannot go on: nothing below it can be
    /// inspected as the runner would see it.
    fn ends_walk(&self) -> bool {
        matches!(
            self,
            Self::Link { .. }
                | Self::NotFolder { .. }
                | Self::Inspect { .. }
                | Self::NonNormal { .. }
        )
    }
}

/// The runner's rule for a folder above the work root: no group or other
/// write bit, or the sticky bit.
pub(crate) fn ancestor_mode_ok(mode: u32) -> bool {
    mode & 0o022 == 0 || mode & 0o1000 != 0
}

/// How an existing work root folder compares with the runner's rule for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootMode {
    /// Owned by another user.
    NotOwned,
    /// Owned, with a group or other write bit: others may already have
    /// written into it, which a chmod would not undo.
    Writable,
    /// Owned and `0700` (the setgid and sticky bits are ignored, as the
    /// runner ignores them).
    Private,
    /// Owned, no group or other write bit, but not `0700`: safe to set to
    /// `0700`.
    Tighten,
}

pub(crate) fn root_mode(uid: u32, euid: u32, mode: u32) -> RootMode {
    if uid != euid {
        RootMode::NotOwned
    } else if mode & 0o022 != 0 {
        RootMode::Writable
    } else if mode & 0o777 == 0o700 {
        RootMode::Private
    } else {
        RootMode::Tighten
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Inspects one entry without following a link. `Ok(None)` when it does not
/// exist; `Err` with the fault when it is not a folder.
fn inspect(path: &Path) -> std::result::Result<Option<fs::Metadata>, Fault> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Fault::Link {
            path: path.to_path_buf(),
            target: fs::read_link(path).ok(),
        }),
        Ok(meta) if !meta.is_dir() => Err(Fault::NotFolder {
            path: path.to_path_buf(),
        }),
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Fault::Inspect {
            path: path.to_path_buf(),
            error: error.to_string(),
        }),
    }
}

/// Every folder from `/` down to `dir`, as written, that the runner's rule
/// for folders above the work root stops, outermost first.
///
/// Uses `lstat` and never resolves a link. Every group- or world-writable
/// folder is recorded; the walk stops after a symbolic link, an entry that is
/// not a folder, or one that cannot be inspected, and ends without a fault at
/// the first entry that does not exist (the runner creates the rest at
/// `0700`).
pub(crate) fn path_faults(dir: &Path) -> Vec<Fault> {
    let mut faults = Vec::new();
    let mut current = PathBuf::new();
    for component in dir.components() {
        match component {
            Component::RootDir => current.push("/"),
            Component::Normal(name) if current.is_absolute() => current.push(name),
            _ => {
                faults.push(Fault::NonNormal {
                    path: dir.to_path_buf(),
                });
                return faults;
            }
        }
        match inspect(&current) {
            Ok(None) => return faults,
            Ok(Some(meta)) => {
                if !ancestor_mode_ok(meta.mode()) {
                    faults.push(Fault::Writable {
                        path: current.clone(),
                        mode: meta.mode(),
                    });
                }
            }
            Err(fault) => {
                faults.push(fault);
                return faults;
            }
        }
    }
    faults
}

/// Whether `root` is absolute with only normal components below `/`, the
/// shape the runner accepts.
fn normal_shape(root: &Path) -> bool {
    let mut components = root.components();
    components.next() == Some(Component::RootDir)
        && root.parent().is_some()
        && components.all(|c| matches!(c, Component::Normal(_)))
}

/// Checks the path to the work root, the work root and `<work root>/runs`
/// against the runner's rules, and sets an owned work root without group or
/// other write bits to `0700`.
///
/// Every check runs before any change. When one fails, the error names each
/// folder at fault, its mode and the fix, and nothing has been changed or
/// created. A missing work root is accepted and not created: the runner
/// creates it, and any missing folder above it, at `0700`.
pub(crate) fn prepare_work_root(root: &Path) -> Result<()> {
    if !normal_shape(root) {
        bail!(describe(
            root,
            &[Fault::NonNormal {
                path: root.to_path_buf(),
            }]
        ));
    }
    let parent = root
        .parent()
        .context("a work root of normal shape has a parent folder")?;
    let mut faults = path_faults(parent);
    let mut tighten = None;
    if !faults.last().is_some_and(Fault::ends_walk) {
        match inspect(root) {
            Ok(None) => {}
            Err(fault) => faults.push(fault),
            Ok(Some(meta)) => {
                match root_mode(meta.uid(), euid(), meta.mode()) {
                    RootMode::NotOwned => faults.push(Fault::NotOwned {
                        path: root.to_path_buf(),
                        uid: meta.uid(),
                    }),
                    RootMode::Writable => faults.push(Fault::NotPrivate {
                        path: root.to_path_buf(),
                        mode: meta.mode(),
                    }),
                    RootMode::Tighten => tighten = Some((meta.dev(), meta.ino())),
                    RootMode::Private => {}
                }
                if let Some(fault) = runs_fault(&root.join("runs")) {
                    faults.push(fault);
                }
            }
        }
    }
    if !faults.is_empty() {
        bail!(describe(root, &faults));
    }
    match tighten {
        Some(identity) => tighten_root(root, identity),
        None => Ok(()),
    }
}

/// `<work root>/runs`, held to the runner's private rule: when it exists it
/// is a folder, owned, and exactly `0700`. It is never changed here.
fn runs_fault(runs: &Path) -> Option<Fault> {
    match inspect(runs) {
        Ok(None) => None,
        Err(fault) => Some(fault),
        Ok(Some(meta)) if meta.uid() != euid() => Some(Fault::NotOwned {
            path: runs.to_path_buf(),
            uid: meta.uid(),
        }),
        Ok(Some(meta)) if meta.mode() & 0o777 != 0o700 => Some(Fault::NotPrivate {
            path: runs.to_path_buf(),
            mode: meta.mode(),
        }),
        Ok(Some(_)) => None,
    }
}

/// Sets the work root to `0700` through a descriptor opened without following
/// a link, after confirming that it is the folder inspected as `expected`
/// (`st_dev`, `st_ino`), that it is owned, and that it has no group or other
/// write bit. Prints one notice on stderr after the change succeeds.
fn tighten_root(root: &Path, expected: (u64, u64)) -> Result<()> {
    let shown = sanitize_path(root);
    let fix = command_or_words(
        "chmod 700",
        root,
        "give it mode 0700 and remove its group and other access",
    );
    let folder = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(|error| {
            anyhow::anyhow!("cannot set work root {shown} to mode 0700 ({error}); {fix}")
        })?;
    let meta = folder
        .metadata()
        .map_err(|error| anyhow::anyhow!("cannot inspect work root {shown} ({error}); {fix}"))?;
    if !meta.is_dir() || (meta.dev(), meta.ino()) != expected {
        bail!(
            "work root {shown} was replaced while amplihack checked it, so its mode was not \
             changed; check the folder and run again"
        );
    }
    if meta.uid() != euid() || meta.mode() & 0o022 != 0 {
        bail!(
            "work root {shown} changed owner or mode while amplihack checked it, so its mode \
             was not changed; check the folder and run again"
        );
    }
    folder
        .set_permissions(Permissions::from_mode(0o700))
        .map_err(|error| {
            anyhow::anyhow!("cannot set work root {shown} to mode 0700 ({error}); {fix}")
        })?;
    eprintln!(
        "amplihack: work root {shown} had mode {:04o}; set it to 0700, the mode the recovery \
         store requires",
        meta.mode() & 0o7777
    );
    Ok(())
}

/// `path` as one shell word, for a suggested command or an
/// `AMPLIHACK_WORK_ROOT` value: unchanged when every byte is a letter, a
/// digit or one of `/._+,:@%=-`, otherwise single-quoted with `'` written as
/// `'\''`. `None` for a path that is not valid UTF-8 or holds a control
/// character, whose printed form cannot be pasted back.
pub(crate) fn shell_word(path: &Path) -> Option<String> {
    debug_assert!(
        path.is_absolute(),
        "only absolute paths are suggested, so a word never starts with `-`, `~` or `=`"
    );
    let text = path.to_str()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    let plain = text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._+,:@%=-".contains(&b));
    Some(if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    })
}

/// "fix it with `<command> <path>`", or the instruction in words when the
/// path cannot be given as a shell word.
fn command_or_words(command: &str, path: &Path, words: &str) -> String {
    match shell_word(path) {
        Some(word) => format!("fix it with `{command} {word}`"),
        None => words.to_string(),
    }
}

/// "Set AMPLIHACK_WORK_ROOT to <path>" for the folder `link` leads to with
/// `rest` below it, or the instruction in words when it cannot be resolved
/// or given as a shell word.
fn resolved_root(link: &Path, rest: &Path) -> String {
    let resolved = fs::canonicalize(link)
        .ok()
        .map(|real| {
            if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            }
        })
        .and_then(|real| shell_word(&real));
    match resolved {
        Some(word) => format!("Set AMPLIHACK_WORK_ROOT to {word}"),
        None => "Set AMPLIHACK_WORK_ROOT to the folder the link leads to".to_string(),
    }
}

fn target_text(target: Option<&Path>) -> String {
    match target {
        Some(target) => sanitize_path(target),
        None => "a target that cannot be read".to_string(),
    }
}

/// Which bits make a folder writable by others, in words, and the fix.
fn writable_words(mode: u32) -> (&'static str, &'static str, &'static str) {
    match (mode & 0o020 != 0, mode & 0o002 != 0) {
        (true, true) => (
            "group-writable and world-writable",
            "chmod go-w",
            "remove group and other write from this folder",
        ),
        (false, true) => (
            "world-writable",
            "chmod o-w",
            "remove other write from this folder",
        ),
        _ => (
            "group-writable",
            "chmod g-w",
            "remove group write from this folder",
        ),
    }
}

/// Where a fault sits on the path to `root`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Above,
    Root,
    Runs,
}

fn place(root: &Path, fault: &Fault) -> Place {
    if fault.path() == root {
        Place::Root
    } else if fault.path() == root.join("runs") {
        Place::Runs
    } else {
        Place::Above
    }
}

/// The reason given on the first line for a group of faults.
fn reason(place: Place, fault: &Fault) -> &'static str {
    match (place, fault) {
        (Place::Runs, _) => "runs folder is not private",
        (Place::Root, _) => "the work root itself is not usable",
        (Place::Above, Fault::Writable { .. }) => "other users can change a folder above it",
        (Place::Above, Fault::Link { .. }) => "a folder above it is a symbolic link",
        (Place::Above, Fault::NotFolder { .. }) => "a step of its path is not a folder",
        (Place::Above, _) => "a folder above it cannot be inspected",
    }
}

/// One line about a fault on the work root itself, in the form of the work
/// root validation's messages.
fn root_line(root: &Path, fault: &Fault) -> String {
    let shown = sanitize_path(root);
    match fault {
        Fault::Link { target, .. } => format!(
            "work root {shown} is a symbolic link to {}; the recovery store needs a folder, not a \
             link. {}",
            target_text(target.as_deref()),
            resolved_root(root, Path::new(""))
        ),
        Fault::NotFolder { .. } => format!("work root {shown} is not a folder"),
        Fault::NotOwned { uid, .. } => {
            format!("work root {shown} is not owned by you (it belongs to uid {uid})")
        }
        Fault::NotPrivate { mode, .. } => format!(
            "work root {shown} is writable by group or others (mode {:04o}); {}",
            mode & 0o7777,
            command_or_words(
                "chmod 700",
                root,
                "give it mode 0700 and remove its group and other access",
            )
        ),
        Fault::Inspect { error, .. } => format!("work root {shown} cannot be inspected ({error})"),
        Fault::Writable { mode, .. } => format!(
            "work root {shown} is writable by group or others (mode {:04o})",
            mode & 0o7777
        ),
        Fault::NonNormal { .. } => format!(
            "work root {shown} must be an absolute path below `/` without `.` or `..` components"
        ),
    }
}

/// One indented line about a fault above the work root or on `runs/`.
fn detail_line(root: &Path, place: Place, fault: &Fault) -> String {
    let shown = sanitize_path(fault.path());
    match fault {
        Fault::Writable { path, mode } => {
            let (bits, command, words) = writable_words(*mode);
            format!(
                "  {shown} is {bits} (mode {:04o}) and has no sticky bit; {}",
                mode & 0o7777,
                command_or_words(command, path, words)
            )
        }
        Fault::NotPrivate { path, mode } => format!(
            "  {shown} has mode {:04o}; {}",
            mode & 0o7777,
            command_or_words("chmod 700", path, "give it mode 0700")
        ),
        Fault::NotOwned { uid, .. } => {
            format!("  {shown} is not owned by you (it belongs to uid {uid})")
        }
        Fault::Link { target, .. } if place == Place::Runs => format!(
            "  {shown} is a symbolic link to {}; the recovery store needs a folder, not a link; \
             replace it with a folder of mode 0700",
            target_text(target.as_deref())
        ),
        Fault::Link { path, target } => {
            let rest = root.strip_prefix(path).unwrap_or(Path::new(""));
            format!(
                "  {shown} is a symbolic link to {}; the recovery store needs a folder, not a \
                 link, at each step of the path. {}",
                target_text(target.as_deref()),
                resolved_root(path, rest)
            )
        }
        Fault::NotFolder { .. } => format!("  {shown} is not a folder"),
        Fault::Inspect { error, .. } => format!("  {shown} cannot be inspected ({error})"),
        Fault::NonNormal { .. } => format!("  {shown} is not a normal absolute path"),
    }
}

/// The message for `faults`, outermost first: a line naming the work root and
/// the reasons, then one indented line per folder with its mode and fix. A
/// fault on the work root alone is given as the work root validation gives
/// its messages, on one line.
fn describe(root: &Path, faults: &[Fault]) -> String {
    if let [fault] = faults
        && matches!(fault, Fault::NonNormal { .. })
    {
        return root_line(root, fault);
    }
    if let [fault] = faults
        && place(root, fault) == Place::Root
    {
        return root_line(root, fault);
    }
    let mut reasons: Vec<&str> = Vec::new();
    for fault in faults {
        let reason = reason(place(root, fault), fault);
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    let mut lines = vec![format!(
        "cannot open the recovery store under work root {}: {}",
        sanitize_path(root),
        reasons.join("; ")
    )];
    for fault in faults {
        lines.push(match place(root, fault) {
            Place::Root => format!("  {}", root_line(root, fault)),
            other => detail_line(root, other, fault),
        });
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "tests_recovery_path.rs"]
mod tests;
