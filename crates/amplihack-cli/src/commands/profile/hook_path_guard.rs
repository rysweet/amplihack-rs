//! Issue #1699: checks an `amplihack-hooks` path before it is written into a
//! profile's `settings.json`. See
//! docs/howto/keep-tests-from-writing-real-profiles.md for the rules and the
//! exact reason strings.
//!
//! An `amplihack install` run under a temp HOME could still find the real
//! profile root (an inherited `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or a
//! `profile-root` file pointing outside the temp HOME) and wrote its temp
//! hooks path into real profiles. Once the temp dir was gone every session in
//! those profiles ran a missing hook, and another local user could recreate
//! that path and have it run as this user.
//!
//! Two rules, checked in order:
//! - R3 ([`check_hook_binary`]): the binary exists and is safe to run.
//! - R4 ([`check_hook_path`]): a binary under the system temp dir is only
//!   written into a profile that is under the temp dir too.
//!
//! The checks run just before the write. A binary swapped between the check
//! and a later Claude session is not detected (accepted race, S2).
//!
//! Reasons are built only from paths, never from settings contents, command
//! strings or environment values.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// R3: the binary is absolute, a regular file after following symlinks and,
/// on Unix, executable, owned by the euid or root, and not writable by group
/// or others. The first failing rule is the error.
pub(crate) fn check_hook_binary(hooks_bin: &Path) -> Result<(), String> {
    if !hooks_bin.is_absolute() {
        return Err(format!(
            "hooks binary {hooks_bin:?} is not an absolute path"
        ));
    }
    let meta = match std::fs::metadata(hooks_bin) {
        Ok(meta) if meta.is_file() => meta,
        _ => return Err(format!("hooks binary {hooks_bin:?} does not exist")),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = meta.mode();
        if mode & 0o111 == 0 {
            return Err(format!("hooks binary {hooks_bin:?} is not executable"));
        }
        let uid = meta.uid();
        // SAFETY: geteuid has no preconditions and cannot fail.
        if uid != 0 && uid != unsafe { libc::geteuid() } {
            return Err(format!(
                "hooks binary {hooks_bin:?} is owned by uid {uid}, not by you or root"
            ));
        }
        if mode & 0o022 != 0 {
            return Err(format!(
                "hooks binary {hooks_bin:?} is writable by group or others"
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    Ok(())
}

/// R3, then R4: a binary under `tmp_dir` may only be written into a
/// `target_dir` that is under `tmp_dir` too. Both sides are canonicalised
/// (`tmp_dir` falls back to itself); an uncanonicalisable `target_dir` counts
/// as not temporary, so a temporary binary is refused (fail closed). Paths
/// are compared by component, so `/tmpfoo` is not under `/tmp`.
pub(crate) fn check_hook_path(
    hooks_bin: &Path,
    target_dir: &Path,
    tmp_dir: &Path,
) -> Result<(), String> {
    check_hook_binary(hooks_bin)?;
    let bin = std::fs::canonicalize(hooks_bin)
        .map_err(|_| format!("hooks binary {hooks_bin:?} does not exist"))?;
    let tmp = std::fs::canonicalize(tmp_dir).unwrap_or_else(|_| tmp_dir.to_path_buf());
    if !bin.starts_with(&tmp) {
        return Ok(());
    }
    let target_is_temp = std::fs::canonicalize(target_dir).is_ok_and(|dir| dir.starts_with(&tmp));
    if target_is_temp {
        Ok(())
    } else {
        Err(format!(
            "hooks binary {hooks_bin:?} is under temporary dir {tmp_dir:?} but profile \
             {target_dir:?} is not"
        ))
    }
}

/// Seeding (writer b): removes every hook handler whose program is an
/// `amplihack-hooks` path that fails [`check_hook_path`] for `target_dir`.
/// Returns the removed paths. Wrappers, events and a `hooks` object emptied
/// by a removal are removed too; everything else, including `statusLine`, is
/// left alone, and shapes that are not the expected JSON are skipped.
pub(crate) fn strip_unsafe_hooks_with(
    settings: &mut Map<String, Value>,
    target_dir: &Path,
    tmp_dir: &Path,
) -> Vec<String> {
    let mut removed = Vec::new();
    let Some(Value::Object(events)) = settings.get_mut("hooks") else {
        return removed;
    };
    let mut emptied_events = Vec::new();
    for (event, wrappers) in events.iter_mut() {
        let Value::Array(wrappers) = wrappers else {
            continue;
        };
        let before = removed.len();
        wrappers.retain_mut(|wrapper| {
            let Some(Value::Array(handlers)) = wrapper.get_mut("hooks") else {
                return true;
            };
            let count = removed.len();
            handlers.retain(
                |handler| match unsafe_hook_path(handler, target_dir, tmp_dir) {
                    Some(path) => {
                        removed.push(path);
                        false
                    }
                    None => true,
                },
            );
            removed.len() == count || !handlers.is_empty()
        });
        if removed.len() > before && wrappers.is_empty() {
            emptied_events.push(event.clone());
        }
    }
    for event in &emptied_events {
        events.remove(event);
    }
    if !emptied_events.is_empty() && events.is_empty() {
        settings.remove("hooks");
    }
    removed
}

/// [`strip_unsafe_hooks_with`] with `std::env::temp_dir()`.
pub(crate) fn strip_unsafe_hooks(
    settings: &mut Map<String, Value>,
    target_dir: &Path,
) -> Vec<String> {
    strip_unsafe_hooks_with(settings, target_dir, &std::env::temp_dir())
}

/// The handler's `amplihack-hooks` path when it fails the check. A bare
/// `amplihack-hooks` (found through `PATH` at run time) has no path to check
/// and is kept; a relative path containing a separator fails R3.
fn unsafe_hook_path(handler: &Value, target_dir: &Path, tmp_dir: &Path) -> Option<String> {
    let command = handler.get("command")?.as_str()?;
    let program = first_token(command)?;
    let has_separator = program.contains('/') || (cfg!(windows) && program.contains('\\'));
    if !has_separator || !is_hooks_binary_name(&program) {
        return None;
    }
    check_hook_path(&PathBuf::from(&program), target_dir, tmp_dir)
        .err()
        .map(|_| program)
}

fn is_hooks_binary_name(program: &str) -> bool {
    let name = Path::new(program).file_name().and_then(|n| n.to_str());
    name == Some("amplihack-hooks") || (cfg!(windows) && name == Some("amplihack-hooks.exe"))
}

/// The program of a hook command. A quoted program is unescaped as written by
/// `install/hooks.rs::shell_quote_path` (`\\` and `\"` inside double quotes);
/// an unterminated quote yields `None`. An unquoted program ends at the first
/// whitespace.
fn first_token(command: &str) -> Option<String> {
    let command = command.trim_start();
    let Some(rest) = command.strip_prefix('"') else {
        let token = command.split_whitespace().next()?;
        return Some(token.to_string());
    };
    let mut token = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return (!token.is_empty()).then_some(token),
            '\\' => token.push(chars.next()?),
            _ => token.push(c),
        }
    }
    None
}
