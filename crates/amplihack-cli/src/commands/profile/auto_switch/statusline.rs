//! The status-line wrapper Claude Code runs after each turn, and `install`,
//! which puts it into every profile's `settings.json`.
//!
//! The wrapper records the `rate_limits` reading, runs the profile's original
//! status-line command with the same input and passes its output through
//! unchanged, then runs the swap check. The only things it writes to stdout
//! itself come after the original output: the login capacity line for the
//! account live in the profile (issue #1745), then the no-capacity warning
//! line when every account is over the auto-switch threshold (see
//! `capacity`).
//!
//! Without `--owner` the mode comes from `CLAUDE_CONFIG_DIR`; see
//! `home_line`. A run in a profile's directory without `--owner` opens and
//! moves no login file: it never recovers, never swaps and never refreshes
//! the capacity file, so a profile that only inherited the `~/.claude` entry
//! stays out of all three.
//! The `--owner` wrapper resolves profile kinds, which opens login files,
//! only for a swap check that needs them and when [`refresh_due`] says so.
//! Its recovery step moves login files when a swap was interrupted or a
//! hosting session has ended; see "Credentials" in
//! docs/reference/STATUSLINE.md.
//!
//! [`refresh_due`]: super::capacity::refresh_due

use super::MARKER;
use super::config;
use super::fsx;
use super::home_line::{self, Mode};
use super::state::State;
use super::statusline_entry::is_home_statusline;
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
/// How long a reading waits for the state lock before giving up.
pub(super) const LOCK_WAIT: Duration = Duration::from_millis(50);
const SETTINGS: &str = "settings.json";
/// Set for every command the wrapper runs. A user's original may itself run
/// `amplihack profile statusline` (`... | my-filter`); that inner run sees
/// this and runs `statusline.sh` only, because in the same
/// `CLAUDE_CONFIG_DIR` it would otherwise run the same original again,
/// without end.
const NESTED: &str = "AMPLIHACK_STATUSLINE_NESTED";

/// Runs one status-line update and returns the exit code to use.
pub fn run(root_arg: Option<&str>, owner_arg: Option<&str>) -> i32 {
    std::panic::set_hook(Box::new(|_| {
        eprintln!("auto-switch: internal error in the status-line wrapper");
    }));
    // fail-quiet: status-line output contract. An unreadable stdin still runs
    // the original status line, with no input.
    let input = fsx::read_bounded(std::io::stdin().lock(), MAX_PASSTHROUGH).unwrap_or_default();
    if std::env::var_os(NESTED).is_some() {
        return stream_command(home_line::script_command().as_deref(), &input, &[]);
    }
    if let Some(owner) = owner_arg {
        // `--root` as given (a relative one is refused with a reason), else
        // the root this environment resolves to.
        let root = match root_arg {
            Some(r) => Some(PathBuf::from(r)),
            None => home_line::root_for(None),
        };
        return owned(root.as_deref(), owner, false, &input);
    }
    let root = home_line::root_for(root_arg);
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    let home = crate::commands::profile::root_config::home_from_env();
    match home_line::mode(config_dir.as_deref(), home.as_deref(), root.as_deref()) {
        Mode::Home => home_line::run(root.as_deref(), &input),
        Mode::Owned { root, owner } => owned(Some(&root), &owner, true, &input),
        Mode::Passthrough => stream_command(home_line::script_command().as_deref(), &input, &[]),
    }
}

