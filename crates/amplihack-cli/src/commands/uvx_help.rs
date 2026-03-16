//! Native `uvx-help` command.

use anyhow::{Result, bail};
use std::env;
use std::path::{Path, PathBuf};

pub fn run_uvx_help(find_path: bool, info: bool) -> Result<()> {
    if find_path {
        if let Some(path) = find_uvx_installation_path() {
            println!("{}", path.display());
            return Ok(());
        }
        bail!("UVX installation path not found");
    }

    if info {
        println!("\nUVX Information:");
        println!("  Is UVX: {}", is_uvx_deployment());
        println!("\nEnvironment Variables:");
        println!(
            "  AMPLIHACK_ROOT={}",
            env::var("AMPLIHACK_ROOT").unwrap_or_else(|_| "(not set)".to_string())
        );
        return Ok(());
    }

    print_uvx_usage_instructions();
    Ok(())
}

fn find_uvx_installation_path() -> Option<PathBuf> {
    if let Some(root) = env::var_os("AMPLIHACK_ROOT").map(PathBuf::from)
        && is_framework_root(&root)
    {
        return Some(root);
    }

    let cwd = env::current_dir().ok()?;
    if is_framework_root(&cwd) {
        return Some(cwd);
    }

    let home = env::var_os("HOME").map(PathBuf::from)?;
    let staged = home.join(".amplihack");
    if is_framework_root(&staged) {
        return Some(staged);
    }

    None
}

fn is_uvx_deployment() -> bool {
    if env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".claude").exists())
        .unwrap_or(false)
    {
        return false;
    }

    env::var_os("UV_PYTHON").is_some() || env::var_os("AMPLIHACK_ROOT").is_some()
}

fn is_framework_root(path: &Path) -> bool {
    path.join(".claude").is_dir()
}

fn print_uvx_usage_instructions() {
    println!("UVX deployment helper");
    println!();
    println!("Commands:");
    println!("  amplihack uvx-help --find-path   Print the detected UVX/framework root");
    println!("  amplihack uvx-help --info        Show UVX detection details");
    println!();
    println!("Detection order:");
    println!("  1. AMPLIHACK_ROOT");
    println!("  2. Current working directory (if it contains .claude/)");
    println!("  3. ~/.amplihack");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn prefers_amplihack_root_environment_variable() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let previous = env::var_os("AMPLIHACK_ROOT");
        unsafe { env::set_var("AMPLIHACK_ROOT", dir.path()) };

        let found = find_uvx_installation_path();

        match previous {
            Some(value) => unsafe { env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { env::remove_var("AMPLIHACK_ROOT") },
        }

        assert_eq!(found.as_deref(), Some(dir.path()));
    }

    #[test]
    fn staged_home_is_used_as_fallback() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".amplihack/.claude")).unwrap();
        let previous_home = env::var_os("HOME");
        let previous_root = env::var_os("AMPLIHACK_ROOT");
        unsafe {
            env::set_var("HOME", dir.path());
            env::remove_var("AMPLIHACK_ROOT");
        }

        let found = find_uvx_installation_path();

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }
        match previous_root {
            Some(value) => unsafe { env::set_var("AMPLIHACK_ROOT", value) },
            None => unsafe { env::remove_var("AMPLIHACK_ROOT") },
        }

        assert_eq!(
            found.as_deref(),
            Some(dir.path().join(".amplihack").as_path())
        );
    }
}
