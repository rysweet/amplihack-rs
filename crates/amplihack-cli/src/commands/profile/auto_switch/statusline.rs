//! The status-line wrapper Claude Code runs after each turn, and `install`,
//! which puts it into every profile's `settings.json`.
//!
//! The wrapper records the `rate_limits` reading, runs the profile's original
//! status-line command with the same input and passes its output through
//! unchanged, then runs the swap check. It never writes to stdout itself.

use super::config;
use super::fsx;
use super::state::State;
use super::swap;
use super::usage::{self, Attribution};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Claude passes a small JSON object; anything this large is passed through
/// to the original command but not parsed.
const MAX_PASSTHROUGH: u64 = 16 * 1024 * 1024;
const MAX_SETTINGS: u64 = 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_millis(50);
const SETTINGS: &str = "settings.json";
const MARKER: &str = "profile statusline";

/// Runs one status-line update and returns the exit code to use.
pub fn run(root_arg: &str, owner_arg: &str) -> i32 {
    std::panic::set_hook(Box::new(|_| {
        eprintln!("auto-switch: internal error in the status-line wrapper");
    }));
    // fail-quiet: status-line output contract. An unreadable stdin still runs
    // the original status line, with no input.
    let input = fsx::read_bounded(std::io::stdin().lock(), MAX_PASSTHROUGH).unwrap_or_default();
    let state = open_state(root_arg);
    let owner = ProfileName::parse(owner_arg).ok();
    let session = match (&state, &owner) {
        (Some(s), Some(o)) if session_matches(s, o) => Some((s, o)),
        _ => None,
    };

    let mut reading = None;
    if let Some((state, owner)) = session {
        reading = std::panic::catch_unwind(|| record(state, owner, &input)).unwrap_or(None);
    }

    let code = match (&state, &owner) {
        (Some(s), Some(o)) => run_original(s, o, &input),
        _ => 0,
    };

    if let (Some((state, owner)), Some(rl)) = (session, reading) {
        let _ = std::panic::catch_unwind(|| swap_step(state, owner, &rl));
    }
    code
}

/// The state for `root_arg`, or `None` after one stderr line naming why the
/// root is unusable. Stdout stays empty either way (status-line contract).
fn open_state(root_arg: &str) -> Option<State> {
    let root = Path::new(root_arg);
    let opened = if root.is_absolute() {
        State::new(root).map_err(|e| e.to_string()).and_then(|s| {
            match fsx::check_root_private(&s.root) {
                Ok(()) => Ok(s),
                Err(e) => Err(e.to_string()),
            }
        })
    } else {
        Err("is not an absolute path".to_string())
    };
    match opened {
        Ok(s) => Some(s),
        Err(why) => {
            eprintln!(
                "auto-switch: status line disabled: profile root {}: {why}",
                root.display()
            );
            None
        }
    }
}

/// The wrapper acts only for the session it was installed for: Claude's
/// `CLAUDE_CONFIG_DIR` must be `<root>/<owner>`.
fn session_matches(state: &State, owner: &ProfileName) -> bool {
    let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) else {
        return false;
    };
    match (
        PathBuf::from(dir).canonicalize(),
        state.profile_dir(owner.as_str()).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Step 1: recovery, and the reading when the switch is on.
fn record(state: &State, owner: &ProfileName, input: &[u8]) -> Option<usage::RateLimits> {
    let on = config::effective(&state.root).on;
    if !on && !state.exists() {
        return None;
    }
    let lock = match state.try_lock(LOCK_WAIT) {
        Ok(Some(l)) => l,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("auto-switch: {e}");
            return None;
        }
    };
    if let Err(e) = swap::recover(state) {
        eprintln!("auto-switch: {e}");
        return None;
    }
    if !on {
        return None;
    }
    let now = fsx::now();
    let rl = usage::parse_statusline_input(input, now)?;
    let ledger = state.load_ledger().ok()?;
    if let Attribution::Record(account) =
        usage::attribute(owner.as_str(), ledger.hosts.get(owner.as_str()), &rl, now)
    {
        let mut file = state.load_usage();
        let merged = usage::merge(file.accounts.get(&account), usage::reading_from(&rl, now));
        file.accounts.insert(account, merged);
        if let Err(e) = state.save_usage(&file) {
            eprintln!("auto-switch: {e}");
        }
    }
    drop(lock);
    Some(rl)
}