/// A profile's own session: `--owner`, or (`implicit`) a run without it
/// whose `CLAUDE_CONFIG_DIR` is the profile's directory. The checks and
/// recording are the same either way; only the installed `--owner` wrapper
/// runs recovery, which can move login files, and the swap check and the
/// capacity-file refresh, which resolve profile kinds and so open them.
fn owned(root: Option<&Path>, owner_arg: &str, implicit: bool, input: &[u8]) -> i32 {
    let state = match root {
        Some(r) => open_state(r),
        None => {
            eprintln!("auto-switch: status line disabled: no profile root");
            None
        }
    };
    let owner = ProfileName::parse(owner_arg).ok();
    let session = match (&state, &owner) {
        (Some(s), Some(o)) if session_matches(s, o) => Some((s, o)),
        _ => None,
    };

    let mut reading = None;
    if let Some((state, owner)) = session {
        reading =
            std::panic::catch_unwind(|| record(state, owner, input, !implicit)).unwrap_or(None);
    }

    // After the reading, so this tick's own reading counts.
    let notice = state.as_ref().and_then(|s| {
        std::panic::catch_unwind(|| super::capacity::statusline_line(&s.root))
            .ok()
            .flatten()
    });
    let saved = match (&state, &owner) {
        (Some(s), Some(o)) => saved_original(s, o),
        _ => None,
    };
    // The capacity line stands in for the home command's: it is added only
    // without `--owner`, or when the saved original is the home command or
    // the `statusline.sh` saved in its place. A wrapper around any other
    // original passes that output through unchanged and adds nothing, and a
    // wrapper with no saved original prints nothing at all, which the #1569
    // wrapper tests require. This is narrower than issue #1745 asked for;
    // see "Differences from issue #1745" in docs/reference/STATUSLINE.md.
    let replaces_home = implicit
        || saved.as_deref().is_some_and(|c| {
            is_home_statusline(c) || home_line::script_command().as_deref() == Some(c)
        });
    let key = session
        .filter(|_| replaces_home)
        .and_then(|(state, owner)| {
            std::panic::catch_unwind(|| home_line::owned_line(state, owner, input, fsx::now()))
                .ok()
                .flatten()
        });
    // The home command is never run as an original: in its place, and
    // without `--owner` when nothing is saved, `statusline.sh` runs.
    let original = match saved {
        Some(c) if !is_home_statusline(&c) => Some(c),
        Some(_) => home_line::script_command(),
        None if implicit => home_line::script_command(),
        None => None,
    };
    let code = if implicit || (state.is_some() && owner.is_some()) {
        stream_command(original.as_deref(), input, &[key, notice])
    } else {
        0
    };

    if let (false, Some((state, owner)), Some(rl)) = (implicit, session, reading) {
        let _ = std::panic::catch_unwind(|| swap_step(state, owner, &rl));
    }
    code
}

