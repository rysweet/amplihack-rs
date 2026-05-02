//! File-based loop-prevention semaphore for the reflection system.
//!
//! Port of `amplifier-bundle/tools/amplihack/reflection/semaphore.py`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const DEFAULT_STALE_TIMEOUT: Duration = Duration::from_secs(60);
const LOCK_FILE_NAME: &str = "reflection.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockData {
    pub pid: u32,
    pub timestamp: f64,
    pub session_id: String,
    pub purpose: String,
}

pub struct ReflectionLock {
    lock_file: PathBuf,
    stale_timeout: Duration,
}

impl ReflectionLock {
    pub fn new(runtime_dir: &Path) -> anyhow::Result<Self> {
        Self::with_stale_timeout(runtime_dir, DEFAULT_STALE_TIMEOUT)
    }

    pub fn with_stale_timeout(runtime_dir: &Path, stale_timeout: Duration) -> anyhow::Result<Self> {
        std::fs::create_dir_all(runtime_dir)?;
        Ok(Self {
            lock_file: runtime_dir.join(LOCK_FILE_NAME),
            stale_timeout,
        })
    }

    pub fn lock_file_path(&self) -> &Path {
        &self.lock_file
    }

    /// Acquire the lock if it is free or stale; returns false if held.
    pub fn acquire(&self, session_id: &str, purpose: &str) -> anyhow::Result<bool> {
        if self.is_locked() && !self.is_stale() {
            return Ok(false);
        }
        if self.is_stale() {
            self.release()?;
        }
        let data = LockData {
            pid: std::process::id(),
            timestamp: now_ts(),
            session_id: session_id.to_string(),
            purpose: purpose.to_string(),
        };
        let bytes = serde_json::to_vec_pretty(&data)?;
        std::fs::write(&self.lock_file, bytes)?;
        Ok(true)
    }

    pub fn release(&self) -> anyhow::Result<()> {
        if self.lock_file.exists() {
            let _ = std::fs::remove_file(&self.lock_file);
        }
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        self.lock_file.exists()
    }

    pub fn is_stale(&self) -> bool {
        if !self.is_locked() {
            return false;
        }
        let Some(data) = self.read_lock() else {
            return true;
        };
        let age = now_ts() - data.timestamp;
        age > self.stale_timeout.as_secs_f64()
    }

    pub fn read_lock(&self) -> Option<LockData> {
        let bytes = std::fs::read(&self.lock_file).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
