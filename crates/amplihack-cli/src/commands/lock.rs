//! Lock tool for continuous-work mode.
//!
//! Rust port of `amplifier-bundle/tools/amplihack/lock_tool.py` (Issue #546).
//! Manages a per-project lock file under `.claude/runtime/locks/` that signals
//! continuous work mode to the agent runtime.

use anyhow::{Context, Result};
use chrono::Local;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const LOCK_FILE_NAME: &str = ".lock_active";
const MESSAGE_FILE_NAME: &str = ".lock_message";

/// Locate the project root from `CLAUDE_PROJECT_DIR` or fall back to cwd.
fn project_root() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_PROJECT_DIR") {
        return Ok(PathBuf::from(dir));
    }
    std::env::current_dir().context("failed to determine current working directory")
}

fn lock_dir(root: &Path) -> PathBuf {
    root.join(".claude").join("runtime").join("locks")
}

/// Enable continuous work mode by creating the lock file.
///
/// If a lock is already active, this is a no-op except that an
/// optional `message` will overwrite the existing message file.
pub fn run_lock(message: Option<&str>) -> Result<()> {
    let root = project_root()?;
    let dir = lock_dir(&root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create lock directory at {}", dir.display()))?;

    let lock = dir.join(LOCK_FILE_NAME);
    let msg = dir.join(MESSAGE_FILE_NAME);

    match OpenOptions::new().create_new(true).write(true).open(&lock) {
        Ok(mut f) => {
            writeln!(f, "locked_at: {}", Local::now().to_rfc3339())
                .with_context(|| format!("failed to write lock file at {}", lock.display()))?;
            println!("✓ Lock enabled - continuous work mode active");
            println!("  Use `amplihack unlock` to disable");
            if let Some(text) = message {
                fs::write(&msg, text)
                    .with_context(|| format!("failed to write {}", msg.display()))?;
                println!("  Custom instruction: {text}");
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            println!("⚠ Lock was already active");
            if let Some(text) = message {
                fs::write(&msg, text)
                    .with_context(|| format!("failed to write {}", msg.display()))?;
                println!("✓ Updated lock message: {text}");
            }
        }
        Err(e) => {
            return Err(anyhow::Error::from(e)
                .context(format!("failed to create lock file at {}", lock.display())));
        }
    }
    Ok(())
}

/// Disable continuous work mode by removing the lock file (and message, if any).
pub fn run_unlock() -> Result<()> {
    let root = project_root()?;
    let dir = lock_dir(&root);
    let lock = dir.join(LOCK_FILE_NAME);
    let msg = dir.join(MESSAGE_FILE_NAME);

    if lock.exists() {
        fs::remove_file(&lock)
            .with_context(|| format!("failed to remove lock file at {}", lock.display()))?;
        println!("✓ Lock disabled - continuous work mode off");
    } else {
        println!("ℹ Lock was not active");
    }
    if msg.exists() {
        fs::remove_file(&msg)
            .with_context(|| format!("failed to remove message file at {}", msg.display()))?;
    }
    Ok(())
}

/// Print whether the lock is currently active.
pub fn run_check() -> Result<()> {
    let root = project_root()?;
    let dir = lock_dir(&root);
    let lock = dir.join(LOCK_FILE_NAME);
    let msg = dir.join(MESSAGE_FILE_NAME);

    if lock.exists() {
        let info = fs::read_to_string(&lock)
            .with_context(|| format!("failed to read lock file at {}", lock.display()))?;
        println!("✓ Lock is ACTIVE");
        for line in info.lines() {
            println!("  {line}");
        }
        if msg.exists() {
            let m = fs::read_to_string(&msg)
                .with_context(|| format!("failed to read {}", msg.display()))?;
            println!("  Custom instruction: {}", m.trim());
        }
    } else {
        println!("ℹ Lock is NOT active");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(path: &Path) -> Self {
            let prev = std::env::var_os("CLAUDE_PROJECT_DIR");
            unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", path) };
            EnvGuard { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", v) },
                None => unsafe { std::env::remove_var("CLAUDE_PROJECT_DIR") },
            }
        }
    }

    #[test]
    fn lock_creates_lock_file_and_message() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_lock(Some("focus on tests")).unwrap();

        let lock = tmp.path().join(".claude/runtime/locks/.lock_active");
        let msg = tmp.path().join(".claude/runtime/locks/.lock_message");
        assert!(lock.exists(), "lock file must exist");
        assert!(msg.exists(), "message file must exist");
        assert_eq!(fs::read_to_string(&msg).unwrap(), "focus on tests");
        let lock_contents = fs::read_to_string(&lock).unwrap();
        assert!(
            lock_contents.starts_with("locked_at: "),
            "got: {lock_contents}"
        );
    }

    #[test]
    fn lock_when_already_locked_is_idempotent_and_updates_message() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_lock(Some("first")).unwrap();
        run_lock(Some("second")).unwrap();

        let msg = tmp.path().join(".claude/runtime/locks/.lock_message");
        assert_eq!(fs::read_to_string(&msg).unwrap(), "second");
    }

    #[test]
    fn lock_without_message_creates_only_lock_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_lock(None).unwrap();

        let lock = tmp.path().join(".claude/runtime/locks/.lock_active");
        let msg = tmp.path().join(".claude/runtime/locks/.lock_message");
        assert!(lock.exists());
        assert!(!msg.exists(), "no message file should be created");
    }

    #[test]
    fn unlock_removes_lock_and_message_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_lock(Some("hello")).unwrap();
        run_unlock().unwrap();

        let lock = tmp.path().join(".claude/runtime/locks/.lock_active");
        let msg = tmp.path().join(".claude/runtime/locks/.lock_message");
        assert!(!lock.exists());
        assert!(!msg.exists());
    }

    #[test]
    fn unlock_when_not_locked_is_noop() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_unlock().unwrap();
    }

    #[test]
    fn check_reports_active_and_inactive_states() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let _env = EnvGuard::set(tmp.path());

        run_check().unwrap();
        run_lock(Some("hi")).unwrap();
        run_check().unwrap();
        run_unlock().unwrap();
        run_check().unwrap();
    }

    #[test]
    fn project_root_falls_back_to_cwd_without_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("CLAUDE_PROJECT_DIR");
        unsafe { std::env::remove_var("CLAUDE_PROJECT_DIR") };
        let cwd = std::env::current_dir().unwrap();
        let got = project_root().unwrap();
        assert_eq!(got, cwd);
        if let Some(v) = prev {
            unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", v) };
        }
    }
}
