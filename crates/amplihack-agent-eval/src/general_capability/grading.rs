//! Grading functions for general capability scenarios.

use std::collections::{HashMap, HashSet};
use super::*;


/// Grade tool use efficiency for a single scenario.
///
/// Metrics:
/// - `tool_selection_accuracy`: fraction of expected tools called minus penalty for unnecessary
/// - `tool_chain_correctness`: ordering constraints satisfied
/// - `call_efficiency`: penalty for exceeding max call budget
pub fn grade_tool_use(
    scenario: &ToolUseScenario,
    trajectory: &ToolTrajectory,
    agent_response: &str,
) -> ScenarioResult {
    let actual_names = trajectory.call_names();
    let expected = &scenario.expected_tool_order;

    // Tool selection accuracy
    let expected_set: HashSet<&str> = expected.iter().map(String::as_str).collect();
    let called_set = trajectory.unique_tools();
    let selection_recall = if expected_set.is_empty() {
        1.0
    } else {
        expected_set.intersection(&called_set).count() as f64 / expected_set.len() as f64
    };
    let unnecessary_set: HashSet<&str> =
        scenario.unnecessary_tools.iter().map(String::as_str).collect();
    let unnecessary_called = called_set.intersection(&unnecessary_set).count();
    let selection_accuracy = (selection_recall - unnecessary_called as f64 * 0.15).max(0.0);

    // Tool chain correctness
    let chain_score = if expected.len() > 1 {
        let mut correct = 0usize;
        let mut total = 0usize;
        for pair in expected.windows(2) {
            total += 1;
            let pos_a = actual_names.iter().position(|n| *n == pair[0]);
            let pos_b = actual_names.iter().position(|n| *n == pair[1]);
            if let (Some(a), Some(b)) = (pos_a, pos_b)
                && a < b {
                    correct += 1;
                }
        }
        if total > 0 {
            correct as f64 / total as f64
        } else {
            1.0
        }
    } else {
        1.0
    };

    // Call efficiency
    let efficiency = if trajectory.call_count() <= scenario.max_calls {
        1.0
    } else {
        let excess = trajectory.call_count() - scenario.max_calls;
        (1.0 - excess as f64 * 0.2).max(0.0)
    };

    let mut scores = HashMap::new();
    scores.insert(
        "tool_selection_accuracy".into(),
        (selection_accuracy * 1000.0).round() / 1000.0,
    );
    scores.insert(
        "tool_chain_correctness".into(),
        (chain_score * 1000.0).round() / 1000.0,
    );
    scores.insert(
        "call_efficiency".into(),
        (efficiency * 1000.0).round() / 1000.0,
    );

    let reasoning = format!(
        "Expected: {:?}, Actual: {:?}. Selection: {selection_accuracy:.2}, \
         Chain: {chain_score:.2}, Efficiency: {efficiency:.2} ({}/{} calls)",
        expected,
        actual_names,
        trajectory.call_count(),
        scenario.max_calls,
    );

    ScenarioResult {
        scenario_id: scenario.scenario_id.clone(),
        scenario_name: scenario.name.clone(),
        agent_response: agent_response.to_string(),
        trajectory: Some(trajectory.clone()),
        scores,
        reasoning,
        metadata: HashMap::new(),
    }
}

/// Grade a planning scenario by checking subtask presence and ordering.
pub fn grade_planning(scenario: &PlanningScenario, agent_response: &str) -> ScenarioResult {
    let response_lower = agent_response.to_lowercase();

    // Subtask identification
    let found_subtasks: Vec<&String> = scenario
        .expected_subtasks
        .iter()
        .filter(|s| response_lower.contains(&s.to_lowercase()))
        .collect();
    let subtask_coverage = if scenario.expected_subtasks.is_empty() {
        1.0
    } else {
        found_subtasks.len() as f64 / scenario.expected_subtasks.len() as f64
    };

    // Ordering constraints
    let ordering_score = if scenario.expected_ordering_constraints.is_empty() {
        1.0
    } else {
        let mut satisfied = 0usize;
        for (before, after) in &scenario.expected_ordering_constraints {
            let pos_before = response_lower.find(&before.to_lowercase());
            let pos_after = response_lower.find(&after.to_lowercase());
            if let (Some(pb), Some(pa)) = (pos_before, pos_after)
                && pb < pa {
                    satisfied += 1;
                }
        }
        satisfied as f64 / scenario.expected_ordering_constraints.len() as f64
    };

    // Completeness (simple length heuristic)
    let word_count = agent_response.split_whitespace().count();
    let completeness = if word_count >= 100 {
        1.0
    } else if word_count >= 50 {
        0.7
    } else {
        0.3
    };

    let mut scores = HashMap::new();
    scores.insert("subtask_coverage".into(), round3(subtask_coverage));
    scores.insert("ordering_correctness".into(), round3(ordering_score));
    scores.insert("completeness".into(), round3(completeness));

    ScenarioResult {
        scenario_id: scenario.scenario_id.clone(),
        scenario_name: scenario.name.clone(),
        agent_response: agent_response.to_string(),
        trajectory: None,
        scores,
        reasoning: format!(
            "Subtasks: {}/{}, Ordering: {ordering_score:.2}, Completeness: {completeness:.2}",
            found_subtasks.len(),
            scenario.expected_subtasks.len(),
        ),
        metadata: HashMap::new(),
    }
}

