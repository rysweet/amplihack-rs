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
//!   it, and says so once on stderr.
//!
//! With no selection nothing here runs and an ordinary launch is unchanged.
//!
//! The dispatch calls [`prepare_launch`] (selection, name rule, refused
//! combinations, reading the inputs; nothing is written), then runs the
//! launch inside [`scoped`]. The launch core reads [`current`], runs
//! [`bootstrap`] in place of the Codex bootstrap, and calls
//! [`Prepared::refresh`] as its last step before spawning.
//!
//! Unix only (D21): elsewhere a selection is an error.

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
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
/// How much of an invalid name an error quotes, in characters.
const QUOTED_CHARS: usize = 64;
const MAX_NAME_LEN: usize = 64;
/// The error for a selection off Unix (D21).
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
/// self-heal uses it to keep the Codex plugin out of the user's Codex home
/// (SF8), so an invalid name counts.
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

/// The bootstrap a local-model launch runs in place of the Codex one: the
/// generic part only (framework assets and the recipe-runner check), with no
/// Codex provider, so nothing is reconciled into the user's Codex home and
/// no plugin is installed (L13).
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
}

impl LocalHome {
    /// The Codex argv tail with `--dangerously-bypass-hook-trust` first,
    /// unless the caller passed it before any `--` among the Codex arguments
    /// (the agent-step injection then adds nothing, so it appears once).
    /// When it is added, one stderr line names the flag, the profile, the
    /// home and the number of user handlers carried over, never a command.
    pub(crate) fn codex_args(&self, args: Vec<String>) -> Vec<String> {
        if super::codex_agent_step::has_hook_trust_bypass(&args) {
            return args;
        }
        let handlers = if self.user_handlers == 1 {
            "handler"
        } else {
            "handlers"
        };
        eprintln!(
            "amplihack: codex local model \"{}\": added {HOOK_TRUST} (Codex home \"{}\", \
             {} user PreToolUse {handlers} carried over)",
            self.name.as_str(),
            crate::commands::install::paths::sanitize_text(&self.home),
            self.user_handlers,
        );
        let mut out = Vec::with_capacity(args.len() + 1);
        out.push(HOOK_TRUST.to_string());
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
