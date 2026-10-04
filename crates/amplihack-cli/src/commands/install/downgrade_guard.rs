//! Refuse implicit re-stages that would downgrade a newer install (issue #1526).
//!
//! A re-stage runs the full installer: it replaces the binaries in
//! `~/.local/bin` and rewrites `~/.claude/settings.json`. That is right when a
//! newer binary meets an older install, and wrong the other way round. A
//! `cargo install --git` build of the v0.18.39 commit reported `0.18.0`, and a
//! plain `amplihack recipe run` from it re-staged over the v0.18.39 install.
//!
//! This module is the only place that decides whether an implicit re-stage
//! would be a downgrade, and the only place that says so. Both implicit
//! triggers call [`warn_if_implicit_downgrade`]: startup self-heal (before and
//! under the install lock) and the launch bootstrap
//! (`ensure_framework_installed`). Explicit `amplihack install` and
//! `amplihack update` are deliberately not guarded: they are how a user
//! deploys a specific build on purpose, including an older one.
//!
//! The guard never touches the filesystem, never runs a process and never
//! logs. It decides, and on refusal writes one line to the injected writer.

use std::cmp::Ordering;
use std::io::Write;

use anyhow::{Context, Result};
use semver::Version;

/// Printed in place of a path that cannot be resolved. Never quoted, so it
/// stays distinct from a real file named `<unknown>`, which prints quoted.
const UNKNOWN_PATH: &str = "<unknown>";

/// True when re-staging `running` over an install stamped `stamp` would be a
/// downgrade.
///
/// - No usable stamp (missing, malformed, or not strict semver): `false`, so
///   the #502 first-install and repair path still runs. An unparseable stamp
///   protects nothing.
/// - Valid stamp and `running` is not valid semver: `true`. A broken
///   comparison must never be the reason a downgrade happens.
/// - Otherwise `true` only when `running` has lower semver precedence. Build
///   metadata is ignored and versions are never compared as strings, so
///   `0.18.9` is older than `0.18.39` and `0.18.0-dev` is older than `0.18.0`.
pub(crate) fn is_implicit_downgrade(stamp: Option<&str>, running: &str) -> bool {
    let Some(installed) = stamp.and_then(|s| Version::parse(s).ok()) else {
        return false;
    };
    match Version::parse(running) {
        Ok(running) => running.cmp_precedence(&installed) == Ordering::Less,
        Err(_) => true,
    }
}

/// Apply [`is_implicit_downgrade`] and, on refusal, write exactly one line to
/// `notice` naming both binaries and both versions.
///
/// Returns `Ok(true)` when the re-stage must not run. Every caller propagates
/// the error with `?` and returns early on `true`, so a failure here can never
/// turn into "go ahead and install".
///
/// Paths are printed with `{:?}`, which escapes newlines, terminal escapes and
/// bidi controls, so a hostile `HOME` cannot forge a second line. The stamp is
/// printed only after it has parsed as semver (restricting it to ASCII
/// `[0-9A-Za-z.+-]`), and `running` is a compile-time constant.
pub(crate) fn warn_if_implicit_downgrade<W: Write>(
    stamp: Option<&str>,
    running: &str,
    notice: &mut W,
) -> Result<bool> {
    let Some(installed_version) = stamp.filter(|s| is_implicit_downgrade(Some(s), running)) else {
        return Ok(false);
    };
    let running_path = std::env::current_exe()
        .map(|exe| format!("{exe:?}"))
        .unwrap_or_else(|_| UNKNOWN_PATH.to_string());
    let installed_path = super::paths::preferred_user_bin_dir()
        .map(|dir| {
            let installed = dir.join(crate::path_conflicts::binary_filename("amplihack"));
            format!("{installed:?}")
        })
        .unwrap_or_else(|_| UNKNOWN_PATH.to_string());
    writeln!(
        notice,
        "amplihack: refusing implicit re-stage: running {running_path} is v{running}, older \
         than installed {installed_path} v{installed_version}; nothing was changed. Run \
         'amplihack install' to deploy this build explicitly."
    )
    .context("emitting implicit downgrade refusal")?;
    Ok(true)
}