/// Step 3: the swap check, with the setting read again.
fn swap_step(state: &State, owner: &ProfileName, rl: &usage::RateLimits) {
    let eff = config::effective(&state.root);
    if !eff.on {
        return;
    }
    let Ok(Some(_lock)) = state.try_lock(LOCK_WAIT) else {
        return;
    };
    let profiles = match ProfileStore::new(state.root.clone()).list() {
        Ok(names) => names
            .iter()
            .filter_map(|n| ProfileName::parse(n).ok())
            .collect::<Vec<_>>(),
        // fail-quiet: status-line output contract. No list, no swap this
        // turn; the next turn tries again.
        Err(_) => return,
    };
    if let Err(e) = swap::swap_check(state, &profiles, owner, rl, &eff.config) {
        eprintln!("auto-switch: {e}");
    }
}

/// Step 2: the profile's original status line, run with `sh -c` like Claude
/// does, with the same input. Its stdout and stderr are the wrapper's.
fn run_original(state: &State, owner: &ProfileName, input: &[u8]) -> i32 {
    if !state.exists() {
        return 0;
    }
    let saved = match state.read_sub_json("statusline", owner.as_str()) {
        Ok(Some(v)) => v,
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("auto-switch: {e}; run `amplihack profile auto-switch repair`");
            return 0;
        }
    };
    let Some(command) = saved.get("command").and_then(|c| c.as_str()) else {
        return 0;
    };
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("auto-switch: could not run the original status line: {e}");
            return 0;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    }
}

// ---------------------------------------------------------------------------
// install / repair
// ---------------------------------------------------------------------------

/// The detail shown for a wrapper that is not the canonical one.
const DIFFERENT_BINARY: &str = "(wrapper pointed at a different binary)";
const EDITED: &str = "(wrapper command differs from the canonical one)";

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn quotable(p: &Path) -> Result<&str, ProfileError> {
    p.to_str()
        .filter(|s| !s.contains(['\0', '\n', '\r']))
        .ok_or_else(|| {
            ProfileError::AutoSwitchInstall(format!(
                "{} contains bytes that cannot be quoted for sh",
                p.display()
            ))
        })
}

/// The exact command `install` writes for `owner`.
pub fn wrapper_command(
    exe: &Path,
    root: &Path,
    owner: &ProfileName,
) -> Result<String, ProfileError> {
    Ok(format!(
        "{} profile statusline --root {} --owner {}",
        sh_quote(quotable(exe)?),
        sh_quote(quotable(root)?),
        sh_quote(owner.as_str())
    ))
}

/// The running amplihack binary, refused if anyone but the user (or root) can
/// replace it: the binary and every directory above it must be owned by the
/// user or root and not writable by other users.
pub fn installed_binary() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let mut cur = Some(exe.as_path());
    while let Some(p) = cur {
        let m = std::fs::symlink_metadata(p)?;
        let owner_ok = m.uid() == fsx::euid() || m.uid() == 0;
        if !owner_ok || fsx::writable_by_others(&m) {
            anyhow::bail!(
                "refusing to install the status-line wrapper: {} can be changed by other users\n\
                 hint: install amplihack in a directory only you (or root) can write to",
                p.display()
            );
        }
        cur = p.parent();
    }
    Ok(exe)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapperState {
    Installed,
    Missing,
    Stale,
    Unsafe,
}

impl std::fmt::Display for WrapperState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Installed => "installed",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Unsafe => "unsafe",
        })
    }
}

fn read_settings(
    dir: &Path,
    name: &ProfileName,
) -> Result<serde_json::Map<String, serde_json::Value>, ProfileError> {
    let shown = format!("{name}/{SETTINGS}");
    match fsx::read_bounded_nofollow(&dir.join(SETTINGS), MAX_SETTINGS) {
        Ok(None) => Ok(serde_json::Map::new()),
        Ok(Some((raw, meta))) => {
            if meta.uid() != fsx::euid() {
                return Err(ProfileError::AutoSwitchInstall(format!(
                    "{shown}: owned by another user"
                )));
            }
            match serde_json::from_slice(&raw) {
                Ok(serde_json::Value::Object(m)) => Ok(m),
                Ok(_) => Err(ProfileError::AutoSwitchInstall(format!(
                    "{shown}: not a JSON object"
                ))),
                Err(e) => Err(ProfileError::AutoSwitchInstall(format!(
                    "{shown}: {}",
                    super::state::parse_position(&e)
                ))),
            }
        }
        Err(e) => Err(ProfileError::AutoSwitchInstall(format!("{shown}: {e}"))),
    }
}

fn command_of(sl: &serde_json::Value) -> Option<&str> {
    sl.get("command").and_then(|c| c.as_str())
}

/// Profiles whose wrapper is `missing` or `stale`, for the `on` hint and the
/// `status` note.
pub fn without_wrapper(store: &ProfileStore, root: &Path, names: &[ProfileName]) -> Vec<String> {
    names
        .iter()
        .filter(|n| {
            matches!(
                wrapper_state(store, root, n),
                WrapperState::Missing | WrapperState::Stale
            )
        })
        .map(ProfileName::to_string)
        .collect()
}

