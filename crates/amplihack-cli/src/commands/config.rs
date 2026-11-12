//! `amplihack config get|set|unset|list|edit|path` (issue #1606).
//!
//! The contract is `docs/reference/user-config.md`. stdout carries data only;
//! errors and file diagnostics go to stderr. A usage error (unknown key,
//! invalid value, `--project` where it cannot apply) exits 2; a file or
//! runtime error exits 1.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::ConfigCommands;
use crate::command_error::exit_error;
use crate::user_config::edit::{ensure_private_dir, set_value, tighten_owned_dir, unset_value};
use crate::user_config::files::Project;
use crate::user_config::safe::display_path;
use crate::user_config::schema::{self, KeyDef, Scope, Value, validate_home_dir};
use crate::user_config::{ConfigError, Effective, Entry, load_for};

/// Dispatch one `amplihack config` subcommand.
pub fn run_config(command: ConfigCommands) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let outcome = match command {
        ConfigCommands::Get { key, show_origin } => get(&cwd, &key, show_origin),
        ConfigCommands::Set {
            key,
            value,
            project,
        } => set(&cwd, &key, &value, project),
        ConfigCommands::Unset { key, project } => unset(&cwd, &key, project),
        ConfigCommands::List { show_origin } => {
            print!("{}", render_list(&load_for(&cwd), show_origin));
            Ok(())
        }
        ConfigCommands::Edit { project } => edit(&cwd, project),
        ConfigCommands::Path => {
            print!("{}", render_paths(&load_for(&cwd)));
            Ok(())
        }
    };
    outcome.map_err(|error| {
        eprintln!("error: {error}");
        exit_error(error.exit_code())
    })
}

/// One `list` row: `key = value`, with `  # origin` when asked.
fn render_row(entry: &Entry, show_origin: bool) -> String {
    let value = entry
        .value
        .as_ref()
        .map_or_else(|| "(unset)".to_string(), Value::to_string);
    if show_origin {
        format!("{} = {value}  # {}", entry.key.name, entry.origin)
    } else {
        format!("{} = {value}", entry.key.name)
    }
}

/// Every key in schema order, then a `Config problems:` block when a file
/// had any.
fn render_list(effective: &Effective, show_origin: bool) -> String {
    let mut out = String::new();
    for entry in &effective.entries {
        let _ = writeln!(out, "{}", render_row(entry, show_origin));
    }
    push_problems(&mut out, effective);
    out
}

/// A `Config problems:` block, one indented line per diagnostic, when a file
/// had any.
fn push_problems(out: &mut String, effective: &Effective) {
    if !effective.diagnostics.is_empty() {
        out.push_str("\nConfig problems:\n");
        for diagnostic in &effective.diagnostics {
            let _ = writeln!(out, "  {diagnostic}");
        }
    }
}

fn render_paths(effective: &Effective) -> String {
    let mut out = String::new();
    match &effective.user_file {
        Some(path) => {
            let _ = writeln!(out, "user: {}", display_path(path));
        }
        None => {
            let _ = writeln!(
                out,
                "user: (none: {})",
                crate::user_config::home_unavailable()
            );
        }
    }
    match &effective.project {
        Project::NotInRepo => out.push_str("project: (none: not in a git repository)\n"),
        Project::File { path, .. } => {
            let _ = writeln!(out, "project: {}", display_path(path));
        }
    }
    if let Some(legacy) = &effective.legacy {
        let _ = writeln!(out, "legacy: {}", display_path(&legacy.path));
    }
    out
}

fn get(cwd: &Path, key: &str, show_origin: bool) -> Result<(), ConfigError> {
    let def = schema::known_key(key)?;
    let effective = load_for(cwd);
    let entry = effective.entry(def.name);
    let value = entry.value.as_ref().map(Value::bare).unwrap_or_default();
    if show_origin {
        println!("{value}  # {}", entry.origin);
    } else {
        println!("{value}");
    }
    Ok(())
}

/// The file a write goes to: the user file, or with `project` the project
/// file at the git root (or the nearest existing one below it).
fn target_file(
    cwd: &Path,
    def: &KeyDef,
    project: bool,
    effective: &Effective,
) -> Result<PathBuf, ConfigError> {
    if project {
        if def.scope == Scope::UserOnly {
            return Err(ConfigError::UserOnly(def.name.to_string()));
        }
        return project_target(cwd);
    }
    effective.user_file.clone().ok_or_else(|| {
        ConfigError::Runtime(format!(
            "{}; cannot find the user file",
            crate::user_config::home_unavailable()
        ))
    })
}

