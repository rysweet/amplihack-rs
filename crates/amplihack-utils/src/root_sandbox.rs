//! Running `claude --dangerously-skip-permissions` as root (issue #1482).
//!
//! Claude Code refuses `--dangerously-skip-permissions` when its effective uid
//! is 0 unless `IS_SANDBOX=1` is set in its environment, and exits 1 with:
//!
//! ```text
//! --dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons
//! ```
//!
//! amplihack passes that flag on every non-interactive `claude` it spawns, so
//! every recipe agent step failed in the places amplihack most often runs as
//! root: Claude Code cloud sessions, root Docker dev containers and root CI
//! runners.
//!
//! The rule implemented by [`decide`]:
//!
//! - Not root: nothing to do.
//! - `IS_SANDBOX` already set by the user: its meaning is respected. `1`
//!   passes through untouched. Claude Code accepts only the exact value `1`, so
//!   another affirmative spelling (`yes`, `true`, `on`) is passed to the child
//!   as `1` — Claude Code cloud containers export `IS_SANDBOX=yes`, which the
//!   CLI rejects. A negative (`0`, `false`, `no`, `off`) or unrecognised value
//!   on root is reported up front and never overridden.
//! - Root inside a detectable sandbox (`CLAUDE_CODE_REMOTE=true`,
//!   `/.dockerenv`, `/run/.containerenv`, or a container marker in
//!   `/proc/1/cgroup`): `IS_SANDBOX=1` is set on the child `claude` process
//!   only. amplihack's own environment is never modified.
//! - Under WSL the marker files do not count. WSL is a workstation with the
//!   Windows drives mounted, and a distribution imported from `docker export`
//!   keeps the image's `/.dockerenv`; WSL imports also log in as root by
//!   default. A container marker in `/proc/1/cgroup` still counts there,
//!   because it describes the process tree, not the root filesystem.
//! - `CLAUDE_CODE_BUBBLEWRAP` set: Claude Code accepts the flag itself, so
//!   nothing is needed.
//! - Root with no sandbox signal: an error naming `IS_SANDBOX=1`, raised before
//!   anything is spawned. amplihack never silently declares a real host a
//!   sandbox.
//!
//! A container marker shows the process is contained, not that the machine is
//! disposable, so every automatic `IS_SANDBOX=1` is announced on stderr with
//! the signal that caused it (see [`SkipPermissionsEnv::notice`]); exporting
//! `IS_SANDBOX=0` turns it off.

use std::fmt;
use std::path::Path;
use std::process::Command;

/// The variable Claude Code reads to allow the flag as root.
pub const IS_SANDBOX_ENV: &str = "IS_SANDBOX";

/// The flag Claude Code refuses as root outside a sandbox.
pub const SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";

/// Set by Claude Code on the web for its cloud containers.
const CLAUDE_CODE_REMOTE_ENV: &str = "CLAUDE_CODE_REMOTE";

/// Claude Code's own bubblewrap sandbox; Claude Code accepts the flag as root
/// when it is truthy.
const CLAUDE_CODE_BUBBLEWRAP_ENV: &str = "CLAUDE_CODE_BUBBLEWRAP";

/// Whether one path component of a `/proc/1/cgroup` entry names a container
/// runtime's cgroup. Whole components, not substrings, so a host unit that
/// merely mentions a runtime (`containerd.service`) does not count.
fn is_container_cgroup_component(component: &str) -> bool {
    component == "docker"
        || component == "lxc"
        || component == "podman"
        || component.starts_with("docker-")
        || component.starts_with("libpod-")
        || component.starts_with("crio-")
        || component.starts_with("cri-containerd-")
        || component.starts_with("kubepods")
        || component.starts_with("lxc.payload")
}

fn cgroup_names_a_container(contents: &str) -> bool {
    contents.lines().any(|line| {
        line.splitn(3, ':')
            .nth(2)
            .is_some_and(|path| path.split('/').any(is_container_cgroup_component))
    })
}

/// Environment variables WSL sets in every session of a distribution.
const WSL_ENV_VARS: [&str; 2] = ["WSL_DISTRO_NAME", "WSL_INTEROP"];

/// Paths that exist in a WSL distribution and not in a container. The
/// `binfmt_misc` entries are how WSL runs Windows executables; `/run/WSL` holds
/// its interop sockets. They catch WSL when `sudo` has reset the environment.
const WSL_PATHS: [&str; 3] = [
    "/proc/sys/fs/binfmt_misc/WSLInterop",
    "/proc/sys/fs/binfmt_misc/WSLInterop-late",
    "/run/WSL",
];

