use thiserror::Error;

/// Errors that can occur during hive operations.
#[derive(Debug, Error)]
pub enum HiveError {
    /// A requested fact was not found.
    #[error("fact not found: {0}")]
    FactNotFound(String),

    /// A confidence value outside the valid range was provided.
    #[error("invalid confidence: {0} (must be 0.0..=1.0)")]
    InvalidConfidence(f64),

    /// An error originating from the knowledge graph.
    #[error("graph error: {0}")]
    Graph(String),

    /// An error originating from the event bus.
    #[error("event bus error: {0}")]
    EventBus(String),

    /// An error originating from the gossip protocol.
    #[error("gossip error: {0}")]
    Gossip(String),

    /// An error originating from the hive controller.
    #[error("controller error: {0}")]
    Controller(String),

    /// An error originating from the orchestrator.
    #[error("orchestrator error: {0}")]
    Orchestrator(String),

    /// An error originating from workload management.
    #[error("workload error: {0}")]
    Workload(String),

    /// A serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// An I/O error propagated from the OS.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A JSON serialization/deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, HiveError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_not_found_display() {
        let err = HiveError::FactNotFound("abc-123".into());
        assert_eq!(err.to_string(), "fact not found: abc-123");
    }

    #[test]
    fn invalid_confidence_display() {
        let err = HiveError::InvalidConfidence(1.5);
        assert_eq!(
            err.to_string(),
            "invalid confidence: 1.5 (must be 0.0..=1.0)"
        );
    }

    #[test]
    fn graph_error_display() {
        let err = HiveError::Graph("cycle detected".into());
        assert_eq!(err.to_string(), "graph error: cycle detected");
    }

    #[test]
    fn event_bus_error_display() {
        let err = HiveError::EventBus("full".into());
        assert_eq!(err.to_string(), "event bus error: full");
    }

    #[test]
    fn gossip_error_display() {
        let err = HiveError::Gossip("no peers".into());
        assert_eq!(err.to_string(), "gossip error: no peers");
    }

    #[test]
    fn controller_error_display() {
        let err = HiveError::Controller("reconcile failed".into());
        assert_eq!(err.to_string(), "controller error: reconcile failed");
    }

    #[test]
    fn orchestrator_error_display() {
        let err = HiveError::Orchestrator("policy rejected".into());
        assert_eq!(err.to_string(), "orchestrator error: policy rejected");
    }

    #[test]
    fn workload_error_display() {
        let err = HiveError::Workload("deploy failed".into());
        assert_eq!(err.to_string(), "workload error: deploy failed");
    }

    #[test]
    fn serialization_error_display() {
        let err = HiveError::Serialization("bad format".into());
        assert_eq!(err.to_string(), "serialization error: bad format");
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: HiveError = io.into();
        assert!(matches!(err, HiveError::Io(_)));
    }

    #[test]
    fn json_error_converts() {
        let raw = serde_json::from_str::<serde_json::Value>("not json");
        let err: HiveError = raw.unwrap_err().into();
        assert!(matches!(err, HiveError::Json(_)));
    }
}
