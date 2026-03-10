//! Native install and uninstall commands.

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REPO_URL: &str = "https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding";
const ESSENTIAL_DIRS: &[&str] = &[
    "agents/amplihack",
    "commands/amplihack",
    "tools/amplihack",
    "tools/xpia",
    "context",
    "workflow",
    "skills",
    "templates",
    "scenarios",
    "docs",
    "schemas",
    "config",
];
const ESSENTIAL_FILES: &[&str] = &["tools/statusline.sh", "AMPLIHACK.md"];
const RUNTIME_DIRS: &[&str] = &[
    "runtime",
    "runtime/logs",
    "runtime/metrics",
    "runtime/security",
    "runtime/analysis",
];
const AMPLIHACK_HOOK_FILES: &[&str] = &[
    "session_start.py",
    "stop.py",
    "pre_tool_use.py",
    "post_tool_use.py",
    "user_prompt_submit.py",
    "workflow_classification_reminder.py",
    "pre_compact.py",
];
const XPIA_HOOK_FILES: &[&str] = &["session_start.py", "post_tool_use.py", "pre_tool_use.py"];

#[derive(Debug, Serialize)]
struct InstallManifest {
    files: Vec<String>,
    dirs: Vec<String>,
}

pub fn run_install() -> Result<()> {
    let temp_dir = tempfile::tempdir().context("failed to create temp dir for install")?;
    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(REPO_URL)
        .arg(temp_dir.path())
        .status()
        .context("failed to run git clone")?;

    if !status.success() {
        let exit_status = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "Failed to install: Command '['git', 'clone', '--depth', '1', '{}', '{}']' returned non-zero exit status {}.",
            REPO_URL,
            temp_dir.path().display(),
            exit_status,
        );
        return Err(exit_error(1));
    }

    local_install(temp_dir.path())
}

pub fn run_uninstall() -> Result<()> {
    let claude_dir = claude_dir()?;
    let manifest_path = manifest_path()?;
    let (files, dirs) = read_manifest(&manifest_path)?;

    let mut removed_any = false;
    let mut removed_files = 0usize;
    for file in files {
        let target = claude_dir.join(&file);
        if target.is_file() {
            match fs::remove_file(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_files += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove file {file}: {error}");
                }
            }
        }
    }

    for dir in dirs
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
    {
        let target = claude_dir.join(&dir);
        if target.is_dir() && fs::remove_dir_all(&target).is_ok() {
            removed_any = true;
        }
    }

    let mut removed_dirs = 0usize;
    for dir in ["agents/amplihack", "commands/amplihack", "tools/amplihack"] {
        let target = claude_dir.join(dir);
        if target.exists() {
            match fs::remove_dir_all(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_dirs += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove {}: {}", target.display(), error);
                }
            }
        }
    }

    let _ = fs::remove_file(&manifest_path);

    if removed_any {
        println!("✅ Uninstalled amplihack from {}", claude_dir.display());
        if removed_files > 0 {
            println!("   • Removed {removed_files} files");
        }
        if removed_dirs > 0 {
            println!("   • Removed {removed_dirs} amplihack directories");
        }
    } else {
        println!("Nothing to uninstall.");
    }

    Ok(())
}

