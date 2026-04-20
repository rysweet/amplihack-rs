//! Lock tool for continuous work mode.
//!
//! Manages `.lock_active` and `.lock_message` files under
//! `$CLAUDE_PROJECT_DIR/.claude/runtime/locks/`.

use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::command_error::exit_error;

fn lock_dir() -> PathBuf {
    let root = std::env::var("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    root.join(".claude/runtime/locks")
}

pub fn run_lock(message: Option<&str>) -> Result<()> {
    let dir = lock_dir();
    let lock_file = dir.join(".lock_active");
    let msg_file = dir.join(".lock_message");

    fs::create_dir_all(&dir)?;

    if lock_file.exists() {
        println!("\u{26a0} WARNING: Lock was already active");
        if let Some(msg) = message {
            fs::write(&msg_file, msg)?;
            println!("\u{2713} Updated lock message: {msg}");
        }
        return Ok(());
    }

    // Atomic create via O_CREAT|O_EXCL
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_file)
    {
        Ok(mut f) => {
            let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f");
            writeln!(f, "locked_at: {ts}")?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            println!("\u{26a0} WARNING: Lock was already active");
            return Ok(());
        }
        Err(e) => {
            println!("\u{2717} ERROR: Failed to create lock: {e}");
            return Err(exit_error(1));
        }
    }

    println!("\u{2713} Lock enabled - Claude will continue working until unlocked");
    println!("  Use /amplihack:unlock to disable continuous work mode");

    if let Some(msg) = message {
        fs::write(&msg_file, msg)?;
        println!("  Custom instruction: {msg}");
    }

    Ok(())
}

pub fn run_unlock() -> Result<()> {
    let dir = lock_dir();
    let lock_file = dir.join(".lock_active");
    let msg_file = dir.join(".lock_message");

    match fs::remove_file(&lock_file) {
        Ok(()) => println!("\u{2713} Lock disabled - Claude will stop normally"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("\u{2139} Lock was not enabled");
        }
        Err(e) => {
            println!("\u{2717} ERROR: Failed to remove lock: {e}");
            return Err(exit_error(1));
        }
    }

    // Clean up message file
    let _ = fs::remove_file(&msg_file);

    Ok(())
}

pub fn run_check() -> Result<()> {
    let dir = lock_dir();
    let lock_file = dir.join(".lock_active");
    let msg_file = dir.join(".lock_message");

    if lock_file.exists() {
        let info = fs::read_to_string(&lock_file).unwrap_or_default();
        println!("\u{2713} Lock is ACTIVE");
        println!("  {}", info.trim());

        if msg_file.exists()
            && let Ok(msg) = fs::read_to_string(&msg_file)
        {
            println!("  Custom instruction: {}", msg.trim());
        }
    } else {
        println!("\u{2139} Lock is NOT active");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize all lock tests that mutate CLAUDE_PROJECT_DIR.
    static LOCK_ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Set CLAUDE_PROJECT_DIR to a temp directory for the duration of the closure.
    fn with_project_dir<F: FnOnce(&std::path::Path)>(f: F) {
        let _guard = LOCK_ENV_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let old = std::env::var("CLAUDE_PROJECT_DIR").ok();
        unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", tmp.path()) };
        f(tmp.path());
        match old {
            Some(v) => unsafe { std::env::set_var("CLAUDE_PROJECT_DIR", v) },
            None => unsafe { std::env::remove_var("CLAUDE_PROJECT_DIR") },
        }
    }

    #[test]
    fn lock_creates_lock_file() {
        with_project_dir(|root| {
            run_lock(None).unwrap();
            assert!(root.join(".claude/runtime/locks/.lock_active").exists());
        });
    }

    #[test]
    fn lock_creates_directory_structure() {
        with_project_dir(|root| {
            assert!(!root.join(".claude").exists());
            run_lock(None).unwrap();
            assert!(root.join(".claude/runtime/locks").is_dir());
        });
    }

    #[test]
    fn lock_file_contains_timestamp() {
        with_project_dir(|root| {
            run_lock(None).unwrap();
            let content =
                fs::read_to_string(root.join(".claude/runtime/locks/.lock_active")).unwrap();
            assert!(
                content.starts_with("locked_at: "),
                "Expected timestamp, got: {content}"
            );
            assert!(
                content.contains("T"),
                "Timestamp should contain 'T' separator"
            );
        });
    }

    #[test]
    fn lock_with_message_creates_message_file() {
        with_project_dir(|root| {
            run_lock(Some("finish all tests")).unwrap();
            let msg = fs::read_to_string(root.join(".claude/runtime/locks/.lock_message")).unwrap();
            assert_eq!(msg, "finish all tests");
        });
    }

    #[test]
    fn lock_when_already_locked_does_not_error() {
        with_project_dir(|_root| {
            run_lock(None).unwrap();
            run_lock(None).unwrap();
        });
    }

    #[test]
    fn lock_when_already_locked_updates_message() {
        with_project_dir(|root| {
            run_lock(Some("old message")).unwrap();
            run_lock(Some("new message")).unwrap();
            let msg = fs::read_to_string(root.join(".claude/runtime/locks/.lock_message")).unwrap();
            assert_eq!(msg, "new message");
        });
    }

    #[test]
    fn unlock_removes_lock_file() {
        with_project_dir(|root| {
            run_lock(None).unwrap();
            assert!(root.join(".claude/runtime/locks/.lock_active").exists());
            run_unlock().unwrap();
            assert!(!root.join(".claude/runtime/locks/.lock_active").exists());
        });
    }

    #[test]
    fn unlock_removes_message_file() {
        with_project_dir(|root| {
            run_lock(Some("test message")).unwrap();
            assert!(root.join(".claude/runtime/locks/.lock_message").exists());
            run_unlock().unwrap();
            assert!(!root.join(".claude/runtime/locks/.lock_message").exists());
        });
    }

    #[test]
    fn unlock_when_not_locked_succeeds() {
        with_project_dir(|_root| {
            run_unlock().unwrap();
        });
    }

    #[test]
    fn full_lifecycle_lock_check_unlock_check() {
        with_project_dir(|root| {
            assert!(!root.join(".claude/runtime/locks/.lock_active").exists());
            run_lock(Some("doing work")).unwrap();
            assert!(root.join(".claude/runtime/locks/.lock_active").exists());
            assert!(root.join(".claude/runtime/locks/.lock_message").exists());
            run_check().unwrap();
            run_unlock().unwrap();
            assert!(!root.join(".claude/runtime/locks/.lock_active").exists());
            assert!(!root.join(".claude/runtime/locks/.lock_message").exists());
            run_check().unwrap();
        });
    }
}
