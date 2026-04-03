//! Memory maintenance — cleanup, compaction, and analysis.
//!
//! Port of Python `amplihack/memory/maintenance.py`.
//! Provides scheduled cleanup of expired memories, old session purging,
//! database vacuum, index optimization, and usage analysis.

#[cfg(feature = "sqlite")]
use crate::database::MemoryDatabase;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tracing::warn;

/// Report returned by each maintenance operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaintenanceReport {
    pub entries: HashMap<String, serde_json::Value>,
}

impl MaintenanceReport {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn set(&mut self, key: &str, value: impl Into<serde_json::Value>) {
        self.entries.insert(key.into(), value.into());
    }
}

/// Options for `run_full_maintenance`.
#[derive(Debug, Clone)]
pub struct MaintenanceOptions {
    pub cleanup_expired: bool,
    pub cleanup_old_sessions: bool,
    pub old_session_days: u64,
    pub vacuum: bool,
    pub optimize: bool,
}

impl Default for MaintenanceOptions {
    fn default() -> Self {
        Self {
            cleanup_expired: true,
            cleanup_old_sessions: false,
            old_session_days: 30,
            vacuum: false,
            optimize: true,
        }
    }
}

/// Memory system maintenance manager.
///
/// Wraps a `MemoryDatabase` and provides cleanup, vacuum, optimization,
/// and usage-analysis operations.
#[cfg(feature = "sqlite")]
pub struct MemoryMaintenance {
    db: MemoryDatabase,
}

#[cfg(feature = "sqlite")]
impl MemoryMaintenance {
    /// Create a new maintenance manager.
    pub fn new(db: MemoryDatabase) -> Self {
        Self { db }
    }

    /// Remove expired memories.
    pub fn cleanup_expired(&self) -> anyhow::Result<MaintenanceReport> {
        let start = Instant::now();
        let count = self.db.cleanup_expired()?;
        let mut report = MaintenanceReport::new();
        report.set("expired_memories_removed", count as u64);
        report.set("cleanup_duration_ms", start.elapsed().as_millis() as u64);
        Ok(report)
    }

    /// Remove sessions older than `older_than_days`.
    pub fn cleanup_old_sessions(
        &self,
        older_than_days: u64,
    ) -> anyhow::Result<MaintenanceReport> {
        let start = Instant::now();
        let sessions = self.db.list_sessions(None)?;
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let cutoff = now_epoch - (older_than_days as f64 * 86400.0);

        let mut removed_sessions: u64 = 0;
        for session in &sessions {
            if session.last_accessed < cutoff {
                match self.db.delete_session(&session.session_id) {
                    Ok(true) => removed_sessions += 1,
                    Ok(false) => {}
                    Err(e) => warn!("failed to delete session {}: {e}", session.session_id),
                }
            }
        }

        let mut report = MaintenanceReport::new();
        report.set("removed_sessions", removed_sessions);
        report.set("cleanup_duration_ms", start.elapsed().as_millis() as u64);
        Ok(report)
    }

    /// Vacuum the database to reclaim space.
    pub fn vacuum_database(&self) -> anyhow::Result<MaintenanceReport> {
        let start = Instant::now();
        let size_before = std::fs::metadata(self.db.db_path())
            .map(|m| m.len())
            .unwrap_or(0);
        self.db.vacuum()?;
        let size_after = std::fs::metadata(self.db.db_path())
            .map(|m| m.len())
            .unwrap_or(0);

        let mut report = MaintenanceReport::new();
        report.set("success", true);
        report.set("size_before_bytes", size_before);
        report.set("size_after_bytes", size_after);
        report.set(
            "space_reclaimed_bytes",
            size_before.saturating_sub(size_after),
        );
        report.set("vacuum_duration_ms", start.elapsed().as_millis() as u64);
        Ok(report)
    }

    /// Optimize database indexes.
    pub fn optimize_indexes(&self) -> anyhow::Result<MaintenanceReport> {
        let start = Instant::now();
        self.db.optimize()?;
        let mut report = MaintenanceReport::new();
        report.set("success", true);
        report.set(
            "optimization_duration_ms",
            start.elapsed().as_millis() as u64,
        );
        Ok(report)
    }

