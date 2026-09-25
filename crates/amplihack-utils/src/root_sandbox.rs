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
//! - `IS_SANDBOX` already set by the user: respected, never overwritten. `1`
//!   passes through; any other value on root is reported up front, because
//!   Claude Code accepts only the exact value `1`.
//! - Root inside a detectable sandbox (`CLAUDE_CODE_REMOTE=true`,
//!   `/.dockerenv`, `/run/.containerenv`, or a container marker in
//!   `/proc/1/cgroup`): `IS_SANDBOX=1` is set on the child `claude` process
//!   only. amplihack's own environment is never modified.
//! - Root with no sandbox signal: an error naming `IS_SANDBOX=1`, raised before
//!   anything is spawned. amplihack never silently declares a real host a
//!   sandbox.

use std::fmt;
use std::path::Path;
use std::process::Command;

/// The variable Claude Code reads to allow the flag as root.
pub const IS_SANDBOX_ENV: &str = "IS_SANDBOX";

/// The flag Claude Code refuses as root outside a sandbox.
pub const SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";

/// Set by Claude Code on the web for its cloud containers.
const CLAUDE_CODE_REMOTE_ENV: &str = "CLAUDE_CODE_REMOTE";

/// Substrings of `/proc/1/cgroup` that only appear inside a container.
const CONTAINER_CGROUP_MARKERS: &[&str] = &[
    "docker",
    "kubepods",
    "containerd",
    "libpod",
    "podman",
    "lxc",
    "crio",
];

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
        )
    }

    /// Pure form of [`SandboxSignals::detect`], for tests.
    pub fn from_state(
        claude_code_remote: Option<&str>,
        dockerenv: bool,
        containerenv: bool,
        cgroup: Option<&str>,
    ) -> Self {
        Self {
            claude_code_remote: claude_code_remote
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("true")),
            dockerenv,
            containerenv,
            container_cgroup: cgroup.is_some_and(|contents| {
                CONTAINER_CGROUP_MARKERS
                    .iter()
                    .any(|marker| contents.contains(marker))
            }),
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
    /// The user already exported `IS_SANDBOX=1`; the child inherits it.
    AlreadySandboxed,
    /// Root inside a detected sandbox; set `IS_SANDBOX=1` on the child only.
    SetSandbox {
        /// The signal that identified the sandbox.
        signal: &'static str,
    },
    /// Root, and the user set `IS_SANDBOX` to something other than `1`.
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
pub fn decide(
    euid: Option<u32>,
    explicit_is_sandbox: Option<&str>,
    signals: SandboxSignals,
) -> SkipPermissionsEnv {
    if euid != Some(0) {
        return SkipPermissionsEnv::NotRoot;
    }
    match explicit_is_sandbox.map(str::trim) {
        Some("1") => return SkipPermissionsEnv::AlreadySandboxed,
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

/// [`decide`] against the real process state.
pub fn detect() -> SkipPermissionsEnv {
    decide(
        effective_uid(),
        std::env::var(IS_SANDBOX_ENV).ok().as_deref(),
        SandboxSignals::detect(),
    )
}

impl SkipPermissionsEnv {
    /// `Err` when Claude Code would refuse the flag; the error says why and
    /// names `IS_SANDBOX=1`.
    pub fn check(&self) -> Result<(), RootSandboxError> {
        match self {
            Self::NotRoot | Self::AlreadySandboxed | Self::SetSandbox { .. } => Ok(()),
            Self::ExplicitlyNotSandboxed { value } => {
                Err(RootSandboxError::ExplicitlyNotSandboxed {
                    value: value.clone(),
                })
            }
            Self::RootOutsideSandbox => Err(RootSandboxError::RootOutsideSandbox),
        }
    }

    /// Prepare `command` (a `claude` child) for the flag: sets `IS_SANDBOX=1`
    /// on it when needed, or returns the error from [`Self::check`].
    pub fn apply(&self, command: &mut Command) -> Result<(), RootSandboxError> {
        self.check()?;
        if let Self::SetSandbox { signal } = self {
            tracing::debug!(
                signal,
                "running as root in a sandbox; setting IS_SANDBOX=1 on the claude child"
            );
            command.env(IS_SANDBOX_ENV, "1");
        }
        Ok(())
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
                 /proc/1/cgroup). If this machine is a disposable sandbox, export \
                 {IS_SANDBOX_ENV}=1 and re-run; otherwise run amplihack as a non-root user."
            ),
            Self::ExplicitlyNotSandboxed { value } => write!(
                f,
                "amplihack runs `claude {SKIP_PERMISSIONS_FLAG}`, which Claude Code refuses as root \
                 (uid 0) unless {IS_SANDBOX_ENV}=1 is set, and {IS_SANDBOX_ENV} is set to \
                 {value:?}. Claude Code accepts only the exact value 1: export \
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
                    decide(Some(1000), explicit, signals),
                    SkipPermissionsEnv::NotRoot
                );
            }
        }
    }

    #[test]
    fn no_uid_platform_is_treated_as_non_root() {
        assert_eq!(decide(None, None, NO_SIGNALS), SkipPermissionsEnv::NotRoot);
    }

    #[test]
    fn non_root_leaves_the_child_environment_alone() {
        let mut command = Command::new("claude");
        decide(Some(1000), None, REMOTE)
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
            let decision = decide(Some(0), None, signals);
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
    fn empty_is_sandbox_counts_as_unset() {
        assert_eq!(
            decide(Some(0), Some("  "), REMOTE),
            SkipPermissionsEnv::SetSandbox {
                signal: "CLAUDE_CODE_REMOTE=true"
            }
        );
    }

    // --- root outside a sandbox ---------------------------------------------

    #[test]
    fn root_outside_a_sandbox_fails_naming_is_sandbox() {
        let decision = decide(Some(0), None, NO_SIGNALS);
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
            let decision = decide(Some(0), Some("1"), signals);
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
    fn explicit_other_value_is_respected_and_reported_as_root() {
        for signals in [NO_SIGNALS, REMOTE] {
            let decision = decide(Some(0), Some("true"), signals);
            assert_eq!(
                decision,
                SkipPermissionsEnv::ExplicitlyNotSandboxed {
                    value: "true".to_string()
                }
            );
            let mut command = Command::new("claude");
            let error = decision.apply(&mut command).unwrap_err();
            assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
            assert!(error.to_string().contains("\"true\""), "{error}");
            assert_eq!(command_is_sandbox(&command), None, "never overwritten");
        }
    }

    // --- signal detection ---------------------------------------------------

    #[test]
    fn claude_code_remote_must_be_true() {
        assert!(SandboxSignals::from_state(Some("true"), false, false, None).claude_code_remote);
        assert!(SandboxSignals::from_state(Some("TRUE"), false, false, None).claude_code_remote);
        assert!(!SandboxSignals::from_state(Some("false"), false, false, None).claude_code_remote);
        assert!(!SandboxSignals::from_state(Some(""), false, false, None).claude_code_remote);
        assert!(!SandboxSignals::from_state(None, false, false, None).claude_code_remote);
    }

    #[test]
    fn container_cgroups_are_recognised_and_host_cgroups_are_not() {
        for cgroup in [
            "12:memory:/docker/0123abcd",
            "0::/kubepods/besteffort/pod1234/abcd",
            "1:name=systemd:/system.slice/containerd.service/x",
            "0::/machine.slice/libpod-abcd.scope",
            "0::/lxc/box",
            "0::/kubepods.slice/crio-abcd.scope",
        ] {
            assert!(
                SandboxSignals::from_state(None, false, false, Some(cgroup)).container_cgroup,
                "{cgroup}"
            );
        }
        for cgroup in ["0::/", "0::/user.slice/user-1000.slice/session-2.scope"] {
            assert_eq!(
                SandboxSignals::from_state(None, false, false, Some(cgroup)),
                NO_SIGNALS,
                "{cgroup}"
            );
        }
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