/// Whether this process runs in a WSL distribution.
fn running_under_wsl() -> bool {
    WSL_ENV_VARS
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        || WSL_PATHS.iter().any(|path| Path::new(path).exists())
}

/// Evidence that this process runs inside a disposable container.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SandboxSignals {
    /// `CLAUDE_CODE_REMOTE=true` — a Claude Code cloud session.
    pub claude_code_remote: bool,
    /// `/.dockerenv` exists.
    pub dockerenv: bool,
    /// `/run/.containerenv` exists (Podman).
    pub containerenv: bool,
    /// `/proc/1/cgroup` names a container runtime.
    pub container_cgroup: bool,
}

impl SandboxSignals {
    /// Probe the real process environment and filesystem.
    pub fn detect() -> Self {
        let cgroup = std::fs::read_to_string("/proc/1/cgroup").ok();
        Self::from_state(
            std::env::var(CLAUDE_CODE_REMOTE_ENV).ok().as_deref(),
            Path::new("/.dockerenv").exists(),
            Path::new("/run/.containerenv").exists(),
            cgroup.as_deref(),
            running_under_wsl(),
        )
    }

    /// Pure form of [`SandboxSignals::detect`], for tests. Under `wsl` the
    /// marker files are ignored (see the module documentation).
    pub fn from_state(
        claude_code_remote: Option<&str>,
        dockerenv: bool,
        containerenv: bool,
        cgroup: Option<&str>,
        wsl: bool,
    ) -> Self {
        Self {
            claude_code_remote: claude_code_remote
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("true")),
            dockerenv: dockerenv && !wsl,
            containerenv: containerenv && !wsl,
            container_cgroup: cgroup.is_some_and(cgroup_names_a_container),
        }
    }

    /// The first signal present, for diagnostics; `None` when there is none.
    pub fn first(&self) -> Option<&'static str> {
        if self.claude_code_remote {
            Some("CLAUDE_CODE_REMOTE=true")
        } else if self.dockerenv {
            Some("/.dockerenv")
        } else if self.containerenv {
            Some("/run/.containerenv")
        } else if self.container_cgroup {
            Some("container cgroup in /proc/1/cgroup")
        } else {
            None
        }
    }
}

/// What a `claude --dangerously-skip-permissions` child needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipPermissionsEnv {
    /// Not running as root; Claude Code accepts the flag as is.
    NotRoot,
    /// Claude Code already accepts the flag: the user exported `IS_SANDBOX=1`
    /// or a truthy `CLAUDE_CODE_BUBBLEWRAP`, and the child inherits it.
    AlreadySandboxed,
    /// Root inside a detected sandbox; set `IS_SANDBOX=1` on the child only.
    SetSandbox {
        /// The signal that identified the sandbox.
        signal: &'static str,
    },
    /// Root, and the user set `IS_SANDBOX` to an affirmative spelling other
    /// than `1`; pass it to the child as `1`, the only value Claude Code takes.
    NormalizeExplicit {
        /// The user's value.
        value: String,
    },
    /// Root, and the user set `IS_SANDBOX` to a negative or unrecognised value.
    ExplicitlyNotSandboxed {
        /// The user's value, which is respected and not overwritten.
        value: String,
    },
    /// Root with no sandbox signal and no `IS_SANDBOX`.
    RootOutsideSandbox,
}

/// Decide what a root-safe `claude --dangerously-skip-permissions` needs.
///
/// `euid` is `None` on platforms without uids. `explicit_is_sandbox` is the
/// inherited `IS_SANDBOX`; an empty or whitespace value counts as unset.
/// `bubblewrap` is the inherited `CLAUDE_CODE_BUBBLEWRAP`.
pub fn decide(
    euid: Option<u32>,
    explicit_is_sandbox: Option<&str>,
    bubblewrap: Option<&str>,
    signals: SandboxSignals,
) -> SkipPermissionsEnv {
    if euid != Some(0) {
        return SkipPermissionsEnv::NotRoot;
    }
    // Claude Code accepts CLAUDE_CODE_BUBBLEWRAP regardless of IS_SANDBOX.
    if bubblewrap.map(str::trim).is_some_and(is_truthy) {
        return SkipPermissionsEnv::AlreadySandboxed;
    }
    match explicit_is_sandbox.map(str::trim) {
        Some("1") => return SkipPermissionsEnv::AlreadySandboxed,
        Some(value) if is_affirmative(value) => {
            return SkipPermissionsEnv::NormalizeExplicit {
                value: value.to_string(),
            };
        }
        Some(value) if !value.is_empty() => {
            return SkipPermissionsEnv::ExplicitlyNotSandboxed {
                value: value.to_string(),
            };
        }
        _ => {}
    }
    match signals.first() {
        Some(signal) => SkipPermissionsEnv::SetSandbox { signal },
        None => SkipPermissionsEnv::RootOutsideSandbox,
    }
}

