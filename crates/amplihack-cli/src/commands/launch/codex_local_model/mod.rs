//! Local-model launches of `amplihack codex`: no inserted hook text and a
//! small Codex home of amplihack's own.
//!
//! A launch is for a local model when it says so, by `--local-model <name>`
//! or by `AMPLIHACK_CODEX_LOCAL_MODEL=<name>` (the flag wins; an empty
//! variable counts as unset). `<name>` is a Codex profile file,
//! `<user Codex home>/<name>.config.toml`, holding the model's provider
//! settings. For such a launch amplihack:
//!
//! - builds or refreshes `~/.amplihack/codex-local/<name>/` (a
//!   `config.toml` from the profile with settings that keep the prompt
//!   small, a `hooks.json` with amplihack's pre-tool-use entry and the
//!   user's own PreToolUse handlers, the dangerous-command rules file and a
//!   short `instructions.md`), and never writes the user's Codex home;
//! - starts Codex with `CODEX_HOME` pointing there, `AMPLIHACK_HOOK_TEXT=none`
//!   and `AMPLIHACK_CODEX_LOCAL_MODEL=<name>`, on the child only;
//! - puts `--dangerously-bypass-hook-trust` first unless the caller passed
//!   it, and says so once on stderr, then `-c features.hooks=true`, so that
//!   a trusted project's `.codex/config.toml` cannot switch hooks off. A
//!   caller's own later `-c` or `--disable hooks` still wins.
//!
//! With no selection nothing here runs and an ordinary launch is unchanged.
//!
//! The dispatch calls [`prepare_launch`] (selection, name rule, refused
//! combinations, reading the inputs; nothing is written), then runs the
//! launch inside [`scoped`]. The launch core reads [`current`], runs
//! [`bootstrap`] in place of the Codex bootstrap, and calls
//! [`Prepared::refresh`] as its last step before spawning.
//!
//! Unix only: elsewhere a selection is an error.

#[cfg_attr(not(unix), allow(dead_code))]
mod config;
#[cfg(unix)]
mod home;
#[cfg(unix)]
mod hooks_json;

#[cfg(test)]
mod tests_config;
#[cfg(all(test, unix))]
mod tests_fixture;
#[cfg(all(test, unix))]
mod tests_home;
#[cfg(all(test, unix))]
mod tests_hooks_json;
#[cfg(all(test, unix))]
mod tests_selection;

use crate::docker::{DockerActivation, DockerDetector};
use crate::env_builder::EnvBuilder;
use anyhow::{Result, bail};
use std::cell::RefCell;
use std::ffi::OsString;
use std::rc::Rc;

/// The variable that selects a local-model launch when the flag is absent.
pub(crate) const LOCAL_MODEL_ENV: &str = "AMPLIHACK_CODEX_LOCAL_MODEL";
const FLAG: &str = "--local-model";
const HOOK_TEXT_ENV: &str = "AMPLIHACK_HOOK_TEXT";
/// The only value amplihack ever writes for [`HOOK_TEXT_ENV`].
const HOOK_TEXT_NONE: &str = "none";
/// What `AMPLIHACK_HOOK_TEXT=none` costs a session: the stop hook approves
/// without running, and the context hooks add nothing.
const HOOK_TEXT_OFF: &str = "AMPLIHACK_HOOK_TEXT=none, so amplihack's hooks add no text and its \
                             stop hook approves without running: lock mode, power steering and \
                             reflection do not run";
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
/// The override that keeps hooks on over a project's own config layer.
const HOOKS_ON: [&str; 2] = ["-c", "features.hooks=true"];
/// How much of an invalid name an error quotes, in characters.
const QUOTED_CHARS: usize = 64;
const MAX_NAME_LEN: usize = 64;
/// The error for a selection off Unix.
const UNIX_ONLY: &str = "local-model launches of amplihack codex are supported on Unix only";

/// The launch options a local-model launch refuses.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Conflicts {
    pub(crate) docker: Option<DockerActivation>,
    pub(crate) auto: bool,
}

/// Docker as the user asked for it: by `--docker`, or by
/// `AMPLIHACK_USE_DOCKER` outside a container. Whether a daemon is running
/// does not matter: a local-model launch cannot honour the request either
/// way, and it is refused rather than quietly launched without Docker.
pub(crate) fn docker_request(docker_flag: bool) -> Option<DockerActivation> {
    if docker_flag {
        Some(DockerActivation::Flag)
    } else if DockerDetector.requested_outside_container(false) {
        Some(DockerActivation::Env)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Flag,
    Variable,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Flag => FLAG,
            Self::Variable => LOCAL_MODEL_ENV,
        }
    }
}

/// A selection, before the name rule.
struct Request {
    value: OsString,
    source: Source,
}

