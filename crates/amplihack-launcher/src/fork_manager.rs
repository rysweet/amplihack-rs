//! Session fork manager for long-running sessions.
//!
//! Matches Python `amplihack/launcher/fork_manager.py`:
//! - Time-based fork detection (default 60 min threshold)
//! - Pre-fork notification before 69-min hard limit
//! - Fork counting and state tracking
//! - Thread-safe via atomic operations

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::info;

/// Default fork threshold in minutes (before 69-min hard limit).
const DEFAULT_FORK_THRESHOLD_MINS: u64 = 60;

/// Hard session limit in minutes.
const HARD_LIMIT_MINS: u64 = 69;

/// Fork manager configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkConfig {
    /// Minutes before triggering a fork.
    pub threshold_mins: u64,
    /// Whether forking is enabled.
    pub enabled: bool,
    /// Maximum number of forks per original session.
    pub max_forks: u32,
}

impl Default for ForkConfig {
    fn default() -> Self {
        Self {
            threshold_mins: DEFAULT_FORK_THRESHOLD_MINS,
            enabled: true,
            max_forks: 10,
        }
    }
}

/// Fork decision result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkDecision {
    /// No fork needed yet.
    NotYet {
        elapsed_mins: u64,
        remaining_mins: u64,
    },
    /// Fork should be triggered now.
    ShouldFork {
        elapsed_mins: u64,
        fork_number: u32,
    },
    /// Forking is disabled.
    Disabled,
    /// Max forks reached, cannot fork again.
    MaxReached { fork_count: u32 },
}

/// Manages session forking to avoid hitting hard time limits.
pub struct ForkManager {
    config: ForkConfig,
    session_start: Instant,
    fork_count: AtomicU32,
}

impl ForkManager {
    pub fn new(config: ForkConfig) -> Self {
        Self {
            config,
            session_start: Instant::now(),
            fork_count: AtomicU32::new(0),
        }
    }

    /// Check if a fork should be triggered.
    pub fn should_fork(&self) -> ForkDecision {
        if !self.config.enabled {
            return ForkDecision::Disabled;
        }

        let count = self.fork_count.load(Ordering::Relaxed);
        if count >= self.config.max_forks {
            return ForkDecision::MaxReached { fork_count: count };
        }

        let elapsed = self.session_start.elapsed();
        let elapsed_mins = elapsed.as_secs() / 60;
        let threshold = Duration::from_secs(self.config.threshold_mins * 60);

        if elapsed >= threshold {
            ForkDecision::ShouldFork {
                elapsed_mins,
                fork_number: count + 1,
            }
        } else {
            let remaining = threshold
                .checked_sub(elapsed)
                .unwrap_or_default()
                .as_secs()
                / 60;
            ForkDecision::NotYet {
                elapsed_mins,
                remaining_mins: remaining,
            }
        }
    }

    /// Record that a fork was triggered.
    pub fn record_fork(&self) -> u32 {
        let count = self.fork_count.fetch_add(1, Ordering::Relaxed) + 1;
        info!(fork_number = count, "Session forked");
        count
    }

    /// Get the current fork count.
    pub fn fork_count(&self) -> u32 {
        self.fork_count.load(Ordering::Relaxed)
    }

    /// Minutes elapsed since session start.
    pub fn elapsed_mins(&self) -> u64 {
        self.session_start.elapsed().as_secs() / 60
    }

    /// Minutes remaining before fork threshold.
    pub fn remaining_mins(&self) -> u64 {
        let elapsed = self.session_start.elapsed();
        let threshold = Duration::from_secs(self.config.threshold_mins * 60);
        threshold
            .checked_sub(elapsed)
            .unwrap_or_default()
            .as_secs()
            / 60
    }

    /// Minutes remaining before hard session limit.
    pub fn hard_limit_remaining_mins(&self) -> u64 {
        let elapsed = self.session_start.elapsed();
        let limit = Duration::from_secs(HARD_LIMIT_MINS * 60);
        limit.checked_sub(elapsed).unwrap_or_default().as_secs() / 60
    }

    /// Generate a fork notification message.
    pub fn fork_message(&self) -> Option<String> {
        match self.should_fork() {
            ForkDecision::ShouldFork {
                elapsed_mins,
                fork_number,
            } => Some(format!(
                "⚠️ Session has been running for {elapsed_mins} minutes. \
                 Forking session (fork #{fork_number}) to avoid {HARD_LIMIT_MINS}-minute limit."
            )),
            ForkDecision::NotYet {
                remaining_mins, ..
            } if remaining_mins <= 5 => Some(format!(
                "⏰ {remaining_mins} minutes until session fork threshold."
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_not_ready_to_fork() {
        let fm = ForkManager::new(ForkConfig::default());
        match fm.should_fork() {
            ForkDecision::NotYet { elapsed_mins, .. } => {
                assert_eq!(elapsed_mins, 0);
            }
            other => panic!("Expected NotYet, got {other:?}"),
        }
    }

    #[test]
    fn disabled_fork_returns_disabled() {
        let fm = ForkManager::new(ForkConfig {
            enabled: false,
            ..Default::default()
        });
        assert_eq!(fm.should_fork(), ForkDecision::Disabled);
    }

    #[test]
    fn max_forks_enforced() {
        let fm = ForkManager::new(ForkConfig {
            max_forks: 2,
            threshold_mins: 0, // immediate
            ..Default::default()
        });
        fm.record_fork();
        fm.record_fork();
        match fm.should_fork() {
            ForkDecision::MaxReached { fork_count } => assert_eq!(fork_count, 2),
            other => panic!("Expected MaxReached, got {other:?}"),
        }
    }

    #[test]
    fn record_fork_increments() {
        let fm = ForkManager::new(ForkConfig::default());
        assert_eq!(fm.fork_count(), 0);
        fm.record_fork();
        assert_eq!(fm.fork_count(), 1);
        fm.record_fork();
        assert_eq!(fm.fork_count(), 2);
    }

    #[test]
    fn fork_message_none_when_not_needed() {
        let fm = ForkManager::new(ForkConfig::default());
        assert!(fm.fork_message().is_none());
    }

    #[test]
    fn zero_threshold_triggers_immediately() {
        let fm = ForkManager::new(ForkConfig {
            threshold_mins: 0,
            ..Default::default()
        });
        match fm.should_fork() {
            ForkDecision::ShouldFork { fork_number, .. } => {
                assert_eq!(fork_number, 1);
            }
            other => panic!("Expected ShouldFork, got {other:?}"),
        }
    }

    #[test]
    fn fork_message_generated_when_threshold_reached() {
        let fm = ForkManager::new(ForkConfig {
            threshold_mins: 0,
            ..Default::default()
        });
        let msg = fm.fork_message().unwrap();
        assert!(msg.contains("Forking session"));
        assert!(msg.contains("fork #1"));
    }

    #[test]
    fn config_serializes() {
        let cfg = ForkConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["threshold_mins"], 60);
        assert_eq!(json["enabled"], true);
        assert_eq!(json["max_forks"], 10);
    }
}