fn local_install(repo_root: &Path) -> Result<()> {
    let claude_dir = claude_dir()?;
    let timestamp = unix_timestamp();

    println!();
    println!("🚀 Starting amplihack installation...");
    println!("   Source: {}", repo_root.display());
    println!("   Target: {}", claude_dir.display());
    println!();
    println!(
        "ℹ️  Profile management unavailable (No module named 'profile_management'), using full installation"
    );
    println!();

    ensure_dirs(&claude_dir)?;
    let pre_dirs = all_rel_dirs(&claude_dir)?;

    println!("📁 Copying essential directories:");
    let source_claude = find_source_claude_dir(repo_root)?;
    let copied_dirs = copytree_manifest(&source_claude, &claude_dir)?;
    if copied_dirs.is_empty() {
        println!();
        println!("❌ No directories were copied. Installation may be incomplete.");
        println!("   Please check that the source repository is valid.");
        println!();
        return Ok(());
    }

    println!();
    println!("📝 Initializing PROJECT.md:");
    initialize_project_md(&claude_dir)?;

    println!();
    println!("📂 Creating runtime directories:");
    create_runtime_dirs(&claude_dir)?;

    println!();
    println!("⚙️  Configuring settings.json:");
    let settings_ok = ensure_settings_json(&claude_dir, timestamp)?;

    println!();
    println!("🔍 Verifying hook files:");
    let hooks_ok = verify_hooks()?;

    println!();
    println!("🦀 Ensuring Rust recipe runner:");
    if find_binary("recipe-runner-rs").is_some() {
        println!("   ✅ recipe-runner-rs is available");
    } else {
        println!("   ❌ recipe-runner-rs not installed (recipe execution will fail without it)");
        println!(
            "   Install: cargo install --git https://github.com/rysweet/amplihack-recipe-runner"
        );
    }

    println!();
    println!("📝 Generating uninstall manifest:");
    let manifest_path = manifest_path()?;
    let mut tracked_roots = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    let (files, post_dirs) = get_all_files_and_dirs(&claude_dir, &tracked_roots)?;
    let new_dirs = post_dirs
        .into_iter()
        .filter(|dir| !pre_dirs.contains(dir))
        .collect::<Vec<_>>();
    write_manifest(&manifest_path, &files, &new_dirs)?;
    println!("   Manifest written to {}", manifest_path.display());

    println!();
    println!("============================================================");
    if settings_ok && hooks_ok && !copied_dirs.is_empty() {
        println!("✅ Amplihack installation completed successfully!");
        println!();
        println!("📍 Installed to: {}", claude_dir.display());
        println!();
        println!("📦 Components installed:");
        for dir in &copied_dirs {
            println!("   • {dir}");
        }
        println!();
        println!("🎯 Features enabled:");
        println!("   • Session start hook");
        println!("   • Stop hook");
        println!("   • Post-tool-use hook");
        println!("   • Pre-compact hook");
        println!("   • Runtime logging and metrics");
        println!();
        println!("💡 To uninstall: amplihack uninstall");
    } else {
        println!("⚠️  Installation completed with warnings");
        if !settings_ok {
            println!("   • Settings.json configuration had issues");
        }
        if !hooks_ok {
            println!("   • Some hook files are missing");
        }
        if copied_dirs.is_empty() {
            println!("   • No directories were copied");
        }
        println!();
        println!("💡 You may need to manually verify the installation");
    }
    println!("============================================================");
    println!();

    Ok(())
}

fn ensure_dirs(claude_dir: &Path) -> Result<()> {
    fs::create_dir_all(claude_dir)
        .with_context(|| format!("failed to create {}", claude_dir.display()))
}

fn find_source_claude_dir(repo_root: &Path) -> Result<PathBuf> {
    let direct = repo_root.join(".claude");
    if direct.exists() {
        return Ok(direct);
    }
    let parent = repo_root.join("..").join(".claude");
    if parent.exists() {
        return Ok(parent);
    }
    anyhow::bail!(
        ".claude not found at {} or {}",
        direct.display(),
        parent.display()
    )
}

fn copytree_manifest(source_claude: &Path, claude_dir: &Path) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let source_dir = source_claude.join(dir);
        if !source_dir.exists() {
            println!("  ⚠️  Warning: {dir} not found in source, skipping");
            continue;
        }

        let target_dir = claude_dir.join(dir);
        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        copy_dir_recursive(&source_dir, &target_dir)?;
        if dir.starts_with("tools/") {
            let files_updated = set_hook_permissions(&target_dir)?;
            if files_updated > 0 {
                println!("  🔐 Set execute permissions on {files_updated} hook files");
            }
        }
        println!("  ✅ Copied {dir}");
        copied.push((*dir).to_string());
    }

    let settings_src = source_claude.join("settings.json");
    let settings_dst = claude_dir.join("settings.json");
    if settings_src.exists() && !settings_dst.exists() {
        fs::copy(&settings_src, &settings_dst).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_src.display(),
                settings_dst.display()
            )
        })?;
        println!("  ✅ Copied settings.json");
    }

    for file in ESSENTIAL_FILES {
        let source_file = source_claude.join(file);
        if !source_file.exists() {
            println!("  ⚠️  Warning: {file} not found in source, skipping");
            continue;
        }
        let target_file = claude_dir.join(file);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&source_file, &target_file).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_file.display(),
                target_file.display()
            )
        })?;
        set_script_permissions(&target_file)?;
        println!("  ✅ Copied {file}");
    }

    let source_claude_md = source_claude
        .parent()
        .context("source .claude dir missing parent")?
        .join("CLAUDE.md");
    if source_claude_md.exists() {
        let target_claude_md = claude_dir
            .parent()
            .context("target .claude dir missing parent")?
            .join("CLAUDE.md");
        fs::copy(&source_claude_md, &target_claude_md).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_claude_md.display(),
                target_claude_md.display()
            )
        })?;
        println!("  ✅ Installed amplihack CLAUDE.md");
    }

    Ok(copied)
}