fn is_affirmative(value: &str) -> bool {
    ["yes", "true", "on", "y"]
        .iter()
        .any(|affirmative| value.eq_ignore_ascii_case(affirmative))
}

/// Claude Code's own truthiness test for `CLAUDE_CODE_BUBBLEWRAP` (`1`,
/// `true`, `yes`, `on`). Narrower than [`is_affirmative`], which also takes `y`
/// as a user's stated intent for `IS_SANDBOX`: a `CLAUDE_CODE_BUBBLEWRAP=y`
/// that Claude Code ignores must not be passed through as accepted.
fn is_truthy(value: &str) -> bool {
    value == "1"
        || ["true", "yes", "on"]
            .iter()
            .any(|truthy| value.eq_ignore_ascii_case(truthy))
}

/// [`decide`] against the real process state. The sandbox probes (a file
/// read and a few `stat`s) run only as root.
pub fn detect() -> SkipPermissionsEnv {
    let euid = effective_uid();
    if euid != Some(0) {
        return SkipPermissionsEnv::NotRoot;
    }
    decide(
        euid,
        std::env::var(IS_SANDBOX_ENV).ok().as_deref(),
        std::env::var(CLAUDE_CODE_BUBBLEWRAP_ENV).ok().as_deref(),
        SandboxSignals::detect(),
    )
}

impl SkipPermissionsEnv {
    /// `Err` when Claude Code would refuse the flag; the error says why and
    /// names `IS_SANDBOX=1`.
    pub fn check(&self) -> Result<(), RootSandboxError> {
        match self {
            Self::NotRoot
            | Self::AlreadySandboxed
            | Self::SetSandbox { .. }
            | Self::NormalizeExplicit { .. } => Ok(()),
            Self::ExplicitlyNotSandboxed { value } => {
                Err(RootSandboxError::ExplicitlyNotSandboxed {
                    value: value.clone(),
                })
            }
            Self::RootOutsideSandbox => Err(RootSandboxError::RootOutsideSandbox),
        }
    }

    /// The line announcing an `IS_SANDBOX=1` amplihack decided on by itself,
    /// naming the signal and how to refuse it; `None` when amplihack changes
    /// nothing or only restates a value the user set.
    pub fn notice(&self) -> Option<String> {
        match self {
            Self::SetSandbox { signal } => Some(format!(
                "amplihack: running as root in a container ({signal}); passing \
                 {IS_SANDBOX_ENV}=1 to claude so {SKIP_PERMISSIONS_FLAG} is accepted. \
                 Set {IS_SANDBOX_ENV}=0 to refuse."
            )),
            _ => None,
        }
    }

    /// Prepare `command` (a `claude` child) for the flag and announce any
    /// automatic `IS_SANDBOX=1` on stderr. See [`Self::apply_quietly`].
    pub fn apply(&self, command: &mut Command) -> Result<(), RootSandboxError> {
        if let Some(notice) = self.apply_quietly(command)? {
            eprintln!("{notice}");
        }
        Ok(())
    }

    /// Set `IS_SANDBOX=1` on `command` when needed, or return the error from
    /// [`Self::check`]. Returns [`Self::notice`] for the caller to show where
    /// stderr is not the right place (a TUI).
    pub fn apply_quietly(&self, command: &mut Command) -> Result<Option<String>, RootSandboxError> {
        self.check()?;
        if matches!(
            self,
            Self::SetSandbox { .. } | Self::NormalizeExplicit { .. }
        ) {
            command.env(IS_SANDBOX_ENV, "1");
        }
        Ok(self.notice())
    }
}