/// The flag, else a non-empty variable. The variable is not trimmed: a value
/// of spaces is a selection, and an invalid one.
fn select(flag: Option<&str>) -> Option<Request> {
    if let Some(value) = flag {
        return Some(Request {
            value: value.into(),
            source: Source::Flag,
        });
    }
    std::env::var_os(LOCAL_MODEL_ENV)
        .filter(|value| !value.is_empty())
        .map(|value| Request {
            value,
            source: Source::Variable,
        })
}

/// Whether this launch selects a local model, valid or not. Start-up
/// self-heal uses it to keep the Codex plugin out of the user's Codex home,
/// and must do so before the name is checked, so an invalid name counts.
pub(crate) fn requested(flag: Option<&str>) -> bool {
    select(flag).is_some()
}

/// A profile name that passed the rule: 1 to 64 ASCII characters from
/// `[A-Za-z0-9._-]`, not `.` or `..`. Safe to use as a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileName(String);

impl ProfileName {
    fn parse(request: &Request) -> Result<Self> {
        if let Some(name) = request.value.to_str()
            && is_valid_name(name)
        {
            return Ok(Self(name.to_owned()));
        }
        let shown: String = request
            .value
            .to_string_lossy()
            .chars()
            .take(QUOTED_CHARS)
            .collect();
        bail!(
            "invalid local-model profile name {shown:?} from {}: a name is 1 to {MAX_NAME_LEN} \
             characters from A-Z, a-z, 0-9, '.', '_' and '-', and is not '.' or '..'",
            request.source.label()
        )
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_LEN).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Refuses what a local-model launch cannot do, before anything is read.
fn refuse_conflicts(conflicts: Conflicts) -> Result<()> {
    if cfg!(not(unix)) {
        bail!(UNIX_ONLY);
    }
    if let Some(docker) = conflicts.docker {
        let source = match docker {
            DockerActivation::Flag => "--docker",
            DockerActivation::Env => "AMPLIHACK_USE_DOCKER",
        };
        bail!(
            "a local-model launch cannot run in Docker, which {source} turned on; \
             launch without Docker"
        );
    }
    if conflicts.auto {
        bail!("a local-model launch cannot run with --auto; launch without --auto");
    }
    Ok(())
}

/// A local-model launch that has been selected, checked and had its inputs
/// read. Nothing has been written yet. Never built off Unix.
#[derive(Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct Prepared {
    name: ProfileName,
    #[cfg(unix)]
    plan: home::Plan,
}

/// Resolves the selection for one `amplihack codex` launch: `Ok(None)` for
/// an ordinary launch. In order: the flag or the variable, the name rule,
/// the platform, Docker and `--auto`, the user Codex home and whether a
/// nested launch reuses the generated home; otherwise the profile (read,
/// parsed and checked) and the user's `hooks.json` (read and filtered).
/// Creates nothing on disk.
pub(crate) fn prepare_launch(flag: Option<&str>, conflicts: Conflicts) -> Result<Option<Prepared>> {
    let Some(request) = select(flag) else {
        return Ok(None);
    };
    let name = ProfileName::parse(&request)?;
    refuse_conflicts(conflicts)?;
    #[cfg(unix)]
    {
        let plan = home::Plan::prepare(&name)?;
        Ok(Some(Prepared { name, plan }))
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        bail!(UNIX_ONLY)
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Rc<Prepared>>> = const { RefCell::new(None) };
}

/// Runs `action` with `prepared` as the launch [`current`] returns, and
/// restores what was there afterwards, on a panic too.
pub(crate) fn scoped<T>(prepared: Option<Prepared>, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Rc<Prepared>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            CURRENT.with(|current| *current.borrow_mut() = previous);
        }
    }
    let previous = CURRENT.with(|current| current.replace(prepared.map(Rc::new)));
    let _restore = Restore(previous);
    action()
}

/// The local-model launch in scope, for a launch of `tool`; `None` for any
/// tool but `codex` and outside [`scoped`].
pub(crate) fn current(tool: &str) -> Option<Rc<Prepared>> {
    if tool != "codex" {
        return None;
    }
    CURRENT.with(|current| current.borrow().clone())
}

/// Whether a value of `AMPLIHACK_HOOK_TEXT` means `none` to the hooks
/// binary: trimmed of ASCII whitespace and compared without regard to ASCII
/// case, as `amplihack-hooks` reads it (`hook_text.rs`). A value that is not
/// valid UTF-8 means `full` there, and here.
fn is_hook_text_none(value: Option<&std::ffi::OsStr>) -> bool {
    value
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|value| {
            value
                .trim_matches(|c: char| c.is_ascii_whitespace())
                .eq_ignore_ascii_case(HOOK_TEXT_NONE)
        })
}

