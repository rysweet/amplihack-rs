//! Native command and inventory boundary.
use super::*;
pub(super) fn native(binary: &Path, args: &[&str], home: &Path) -> Result<Value> {
    path_scope::check()?;
    let mut cmd = Command::new(binary);
    cmd.args(args).env("CODEX_HOME", home);
    if !args.contains(&"--json") {
        cmd.arg("--json");
    }
    let output =
        crate::util::run_output_with_timeout_limited(cmd, Duration::from_secs(30), 1024 * 1024)?;
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
