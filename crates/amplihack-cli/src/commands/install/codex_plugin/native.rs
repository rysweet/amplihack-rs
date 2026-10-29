//! Native command and inventory boundary.
use super::*;
use std::process::Stdio;

/// amplihack waits for Codex to exit; there is no time limit on any native
/// plugin call. Output is capped per stream and stdin is null.
pub(super) fn native(binary: &Path, args: &[&str], home: &Path) -> Result<Value> {
    path_scope::check()?;
    let mut cmd = Command::new(binary);
    cmd.args(args).env("CODEX_HOME", home).stdin(Stdio::null());
    if !args.contains(&"--json") {
        cmd.arg("--json");
    }
    let output = crate::util::run_output_limited(cmd, 1024 * 1024)?;
    path_scope::check()?;
    ensure!(
        output.status.success(),
        "Codex native plugin command failed ({}); check native plugin support and authentication",
        output.status
    );
    serde_json::from_slice(&output.stdout).map_err(|_| {
        anyhow::anyhow!("invalid native Codex plugin response; expected JSON inventory")
    })
}

/// Whether the pinned Codex offers the `plugin list` subcommand, judged from
/// `plugin --help`. Returns `Ok(false)` only when the subcommand is clearly
/// missing: help exits 0 without plugin help, or exits 2 reporting an
/// unrecognized subcommand. Any other failure is an error. Child output is
/// matched against fixed literals only and never leaves this function:
/// callers receive a bool or an amplihack error.
pub(super) fn plugin_capability(binary: &Path, home: &Path) -> Result<bool> {
    path_scope::check()?;
    let mut cmd = Command::new(binary);
    cmd.args(["plugin", "--help"])
        .env("CODEX_HOME", home)
        .stdin(Stdio::null());
    let output = crate::util::run_output_limited(cmd, 64 * 1024)?;
    path_scope::check()?;
    match output.status.code() {
        Some(0) => Ok(is_plugin_help(&String::from_utf8_lossy(&output.stdout))),
        Some(2) if String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand") => {
            Ok(false)
        }
        _ => bail!(
            "native plugin capability check `plugin --help` failed with {}",
            output.status
        ),
    }
}

/// Plugin help has a usage line whose subcommand is `plugin` and a `list`
/// command line. The binary name in the usage line is not checked, so a
/// renamed or symlinked Codex still matches.
fn is_plugin_help(help: &str) -> bool {
    let usage = help.lines().any(|line| {
        let mut words = line.split_whitespace();
        words.next() == Some("Usage:") && words.nth(1) == Some("plugin")
    });
    let list = help.lines().any(|line| {
        line.trim_start()
            .strip_prefix("list")
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    });
    usage && list
}

/// Explain a failed inventory probe for selected Codex. Only errors amplihack
/// generated are formatted here; they never quote child output.
pub(super) fn classify_probe_failure(
    probe_err: anyhow::Error,
    capability: Result<bool>,
) -> anyhow::Error {
    match capability {
        Ok(false) => {
            anyhow::anyhow!("selected Codex requires native plugin support; update Codex and retry")
        }
        Ok(true) => probe_err.context(
            "selected Codex supports native plugins but the plugin inventory probe failed",
        ),
        Err(cap) => probe_err.context(format!(
            "native plugin inventory probe failed; capability check also failed: {cap:#}"
        )),
    }
}
pub(super) fn installed(value: &Value) -> bool {
    value["installed"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|p| p["pluginId"].as_str() == Some(ID) && p["installed"].as_bool() == Some(true))
    })
}
pub(super) fn verify_identity(list: &Value, expected: &Path) -> Result<()> {
    for plugin in list["installed"]
        .as_array()
        .context("native installed inventory must be an array")?
    {
        if plugin["pluginId"].as_str() == Some(ID) {
            let source = plugin
                .pointer("/source/path")
                .and_then(Value::as_str)
                .context("native Codex identity lacks source provenance")?;
            ensure!(
                Path::new(source).canonicalize()? == expected.canonicalize()?,
                "Codex plugin identity belongs to a different source; refusing replacement/removal"
            );
        }
    }
    Ok(())
}

thread_local! {
    static SELECTED_BINARY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Keep native registration on the executable the frontdoor already resolved.
pub(super) fn with_binary<T>(binary: &Path, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SELECTED_BINARY.with(|selected| *selected.borrow_mut() = self.0.take());
        }
    }
    let _restore =
        Restore(SELECTED_BINARY.with(|selected| selected.replace(Some(binary.to_path_buf()))));
    action()
}

/// Use the same override-aware resolution contract as the launch frontdoor.
pub(super) fn selected_binary() -> Result<Option<PathBuf>> {
    if let Some(binary) = SELECTED_BINARY.with(|selected| selected.borrow().clone()) {
        return Ok(Some(binary));
    }
    use amplihack_utils::launch_target::{OverrideOrigin, resolve_uncached};
    let resolution = resolve_uncached("codex", OverrideOrigin::User);
    if let Some(target) = resolution.target {
        return Ok(Some(target.path));
    }
    if crate::freshness::codex_selected() {
        bail!(
            "selected Codex executable unavailable; {}",
            resolution.rejection_report("codex", "@openai/codex")
        );
    }
    if !resolution.rejected.is_empty() {
        tracing::warn!(
            "optional Codex skipped: {}",
            resolution.rejection_report("codex", "@openai/codex")
        );
    }
    Ok(None)
}