fn initialize_project_md(claude_dir: &Path) -> Result<()> {
    let project_md = claude_dir.join("context").join("PROJECT.md");
    if let Some(parent) = project_md.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(
        &project_md,
        "# Project Overview\n\nThis PROJECT.md was initialized by amplihack.\n",
    )
    .with_context(|| format!("failed to write {}", project_md.display()))?;
    println!("   ✅ PROJECT.md initialized using template");
    Ok(())
}

fn create_runtime_dirs(claude_dir: &Path) -> Result<()> {
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        fs::create_dir_all(&full)
            .with_context(|| format!("failed to create {}", full.display()))?;
        println!("  ✅ Runtime directory {dir} ready");
    }
    Ok(())
}

fn ensure_settings_json(claude_dir: &Path, timestamp: u64) -> Result<bool> {
    let settings_path = claude_dir.join("settings.json");
    let backup_path = claude_dir.join(format!("settings.json.backup.{timestamp}"));
    if settings_path.exists() {
        fs::copy(&settings_path, &backup_path).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_path.display(),
                backup_path.display()
            )
        })?;
        let backup_dir = claude_dir.join("runtime").join("sessions");
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("failed to create {}", backup_dir.display()))?;
        fs::write(
            backup_dir.join(format!("install_{timestamp}_backup.json")),
            format!(
                "{{\"settings_path\":\"{}\",\"backup_path\":\"{}\"}}",
                settings_path.display(),
                backup_path.display()
            ),
        )
        .context("failed to write install backup metadata")?;
        println!("  💾 Backup created at {}", backup_path.display());
        println!("  📋 Found existing settings.json");
    } else {
        fs::write(&settings_path, "{}")
            .with_context(|| format!("failed to write {}", settings_path.display()))?;
    }

    let missing = missing_hook_paths("amplihack", AMPLIHACK_HOOK_FILES)?;
    if !missing.is_empty() {
        println!("  ❌ Hook validation failed - missing required hooks:");
        for missing_hook in missing {
            println!("     • {missing_hook}");
        }
        println!("  💡 Please reinstall amplihack to restore missing hooks");
        return Ok(false);
    }
    println!("  ✅ settings.json configured");
    Ok(true)
}

fn verify_hooks() -> Result<bool> {
    let mut all_exist = true;
    println!("  📋 Amplihack hooks:");
    for file in AMPLIHACK_HOOK_FILES {
        let hook_path = amplihack_hooks_dir()?.join(file);
        if hook_path.exists() {
            println!("    ✅ {file} found");
        } else {
            println!("    ❌ {file} missing");
            all_exist = false;
        }
    }

    let xpia_dir = xpia_hooks_dir()?;
    if !xpia_dir.exists() {
        println!("  ℹ️  XPIA security hooks not installed (optional feature)");
    } else {
        for file in XPIA_HOOK_FILES {
            let hook_path = xpia_dir.join(file);
            if hook_path.exists() {
                println!("    ✅ {file} found");
            } else {
                println!("    ❌ {file} missing");
            }
        }
    }

    Ok(all_exist)
}

fn missing_hook_paths(system: &str, files: &[&str]) -> Result<Vec<String>> {
    let base = home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join(system)
        .join("hooks");
    let mut missing = Vec::new();
    for file in files {
        let path = base.join(file);
        if !path.exists() {
            missing.push(format!("{system}/{file} (expected at {})", path.display()));
        }
    }
    Ok(missing)
}

fn manifest_path() -> Result<PathBuf> {
    Ok(claude_dir()?
        .join("install")
        .join("amplihack-manifest.json"))
}