/// The wrapper state of one profile, for `status`.
pub fn wrapper_state(store: &ProfileStore, root: &Path, name: &ProfileName) -> WrapperState {
    let Ok(dir) = store.resolve(name) else {
        return WrapperState::Missing;
    };
    let Ok(settings) = read_settings(&dir, name) else {
        return WrapperState::Stale;
    };
    let Some(cmd) = settings.get("statusLine").and_then(command_of) else {
        return WrapperState::Missing;
    };
    if !cmd.contains(MARKER) {
        return WrapperState::Missing;
    }
    let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) else {
        return WrapperState::Stale;
    };
    match wrapper_command(&exe, root, name) {
        Ok(want) if want == cmd => {
            if installed_binary().is_err() {
                WrapperState::Unsafe
            } else {
                WrapperState::Installed
            }
        }
        _ => WrapperState::Stale,
    }
}

/// `install`, or with `repoint` set, `repair`'s install step: stale wrappers
/// are rewritten to the canonical one instead of only being reported.
pub fn install(store: &ProfileStore, repoint: bool) -> anyhow::Result<()> {
    let names = store.list()?;
    let state = State::new(store.root())?;
    let exe = installed_binary()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        eprintln!(
            "warning: {} looks like a build output; the wrapper stops working if it is rebuilt or removed",
            exe.display()
        );
    }
    println!("root: {}", state.root.display());
    if names.is_empty() {
        eprintln!(
            "warning: no profiles under this root; is AMPLIHACK_CLAUDE_PROFILE_ROOT set to the root your launches use?"
        );
        return Ok(());
    }
    state.ensure()?;
    let width = names.iter().map(String::len).max().unwrap_or(0);
    for raw in names {
        let name = ProfileName::parse(&raw)?;
        let (outcome, detail) = install_one(store, &state, &exe, &name, repoint)?;
        let line = format!("{outcome:<10} {:<width$} {detail}", name.as_str());
        println!("{}", line.trim_end());
    }
    Ok(())
}

fn install_one(
    store: &ProfileStore,
    state: &State,
    exe: &Path,
    name: &ProfileName,
    repoint: bool,
) -> Result<(&'static str, String), ProfileError> {
    let dir = store.verify_profile_dir(name)?;
    let mut settings = read_settings(&dir, name)?;
    let want = wrapper_command(exe, &state.root, name)?;
    let current = settings.get("statusLine").cloned();
    let (padding, outcome) = match &current {
        Some(sl)
            if sl.get("type").and_then(|t| t.as_str()) == Some("command")
                && command_of(sl) == Some(want.as_str()) =>
        {
            return Ok(("unchanged", String::new()));
        }
        Some(sl) if command_of(sl).is_some_and(|c| c.contains(MARKER)) => {
            // A wrapper, never saved as the original.
            let ours = command_of(sl).is_some_and(|c| {
                quotable(exe).is_ok_and(|e| c.starts_with(&format!("{} ", sh_quote(e))))
            });
            let why = if ours { EDITED } else { DIFFERENT_BINARY };
            if !repoint {
                return Ok((
                    "stale",
                    format!("{why} — run: amplihack profile auto-switch repair"),
                ));
            }
            (sl.get("padding").cloned(), ("repaired", why))
        }
        Some(sl) => {
            state.write_sub_json("statusline", name.as_str(), sl)?;
            (
                sl.get("padding").cloned(),
                ("installed", "(wrapped existing status line)"),
            )
        }
        None => {
            let saved = state
                .read_sub_json("statusline", name.as_str())
                .ok()
                .flatten();
            let detail = if saved.is_some() {
                "(restored the saved status line)"
            } else {
                "(no previous status line)"
            };
            (
                saved.and_then(|s| s.get("padding").cloned()),
                ("installed", detail),
            )
        }
    };
    let mut sl = serde_json::Map::new();
    sl.insert("type".into(), "command".into());
    sl.insert("command".into(), want.into());
    if let Some(p) = padding {
        sl.insert("padding".into(), p);
    }
    settings.insert("statusLine".into(), serde_json::Value::Object(sl));
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(settings))
        .map_err(|e| ProfileError::AutoSwitchInstall(e.to_string()))?;
    fsx::write_atomic_keep_mode(&dir, SETTINGS, &bytes)
        .map_err(|e| ProfileError::AutoSwitchInstall(format!("{name}/{SETTINGS}: {e}")))?;
    Ok((outcome.0, outcome.1.to_string()))
}

#[cfg(test)]
#[path = "tests_statusline.rs"]
mod tests;
