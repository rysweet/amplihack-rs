//! Copilot staging — agents, skills, hooks, and instructions.
//!
//! Matches Python `amplihack/launcher/copilot.py` staging logic:
//! - Stage agents, skills, workflows, and context files
//! - Generate hook bash wrappers with Rust binary support
//! - Inject amplihack section into copilot-instructions.md

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Marker comments for the injected section in copilot-instructions.md.
const INSTRUCTIONS_MARKER_START: &str = "<!-- AMPLIHACK_INSTRUCTIONS_START -->";
const INSTRUCTIONS_MARKER_END: &str = "<!-- AMPLIHACK_INSTRUCTIONS_END -->";

/// Stage agent `.md` files from source to `~/.copilot/agents/amplihack/`.
pub fn stage_agents(source_agents: &Path, copilot_home: &Path) -> Result<u32> {
    let dest = copilot_home.join("agents").join("amplihack");
    stage_md_files(source_agents, &dest, true)
}

/// Stage files from a source directory to a destination under copilot home.
pub fn stage_directory(source_dir: &Path, copilot_home: &Path, dest_name: &str) -> Result<u32> {
    let dest = copilot_home.join(dest_name).join("amplihack");
    stage_md_files(source_dir, &dest, false)
}

/// Generate copilot-instructions.md with amplihack section injected.
pub fn generate_copilot_instructions(copilot_home: &Path, agents_context: &str) -> Result<()> {
    let path = copilot_home.join("copilot-instructions.md");
    let existing = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };

    let cleaned = remove_marker_section(&existing);
    let section = format!("{INSTRUCTIONS_MARKER_START}\n{agents_context}\n{INSTRUCTIONS_MARKER_END}");
    let final_content = if cleaned.trim().is_empty() {
        section
    } else {
        format!("{cleaned}\n\n{section}")
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, final_content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    info!("Generated copilot-instructions.md");
    Ok(())
}

/// Stage hook bash wrappers into `.github/hooks/`.
pub fn stage_hooks(package_dir: &Path, user_dir: &Path) -> Result<u32> {
    let hooks_dir = user_dir.join(".github").join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let source_hooks = package_dir.join("hooks");
    if !source_hooks.exists() {
        return Ok(0);
    }

    let rust_binary = which_binary("amplihack-hooks");
    let mut count = 0u32;

    let entries = std::fs::read_dir(&source_hooks)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let dest = hooks_dir.join(&name);

        // Don't overwrite user-managed hooks
        if dest.exists() {
            let content = std::fs::read_to_string(&dest).unwrap_or_default();
            if !content.contains("amplihack") {
                debug!(hook = ?name, "Skipping user-managed hook");
                continue;
            }
        }

        let hook_name = name.to_string_lossy();
        let wrapper = generate_hook_wrapper(&hook_name, &path, rust_binary.as_deref());
        std::fs::write(&dest, wrapper)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dest)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms)?;
        }
        count += 1;
    }

    if count > 0 {
        info!(count, "Staged hook wrappers");
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Stage `.md` files from source to dest. `flatten` puts all files at top level.
pub(crate) fn stage_md_files(source: &Path, dest: &Path, flatten: bool) -> Result<u32> {
    if !source.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dest)?;
    let mut count = 0u32;
    stage_recursive(source, source, dest, flatten, &mut count)?;
    if count > 0 {
        debug!(count, dest = %dest.display(), "Staged markdown files");
    }
    Ok(count)
}

fn stage_recursive(
    root: &Path,
    current: &Path,
    dest: &Path,
    flatten: bool,
    count: &mut u32,
) -> Result<()> {
    let entries = std::fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            stage_recursive(root, &path, dest, flatten, count)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            let target = if flatten {
                dest.join(entry.file_name())
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let target = dest.join(relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                target
            };
            std::fs::copy(&path, &target)
                .with_context(|| format!("failed to copy {}", path.display()))?;
            *count += 1;
        }
    }
    Ok(())
}

fn remove_marker_section(text: &str) -> String {
    if let Some(start_idx) = text.find(INSTRUCTIONS_MARKER_START)
        && let Some(end_idx) = text.find(INSTRUCTIONS_MARKER_END)
    {
        let end = end_idx + INSTRUCTIONS_MARKER_END.len();
        let mut result = text[..start_idx].to_string();
        result.push_str(&text[end..]);
        return result.trim().to_string();
    }
    text.to_string()
}

