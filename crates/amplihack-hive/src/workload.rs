use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Configuration for deploying a hive workload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HiveWorkloadConfig {
    pub num_containers: u32,
    pub agents_per_container: u32,
    pub image: String,
    pub resource_group: String,
}

impl HiveWorkloadConfig {
    /// Validate that the configuration is well-formed.
    pub fn validate(&self) -> Result<()> {
        if self.image.is_empty() {
            return Err(crate::error::HiveError::Workload(
                "image must not be empty".into(),
            ));
        }
        if self.resource_group.is_empty() {
            return Err(crate::error::HiveError::Workload(
                "resource_group must not be empty".into(),
            ));
        }
        if self.num_containers == 0 {
            return Err(crate::error::HiveError::Workload(
                "num_containers must be greater than 0".into(),
            ));
        }
        if self.agents_per_container == 0 {
            return Err(crate::error::HiveError::Workload(
                "agents_per_container must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    /// Return the total number of agents across all containers.
    pub fn total_agents(&self) -> u32 {
        self.num_containers * self.agents_per_container
    }
}

/// Domain events emitted during hive operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiveEvent {
    LearnContent {
        content: String,
        source: String,
    },
    FeedComplete {
        feed_id: String,
        items: u32,
    },
    AgentReady {
        agent_id: String,
    },
    Query {
        query_id: String,
        question: String,
    },
    QueryResponse {
        query_id: String,
        answer: String,
        confidence: f64,
    },
}

/// Lifecycle status of a hive workload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadStatus {
    Pending,
    Deploying,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl fmt::Display for WorkloadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Deploying => "deploying",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        };
        f.write_str(s)
    }
}

impl WorkloadStatus {
    /// Whether this status represents a terminal (final) state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }

    /// Whether a transition from this status to `next` is valid.
    pub fn can_transition_to(&self, next: &WorkloadStatus) -> bool {
        matches!(
            (self, next),
            (WorkloadStatus::Pending, WorkloadStatus::Deploying)
                | (WorkloadStatus::Pending, WorkloadStatus::Failed)
                | (WorkloadStatus::Deploying, WorkloadStatus::Running)
                | (WorkloadStatus::Deploying, WorkloadStatus::Failed)
                | (WorkloadStatus::Deploying, WorkloadStatus::Stopping)
                | (WorkloadStatus::Running, WorkloadStatus::Stopping)
                | (WorkloadStatus::Running, WorkloadStatus::Failed)
                | (WorkloadStatus::Stopping, WorkloadStatus::Stopped)
                | (WorkloadStatus::Stopping, WorkloadStatus::Failed)
                | (WorkloadStatus::Failed, WorkloadStatus::Pending)
        )
    }
}
