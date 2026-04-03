use amplihack_domain_agents::{Answer, LearnedContent, LearningAgent, LearningConfig};
use chrono::Utc;

// ── Construction & accessors (PASS) ─────────────────────────────────────────

#[test]
fn new_with_config_stores_config() {
    let cfg = LearningConfig {
        retention_strategy: "active_recall".to_string(),
        max_memory_items: 500,
    };
    let agent = LearningAgent::new(cfg.clone());
    let got = agent.config();
    assert_eq!(got.retention_strategy, "active_recall");
    assert_eq!(got.max_memory_items, 500);
}

#[test]
fn with_defaults_uses_default_config() {
    let agent = LearningAgent::with_defaults();
    let cfg = agent.config();
    assert_eq!(cfg.retention_strategy, "spaced_repetition");
    assert_eq!(cfg.max_memory_items, 1000);
}

#[test]
fn config_accessor_returns_config() {
    let cfg = LearningConfig {
        retention_strategy: "cramming".to_string(),
        max_memory_items: 100,
    };
    let agent = LearningAgent::new(cfg);
    let got = agent.config();
    assert_eq!(got.retention_strategy, "cramming");
    assert_eq!(got.max_memory_items, 100);
}

#[test]
fn initial_learned_count_is_zero() {
    let agent = LearningAgent::with_defaults();
    assert_eq!(agent.learned_count(), 0);
}

// ── learn_from_content (todo → should_panic) ────────────────────────────────

#[test]
#[should_panic]
fn learn_from_content_basic() {
    let mut agent = LearningAgent::with_defaults();
    let _ = agent.learn_from_content("Rust ownership ensures memory safety without a GC.");
}

#[test]
#[should_panic]
fn learn_from_content_empty() {
    let mut agent = LearningAgent::with_defaults();
    let _ = agent.learn_from_content("");
}

// ── answer_question (todo → should_panic) ───────────────────────────────────

#[test]
#[should_panic]
fn answer_question_basic() {
    let agent = LearningAgent::with_defaults();
    let _ = agent.answer_question("What is Rust ownership?");
}

#[test]
#[should_panic]
fn answer_question_no_knowledge() {
    let agent = LearningAgent::with_defaults();
    let _ = agent.answer_question("Explain quantum entanglement");
}

// ── recall (todo → should_panic) ────────────────────────────────────────────

#[test]
#[should_panic]
fn recall_concept() {
    let agent = LearningAgent::with_defaults();
    let _ = agent.recall("ownership");
}

#[test]
#[should_panic]
fn recall_unknown_concept() {
    let agent = LearningAgent::with_defaults();
    let _ = agent.recall("nonexistent_concept_xyz");
}

// ── serde roundtrip (PASS) ──────────────────────────────────────────────────

#[test]
fn learning_config_serde_roundtrip() {
    let cfg = LearningConfig {
        retention_strategy: "interleaving".to_string(),
        max_memory_items: 250,
    };
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: LearningConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cfg, back);
}

#[test]
fn learned_content_serde_roundtrip() {
    let lc = LearnedContent {
        content_id: "lc-001".to_string(),
        summary: "Rust ownership model".to_string(),
        key_concepts: vec![
            "ownership".to_string(),
            "borrowing".to_string(),
            "lifetimes".to_string(),
        ],
        learned_at: Utc::now(),
    };
    let json = serde_json::to_string(&lc).expect("serialize");
    let back: LearnedContent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(lc.content_id, back.content_id);
    assert_eq!(lc.summary, back.summary);
    assert_eq!(lc.key_concepts, back.key_concepts);
}