fn read_manifest(path: &Path) -> Result<(Vec<String>, Vec<String>)> {
    if !path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let Ok(raw) = fs::read_to_string(path) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let files = value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let dirs = value
        .get("dirs")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((files, dirs))
}

fn write_manifest(path: &Path, files: &[String], dirs: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let manifest = InstallManifest {
        files: files.to_vec(),
        dirs: dirs.to_vec(),
    };
    fs::write(path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn all_rel_dirs(claude_dir: &Path) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    if !claude_dir.exists() {
        return Ok(result);
    }
    for path in walk_dirs(claude_dir)? {
        let rel = path
            .strip_prefix(claude_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        result.insert(if rel.is_empty() { ".".to_string() } else { rel });
    }
    Ok(result)
}

fn get_all_files_and_dirs(
    claude_dir: &Path,
    root_dirs: &[PathBuf],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut files = Vec::new();
    let mut dirs = BTreeSet::new();
    for root in root_dirs {
        if !root.exists() {
            continue;
        }
        for entry in walk_all(root)? {
            let rel = entry
                .strip_prefix(claude_dir)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.is_dir() {
                dirs.insert(rel);
            } else if entry.is_file() {
                files.push(rel);
            }
        }
    }
    files.sort();
    Ok((files, dirs.into_iter().collect()))
}

fn walk_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = vec![root.to_path_buf()];
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            dirs.extend(walk_dirs(&path)?);
        }
    }
    Ok(dirs)
}

fn walk_all(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![root.to_path_buf()];
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        paths.push(path.clone());
        if path.is_dir() {
            paths.extend(walk_all(&path)?);
        }
    }
    Ok(paths)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

fn set_hook_permissions(target_dir: &Path) -> Result<usize> {
    let mut updated = 0usize;
    for path in walk_all(target_dir)? {
        if path.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("py")
            && path
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                == Some("hooks")
        {
            set_script_permissions(&path)?;
            updated += 1;
        }
    }
    Ok(updated)
}

fn set_script_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o110);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

fn claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude"))
}

fn amplihack_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("amplihack")
        .join("hooks"))
}

fn xpia_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("xpia")
        .join("hooks"))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_source_repo(root: &Path) {
        for dir in ESSENTIAL_DIRS {
            fs::create_dir_all(root.join(".claude").join(dir)).unwrap();
        }
        fs::create_dir_all(root.join(".claude/tools/amplihack/hooks")).unwrap();
        for hook in AMPLIHACK_HOOK_FILES {
            fs::write(
                root.join(".claude/tools/amplihack/hooks").join(hook),
                "print(1)\n",
            )
            .unwrap();
        }
        fs::write(root.join(".claude/settings.json"), "{}\n").unwrap();
        fs::write(root.join(".claude/tools/statusline.sh"), "echo hi\n").unwrap();
        fs::write(root.join(".claude/AMPLIHACK.md"), "framework\n").unwrap();
        fs::write(root.join("CLAUDE.md"), "root\n").unwrap();
    }

    #[test]
    fn local_install_writes_manifest() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        create_source_repo(temp.path());
        local_install(temp.path()).unwrap();
        assert!(
            temp.path()
                .join(".claude/install/amplihack-manifest.json")
                .exists()
        );
        assert!(
            temp.path()
                .join(".claude/tools/amplihack/hooks/pre_tool_use.py")
                .exists()
        );
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn uninstall_removes_manifest_tracked_files() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        fs::create_dir_all(temp.path().join(".claude/install")).unwrap();
        fs::create_dir_all(temp.path().join(".claude/agents/amplihack")).unwrap();
        fs::write(temp.path().join(".claude/agents/amplihack/demo.txt"), "x").unwrap();
        write_manifest(
            &temp.path().join(".claude/install/amplihack-manifest.json"),
            &[String::from("agents/amplihack/demo.txt")],
            &[String::from("agents/amplihack")],
        )
        .unwrap();
        run_uninstall().unwrap();
        assert!(!temp.path().join(".claude/agents/amplihack").exists());
        crate::test_support::restore_home(previous);
    }

    #[test]
    fn read_manifest_treats_invalid_json_as_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("amplihack-manifest.json");
        fs::write(&path, "{invalid json\n").unwrap();

        let (files, dirs) = read_manifest(&path).unwrap();

        assert!(files.is_empty());
        assert!(dirs.is_empty());
    }
}
