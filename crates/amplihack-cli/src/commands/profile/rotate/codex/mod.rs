//! The Codex part of `profile rotate` (issue #2005): each cycle reads every
//! Codex account live once, writes one `codex cycle` line, and keeps the
//! default Codex home link on the account with the most room by relinking
//! it. `profile codex-resume` resumes a session that stopped on a usage
//! limit. See "Rotate Codex" and "Codex resume command" in
//! docs/reference/profile-command.md.
//!
//! No file inside an account directory is ever opened: accounts and the
//! link are touched only by `read_dir`, `lstat`, `readlink`, `stat`,
//! `symlink`, `rename` and `mkdir`. Only `state.rs` (`codex-state.json` and
//! its lock), `session.rs` (the `--events` file) and `inuse.rs` (`/proc`)
//! open files.

mod clock;
mod cycle;
mod decide;
mod inuse;
mod limit;
mod lines;
mod link;
mod recover;
mod resume;
mod session;
pub(super) mod settings;
mod state;
mod swap;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_hardening;
#[cfg(test)]
mod tests_inuse;
#[cfg(test)]
mod tests_link;
#[cfg(test)]
mod tests_session;
#[cfg(test)]
mod tests_settings;
#[cfg(test)]
mod tests_state;

pub(super) use cycle::{CodexCtx, run};

use super::ledger::RotateState;
use super::log::{Sink, open_or_warn};
use super::settings::{self as rotate_settings, Settings};
use crate::commands::profile::ProfileError;
use amplihack_utils::codex_capacity::{Account, AccountResult, read_all};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The variable naming the `/proc` root of the in-use scan; read only in
/// debug builds, so tests never examine a real process.
#[cfg(debug_assertions)]
const PROC_ROOT_ENV: &str = "AMPLIHACK_TEST_CODEX_PROC_ROOT";
/// The variable naming a point at which a swap stops the process, as if it
/// were killed; read only in debug builds.
#[cfg(debug_assertions)]
const FAULT_ENV: &str = "AMPLIHACK_TEST_CODEX_FAULT";

/// Where a swap may be stopped by the test-only fault hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultPoint {
    /// After the `prepared` journal is written, before the link moves.
    AfterJournal,
    /// After the `rename()` over the link, before the journal is closed.
    AfterRename,
}

impl FaultPoint {
    #[cfg(debug_assertions)]
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "after-journal" => Some(Self::AfterJournal),
            "after-rename" => Some(Self::AfterRename),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::AfterJournal => "after-journal",
            Self::AfterRename => "after-rename",
        }
    }
}

/// Stops the process at `point` when the fault hook names it.
pub(super) fn fault_stop(fault: Option<FaultPoint>, point: FaultPoint) {
    if fault == Some(point) {
        eprintln!("amplihack: test fault hook: stopping at {}", point.name());
        std::process::exit(75);
    }
}

/// The live read: [`read_all`], or a fake in unit tests.
pub(super) type ReadFn = fn(&OsStr, &[Account], Duration) -> Vec<AccountResult>;

/// What the Codex rotation reads from outside: accounts, `/proc`, the time
/// and the fault hook.
pub(super) struct Hooks {
    pub read: ReadFn,
    pub proc_root: PathBuf,
    pub now: fn() -> i64,
    pub fault: Option<FaultPoint>,
}

impl Hooks {
    /// The real hooks. In debug builds `AMPLIHACK_TEST_CODEX_PROC_ROOT` and
    /// `AMPLIHACK_TEST_CODEX_FAULT` may replace `/proc` and set a fault
    /// point; release builds read neither.
    pub(super) fn real() -> Self {
        #[cfg(debug_assertions)]
        let (proc_root, fault) = (
            std::env::var_os(PROC_ROOT_ENV)
                .filter(|v| !v.is_empty())
                .map_or_else(|| PathBuf::from("/proc"), PathBuf::from),
            std::env::var(FAULT_ENV)
                .ok()
                .and_then(|v| FaultPoint::parse(&v)),
        );
        #[cfg(not(debug_assertions))]
        let (proc_root, fault) = (PathBuf::from("/proc"), None);
        Self {
            read: read_all,
            proc_root,
            now: crate::commands::profile::auto_switch::kit::now,
            fault,
        }
    }
}

/// The Codex program as a path: `CODEX_BIN` when set and not empty, else
/// `codex`; a name without `/` is looked up on `PATH`. `None` when it is
/// not an existing file.
pub(super) fn find_codex_bin() -> Option<PathBuf> {
    let bin = PathBuf::from(amplihack_utils::codex_capacity::codex_bin());
    if bin.as_os_str().as_encoded_bytes().contains(&b'/') {
        return bin.is_file().then_some(bin);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(&bin))
        .find(|p| is_executable(p))
}