    /// Analyze memory usage and produce recommendations.
    pub fn analyze_usage(&self) -> anyhow::Result<MaintenanceReport> {
        let stats = self.db.get_stats()?;
        let sessions = self.db.list_sessions(Some(100))?;

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let active_7d = sessions
            .iter()
            .filter(|s| (now_epoch - s.last_accessed) < 7.0 * 86400.0)
            .count();
        let inactive_30d = sessions
            .iter()
            .filter(|s| (now_epoch - s.last_accessed) >= 30.0 * 86400.0)
            .count();

        let avg_per_session = if stats.total_sessions > 0 {
            stats.total_memories as f64 / stats.total_sessions as f64
        } else {
            0.0
        };

        let mut recommendations: Vec<String> = Vec::new();
        if inactive_30d > 10 {
            recommendations.push(format!(
                "Consider cleaning up {inactive_30d} inactive sessions (>30 days old)"
            ));
        }
        if stats.db_size_bytes > 100 * 1024 * 1024 {
            recommendations.push("Database is large (>100MB), consider running vacuum".into());
        }
        if avg_per_session > 1000.0 {
            recommendations.push(
                "High memory count per session, consider memory lifecycle policies".into(),
            );
        }

        let mut report = MaintenanceReport::new();
        report.set("total_memories", stats.total_memories as u64);
        report.set("total_sessions", stats.total_sessions as u64);
        report.set("active_sessions_7d", active_7d as u64);
        report.set("inactive_sessions_30d", inactive_30d as u64);
        report.set(
            "avg_memories_per_session",
            serde_json::json!((avg_per_session * 10.0).round() / 10.0),
        );
        report.set(
            "db_size_mb",
            serde_json::json!(
                (stats.db_size_bytes as f64 / (1024.0 * 1024.0) * 100.0).round() / 100.0
            ),
        );
        report.set("recommendations", serde_json::json!(recommendations));
        Ok(report)
    }

    /// Run full maintenance with given options.
    pub fn run_full(
        &self,
        options: &MaintenanceOptions,
    ) -> anyhow::Result<MaintenanceReport> {
        let start = Instant::now();
        let mut report = MaintenanceReport::new();

        if options.cleanup_expired {
            report.set(
                "expired_cleanup",
                serde_json::to_value(self.cleanup_expired()?)?,
            );
        }
        if options.cleanup_old_sessions {
            report.set(
                "session_cleanup",
                serde_json::to_value(self.cleanup_old_sessions(options.old_session_days)?)?,
            );
        }
        if options.vacuum {
            report.set(
                "vacuum",
                serde_json::to_value(self.vacuum_database()?)?,
            );
        }
        if options.optimize {
            report.set(
                "optimization",
                serde_json::to_value(self.optimize_indexes()?)?,
            );
        }
        report.set(
            "final_analysis",
            serde_json::to_value(self.analyze_usage()?)?,
        );
        report.set("total_duration_ms", start.elapsed().as_millis() as u64);
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_report_set_get() {
        let mut r = MaintenanceReport::new();
        r.set("key", 42_u64);
        assert_eq!(r.entries["key"], serde_json::json!(42));
    }

    #[test]
    fn default_options() {
        let opts = MaintenanceOptions::default();
        assert!(opts.cleanup_expired);
        assert!(!opts.cleanup_old_sessions);
        assert_eq!(opts.old_session_days, 30);
        assert!(!opts.vacuum);
        assert!(opts.optimize);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn cleanup_expired_empty_db() {
        let db = MemoryDatabase::open_in_memory().unwrap();
        let maint = MemoryMaintenance::new(db);
        let report = maint.cleanup_expired().unwrap();
        assert_eq!(report.entries["expired_memories_removed"], 0);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn analyze_empty_db() {
        let db = MemoryDatabase::open_in_memory().unwrap();
        let maint = MemoryMaintenance::new(db);
        let report = maint.analyze_usage().unwrap();
        assert_eq!(report.entries["total_memories"], 0);
        assert_eq!(report.entries["total_sessions"], 0);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn optimize_indexes_succeeds() {
        let db = MemoryDatabase::open_in_memory().unwrap();
        let maint = MemoryMaintenance::new(db);
        let report = maint.optimize_indexes().unwrap();
        assert_eq!(report.entries["success"], true);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn run_full_default_options() {
        let db = MemoryDatabase::open_in_memory().unwrap();
        let maint = MemoryMaintenance::new(db);
        let opts = MaintenanceOptions::default();
        let report = maint.run_full(&opts).unwrap();
        assert!(report.entries.contains_key("expired_cleanup"));
        assert!(report.entries.contains_key("optimization"));
        assert!(report.entries.contains_key("final_analysis"));
        assert!(report.entries.contains_key("total_duration_ms"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn cleanup_old_sessions_empty() {
        let db = MemoryDatabase::open_in_memory().unwrap();
        let maint = MemoryMaintenance::new(db);
        let report = maint.cleanup_old_sessions(30).unwrap();
        assert_eq!(report.entries["removed_sessions"], 0);
    }
}