/// Claude Code would refuse `--dangerously-skip-permissions` as root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootSandboxError {
    /// Root, no sandbox detected, `IS_SANDBOX` unset.
    RootOutsideSandbox,
    /// Root, `IS_SANDBOX` set to a value Claude Code does not accept.
    ExplicitlyNotSandboxed {
        /// The user's value.
        value: String,
    },
}

impl fmt::Display for RootSandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootOutsideSandbox => write!(
                f,
                "amplihack runs `claude {SKIP_PERMISSIONS_FLAG}`, which Claude Code refuses as root \
                 (uid 0) unless {IS_SANDBOX_ENV}=1 is set, and no container sandbox was detected \
                 (checked {CLAUDE_CODE_REMOTE_ENV}=true, /.dockerenv, /run/.containerenv and \
                 /proc/1/cgroup; the marker files do not count under WSL). If this machine is a disposable sandbox, export \
                 {IS_SANDBOX_ENV}=1 and re-run; otherwise run amplihack as a non-root user."
            ),
            Self::ExplicitlyNotSandboxed { value } => write!(
                f,
                "amplihack runs `claude {SKIP_PERMISSIONS_FLAG}`, which Claude Code refuses as root \
                 (uid 0) unless {IS_SANDBOX_ENV}=1 is set, and {IS_SANDBOX_ENV} is set to \
                 {value:?}, which amplihack does not override. Claude Code accepts only the \
                 exact value 1: export \
                 {IS_SANDBOX_ENV}=1 if this machine is a disposable sandbox, or run amplihack \
                 as a non-root user."
            ),
        }
    }
}

impl std::error::Error for RootSandboxError {}

impl From<RootSandboxError> for std::io::Error {
    fn from(error: RootSandboxError) -> Self {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, error)
    }
}

/// Whether `args` pass [`SKIP_PERMISSIONS_FLAG`] (bare or `=value`).
pub fn args_skip_permissions<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> bool {
    args.iter().any(|arg| {
        let arg = arg.as_ref().to_string_lossy();
        arg == SKIP_PERMISSIONS_FLAG || arg.starts_with("--dangerously-skip-permissions=")
    })
}

