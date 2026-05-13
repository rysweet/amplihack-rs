//! Evaluation scenarios: Navigation, Analysis, Modification.

use super::adapter::{McpToolAdapter, MeasurementResult};
use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

/// A named evaluation scenario with a set of operations.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    pub operations: Vec<String>,
    /// Baseline duration (what a manual approach would take).
    pub baseline_duration: Duration,
}

/// Result of running one scenario.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub scenario_name: String,
    pub measurements: Vec<MeasurementResult>,
    pub total_duration: Duration,
    pub success_rate: f64,
}

/// Runs scenarios against an adapter.
pub struct ScenarioRunner<'a> {
    adapter: &'a dyn McpToolAdapter,
    /// When true, adapter returns simulated data without MCP server connection.
    _mock_mode: bool,
}

impl<'a> ScenarioRunner<'a> {
    pub fn new(adapter: &'a dyn McpToolAdapter, mock_mode: bool) -> Self {
        Self {
            adapter,
            _mock_mode: mock_mode,
        }
    }

    /// Run a single scenario, collecting measurements.
    pub fn run(&self, scenario: &Scenario) -> Result<ScenarioResult> {
        let mut measurements = Vec::new();
        let mut total_duration = Duration::ZERO;
        let mut successes = 0u32;

        for op in &scenario.operations {
            let result = self.adapter.measure(op)?;
            total_duration += result.duration;
            if result.success {
                successes += 1;
            }
            measurements.push(result);
        }

        let success_rate = if measurements.is_empty() {
            0.0
        } else {
            successes as f64 / measurements.len() as f64
        };

        Ok(ScenarioResult {
            scenario_name: scenario.name.clone(),
            measurements,
            total_duration,
            success_rate,
        })
    }
}

/// Return the 3 built-in scenarios.
pub fn built_in_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "Navigation".to_string(),
            description: "Test code navigation capabilities (go-to-definition, find-references)"
                .to_string(),
            operations: vec![
                "find_definition".to_string(),
                "find_references".to_string(),
                "navigate_to_symbol".to_string(),
                "find_implementations".to_string(),
            ],
            baseline_duration: Duration::from_secs(10),
        },
        Scenario {
            name: "Analysis".to_string(),
            description: "Test code analysis capabilities (search, diagnostics, hover)"
                .to_string(),
            operations: vec![
                "analyze_code".to_string(),
                "search_symbols".to_string(),
                "get_diagnostics".to_string(),
                "hover_info".to_string(),
            ],
            baseline_duration: Duration::from_secs(15),
        },
        Scenario {
            name: "Modification".to_string(),
            description: "Test code modification capabilities (rename, refactor, edit)".to_string(),
            operations: vec![
                "modify_rename_symbol".to_string(),
                "modify_extract_method".to_string(),
                "modify_inline_variable".to_string(),
                "edit_file_content".to_string(),
            ],
            baseline_duration: Duration::from_secs(20),
        },
    ]
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp_eval::adapter::MockAdapter;

    #[test]
    fn built_in_scenarios_returns_three() {
        let scenarios = built_in_scenarios();
        assert_eq!(scenarios.len(), 3);
        assert_eq!(scenarios[0].name, "Navigation");
        assert_eq!(scenarios[1].name, "Analysis");
        assert_eq!(scenarios[2].name, "Modification");
    }

    #[test]
    fn each_scenario_has_operations() {
        for scenario in built_in_scenarios() {
            assert!(
                !scenario.operations.is_empty(),
                "Scenario '{}' has no operations",
                scenario.name
            );
            assert!(
                scenario.baseline_duration > Duration::ZERO,
                "Scenario '{}' has zero baseline",
                scenario.name
            );
        }
    }

    #[test]
    fn scenario_runner_runs_all_operations() {
        let adapter = MockAdapter::new();
        adapter.enable().unwrap();
        let runner = ScenarioRunner::new(&adapter, true);

        let scenario = &built_in_scenarios()[0]; // Navigation
        let result = runner.run(scenario).unwrap();

        assert_eq!(result.scenario_name, "Navigation");
        assert_eq!(result.measurements.len(), scenario.operations.len());
        assert!(result.total_duration > Duration::ZERO);
        assert_eq!(result.success_rate, 1.0); // Mock always succeeds
    }

    #[test]
    fn scenario_runner_computes_success_rate() {
        let adapter = MockAdapter::new();
        adapter.enable().unwrap();
        let runner = ScenarioRunner::new(&adapter, true);

        let scenarios = built_in_scenarios();
        for scenario in &scenarios {
            let result = runner.run(scenario).unwrap();
            assert!(result.success_rate >= 0.0 && result.success_rate <= 1.0);
        }
    }

    #[test]
    fn scenario_result_serializes_to_json() {
        let result = ScenarioResult {
            scenario_name: "Test".to_string(),
            measurements: Vec::new(),
            total_duration: Duration::from_millis(500),
            success_rate: 0.75,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"scenario_name\":\"Test\""));
        assert!(json.contains("0.75"));
    }

    #[test]
    fn empty_scenario_has_zero_success_rate() {
        let adapter = MockAdapter::new();
        adapter.enable().unwrap();
        let runner = ScenarioRunner::new(&adapter, true);

        let empty_scenario = Scenario {
            name: "Empty".to_string(),
            description: "".to_string(),
            operations: Vec::new(),
            baseline_duration: Duration::from_secs(1),
        };

        let result = runner.run(&empty_scenario).unwrap();
        assert_eq!(result.success_rate, 0.0);
        assert_eq!(result.total_duration, Duration::ZERO);
    }
}
