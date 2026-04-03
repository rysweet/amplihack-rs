use amplihack_domain_agents::{
    EvaluationResult, QuizQuestion, TeachingAgent, TeachingConfig, TeachingResult,
};

// ── Construction & accessors (PASS) ─────────────────────────────────────────

#[test]
fn new_with_config_stores_config() {
    let cfg = TeachingConfig {
        max_quiz_questions: 5,
        difficulty_level: "hard".to_string(),
        subject_area: "math".to_string(),
    };
    let agent = TeachingAgent::new(cfg.clone());
    let got = agent.config();
    assert_eq!(got.max_quiz_questions, 5);
    assert_eq!(got.difficulty_level, "hard");
    assert_eq!(got.subject_area, "math");
}

#[test]
fn with_defaults_uses_default_config() {
    let agent = TeachingAgent::with_defaults();
    let cfg = agent.config();
    assert_eq!(cfg.max_quiz_questions, 10);
    assert_eq!(cfg.difficulty_level, "medium");
    assert_eq!(cfg.subject_area, "general");
}

#[test]
fn config_accessor_returns_config() {
    let cfg = TeachingConfig {
        max_quiz_questions: 20,
        difficulty_level: "easy".to_string(),
        subject_area: "history".to_string(),
    };
    let agent = TeachingAgent::new(cfg);
    let got = agent.config();
    assert_eq!(got.max_quiz_questions, 20);
    assert_eq!(got.difficulty_level, "easy");
    assert_eq!(got.subject_area, "history");
}

// ── teach (todo → should_panic) ─────────────────────────────────────────────

#[test]
#[should_panic]
fn teach_basic_content() {
    let agent = TeachingAgent::with_defaults();
    let _ = agent.teach("Introduction to Rust ownership");
}

#[test]
#[should_panic]
fn teach_empty_content() {
    let agent = TeachingAgent::with_defaults();
    let _ = agent.teach("");
}

// ── quiz (todo → should_panic) ──────────────────────────────────────────────

#[test]
#[should_panic]
fn quiz_generates_questions() {
    let agent = TeachingAgent::with_defaults();
    let _ = agent.quiz("Rust lifetimes", 5);
}

#[test]
#[should_panic]
fn quiz_respects_num_questions() {
    let agent = TeachingAgent::with_defaults();
    let _ = agent.quiz("Rust traits", 3);
}

// ── evaluate_response (todo → should_panic) ─────────────────────────────────

#[test]
#[should_panic]
fn evaluate_response_correct() {
    let agent = TeachingAgent::with_defaults();
    let q = QuizQuestion {
        question: "What is ownership?".to_string(),
        options: vec![
            "A borrowing rule".to_string(),
            "A memory management model".to_string(),
            "A type system".to_string(),
        ],
        correct_index: 1,
    };
    let _ = agent.evaluate_response(&q, 1);
}

#[test]
#[should_panic]
fn evaluate_response_incorrect() {
    let agent = TeachingAgent::with_defaults();
    let q = QuizQuestion {
        question: "What is ownership?".to_string(),
        options: vec![
            "A borrowing rule".to_string(),
            "A memory management model".to_string(),
            "A type system".to_string(),
        ],
        correct_index: 1,
    };
    let _ = agent.evaluate_response(&q, 0);
}

// ── evaluate_batch (todo → should_panic) ─────────────────────────────────────

#[test]
#[should_panic]
fn evaluate_batch_all_correct() {
    let agent = TeachingAgent::with_defaults();
    let questions = vec![
        QuizQuestion {
            question: "Q1".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            correct_index: 0,
        },
        QuizQuestion {
            question: "Q2".to_string(),
            options: vec!["X".to_string(), "Y".to_string()],
            correct_index: 1,
        },
    ];
    let answers = vec![0, 1];
    let _ = agent.evaluate_batch(&questions, &answers);
}

#[test]
#[should_panic]
fn evaluate_batch_mixed() {
    let agent = TeachingAgent::with_defaults();
    let questions = vec![
        QuizQuestion {
            question: "Q1".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            correct_index: 0,
        },
        QuizQuestion {
            question: "Q2".to_string(),
            options: vec!["X".to_string(), "Y".to_string()],
            correct_index: 1,
        },
    ];
    let answers = vec![0, 0]; // second is wrong
    let _ = agent.evaluate_batch(&questions, &answers);
}

// ── serde roundtrip (PASS) ──────────────────────────────────────────────────

#[test]
fn teaching_config_serde_roundtrip() {
    let cfg = TeachingConfig {
        max_quiz_questions: 7,
        difficulty_level: "hard".to_string(),
        subject_area: "physics".to_string(),
    };
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: TeachingConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cfg, back);
}