#[cfg(unix)]
fn effective_uid() -> Option<u32> {
    // SAFETY: `geteuid` takes no arguments, cannot fail and has no side effects.
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn effective_uid() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_SIGNALS: SandboxSignals = SandboxSignals {
        claude_code_remote: false,
        dockerenv: false,
        containerenv: false,
        container_cgroup: false,
    };

    const REMOTE: SandboxSignals = SandboxSignals {
        claude_code_remote: true,
        ..NO_SIGNALS
    };

    fn command_is_sandbox(command: &Command) -> Option<Option<String>> {
        command
            .get_envs()
            .find(|(key, _)| *key == IS_SANDBOX_ENV)
            .map(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
    }

    // --- non-root -----------------------------------------------------------

    #[test]
    fn non_root_needs_nothing_with_or_without_a_sandbox() {
        for signals in [NO_SIGNALS, REMOTE] {
            for explicit in [None, Some("1"), Some("0")] {
                assert_eq!(
                    decide(Some(1000), explicit, None, signals),
                    SkipPermissionsEnv::NotRoot
                );
            }
        }
    }

    #[test]
    fn no_uid_platform_is_treated_as_non_root() {
        assert_eq!(
            decide(None, None, None, NO_SIGNALS),
            SkipPermissionsEnv::NotRoot
        );
    }

    #[test]
    fn non_root_leaves_the_child_environment_alone() {
        let mut command = Command::new("claude");
        decide(Some(1000), None, None, REMOTE)
            .apply(&mut command)
            .unwrap();
        assert_eq!(command_is_sandbox(&command), None);
    }

    // --- root in a sandbox --------------------------------------------------

    #[test]
    fn root_in_each_kind_of_sandbox_sets_is_sandbox_on_the_child() {
        let cases = [
            (REMOTE, "CLAUDE_CODE_REMOTE=true"),
            (
                SandboxSignals {
                    dockerenv: true,
                    ..NO_SIGNALS
                },
                "/.dockerenv",
            ),
            (
                SandboxSignals {
                    containerenv: true,
                    ..NO_SIGNALS
                },
                "/run/.containerenv",
            ),
            (
                SandboxSignals {
                    container_cgroup: true,
                    ..NO_SIGNALS
                },
                "container cgroup in /proc/1/cgroup",
            ),
        ];
        for (signals, expected) in cases {
            let decision = decide(Some(0), None, None, signals);
            assert_eq!(
                decision,
                SkipPermissionsEnv::SetSandbox { signal: expected }
            );
            let mut command = Command::new("claude");
            decision.apply(&mut command).unwrap();
            assert_eq!(command_is_sandbox(&command), Some(Some("1".to_string())));
        }
    }

    #[test]
    fn auto_enable_is_announced_naming_the_signal_and_the_opt_out() {
        let decision = decide(Some(0), None, None, REMOTE);
        let notice = decision.notice().expect("an automatic enable is announced");
        assert!(notice.contains("CLAUDE_CODE_REMOTE=true"), "{notice}");
        assert!(notice.contains("IS_SANDBOX=1"), "{notice}");
        assert!(notice.contains("IS_SANDBOX=0"), "{notice}");
        let mut command = Command::new("claude");
        assert_eq!(decision.apply_quietly(&mut command), Ok(Some(notice)));
        assert_eq!(command_is_sandbox(&command), Some(Some("1".to_string())));
    }

    #[test]
    fn nothing_is_announced_when_amplihack_decides_nothing() {
        for decision in [
            decide(Some(1000), None, None, REMOTE),
            decide(Some(0), Some("1"), None, NO_SIGNALS),
            decide(Some(0), Some("yes"), None, NO_SIGNALS),
            decide(Some(0), None, Some("1"), NO_SIGNALS),
        ] {
            assert_eq!(decision.notice(), None, "{decision:?}");
        }
    }

    #[test]
    fn truthy_bubblewrap_is_accepted_as_root_like_claude_code_does() {
        for value in ["1", "true", "yes", "on"] {
            for (explicit, signals) in [(None, NO_SIGNALS), (Some("0"), NO_SIGNALS), (None, REMOTE)]
            {
                let decision = decide(Some(0), explicit, Some(value), signals);
                assert_eq!(decision, SkipPermissionsEnv::AlreadySandboxed);
                let mut command = Command::new("claude");
                decision.apply(&mut command).unwrap();
                assert_eq!(command_is_sandbox(&command), None);
            }
        }
        for value in ["", "0", "false", "y"] {
            assert_eq!(
                decide(Some(0), None, Some(value), NO_SIGNALS),
                SkipPermissionsEnv::RootOutsideSandbox,
                "{value:?}"
            );
        }
    }

    #[test]
    fn empty_is_sandbox_counts_as_unset() {
        assert_eq!(
            decide(Some(0), Some("  "), None, REMOTE),
            SkipPermissionsEnv::SetSandbox {
                signal: "CLAUDE_CODE_REMOTE=true"
            }
        );
    }

    // --- root outside a sandbox ---------------------------------------------

    #[test]
    fn root_outside_a_sandbox_fails_naming_is_sandbox() {
        let decision = decide(Some(0), None, None, NO_SIGNALS);
        assert_eq!(decision, SkipPermissionsEnv::RootOutsideSandbox);
        let mut command = Command::new("claude");
        let error = decision.apply(&mut command).unwrap_err();
        assert_eq!(error, RootSandboxError::RootOutsideSandbox);
        assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
        assert_eq!(
            command_is_sandbox(&command),
            None,
            "a real host must never be declared a sandbox"
        );
    }

    // --- explicit override --------------------------------------------------

    #[test]
    fn explicit_is_sandbox_1_is_respected_as_root_anywhere() {
        for signals in [NO_SIGNALS, REMOTE] {
            let decision = decide(Some(0), Some("1"), None, signals);
            assert_eq!(decision, SkipPermissionsEnv::AlreadySandboxed);
            let mut command = Command::new("claude");
            decision.apply(&mut command).unwrap();
            assert_eq!(
                command_is_sandbox(&command),
                None,
                "the inherited value flows through untouched"
            );
        }
    }

    #[test]
    fn explicit_affirmative_spelling_reaches_the_child_as_1() {
        // Claude Code cloud containers export IS_SANDBOX=yes; the CLI takes only 1.
        for value in ["yes", "YES", "true", "on", " y "] {
            for signals in [NO_SIGNALS, REMOTE] {
                let decision = decide(Some(0), Some(value), None, signals);
                assert_eq!(
                    decision,
                    SkipPermissionsEnv::NormalizeExplicit {
                        value: value.trim().to_string()
                    }
                );
                let mut command = Command::new("claude");
                decision.apply(&mut command).unwrap();
                assert_eq!(command_is_sandbox(&command), Some(Some("1".to_string())));
            }
        }
    }

    #[test]
    fn explicit_negative_or_unknown_value_is_respected_and_reported_as_root() {
        for value in ["0", "false", "no", "off", "maybe"] {
            for signals in [NO_SIGNALS, REMOTE] {
                let decision = decide(Some(0), Some(value), None, signals);
                assert_eq!(
                    decision,
                    SkipPermissionsEnv::ExplicitlyNotSandboxed {
                        value: value.to_string()
                    }
                );
                let mut command = Command::new("claude");
                let error = decision.apply(&mut command).unwrap_err();
                assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
                assert!(error.to_string().contains(&format!("{value:?}")), "{error}");
                assert_eq!(command_is_sandbox(&command), None, "never overridden");
            }
        }
    }

    // --- signal detection ---------------------------------------------------

    #[test]
    fn claude_code_remote_must_be_true() {
        assert!(
            SandboxSignals::from_state(Some("true"), false, false, None, false).claude_code_remote
        );
        assert!(
            SandboxSignals::from_state(Some("TRUE"), false, false, None, false).claude_code_remote
        );
        assert!(
            !SandboxSignals::from_state(Some("false"), false, false, None, false)
                .claude_code_remote
        );
        assert!(
            !SandboxSignals::from_state(Some(""), false, false, None, false).claude_code_remote
        );
        assert!(!SandboxSignals::from_state(None, false, false, None, false).claude_code_remote);
    }

    #[test]
    fn container_cgroups_are_recognised_and_host_cgroups_are_not() {
        for cgroup in [
            "12:memory:/docker/0123abcd",
            "0::/system.slice/docker-0123abcd.scope",
            "0::/kubepods/besteffort/pod1234/abcd",
            "0::/kubepods.slice/kubepods-burstable.slice/cri-containerd-abcd.scope",
            "0::/machine.slice/libpod-abcd.scope",
            "0::/lxc/box",
            "0::/lxc.payload.box",
            "0::/kubepods.slice/crio-abcd.scope",
            "1:name=systemd:/\n0::/docker/abcd",
        ] {
            assert!(
                SandboxSignals::from_state(None, false, false, Some(cgroup), false)
                    .container_cgroup,
                "{cgroup}"
            );
        }
        for cgroup in [
            "0::/",
            "0::/init.scope",
            "0::/user.slice/user-1000.slice/session-2.scope",
            // A host unit that merely mentions a runtime is not a container.
            "0::/system.slice/containerd.service",
            "0::/system.slice/docker.service",
            "0::/user.slice/mydocker-notes.scope",
            // A marker in the controller name, not the path, does not count.
            "3:docker:/",
        ] {
            assert_eq!(
                SandboxSignals::from_state(None, false, false, Some(cgroup), false),
                NO_SIGNALS,
                "{cgroup}"
            );
        }
    }

    #[test]
    fn wsl_ignores_marker_files_but_not_a_container_cgroup() {
        // A distribution imported from `docker export` keeps `/.dockerenv`.
        assert_eq!(
            SandboxSignals::from_state(None, true, true, Some("0::/"), true),
            NO_SIGNALS
        );
        assert_eq!(
            decide(
                Some(0),
                None,
                None,
                SandboxSignals::from_state(None, true, true, None, true)
            ),
            SkipPermissionsEnv::RootOutsideSandbox
        );
        let outside_wsl = SandboxSignals::from_state(None, true, true, None, false);
        assert!(outside_wsl.dockerenv && outside_wsl.containerenv);
        assert!(
            SandboxSignals::from_state(None, false, false, Some("0::/docker/abcd"), true)
                .container_cgroup
        );
        assert!(
            SandboxSignals::from_state(Some("true"), false, false, None, true).claude_code_remote
        );
    }

    #[test]
    fn args_skip_permissions_matches_bare_and_valued_forms() {
        assert!(args_skip_permissions(&[
            "-p",
            "--dangerously-skip-permissions"
        ]));
        assert!(args_skip_permissions(&[
            "--dangerously-skip-permissions=true"
        ]));
        assert!(!args_skip_permissions(&["-p", "--verbose"]));
        assert!(!args_skip_permissions::<&str>(&[]));
    }
}
