//! Which nested `-p` launches run under a supervisor: the auto-switch rerun
//! supervisor (#1569) and the usage-limit wait (#1618). See the Gate section
//! of docs/reference/profile-command.md#usage-limit-wait.

use anyhow::Result;

/// Runs a nested `-p` launch under the usage-limit supervisor when
/// auto-switch is on for an inherited profile, or when the usage-limit wait
/// applies. Returns the exit code to use, or `None` to launch normally. With
/// neither, this reads the environment and at most the settings files.
pub fn maybe_supervise(
    flag: Option<&str>,
    docker: bool,
    auto: bool,
    claude_args: &[String],
) -> Result<Option<i32>> {
    #[cfg(unix)]
    return unix::maybe_supervise(flag, docker, auto, claude_args);
    #[cfg(not(unix))]
    {
        let _ = (flag, docker, auto, claude_args);
        Ok(None)
    }
}

#[cfg(unix)]
mod unix {
    use super::super::auto_switch::{self, SUPERVISED_ENV};
    use super::super::limit_wait::{self, Waiter, fingerprint::Watch};
    use super::super::name::ProfileName;
    use super::super::store::ProfileStore;
    use super::super::{PROFILE_ENV, docker_env_active, unsupported_mode};
    use anyhow::Result;
    use std::ffi::OsStr;
    use std::path::Path;

    pub(super) fn maybe_supervise(
        flag: Option<&str>,
        docker: bool,
        auto: bool,
        claude_args: &[String],
    ) -> Result<Option<i32>> {
        // 1. Only an inherited profile, or none, is supervised.
        if flag.is_some() {
            return Ok(None);
        }
        // 2. Only a print launch.
        let printing = claude_args
            .iter()
            .any(|a| a == "-p" || a == "--print" || a.starts_with("--print="));
        if !printing {
            return Ok(None);
        }
        // 3. Never a launch that is itself a supervised attempt.
        if std::env::var_os(SUPERVISED_ENV).is_some_and(|v| !v.is_empty()) {
            return Ok(None);
        }
        // 4. The wait's settings are read only for a recipe step. Exactly
        // "1": `util::is_noninteractive` also counts piped stdin.
        let wait = (std::env::var_os("AMPLIHACK_NONINTERACTIVE").as_deref()
            == Some(OsStr::new("1")))
        .then(crate::wait_config::resolve);
        let wait_on = wait.is_some_and(|s| s.enabled);
        let inherited = std::env::var_os(PROFILE_ENV).filter(|v| !v.is_empty());
        let Some(inherited) = inherited else {
            let Some(settings) = wait.filter(|s| s.enabled) else {
                return Ok(None);
            };
            // 6. Docker and auto modes cannot be supervised.
            if unsupported_mode(docker, docker_env_active(), auto).is_some() {
                limit_wait::not_supervised(wait);
                return Ok(None);
            }
            return limit_wait::supervise_unprofiled(settings);
        };
        let Some(inherited) = inherited.to_str().map(str::to_string) else {
            return Ok(None);
        };
        // Never where a profile cannot be applied (the check
        // `launch_profile_dir` makes); that launch is refused instead.
        if unsupported_mode(docker, docker_env_active(), auto).is_some() {
            return Ok(None);
        }
        let store = match ProfileStore::from_env() {
            Ok(store) => store,
            Err(e) => {
                // The launch reports a bad root itself; say why it is unsupervised.
                if auto_switch::env_forces_on() || wait_on {
                    eprintln!("warning: cannot supervise this step ({e}); running it once");
                }
                limit_wait::not_supervised(wait);
                return Ok(None);
            }
        };
        let Ok(name) = ProfileName::parse(&inherited) else {
            return Ok(None); // the normal launch reports the bad value
        };
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
        let watch = Watch::new(
            Some(&store),
            Some(&name),
            None,
            home.as_deref().map(Path::new),
        );
        auto_switch::rerun::supervise(&store, name, Waiter::new(wait, watch))
    }
}