fn generate_hook_wrapper(hook_name: &str, py_path: &Path, rust_binary: Option<&Path>) -> String {
    let mut script = String::from("#!/usr/bin/env bash\n# Auto-generated by amplihack\n");
    if let Some(binary) = rust_binary {
        script.push_str(&format!("exec {} {} \"$@\"\n", binary.display(), hook_name));
    } else {
        script.push_str(&format!("exec python3 {} \"$@\"\n", py_path.display()));
    }
    script
}

fn which_binary(name: &str) -> Option<PathBuf> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

/// Simple ISO 8601 timestamp from epoch seconds.
pub(crate) fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    epoch_to_iso(secs)
}

pub(crate) fn epoch_to_iso(secs: u64) -> String {
    const SECS_PER_DAY: u64 = 86400;
    let days = secs / SECS_PER_DAY;
    let time_secs = secs % SECS_PER_DAY;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}+00:00")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_agents_from_dir() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("a1.md"), "# A1").unwrap();
        std::fs::write(src.path().join("a2.md"), "# A2").unwrap();
        std::fs::write(src.path().join("readme.txt"), "skip").unwrap();
        assert_eq!(stage_agents(src.path(), dest.path()).unwrap(), 2);
        assert!(dest.path().join("agents/amplihack/a1.md").exists());
        assert!(!dest.path().join("agents/amplihack/readme.txt").exists());
    }
    #[test]
    fn stage_directory_preserves_tree() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub/nested.md"), "c").unwrap();
        std::fs::write(src.path().join("top.md"), "t").unwrap();
        assert_eq!(stage_directory(src.path(), dest.path(), "workflows").unwrap(), 2);
        assert!(dest.path().join("workflows/amplihack/sub/nested.md").exists());
    }
    #[test]
    fn generate_instructions_creates() {
        let h = tempfile::tempdir().unwrap();
        generate_copilot_instructions(h.path(), "# Test").unwrap();
        let c = std::fs::read_to_string(h.path().join("copilot-instructions.md")).unwrap();
        assert!(c.contains("AMPLIHACK_INSTRUCTIONS_START")); assert!(c.contains("# Test"));
    }
    #[test]
    fn generate_instructions_replaces() {
        let h = tempfile::tempdir().unwrap();
        let p = h.path().join("copilot-instructions.md");
        std::fs::write(&p, format!("User\n\n{INSTRUCTIONS_MARKER_START}\nOLD\n{INSTRUCTIONS_MARKER_END}")).unwrap();
        generate_copilot_instructions(h.path(), "NEW").unwrap();
        let c = std::fs::read_to_string(&p).unwrap();
        assert!(c.contains("User")); assert!(c.contains("NEW")); assert!(!c.contains("OLD"));
    }
    #[test]
    fn remove_marker_noop() { assert_eq!(remove_marker_section("plain"), "plain"); }
    #[test]
    fn hook_wrapper_rust() {
        let w = generate_hook_wrapper("pre-tool-use", Path::new("/h.py"), Some(Path::new("/bin/amplihack-hooks")));
        assert!(w.contains("amplihack-hooks")); assert!(w.contains("pre-tool-use"));
    }
    #[test]
    fn hook_wrapper_python() {
        let w = generate_hook_wrapper("post", Path::new("/h.py"), None);
        assert!(w.contains("python3")); assert!(w.contains("/h.py"));
    }
    #[test]
    fn epoch_to_iso_known() { assert_eq!(epoch_to_iso(1704067200), "2024-01-01T00:00:00+00:00"); }
    #[test]
    fn stage_hooks_creates() {
        let pkg = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(pkg.path().join("hooks")).unwrap();
        std::fs::write(pkg.path().join("hooks/pre-tool-use"), "#!/bin/bash\necho hi").unwrap();
        assert_eq!(stage_hooks(pkg.path(), user.path()).unwrap(), 1);
        assert!(std::fs::read_to_string(user.path().join(".github/hooks/pre-tool-use")).unwrap().contains("amplihack"));
    }
}