/// Grade an uncertainty scenario by checking key criteria presence.
pub fn grade_uncertainty(scenario: &UncertaintyScenario, agent_response: &str) -> ScenarioResult {
    let response_lower = agent_response.to_lowercase();
    let matched: Vec<&String> = scenario
        .key_criteria
        .iter()
        .filter(|c| {
            // Convert snake_case criterion to words for matching
            let words = c.replace('_', " ").to_lowercase();
            response_lower.contains(&words)
                || c.split('_')
                    .all(|w| response_lower.contains(&w.to_lowercase()))
        })
        .collect();

    let criteria_score = if scenario.key_criteria.is_empty() {
        1.0
    } else {
        matched.len() as f64 / scenario.key_criteria.len() as f64
    };

    // Hedging check
    let hedging_words = ["however", "although", "uncertain", "conflicting", "may", "possible"];
    let hedging_count = hedging_words
        .iter()
        .filter(|w| response_lower.contains(**w))
        .count();
    let hedging_score = (hedging_count as f64 / 3.0).min(1.0);

    let mut scores = HashMap::new();
    scores.insert("criteria_coverage".into(), round3(criteria_score));
    scores.insert("appropriate_hedging".into(), round3(hedging_score));

    ScenarioResult {
        scenario_id: scenario.scenario_id.clone(),
        scenario_name: scenario.name.clone(),
        agent_response: agent_response.to_string(),
        trajectory: None,
        scores,
        reasoning: format!(
            "Criteria: {}/{}, Hedging: {hedging_score:.2}",
            matched.len(),
            scenario.key_criteria.len(),
        ),
        metadata: HashMap::new(),
    }
}

/// Grade a transfer scenario by checking criteria presence.
pub fn grade_transfer(scenario: &TransferScenario, agent_response: &str) -> ScenarioResult {
    let response_lower = agent_response.to_lowercase();
    let matched: Vec<&String> = scenario
        .key_criteria
        .iter()
        .filter(|c| {
            let words = c.replace('_', " ").to_lowercase();
            response_lower.contains(&words)
                || c.split('_')
                    .all(|w| response_lower.contains(&w.to_lowercase()))
        })
        .collect();

    let criteria_score = if scenario.key_criteria.is_empty() {
        1.0
    } else {
        matched.len() as f64 / scenario.key_criteria.len() as f64
    };

    let mut scores = HashMap::new();
    scores.insert("criteria_coverage".into(), round3(criteria_score));

    ScenarioResult {
        scenario_id: scenario.scenario_id.clone(),
        scenario_name: scenario.name.clone(),
        agent_response: agent_response.to_string(),
        trajectory: None,
        scores,
        reasoning: format!(
            "Transfer criteria: {}/{}",
            matched.len(),
            scenario.key_criteria.len(),
        ),
        metadata: HashMap::new(),
    }
}

/// Grade a collaborative scenario by checking delegation and synthesis criteria.
pub fn grade_collaborative(
    scenario: &CollaborativeScenario,
    agent_response: &str,
) -> ScenarioResult {
    let response_lower = agent_response.to_lowercase();

    let delegations_found = scenario
        .expected_delegations
        .iter()
        .filter(|d| response_lower.contains(&d.to_lowercase()))
        .count();
    let delegation_score = if scenario.expected_delegations.is_empty() {
        1.0
    } else {
        delegations_found as f64 / scenario.expected_delegations.len() as f64
    };

    let synthesis_found = scenario
        .synthesis_criteria
        .iter()
        .filter(|c| {
            let words = c.replace('_', " ").to_lowercase();
            response_lower.contains(&words)
                || c.split('_')
                    .all(|w| response_lower.contains(&w.to_lowercase()))
        })
        .count();
    let synthesis_score = if scenario.synthesis_criteria.is_empty() {
        1.0
    } else {
        synthesis_found as f64 / scenario.synthesis_criteria.len() as f64
    };

    let mut scores = HashMap::new();
    scores.insert("delegation_coverage".into(), round3(delegation_score));
    scores.insert("synthesis_quality".into(), round3(synthesis_score));

    ScenarioResult {
        scenario_id: scenario.scenario_id.clone(),
        scenario_name: scenario.name.clone(),
        agent_response: agent_response.to_string(),
        trajectory: None,
        scores,
        reasoning: format!(
            "Delegations: {}/{}, Synthesis: {}/{}",
            delegations_found,
            scenario.expected_delegations.len(),
            synthesis_found,
            scenario.synthesis_criteria.len(),
        ),
        metadata: HashMap::new(),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

