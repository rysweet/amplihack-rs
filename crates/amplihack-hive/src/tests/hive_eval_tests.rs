use super::*;
use crate::event_bus::LocalEventBus;

#[test]
fn eval_config_defaults() {
    let config = HiveEvalConfig::new(vec!["q1".into()]);
    assert_eq!(config.timeout_seconds, 60);
    assert_eq!(config.min_responses_per_query, 1);
}

#[test]
fn eval_config_builder() {
    let config = HiveEvalConfig::new(vec!["q1".into()])
        .with_timeout(120)
        .with_min_responses(3);
    assert_eq!(config.timeout_seconds, 120);
    assert_eq!(config.min_responses_per_query, 3);
}

#[test]
fn run_eval_publishes_queries() {
    let mut bus = LocalEventBus::new();
    let config = HiveEvalConfig::new(vec!["Question 1?".into(), "Question 2?".into()]);
    let result = run_eval(&mut bus, &config).unwrap();
    assert_eq!(result.total_queries, 2);
}

#[test]
fn run_eval_rejects_empty_questions() {
    let mut bus = LocalEventBus::new();
    let config = HiveEvalConfig::new(vec![]);
    assert!(run_eval(&mut bus, &config).is_err());
}

#[test]
fn query_result_best_answer() {
    let result = QueryResult {
        query_id: "q1".into(),
        question: "test".into(),
        answers: vec![
            AgentAnswer {
                agent_id: "a1".into(),
                answer: "low".into(),
                confidence: 0.3,
            },
            AgentAnswer {
                agent_id: "a2".into(),
                answer: "high".into(),
                confidence: 0.9,
            },
        ],
    };
    let best = result.best_answer().unwrap();
    assert_eq!(best.agent_id, "a2");
    assert!((best.confidence - 0.9).abs() < f64::EPSILON);
}

#[test]
fn query_result_average_confidence() {
    let result = QueryResult {
        query_id: "q1".into(),
        question: "test".into(),
        answers: vec![
            AgentAnswer {
                agent_id: "a1".into(),
                answer: "a".into(),
                confidence: 0.4,
            },
            AgentAnswer {
                agent_id: "a2".into(),
                answer: "b".into(),
                confidence: 0.8,
            },
        ],
    };
    assert!((result.average_confidence() - 0.6).abs() < f64::EPSILON);
}

#[test]
fn query_result_empty() {
    let result = QueryResult {
        query_id: "q1".into(),
        question: "test".into(),
        answers: vec![],
    };
    assert!(result.best_answer().is_none());
    assert_eq!(result.average_confidence(), 0.0);
    assert_eq!(result.response_count(), 0);
}

#[test]
fn hive_eval_result_aggregation() {
    let results = vec![
        QueryResult {
            query_id: "q1".into(),
            question: "a".into(),
            answers: vec![AgentAnswer {
                agent_id: "a1".into(),
                answer: "x".into(),
                confidence: 0.8,
            }],
        },
        QueryResult {
            query_id: "q2".into(),
            question: "b".into(),
            answers: vec![],
        },
    ];
    let eval = HiveEvalResult::from_results(results);
    assert_eq!(eval.total_queries, 2);
    assert_eq!(eval.total_responses, 1);
}

#[test]
fn default_eval_questions_non_empty() {
    let questions = build_default_eval_questions();
    assert!(!questions.is_empty());
    assert!(questions.len() >= 3);
}

#[test]
fn hive_eval_config_serde() {
    let config = HiveEvalConfig::new(vec!["q".into()]);
    let json = serde_json::to_string(&config).unwrap();
    let restored: HiveEvalConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.questions.len(), 1);
}
