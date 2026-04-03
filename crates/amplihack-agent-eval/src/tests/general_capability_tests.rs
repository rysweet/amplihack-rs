//! Tests for general_capability module.

use super::*;
use std::collections::HashMap;

fn make_trajectory(calls: Vec<&str>) -> ToolTrajectory {
    ToolTrajectory {
        task_description: "test task".into(),
        calls: calls
            .into_iter()
            .map(|name| ToolCall {
                tool_name: name.into(),
                arguments: HashMap::new(),
                result: String::new(),
                timestamp_ms: 0,
            })
            .collect(),
        total_time_ms: 100,
    }
}

#[test]
fn trajectory_call_names() {
    let t = make_trajectory(vec!["search", "calculate", "synthesize"]);
    assert_eq!(t.call_names(), vec!["search", "calculate", "synthesize"]);
}

#[test]
fn trajectory_unique_tools() {
    let t = make_trajectory(vec!["search", "search", "calculate"]);
    let unique = t.unique_tools();
    assert_eq!(unique.len(), 2);
    assert!(unique.contains("search"));
    assert!(unique.contains("calculate"));
}

#[test]
fn trajectory_empty() {
    let t = ToolTrajectory::default();
    assert_eq!(t.call_count(), 0);
    assert!(t.call_names().is_empty());
    assert!(t.unique_tools().is_empty());
}

#[test]
fn eval_type_result_compute_averages() {
    let mut result = EvalTypeResult {
        eval_type: "tool_use".into(),
        scenarios: vec![
            ScenarioResult {
                scenario_id: "S1".into(),
                scenario_name: "test1".into(),
                agent_response: String::new(),
                trajectory: None,
                scores: {
                    let mut s = HashMap::new();
                    s.insert("accuracy".into(), 0.8);
                    s.insert("efficiency".into(), 1.0);
                    s
                },
                reasoning: String::new(),
                metadata: HashMap::new(),
            },
            ScenarioResult {
                scenario_id: "S2".into(),
                scenario_name: "test2".into(),
                agent_response: String::new(),
                trajectory: None,
                scores: {
                    let mut s = HashMap::new();
                    s.insert("accuracy".into(), 0.6);
                    s.insert("efficiency".into(), 0.8);
                    s
                },
                reasoning: String::new(),
                metadata: HashMap::new(),
            },
        ],
        metric_averages: HashMap::new(),
        overall_score: 0.0,
        duration_s: 0.0,
    };
    result.compute_averages();
    assert!((result.metric_averages["accuracy"] - 0.7).abs() < 0.001);
    assert!((result.metric_averages["efficiency"] - 0.9).abs() < 0.001);
    assert!(result.overall_score > 0.0);
}

#[test]
fn eval_type_result_empty() {
    let mut result = EvalTypeResult {
        eval_type: "test".into(),
        scenarios: vec![],
        metric_averages: HashMap::new(),
        overall_score: 0.0,
        duration_s: 0.0,
    };
    result.compute_averages();
    assert!((result.overall_score).abs() < f64::EPSILON);
}

