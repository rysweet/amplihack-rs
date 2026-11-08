//! `stage-strict LIST`: strict callers supply a complete NUL-terminated list
//! of literal paths. Validate all of it before the first index write, then
//! pass the same bytes to `git add`.

use super::repo::{canonical, os, split_nul, status, strip_newlines};
use super::{Fail, Res};
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn refusal(reason: &str) -> Fail {
    Fail::Refusal(reason.to_owned())
}

fn os_error(error: std::io::Error) -> Fail {
    Fail::Refusal(error.to_string())
}

/// Run Git with the inherited environment, less the repository variables
/// `amplihack_git::command()` removes. Input is written on its own
/// thread while the output is read. Exit 79 passes through; any other
/// failure shows Git's stderr and refuses.
fn git(args: &[&OsStr], data: Option<&[u8]>) -> Res<Vec<u8>> {
    let mut child = amplihack_git::command()
        .arg("--literal-pathspecs")
        .args(args)
        .stdin(if data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(os_error)?;
    let stdin = child.stdin.take();
    let output = std::thread::scope(|scope| {
        if let (Some(data), Some(mut stdin)) = (data, stdin) {
            scope.spawn(move || {
                // A write error shows up as Git's own failure status.
                let _ = stdin.write_all(data);
            });
        }
        child.wait_with_output()
    })
    .map_err(os_error)?;
    match status(&output) {
        0 => Ok(output.stdout),
        79 => Err(Fail::Terminal79),
        _ => {
            let _ = std::io::stderr().write_all(&output.stderr);
            Err(refusal("Git command failed"))
        }
    }
}

fn validate(list: &Path) -> Res<(PathBuf, Vec<u8>, usize)> {
    let fail = |reason: &str| Err(refusal(reason));
    let info = std::fs::symlink_metadata(list).map_err(os_error)?;
    if !info.file_type().is_file() {
        return fail("path list must be a regular file");
    }
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(list)
            .map_err(os_error)?,
        &mut raw,
    )
    .map_err(os_error)?;
    if !raw.is_empty() && !raw.ends_with(&[0]) {
        return fail("unterminated path list");
    }
    let paths: Vec<&[u8]> = if raw.is_empty() {
        Vec::new()
    } else {
        raw[..raw.len() - 1].split(|&b| b == 0).collect()
    };
    if paths.iter().collect::<BTreeSet<_>>().len() != paths.len() {
        return fail("duplicate path");
    }
    let top = canonical(Path::new(OsStr::from_bytes(&strip_newlines(git(
        &[os("rev-parse"), os("--show-toplevel")],
        None,
    )?))));
    let tracked = split_nul(&git(
        &[os("-C"), top.as_os_str(), os("ls-files"), os("-z")],
        None,
    )?);
    let mut directory_entries: HashMap<PathBuf, BTreeSet<Vec<u8>>> = HashMap::new();
    for path in &paths {
        if path.is_empty()
            || path.starts_with(b"/")
            || path
                .split(|&b| b == b'/')
                .any(|p| p.is_empty() || p == b"." || p == b"..")
        {
            return fail("path must be an exact worktree-relative identity");
        }
        let components: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
        let mut parent = top.clone();
        for (number, component) in components.iter().enumerate() {
            // A case-insensitive filesystem can resolve a different spelling.
            // Compare directory-entry bytes without normalisation or folding.
            if parent.is_dir() {
                if !directory_entries.contains_key(&parent) {
                    let names = std::fs::read_dir(&parent)
                        .map_err(os_error)?
                        .flatten()
                        .map(|e| e.file_name().as_bytes().to_vec())
                        .collect();
                    directory_entries.insert(parent.clone(), names);
                }
                let child = parent.join(OsStr::from_bytes(component));
                if !directory_entries[&parent].contains(*component) && super::repo::lexists(&child)
                {
                    return fail("path spelling differs from filesystem identity");
                }
            }
            if number == components.len() - 1 {
                break;
            }
            parent.push(OsStr::from_bytes(component));
            if super::repo::is_link(&parent) {
                return fail("symlink parent is unsupported");
            }
            if super::repo::lexists(&parent) && !parent.is_dir() {
                return fail("non-directory parent");
            }
        }
        let full = top.join(OsStr::from_bytes(path));
        match std::fs::symlink_metadata(&full) {
            Ok(meta) => {
                let kind = meta.file_type();
                if !(kind.is_file() || kind.is_symlink()) {
                    return fail("directory or special-file selector");
                }
            }
            Err(_) if !tracked.contains(*path) => return fail("missing untracked path"),
            Err(_) => {}
        }
    }
    let count = paths.len();
    Ok((top, raw, count))
}

pub(super) fn stage_strict(args: &[OsString]) -> Res<Vec<u8>> {
    let [list] = args else {
        return Err(refusal("expected one path list"));
    };
    let (top, raw, count) = validate(Path::new(list))?;
    if count > 0 {
        git(
            &[
                os("-C"),
                top.as_os_str(),
                os("add"),
                os("--pathspec-from-file=-"),
                os("--pathspec-file-nul"),
            ],
            Some(&raw),
        )?;
    }
    Ok(Vec::new())
}
