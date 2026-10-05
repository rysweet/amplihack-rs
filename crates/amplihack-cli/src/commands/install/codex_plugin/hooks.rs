//! Exact owned hook reconciliation preserving foreign definitions.
use super::*;
pub(super) fn reconcile_hooks(home: &Path, previous: &Value, desired: &Value) -> Result<()> {
    fs::create_dir_all(home)?;
    ensure!(
        !fs::symlink_metadata(home)?.file_type().is_symlink(),
        "refusing symlink Codex home"
    );
    let lock = fs::File::open(home)?;
    lock.lock_exclusive()?;
    let path = home.join("hooks.json");
    let original = regular_json(&path)?;
    let value = merged_hooks(original.clone(), previous, desired)?;
    atomic_json(&path, &value, original)?;
    ensure!(
        regular_json(&path)? == Some(value),
        "native Codex hooks changed before ownership commit"
    );
    Ok(())
}
pub(super) fn merged_hooks(
    original: Option<Value>,
    previous: &Value,
    desired: &Value,
) -> Result<Value> {
    let mut value = original.unwrap_or_else(|| json!({"hooks":{}}));
    ensure!(value.is_object(), "Codex hooks.json must be an object");
    if value.get("hooks").is_none() {
        value["hooks"] = json!({});
    }
    let map = value["hooks"]
        .as_object_mut()
        .context("Codex hooks must be an object")?;
    for (event, owned) in previous.as_object().context("invalid owned hook record")? {
        if let Some(entries) = map.get_mut(event) {
            let entries = entries
                .as_array_mut()
                .context("Codex event handlers must be arrays")?;
            entries.retain(|entry| !owned.as_array().is_some_and(|a| a.contains(entry)));
        }
    }
    for (event, handlers) in desired.as_object().context("invalid desired hooks")? {
        let entries = map
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Codex event handlers must be arrays")?;
        for handler in handlers.as_array().context("invalid hook handlers")? {
            ensure!(
                !entries.contains(handler),
                "identical unowned Codex hook definition exists; preserve it and reconcile ownership manually"
            );
            entries.push(handler.clone());
        }
    }
    Ok(value)
}

pub(super) fn declarations(package: &Path) -> Result<Value> {
    let mut hooks = json!({});
    let installed_wrapper = package.join("bin/hook");
    let quoted = format!(
        "'{}'",
        installed_wrapper.to_string_lossy().replace('\'', "'\\''")
    );
    for (event, subcommand) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt"),
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
        ("Stop", "stop"),
        ("SessionEnd", "session-stop"),
    ] {
        hooks[event] =
            json!([{"hooks":[{"type":"command","command":format!("{quoted} {subcommand}")}]}]);
    }
    hooks["UserPromptSubmit"][0]["hooks"].as_array_mut().context("user hook array")?
        .push(json!({"type":"command","command":format!("{quoted} workflow-classification-reminder")}));
    Ok(hooks)
}
