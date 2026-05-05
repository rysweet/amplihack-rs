use amplihack_hive::{HiveEvent, HiveWorkloadConfig, WorkloadStatus};

// --- HiveWorkloadConfig tests ---

#[test]
fn total_agents_calculation() {
    let config = HiveWorkloadConfig {
        num_containers: 3,
        agents_per_container: 4,
        image: "hive:latest".to_string(),
        resource_group: "rg-hive".to_string(),
    };
    assert_eq!(config.total_agents().unwrap(), 12);
}

#[test]
fn total_agents_zero_containers() {
    let config = HiveWorkloadConfig {
        num_containers: 0,
        agents_per_container: 10,
        image: "hive:latest".to_string(),
        resource_group: "rg-hive".to_string(),
    };
    assert_eq!(config.total_agents().unwrap(), 0);
}

#[test]
fn total_agents_single_container() {
    let config = HiveWorkloadConfig {
        num_containers: 1,
        agents_per_container: 1,
        image: "hive:latest".to_string(),
        resource_group: "rg-hive".to_string(),
    };
    assert_eq!(config.total_agents().unwrap(), 1);
}

#[test]
fn validate_config() {
    let config = HiveWorkloadConfig {
        num_containers: 3,
        agents_per_container: 4,
        image: "hive:latest".to_string(),
        resource_group: "rg-hive".to_string(),
    };
    config.validate().unwrap();
}

#[test]
fn validate_empty_image() {
    let config = HiveWorkloadConfig {
        num_containers: 1,
        agents_per_container: 1,
        image: "".to_string(),
        resource_group: "rg-hive".to_string(),
    };
    assert!(config.validate().is_err());
}

#[test]
fn validate_empty_resource_group() {
    let config = HiveWorkloadConfig {
        num_containers: 1,
        agents_per_container: 1,
        image: "hive:latest".to_string(),
        resource_group: "".to_string(),
    };
    assert!(config.validate().is_err());
}

// --- HiveEvent serde tests (should pass) ---

#[test]
fn hive_event_serde_learn_content() {
    let event = HiveEvent::LearnContent {
        content: "Rust is fast".to_string(),
        source: "docs".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        HiveEvent::LearnContent { content, source } => {
            assert_eq!(content, "Rust is fast");
            assert_eq!(source, "docs");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn hive_event_serde_query_response() {
    let event = HiveEvent::QueryResponse {
        query_id: "q-1".to_string(),
        answer: "42".to_string(),
        confidence: 0.99,
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        HiveEvent::QueryResponse {
            query_id,
            answer,
            confidence,
        } => {
            assert_eq!(query_id, "q-1");
            assert_eq!(answer, "42");
            assert!((confidence - 0.99).abs() < f64::EPSILON);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn hive_event_serde_feed_complete() {
    let event = HiveEvent::FeedComplete {
        feed_id: "feed-123".to_string(),
        items: 50,
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        HiveEvent::FeedComplete { feed_id, items } => {
            assert_eq!(feed_id, "feed-123");
            assert_eq!(items, 50);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn hive_event_serde_agent_ready() {
    let event = HiveEvent::AgentReady {
        agent_id: "agent-7".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: HiveEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        HiveEvent::AgentReady { agent_id } => assert_eq!(agent_id, "agent-7"),
        _ => panic!("wrong variant"),
    }
}

// --- WorkloadStatus tests ---

#[test]
fn workload_status_terminal_states() {
    assert!(WorkloadStatus::Stopped.is_terminal());
    assert!(WorkloadStatus::Failed.is_terminal());
}

#[test]
fn workload_status_non_terminal_states() {
    assert!(!WorkloadStatus::Pending.is_terminal());
    assert!(!WorkloadStatus::Deploying.is_terminal());
    assert!(!WorkloadStatus::Running.is_terminal());
    assert!(!WorkloadStatus::Stopping.is_terminal());
}

#[test]
fn workload_status_display() {
    let status = WorkloadStatus::Running;
    let debug = format!("{status:?}");
    assert_eq!(debug, "Running");
}

#[test]
fn workload_status_can_transition() {
    assert!(WorkloadStatus::Pending.can_transition_to(&WorkloadStatus::Deploying));
    assert!(WorkloadStatus::Deploying.can_transition_to(&WorkloadStatus::Running));
    assert!(WorkloadStatus::Running.can_transition_to(&WorkloadStatus::Stopping));
    assert!(WorkloadStatus::Stopping.can_transition_to(&WorkloadStatus::Stopped));
    assert!(WorkloadStatus::Failed.can_transition_to(&WorkloadStatus::Pending));
}

#[test]
fn workload_status_cannot_transition_terminal() {
    assert!(!WorkloadStatus::Stopped.can_transition_to(&WorkloadStatus::Running));
    assert!(!WorkloadStatus::Stopped.can_transition_to(&WorkloadStatus::Pending));
    assert!(!WorkloadStatus::Stopped.can_transition_to(&WorkloadStatus::Deploying));
}

#[test]
fn workload_status_invalid_transitions() {
    assert!(!WorkloadStatus::Pending.can_transition_to(&WorkloadStatus::Running));
    assert!(!WorkloadStatus::Pending.can_transition_to(&WorkloadStatus::Stopped));
    assert!(!WorkloadStatus::Running.can_transition_to(&WorkloadStatus::Deploying));
}