/// Whether the Codex program `bin` lies where a swap, or the account a
/// resume runs in, could change it (EXEC-1): `bin` as given or made
/// canonical is under the default home link, the canonical profiles root
/// `root_c`, or `live`, the link's current target, when given. A program
/// that cannot be made canonical counts as inside, so the check fails
/// closed.
pub(super) fn bin_in_home(bin: &Path, root_c: &Path, link: &Path, live: Option<&Path>) -> bool {
    let Ok(bin_c) = std::fs::canonicalize(bin) else {
        return true;
    };
    [bin, bin_c.as_path()].iter().any(|b| {
        b.starts_with(link) || b.starts_with(root_c) || live.is_some_and(|t| b.starts_with(t))
    })
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The label of `path` when, made canonical, it is a directory that is a
/// direct child of the canonical `root` whose name is one normal component
/// not starting with `.`; with its canonical path.
pub(super) fn account_of(path: &Path, root_c: &Path) -> Option<(String, PathBuf)> {
    let target = std::fs::canonicalize(path).ok()?;
    if target.parent() != Some(root_c) || !target.is_dir() {
        return None;
    }
    let label = target.file_name()?.to_str()?.to_string();
    (!label.is_empty() && !label.starts_with('.')).then_some((label, target))
}

/// `$HOME` and the default Codex home link for `tune`, checked before
/// anything runs.
pub(super) fn home_and_link(
    tune: &super::TuneFlags,
    home: Result<PathBuf, ProfileError>,
) -> Result<(PathBuf, PathBuf), ProfileError> {
    let home = home?;
    let env = |k: &str| std::env::var_os(k);
    let link = settings::resolve_codex(tune.codex_home_link.as_deref(), &env, &home)?.link;
    Ok((home, link))
}

/// The link `rotate start` resolved from its own flags and variables, for
/// the process it spawns; unset in every other command.
static START_LINK: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// `rotate start`: resolves the link as [`home_and_link`] does and keeps it
/// for [`spawn_link_args`]; returns `$HOME`.
pub(super) fn start_home(
    tune: &super::TuneFlags,
    home: Result<PathBuf, ProfileError>,
) -> Result<PathBuf, ProfileError> {
    let (home, link) = home_and_link(tune, home)?;
    let first = START_LINK.set(link).is_ok();
    debug_assert!(first, "start runs once");
    Ok(home)
}

/// `--codex-home-link <link>` for the background process, which gets no
/// `AMPLIHACK_ROTATE*` variable: the link `rotate start` resolved, else (an
/// autostart) the one this process's environment gives. Nothing when the
/// Codex part is off.
pub(super) fn spawn_link_args(s: &Settings) -> Result<Vec<std::ffi::OsString>, ProfileError> {
    if !s.codex {
        return Ok(Vec::new());
    }
    let link = match START_LINK.get() {
        Some(l) => l.clone(),
        None => home_and_link(&super::TuneFlags::default(), rotate_settings::home_dir())?.1,
    };
    Ok(vec![settings::LINK_FLAG.into(), link.into_os_string()])
}

/// The Codex part of a `watch` cycle, after the Claude cycle; its lines go
/// to the log only.
pub(super) fn in_watch(ctx: &super::cycle::Ctx, link: &Path) {
    if !ctx.settings.codex || ctx.stopping() {
        return;
    }
    let mut out = match ctx.rs.ensure() {
        Ok(()) => Sink::new(open_or_warn(&ctx.rs.log, &ctx.rs.log1), false, true),
        Err(e) => {
            eprintln!("warning: rotate log unavailable ({e}); Codex cycle skipped");
            return;
        }
    };
    run_real(ctx, link, &mut out);
}

/// Runs one Codex cycle for `ctx` with the real hooks.
fn run_real(ctx: &super::cycle::Ctx, link: &Path, out: &mut Sink) {
    let hooks = Hooks::real();
    let codex = CodexCtx {
        rs: &ctx.rs,
        home: &ctx.home,
        settings: ctx.settings,
        link,
        read_only: ctx.read_only,
        hooks: &hooks,
    };
    run(&codex, out);
}

/// The Codex part of a one-shot or dry-run rotate, after the Claude cycle
/// `claude` (returned as it is): its lines go where that cycle's go (the
/// log and stdout, or stdout only and untimed in a dry run). It never
/// changes the Claude outcome or the exit status.
pub(super) fn after_cycle<T>(
    claude: Result<T, ProfileError>,
    ctx: &super::cycle::Ctx,
    link: &Path,
) -> Result<T, ProfileError> {
    if !ctx.settings.codex {
        return claude;
    }
    let (rs, dry) = (&ctx.rs, ctx.read_only);
    let log = if dry {
        None
    } else {
        match rs.ensure() {
            Ok(()) => open_or_warn(&rs.log, &rs.log1),
            Err(e) => {
                eprintln!("warning: not writing the rotate log: {e}");
                None
            }
        }
    };
    let mut out = Sink::new(log, true, !dry);
    run_real(ctx, link, &mut out);
    claude
}

/// `profile codex-resume` with the settings from the environment; the
/// exit status.
pub(in crate::commands::profile) fn resume_command(
    root: &Path,
    a: crate::commands::profile::rotate_args::CodexResumeArgs,
) -> Result<i32, ProfileError> {
    let env = |k: &str| std::env::var_os(k);
    let home = rotate_settings::home_dir()?;
    let s = rotate_settings::resolve(&super::TuneFlags::default(), None, &env)?;
    let link = settings::resolve_codex(None, &env, &home)?.link;
    let rs = RotateState::new(root);
    let req = resume::ResumeRequest {
        session: a.session,
        events: a.events,
        from_home: a.from_home,
        extra: a.extra,
    };
    let hooks = Hooks::real();
    Ok(resume::cli(
        &rs,
        &home,
        s.codex_floor,
        &link,
        req,
        &hooks,
        &clock::SystemClock,
    ))
}
