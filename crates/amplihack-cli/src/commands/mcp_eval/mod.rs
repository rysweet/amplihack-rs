//! MCP tool evaluation framework.
//!
//! Evaluates MCP tools using scenarios (Navigation, Analysis, Modification)
//! and produces a recommendation: INTEGRATE / CONSIDER / DONT_INTEGRATE.

pub mod adapter;
pub mod metrics;
pub mod report;
pub mod scenario;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use adapter::{McpToolAdapter, MockAdapter};
pub use metrics::{EvaluationMetrics, Recommendation, ScoringEngine};
pub use report::generate_report;
pub use scenario::{Scenario, ScenarioResult, ScenarioRunner};

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEvalConfig {
    /// Adapter name to evaluate
    pub adapter: String,
    /// Which scenarios to run (empty = all)
    #[serde(default)]
    pub scenarios: Vec<String>,
    /// Use mock mode (no actual MCP calls)
    #[serde(default = "default_mock")]
    pub mock: bool,
    /// Output path for report
    pub output: Option<String>,
}

fn default_mock() -> bool {
    true
}

impl Default for McpEvalConfig {
    fn default() -> Self {
        Self {
            adapter: "mock".to_string(),
            scenarios: Vec::new(),
            mock: true,
            output: None,
        }
    }
}

// ── CLI dispatch ─────────────────────────────────────────────────────────────

/// Entry point from the CLI dispatch layer.
pub fn dispatch(
    adapter: String,
    scenario: Option<String>,
    mock: bool,
    output: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    let config = if let Some(path) = config_path {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)?
    } else {
        McpEvalConfig {
            adapter,
            scenarios: scenario.map(|s| vec![s]).unwrap_or_default(),
            mock,
            output: output.as_ref().map(|p| p.display().to_string()),
        }
    };

    let report = run_evaluation(&config)?;
    let markdown = generate_report(&report)?;

    if let Some(out_path) = output.or_else(|| config.output.as_ref().map(std::path::PathBuf::from))
    {
        std::fs::write(&out_path, &markdown)?;
        eprintln!("Report written to: {}", out_path.display());
    } else {
        println!("{markdown}");
    }

    Ok(())
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Run the full MCP evaluation pipeline.
pub fn run_evaluation(config: &McpEvalConfig) -> Result<EvaluationReport> {
    let adapter: Box<dyn McpToolAdapter> = match config.adapter.as_str() {
        "mock" => Box::new(MockAdapter::new()),
        other => anyhow::bail!("Unknown adapter: '{}'. Available: mock", other),
    };

    // Enable the adapter
    adapter.enable()?;

    // Build scenario list
    let scenarios = scenario::built_in_scenarios();
    let scenarios_to_run: Vec<&Scenario> = if config.scenarios.is_empty() {
        scenarios.iter().collect()
    } else {
        scenarios
            .iter()
            .filter(|s| config.scenarios.contains(&s.name))
            .collect()
    };

    if scenarios_to_run.is_empty() {
        anyhow::bail!("No matching scenarios found");
    }

    // Run scenarios
    let runner = ScenarioRunner::new(adapter.as_ref(), config.mock);
    let mut results = Vec::new();
    for scenario in &scenarios_to_run {
        let result = runner.run(scenario)?;
        results.push(result);
    }

    // Score
    let engine = ScoringEngine::default();
    let metrics = engine.compute(&results);
    let recommendation = engine.recommend(&metrics);

    // Disable adapter
    adapter.disable()?;

    Ok(EvaluationReport {
        adapter_name: adapter.name().to_string(),
        scenarios_run: results,
        metrics,
        recommendation,
    })
}

// ── Report structure ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationReport {
    pub adapter_name: String,
    pub scenarios_run: Vec<ScenarioResult>,
    pub metrics: EvaluationMetrics,
    pub recommendation: Recommendation,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_mock_adapter() {
        let cfg = McpEvalConfig::default();
        assert_eq!(cfg.adapter, "mock");
        assert!(cfg.mock);
        assert!(cfg.scenarios.is_empty());
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "adapter": "serena",
            "scenarios": ["Navigation", "Analysis"],
            "mock": false,
            "output": "/tmp/report.md"
        }"#;
        let cfg: McpEvalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.adapter, "serena");
        assert_eq!(cfg.scenarios, vec!["Navigation", "Analysis"]);
        assert!(!cfg.mock);
        assert_eq!(cfg.output, Some("/tmp/report.md".to_string()));
    }

    #[test]
    fn run_evaluation_with_mock_adapter_succeeds() {
        let cfg = McpEvalConfig::default();
        let report = run_evaluation(&cfg).unwrap();
        assert_eq!(report.adapter_name, "mock");
        assert!(!report.scenarios_run.is_empty());
        // Mock adapter should produce a recommendation
        assert!(matches!(
            report.recommendation,
            Recommendation::Integrate | Recommendation::Consider | Recommendation::DontIntegrate
        ));
    }

    #[test]
    fn run_evaluation_unknown_adapter_errors() {
        let cfg = McpEvalConfig {
            adapter: "nonexistent".to_string(),
            ..Default::default()
        };
        let err = run_evaluation(&cfg).unwrap_err();
        assert!(err.to_string().contains("Unknown adapter"));
    }

    #[test]
    fn run_evaluation_filters_scenarios_by_name() {
        let cfg = McpEvalConfig {
            scenarios: vec!["Navigation".to_string()],
            ..Default::default()
        };
        let report = run_evaluation(&cfg).unwrap();
        assert_eq!(report.scenarios_run.len(), 1);
        assert_eq!(report.scenarios_run[0].scenario_name, "Navigation");
    }

    #[test]
    fn run_evaluation_no_matching_scenarios_errors() {
        let cfg = McpEvalConfig {
            scenarios: vec!["NonexistentScenario".to_string()],
            ..Default::default()
        };
        let err = run_evaluation(&cfg).unwrap_err();
        assert!(err.to_string().contains("No matching scenarios"));
    }

    #[test]
    fn evaluation_report_serializes_to_json() {
        let cfg = McpEvalConfig::default();
        let report = run_evaluation(&cfg).unwrap();
        let json = serde_json::to_string_pretty(&report).unwrap();
        assert!(json.contains("adapter_name"));
        assert!(json.contains("recommendation"));
        assert!(json.contains("metrics"));
    }
}
