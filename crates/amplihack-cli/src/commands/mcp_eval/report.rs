//! Report generation for MCP evaluation results.
//!
//! Generates Markdown reports with scenario details, metrics, and recommendations.

use super::{EvaluationReport, Recommendation};
use anyhow::Result;
use std::fmt::Write;
use std::path::Path;

/// Generate a Markdown report from evaluation results.
pub fn generate_report(report: &EvaluationReport) -> Result<String> {
    let mut out = String::new();

    writeln!(out, "# MCP Tool Evaluation Report")?;
    writeln!(out)?;
    writeln!(out, "## Adapter: {}", report.adapter_name)?;
    writeln!(out)?;

    // Summary
    writeln!(out, "## Summary")?;
    writeln!(out)?;
    writeln!(out, "| Metric | Value |")?;
    writeln!(out, "|--------|-------|")?;
    writeln!(
        out,
        "| Quality Score | {:.1}% |",
        report.metrics.quality_score * 100.0
    )?;
    writeln!(
        out,
        "| Efficiency Score | {:.2}x |",
        report.metrics.efficiency_score
    )?;
    writeln!(
        out,
        "| Total Operations | {} |",
        report.metrics.total_operations
    )?;
    writeln!(
        out,
        "| Successes | {} |",
        report.metrics.total_successes
    )?;
    writeln!(
        out,
        "| Total Duration | {:.2}s |",
        report.metrics.total_duration.as_secs_f64()
    )?;
    writeln!(out)?;

    // Recommendation
    writeln!(out, "## Recommendation: **{}**", report.recommendation)?;
    writeln!(out)?;
    match report.recommendation {
        Recommendation::Integrate => {
            writeln!(out, "> ✅ This tool meets quality and efficiency thresholds for integration.")?;
        }
        Recommendation::Consider => {
            writeln!(out, "> ⚠️ This tool partially meets thresholds. Consider for specific use cases.")?;
        }
        Recommendation::DontIntegrate => {
            writeln!(out, "> ❌ This tool does not meet minimum thresholds for integration.")?;
        }
    }
    writeln!(out)?;

    // Scenario details
    writeln!(out, "## Scenario Results")?;
    writeln!(out)?;
    for result in &report.scenarios_run {
        writeln!(out, "### {}", result.scenario_name)?;
        writeln!(out)?;
        writeln!(
            out,
            "- Success rate: {:.1}%",
            result.success_rate * 100.0
        )?;
        writeln!(
            out,
            "- Duration: {:.2}s",
            result.total_duration.as_secs_f64()
        )?;
        writeln!(out, "- Operations: {}", result.measurements.len())?;
        writeln!(out)?;
    }

    Ok(out)
}

/// Write report to file (creates parent dirs if needed).
pub fn write_report_to_file(report: &EvaluationReport, path: &Path) -> Result<()> {
    let content = generate_report(report)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &content)?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::mcp_eval::adapter::MeasurementResult;
    use crate::commands::mcp_eval::metrics::EvaluationMetrics;
    use crate::commands::mcp_eval::scenario::ScenarioResult;
    use std::time::Duration;
    use tempfile::TempDir;

    fn sample_report() -> EvaluationReport {
        EvaluationReport {
            adapter_name: "mock".to_string(),
            scenarios_run: vec![
                ScenarioResult {
                    scenario_name: "Navigation".to_string(),
                    measurements: vec![MeasurementResult {
                        operation: "find_def".to_string(),
                        duration: Duration::from_millis(150),
                        success: true,
                        output: None,
                    }],
                    total_duration: Duration::from_millis(150),
                    success_rate: 1.0,
                },
                ScenarioResult {
                    scenario_name: "Analysis".to_string(),
                    measurements: vec![MeasurementResult {
                        operation: "analyze".to_string(),
                        duration: Duration::from_millis(300),
                        success: true,
                        output: None,
                    }],
                    total_duration: Duration::from_millis(300),
                    success_rate: 1.0,
                },
            ],
            metrics: EvaluationMetrics {
                quality_score: 0.95,
                efficiency_score: 2.0,
                total_operations: 2,
                total_successes: 2,
                total_duration: Duration::from_millis(450),
            },
            recommendation: Recommendation::Integrate,
        }
    }

    #[test]
    fn generate_report_contains_header() {
        let report = sample_report();
        let md = generate_report(&report).unwrap();
        assert!(md.contains("# MCP Tool Evaluation Report"));
    }

    #[test]
    fn generate_report_contains_adapter_name() {
        let report = sample_report();
        let md = generate_report(&report).unwrap();
        assert!(md.contains("## Adapter: mock"));
    }

    #[test]
    fn generate_report_contains_metrics_table() {
        let report = sample_report();
        let md = generate_report(&report).unwrap();
        assert!(md.contains("Quality Score"));
        assert!(md.contains("Efficiency Score"));
        assert!(md.contains("95.0%"));
        assert!(md.contains("2.00x"));
    }

    #[test]
    fn generate_report_contains_recommendation() {
        let report = sample_report();
        let md = generate_report(&report).unwrap();
        assert!(md.contains("**INTEGRATE**"));
        assert!(md.contains("✅"));
    }

    #[test]
    fn generate_report_contains_scenario_details() {
        let report = sample_report();
        let md = generate_report(&report).unwrap();
        assert!(md.contains("### Navigation"));
        assert!(md.contains("### Analysis"));
        assert!(md.contains("Success rate: 100.0%"));
    }

    #[test]
    fn generate_report_dont_integrate_shows_x() {
        let mut report = sample_report();
        report.recommendation = Recommendation::DontIntegrate;
        let md = generate_report(&report).unwrap();
        assert!(md.contains("**DONT_INTEGRATE**"));
        assert!(md.contains("❌"));
    }

    #[test]
    fn generate_report_consider_shows_warning() {
        let mut report = sample_report();
        report.recommendation = Recommendation::Consider;
        let md = generate_report(&report).unwrap();
        assert!(md.contains("**CONSIDER**"));
        assert!(md.contains("⚠️"));
    }

    #[test]
    fn write_report_to_file_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("reports").join("eval.md");

        let report = sample_report();
        write_report_to_file(&report, &path).unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("MCP Tool Evaluation Report"));
    }

    #[test]
    fn write_report_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deep").join("nested").join("report.md");

        let report = sample_report();
        write_report_to_file(&report, &path).unwrap();
        assert!(path.exists());
    }
}