/// The stderr line for a launch that is not a local-model launch but
/// inherits `AMPLIHACK_HOOK_TEXT=none`, typically from a local-model Codex
/// session it was started in: the variable passes through every launcher,
/// so without this line the launched session would lose lock mode, power
/// steering and reflection without a word. `None` when the variable does
/// not mean `none`.
fn inherited_hook_text_notice(tool: &str, value: Option<&std::ffi::OsStr>) -> Option<String> {
    is_hook_text_none(value).then(|| {
        format!(
            "amplihack: {tool}: {HOOK_TEXT_OFF} in this session; the variable was set in this \
             environment, for example by a local-model Codex session; unset it to restore them"
        )
    })
}

/// Prints [`inherited_hook_text_notice`] for a launch of `tool` that is not
/// a local-model launch. A local-model launch prints its own line.
pub(crate) fn warn_inherited_hook_text(tool: &str) {
    if let Some(line) = inherited_hook_text_notice(tool, std::env::var_os(HOOK_TEXT_ENV).as_deref())
    {
        eprintln!("{line}");
    }
}

/// The bootstrap a local-model launch runs in place of the Codex one: the
/// generic part only (framework assets and the recipe-runner check), with no
/// Codex provider, so nothing is reconciled into the user's Codex home and
/// no plugin is installed.
pub(crate) fn bootstrap() -> Result<()> {
    crate::bootstrap::prepare_launcher("")
}

impl Prepared {
    /// Builds, refreshes or (for a nested launch) verifies the generated
    /// home: the last step before Codex is spawned.
    pub(crate) fn refresh(&self) -> Result<LocalHome> {
        #[cfg(unix)]
        {
            self.plan.refresh(&self.name)
        }
        #[cfg(not(unix))]
        {
            bail!(UNIX_ONLY)
        }
    }
}

/// A generated home ready for Codex. Never built off Unix.
#[derive(Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct LocalHome {
    name: ProfileName,
    /// The generated home, valid UTF-8 (checked when it was built).
    home: String,
    /// The user's PreToolUse handlers carried into `hooks.json`.
    user_handlers: usize,
    /// The profile's `sandbox_mode`, passed on as the caller's choice.
    sandbox_mode: Option<String>,
}

impl LocalHome {
    /// The Codex argv tail: `--dangerously-bypass-hook-trust` first, unless
    /// the caller passed it before any `--` among the Codex arguments (the
    /// agent-step injection then adds nothing, so it appears once), then
    /// `-c features.hooks=true` always, then `-c sandbox_mode=<value>` when
    /// the profile sets one, then the caller's arguments unchanged. The
    /// sandbox override counts as the caller's sandbox choice, so an `exec`
    /// launch keeps the profile's sandbox instead of getting
    /// `--sandbox danger-full-access`; a caller's own later choice still wins.
    ///
    /// One stderr line always says that hook text is off, so that lock mode,
    /// power steering and reflection do not run. When the hook-trust flag is
    /// added, a second line names what was added, the profile, the home and
    /// the number of user handlers carried over, never a command.
    pub(crate) fn codex_args(&self, args: Vec<String>) -> Vec<String> {
        eprintln!(
            "amplihack: codex local model \"{}\": {HOOK_TEXT_OFF} in this session and in \
             amplihack sessions started from it",
            self.name.as_str()
        );
        let add_trust = !super::codex_agent_step::has_hook_trust_bypass(&args);
        if add_trust {
            let handlers = if self.user_handlers == 1 {
                "handler"
            } else {
                "handlers"
            };
            eprintln!(
                "amplihack: codex local model \"{}\": added {HOOK_TRUST} and {} {} (Codex home \
                 \"{}\", {} user PreToolUse {handlers} carried over)",
                self.name.as_str(),
                HOOKS_ON[0],
                HOOKS_ON[1],
                crate::commands::install::paths::sanitize_text(&self.home),
                self.user_handlers,
            );
        }
        let mut out = Vec::with_capacity(args.len() + 5);
        if add_trust {
            out.push(HOOK_TRUST.to_string());
        }
        out.extend(HOOKS_ON.map(String::from));
        if let Some(mode) = &self.sandbox_mode {
            out.push("-c".to_string());
            out.push(format!(
                "sandbox_mode={}",
                toml::Value::String(mode.clone())
            ));
        }
        out.extend(args);
        out
    }

    /// The three variables the Codex child gets, set on `builder` only:
    /// amplihack's own environment is not changed, and
    /// `AMPLIHACK_HOOK_PROTOCOL` is never set (the hook command sets it).
    pub(crate) fn with_env(&self, builder: EnvBuilder) -> EnvBuilder {
        builder
            .set("CODEX_HOME", self.home.as_str())
            .set(HOOK_TEXT_ENV, HOOK_TEXT_NONE)
            .set(LOCAL_MODEL_ENV, self.name.as_str())
    }
}
