//! Launcher generation and process spawning for workstreams.
use super::command_builder;
#[cfg(test)]
pub(super) use super::command_builder::VALID_DELEGATES;
pub(super) use super::command_builder::{detect_delegate, populate_workstream_scope};
use super::models::Workstream;
use super::utils::set_executable;
use amplihack_types::hook_io::normalize_executable_script_line_endings;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;

/// Write recipe-mode launcher scripts for a workstream, reading the pinned
/// Claude profile from the process environment.
pub(super) fn write_recipe_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    write_recipe_launcher_with(
        ws,
        delegate,
        command_builder::env_var_os,
        crate::commands::profile::substitute_inherited_profile,
    )
}

/// Write recipe-mode launcher scripts for a workstream, reading the profile
/// variables through `get` instead of the process environment (issue #2014).
pub(super) fn write_recipe_launcher_with(
    ws: &Workstream,
    delegate: &str,
    get: impl Fn(&str) -> Option<OsString>,
    substitute: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    // Built first so a refused profile export or recipe name leaves no
    // partial scripts behind.
    let run_content = command_builder::recipe_run_script(ws, delegate, get, substitute)?;
    let launcher_content = command_builder::recipe_launcher_script(&ws.recipe)?;
    let resume_context = command_builder::build_resume_context(ws);
    let safe_context = serde_json::to_string(&resume_context)?;

    // Issue #1570: the launcher passes this file with `--context-file`. It
    // holds the task description, so only the owner may read it.
    let context_json = ws.work_dir.join("context.json");
    write_private_file(&context_json, safe_context.as_bytes())?;

    let launcher_sh = ws.work_dir.join("launcher.sh");
    write_executable_script(&launcher_sh, &launcher_content)?;
    set_executable(&launcher_sh)?;

    let run_sh = ws.work_dir.join("run.sh");
    write_executable_script(&run_sh, &run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

/// Write classic-mode launcher scripts for a workstream, reading the pinned
/// Claude profile from the process environment.
pub(super) fn write_classic_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    write_classic_launcher_with(
        ws,
        delegate,
        command_builder::env_var_os,
        crate::commands::profile::substitute_inherited_profile,
    )
}

/// Write classic-mode launcher scripts for a workstream, reading the profile
/// variables through `get` instead of the process environment (issue #2014).
pub(super) fn write_classic_launcher_with(
    ws: &Workstream,
    delegate: &str,
    get: impl Fn(&str) -> Option<OsString>,
    substitute: impl Fn(&str) -> Option<String>,
) -> Result<()> {
    let run_content = command_builder::classic_run_script(ws, delegate, get, substitute)?;
    let task_md = ws.work_dir.join("TASK.md");
    fs::write(&task_md, command_builder::classic_task_markdown(ws))?;

    let run_sh = ws.work_dir.join("run.sh");
    write_executable_script(&run_sh, &run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

/// Write `contents` to `path` with mode 0600 on Unix, tightening the mode of
/// an existing file before its contents are replaced.
fn write_private_file(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("restrict permissions of {}", path.display()))?;
        }
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("write {}", path.display()))
}

pub(super) fn write_executable_script(path: &std::path::Path, content: &str) -> Result<()> {
    let normalized = normalize_executable_script_line_endings(content);
    fs::write(path, normalized)
        .with_context(|| format!("write executable launcher script {}", path.display()))
}