#[test]
fn capability_report_overall() {
    let report = CapabilityReport {
        eval_results: vec![
            EvalTypeResult {
                eval_type: "a".into(),
                overall_score: 0.8,
                ..Default::default()
            },
            EvalTypeResult {
                eval_type: "b".into(),
                overall_score: 0.6,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!((report.overall_score() - 0.7).abs() < 0.001);
}

#[test]
fn capability_report_empty() {
    let report = CapabilityReport::default();
    assert!((report.overall_score()).abs() < f64::EPSILON);
}

// Tool use grading tests

#[test]
fn grade_tool_use_perfect() {
    let scenario = ToolUseScenario {
        scenario_id: "TU-1".into(),
        name: "test".into(),
        task: "test task".into(),
        context_content: String::new(),
        expected_tool_order: vec!["search_memory".into(), "synthesize_answer".into()],
        unnecessary_tools: vec!["calculate".into()],
        max_calls: 4,
    };
    let trajectory = make_trajectory(vec!["search_memory", "synthesize_answer"]);
    let result = grade_tool_use(&scenario, &trajectory, "answer");
    assert!((result.scores["tool_selection_accuracy"] - 1.0).abs() < 0.001);
    assert!((result.scores["tool_chain_correctness"] - 1.0).abs() < 0.001);
    assert!((result.scores["call_efficiency"] - 1.0).abs() < 0.001);
}

#[test]
fn grade_tool_use_wrong_order() {
    let scenario = ToolUseScenario {
        scenario_id: "TU-2".into(),
        name: "test".into(),
        task: "test task".into(),
        context_content: String::new(),
        expected_tool_order: vec!["search_memory".into(), "synthesize_answer".into()],
        unnecessary_tools: vec![],
        max_calls: 4,
    };
    let trajectory = make_trajectory(vec!["synthesize_answer", "search_memory"]);
    let result = grade_tool_use(&scenario, &trajectory, "answer");
    assert!((result.scores["tool_chain_correctness"]).abs() < f64::EPSILON);
}

#[test]
fn grade_tool_use_excess_calls() {
    let scenario = ToolUseScenario {
        scenario_id: "TU-3".into(),
        name: "test".into(),
        task: "test task".into(),
        context_content: String::new(),
        expected_tool_order: vec!["search".into()],
        unnecessary_tools: vec![],
        max_calls: 2,
    };
    let trajectory = make_trajectory(vec!["search", "search", "search", "search"]);
    let result = grade_tool_use(&scenario, &trajectory, "answer");
    assert!(result.scores["call_efficiency"] < 1.0);
}

#[test]
fn grade_tool_use_unnecessary_penalty() {
    let scenario = ToolUseScenario {
        scenario_id: "TU-4".into(),
        name: "test".into(),
        task: "test task".into(),
        context_content: String::new(),
        expected_tool_order: vec!["search".into()],
        unnecessary_tools: vec!["calculate".into()],
        max_calls: 4,
    };
    let trajectory = make_trajectory(vec!["search", "calculate"]);
    let result = grade_tool_use(&scenario, &trajectory, "answer");
    assert!(result.scores["tool_selection_accuracy"] < 1.0);
}

// Planning grading tests

#[test]
fn grade_planning_full_coverage() {
    let scenario = PlanningScenario {
        scenario_id: "PL-1".into(),
        name: "test".into(),
        task: "plan task".into(),
        context_content: String::new(),
        expected_subtasks: vec!["identify systems".into(), "assess risk".into()],
        expected_ordering_constraints: vec![("identify systems".into(), "assess risk".into())],
        success_criteria: String::new(),
    };
    // Long response with all subtasks in order
    let response = "First we identify systems in the environment, then we assess risk and mitigate. \
                    We should also consider additional factors including monitoring and alerting for \
                    the identified systems. The risk assessment should cover both internal and external \
                    threats to provide comprehensive coverage.";
    let result = grade_planning(&scenario, response);
    assert!((result.scores["subtask_coverage"] - 1.0).abs() < 0.001);
    assert!((result.scores["ordering_correctness"] - 1.0).abs() < 0.001);
}

#[test]
fn grade_planning_missing_subtask() {
    let scenario = PlanningScenario {
        scenario_id: "PL-2".into(),
        name: "test".into(),
        task: "plan task".into(),
        context_content: String::new(),
        expected_subtasks: vec!["identify systems".into(), "assess risk".into()],
        expected_ordering_constraints: vec![],
        success_criteria: String::new(),
    };
    let result = grade_planning(&scenario, "We should identify systems first");
    assert!((result.scores["subtask_coverage"] - 0.5).abs() < 0.001);
}

// Uncertainty grading tests

#[test]
fn grade_uncertainty_with_hedging() {
    let scenario = UncertaintyScenario {
        scenario_id: "RU-1".into(),
        name: "test".into(),
        question: "test question".into(),
        evidence_pieces: vec![],
        expected_behavior: String::new(),
        key_criteria: vec!["acknowledges_conflict".into()],
    };
    let response = "However, there is uncertain evidence that acknowledges the conflict between sources";
    let result = grade_uncertainty(&scenario, response);
    assert!(result.scores["appropriate_hedging"] > 0.0);
}

// Transfer grading tests

#[test]
fn grade_transfer_criteria() {
    let scenario = TransferScenario {
        scenario_id: "CT-1".into(),
        name: "test".into(),
        source_domain_content: String::new(),
        target_domain_question: String::new(),
        expected_analogy: String::new(),
        key_criteria: vec!["applies_defense".into()],
    };
    let result = grade_transfer(&scenario, "We should applies defense in depth");
    assert!(result.scores["criteria_coverage"] > 0.0);
}

// Collaborative grading tests

#[test]
fn grade_collaborative_with_delegations() {
    let scenario = CollaborativeScenario {
        scenario_id: "CO-1".into(),
        name: "test".into(),
        task: "review".into(),
        context_content: String::new(),
        expected_delegations: vec!["security".into(), "performance".into()],
        synthesis_criteria: vec![],
    };
    let result = grade_collaborative(&scenario, "security review and performance analysis");
    assert!((result.scores["delegation_coverage"] - 1.0).abs() < 0.001);
}

#[test]
fn scenario_result_serde() {
    let result = ScenarioResult {
        scenario_id: "S1".into(),
        scenario_name: "test".into(),
        agent_response: "answer".into(),
        trajectory: None,
        scores: HashMap::new(),
        reasoning: "ok".into(),
        metadata: HashMap::new(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let result2: ScenarioResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result2.scenario_id, "S1");
}

impl Default for EvalTypeResult {
    fn default() -> Self {
        Self {
            eval_type: String::new(),
            scenarios: vec![],
            metric_averages: HashMap::new(),
            overall_score: 0.0,
            duration_s: 0.0,
        }
    }
}