/// The project file a `--project` write may go to. It is refused when it is a
/// symlink, or when its directory resolves outside the git root: a cloned
/// repository could otherwise point the write at any file this user owns.
fn project_target(cwd: &Path) -> Result<PathBuf, ConfigError> {
    let path = crate::user_config::files::project(cwd)
        .path()
        .map(Path::to_path_buf)
        .ok_or(ConfigError::NoProject)?;
    let refuse = |reason: &str| ConfigError::File {
        path: display_path(&path),
        reason: reason.to_string(),
    };
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(refuse("is a symlink; refusing to write through it"));
    }
    // The same rule as reading a project file: under the test guard, never
    // outside the temp dir.
    if crate::user_config::guard_forbids(&path) {
        return Err(refuse(
            "is outside the temp dir and AMPLIHACK_TEST_HOME_GUARD is set; refusing to write",
        ));
    }
    let root = crate::user_config::files::git_root(cwd).ok_or(ConfigError::NoProject)?;
    let root = root.canonicalize().unwrap_or(root);
    if let Some(dir) = path.parent()
        && let Ok(resolved) = dir.canonicalize()
        && !resolved.starts_with(&root)
    {
        return Err(refuse(
            "resolves outside the git repository; refusing to write",
        ));
    }
    Ok(path)
}

/// `node.heap_consent` cannot be changed while the legacy file still holds
/// `node_options_consent`: the legacy value would be read again (as the
/// `legacy` layer) the moment the user file stops setting it.
fn refuse_while_legacy_remains(def: &KeyDef, effective: &Effective) -> Result<(), ConfigError> {
    match &effective.legacy {
        Some(legacy) if def.name == "node.heap_consent" => Err(ConfigError::File {
            path: display_path(&legacy.path),
            reason: "still holds node_options_consent because its migration did not complete; \
                     fix the reported problem and retry"
                .to_string(),
        }),
        _ => Ok(()),
    }
}

/// After a write to the user file, `~/.amplihack` is private.
fn tighten_user_dir(path: &Path, effective: &Effective) {
    if effective.user_file.as_deref() == Some(path)
        && let Some(dir) = path.parent()
    {
        tighten_owned_dir(dir);
    }
}

fn set(cwd: &Path, key: &str, raw: &str, project: bool) -> Result<(), ConfigError> {
    let def = schema::known_key(key)?;
    let value = schema::parse_cli(def.name, raw)?;
    if def.name == "paths.home" {
        validate_home_dir(Path::new(raw)).map_err(|reason| ConfigError::InvalidValue {
            key: def.name.to_string(),
            reason,
        })?;
    }
    // Loading first runs the legacy migration, so a set never races it.
    let effective = load_for(cwd);
    let path = target_file(cwd, def, project, &effective)?;
    refuse_while_legacy_remains(def, &effective)?;
    set_value(&path, def, &value)?;
    tighten_user_dir(&path, &effective);
    println!("set {} = {value} in {}", def.name, display_path(&path));
    Ok(())
}

fn unset(cwd: &Path, key: &str, project: bool) -> Result<(), ConfigError> {
    let def = schema::known_key(key)?;
    let effective = load_for(cwd);
    let path = target_file(cwd, def, project, &effective)?;
    refuse_while_legacy_remains(def, &effective)?;
    if unset_value(&path, def)? {
        println!("unset {} in {}", def.name, display_path(&path));
    } else {
        println!("{} is not set in {}", def.name, display_path(&path));
    }
    Ok(())
}

/// Open the user (or project) file in `$VISUAL`, `$EDITOR` or `vi`, then
/// re-read it and report every problem left in it.
///
/// The editor string is run by `sh` the way git runs it, with the path as
/// `"$1"`, so an editor with arguments works and the path is never re-parsed.
fn edit(cwd: &Path, project: bool) -> Result<(), ConfigError> {
    let effective = load_for(cwd);
    let path = if project {
        project_target(cwd)?
    } else {
        effective.user_file.clone().ok_or_else(|| {
            ConfigError::Runtime(format!(
                "{}; cannot find the user file",
                crate::user_config::home_unavailable()
            ))
        })?
    };
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
        tighten_user_dir(&path, &effective);
    }
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg("amplihack-config-edit")
        .arg(&path)
        .status()
        .map_err(|error| ConfigError::Runtime(format!("cannot start the editor: {error}")))?;
    if !status.success() {
        return Err(ConfigError::Runtime(format!(
            "the editor exited with {status}; {} was left as the editor wrote it",
            display_path(&path)
        )));
    }
    let shown = display_path(&path);
    let problems: Vec<String> = load_for(cwd)
        .diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.path == shown)
        .map(|diagnostic| diagnostic.to_string())
        .collect();
    if problems.is_empty() {
        return Ok(());
    }
    Err(ConfigError::File {
        path: shown,
        reason: format!(
            "still has {} problem(s), reported above; the file was kept as edited",
            problems.len()
        ),
    })
}

/// The `Effective config` section `amplihack doctor` prints: the rows of
/// `config list --show-origin`, indented under a heading, then any
/// `Config problems:` block.
pub(crate) fn doctor_section(cwd: &Path) -> String {
    let effective = load_for(cwd);
    let mut out = String::from("\nEffective config:\n");
    for entry in &effective.entries {
        let _ = writeln!(out, "  {}", render_row(entry, true));
    }
    push_problems(&mut out, &effective);
    out
}