/// The state for `root`, or `None` after one stderr line naming why the
/// root is unusable. Stdout stays empty either way (status-line contract).
fn open_state(root: &Path) -> Option<State> {
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

/// Step 1: the reading when the switch is on. With `wrapper` (the `--owner`
/// wrapper), recovery first and the capacity file too when this reading can
/// change it. Without it nothing moves a login, so a run that finds a swap
/// journal records nothing: which login is live here is not settled.
fn record(
    state: &State,
    owner: &ProfileName,
    input: &[u8],
    wrapper: bool,
) -> Option<usage::RateLimits> {
    let eff = config::effective(&state.root);
    let on = eff.on;
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
    if !wrapper {
        if swap::pending_journal(state) {
            return None;
        }
    } else if let Err(e) = swap::recover(state) {
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
        let reading = usage::reading_from(&rl, now);
        let due = wrapper
            && super::capacity::refresh_due(&state.root, &reading, eff.config.threshold, now);
        let merged = usage::merge(file.accounts.get(&account), reading);
        file.accounts.insert(account, merged);
        match state.save_usage(&file) {
            Ok(()) if due => super::capacity::refresh(state, &file, &ledger, "statusline"),
            Ok(()) => {}
            Err(e) => eprintln!("auto-switch: {e}"),
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

/// The profile's saved original status-line command, if any.
fn saved_original(state: &State, owner: &ProfileName) -> Option<String> {
    if !state.exists() {
        return None;
    }
    let saved = match state.read_sub_json("statusline", owner.as_str()) {
        Ok(Some(v)) => v,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("auto-switch: {e}; run `amplihack profile auto-switch repair`");
            return None;
        }
    };
    saved
        .get("command")
        .and_then(|c| c.as_str())
        .map(str::to_string)
}

/// Step 2: `cmd` (the profile's original status line, or `statusline.sh`),
/// run with `sh -c` like Claude does, with the same input. Its stdout and
/// stderr are ours. With any `tail` line, its stdout is streamed through
/// unchanged and each line follows on a line of its own. Returns its exit
/// code, or 0 when there is no command or it cannot start.
pub(super) fn stream_command(cmd: Option<&str>, input: &[u8], tail: &[Option<String>]) -> i32 {
    let lines: Vec<&str> = tail.iter().flatten().map(String::as_str).collect();
    if lines.is_empty() {
        return cmd.map_or(0, |c| spawn(c, input, None));
    }
    let mut out = super::capacity::Tail::new(std::io::stdout().lock());
    let code = cmd.map_or(0, |c| spawn(c, input, Some(&mut out)));
    out.finish_with(&lines.join("\n"));
    code
}

/// `command` under `sh -c`; `piped` receives its stdout instead of the
/// terminal.
fn spawn(command: &str, input: &[u8], piped: Option<&mut dyn Write>) -> i32 {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .env(NESTED, "1")
        .stdin(Stdio::piped())
        .stdout(if piped.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
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
    if let (Some(out), Some(mut stdout)) = (piped, child.stdout.take()) {
        // fail-quiet: status-line output contract. A broken pipe ends the
        // copy; the exit status still comes from the child.
        let _ = std::io::copy(&mut stdout, out);
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

pub(super) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub(super) fn quotable(p: &Path) -> Result<&str, ProfileError> {
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

/// The command `amplihack install` writes into `~/.claude/settings.json`
/// (issue #1745): no `--root`, no `--owner`.
pub fn home_command(exe: &Path) -> Result<String, ProfileError> {
    Ok(format!("{} {MARKER}", sh_quote(quotable(exe)?)))
}

/// The running amplihack binary, refused as [`trusted_binary`] refuses.
pub fn installed_binary() -> anyhow::Result<PathBuf> {
    trusted_binary(&std::env::current_exe()?)
}

/// `path`, resolved, refused if anyone but the user (or root) can replace
/// it: the binary and every directory above it must be owned by the user or
/// root and not writable by other users.
pub fn trusted_binary(path: &Path) -> anyhow::Result<PathBuf> {
    let exe = path.canonicalize()?;
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
    if !is_wrapper(cmd) {
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
    let script = home_line::script_command();
    let width = names.iter().map(String::len).max().unwrap_or(0);
    for raw in names {
        let name = ProfileName::parse(&raw)?;
        let (outcome, detail) =
            install_one(store, &state, &exe, &name, repoint, script.as_deref())?;
        let line = format!("{outcome:<10} {:<width$} {detail}", name.as_str());
        println!("{}", line.trim_end());
    }
    Ok(())
}

/// Whether `cmd` is a profile's wrapper: the words `profile statusline` and
/// `--owner`, each on its own. Any other command is not: neither the one in
/// `~/.claude`, copied in by the shared settings keys, nor a command that
/// only contains the same letters, such as `my-profile statusline-helper.sh`.
fn is_wrapper(cmd: &str) -> bool {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    words.contains(&"--owner") && words.windows(2).any(|w| w == ["profile", "statusline"])
}

/// `script` is `statusline.sh` as a command, when it passes its checks.
fn install_one(
    store: &ProfileStore,
    state: &State,
    exe: &Path,
    name: &ProfileName,
    repoint: bool,
    script: Option<&str>,
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
        Some(sl) if command_of(sl).is_some_and(is_wrapper) => {
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
        // The `~/.claude` command is never saved as an original (SR7): the
        // trusted `statusline.sh` it would run is saved in its place, or
        // nothing. Only the exact command: anything else is the user's and
        // is saved below, `NESTED` keeping it from running itself.
        Some(sl) if command_of(sl).is_some_and(is_home_statusline) => {
            let detail = match script {
                Some(cmd) => {
                    let original = serde_json::json!({"type": "command", "command": cmd});
                    state.write_sub_json("statusline", name.as_str(), &original)?;
                    "(wrapped statusline.sh in place of the ~/.claude command)"
                }
                None => "(no previous status line)",
            };
            (sl.get("padding").cloned(), ("installed", detail))
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
