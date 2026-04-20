//! Report generation: JSON and human-readable text output.
//!
//! [`Reporter`] is a stateless helper — every method is a free function on
//! `RunScore` or `RunComparison` values produced by [`crate::scorer::Scorer`].

use crate::error::EvalError;
use crate::scorer::{RunComparison, RunScore};

/// Stateless report generator.
pub struct Reporter;

impl Reporter {
    /// Serialise a [`RunScore`] to a pretty-printed JSON string.
    pub fn json(score: &RunScore) -> Result<String, EvalError> {
        Ok(serde_json::to_string_pretty(score)?)
    }

    /// Format a [`RunScore`] as human-readable text.
    pub fn text(score: &RunScore) -> String {
        let status = if score.overall_passed { "PASS" } else { "FAIL" };
        let bar = progress_bar(score.composite_score, 20);
        format!(
            "Benchmark: {name}\n\
             Status   : [{status}]\n\
             Cases    : {passed}/{total} passed ({pass_pct:.1}%)\n\
             Mean     : {mean:.3}\n\
             Composite: {bar} {composite:.3}\n\
             Duration : {dur_ms}ms",
            name = score.benchmark_name,
            status = status,
            passed = score.passed_cases,
            total = score.total_cases,
            pass_pct = score.pass_rate * 100.0,
            mean = score.mean_score,
            bar = bar,
            composite = score.composite_score,
            dur_ms = score.total_duration_ms,
        )
    }

    /// Format a [`RunComparison`] as human-readable text.
    pub fn comparison_text(cmp: &RunComparison) -> String {
        let direction = if cmp.improved {
            "▲ improved"
        } else {
            "▼ regressed"
        };
        let sign = |v: f64| if v >= 0.0 { "+" } else { "" };
        format!(
            "Comparison: {name}\n\
             Result    : {direction}\n\
             Composite : {sc}{composite_delta:.3}\n\
             Mean score: {sm}{mean_delta:.3}\n\
             Pass rate : {sp}{pass_delta:.3}",
            name = cmp.benchmark_name,
            direction = direction,
            sc = sign(cmp.composite_delta),
            composite_delta = cmp.composite_delta,
            sm = sign(cmp.mean_score_delta),
            mean_delta = cmp.mean_score_delta,
            sp = sign(cmp.pass_rate_delta),
            pass_delta = cmp.pass_rate_delta,
        )
    }

    /// Serialise a [`RunComparison`] to a pretty-printed JSON string.
    pub fn comparison_json(cmp: &RunComparison) -> Result<String, EvalError> {
        Ok(serde_json::to_string_pretty(cmp)?)
    }
}

/// Render a simple ASCII progress bar for a value in [0.0, 1.0].
fn progress_bar(value: f64, width: usize) -> String {
    let filled = ((value.clamp(0.0, 1.0) * width as f64).round()) as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "#".repeat(filled), ".".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::BenchmarkResult;
    use crate::scorer::{Scorer, ScorerConfig};

    fn sample_score(passed: bool) -> RunScore {
        let mut r = BenchmarkResult::new("demo");
        r.add_case("c1", true, 0.9, 5);
        r.add_case("c2", passed, 0.5, 10);
        r.finish();
        Scorer::new(ScorerConfig::default()).score(&r)
    }

    #[test]
    fn json_is_valid_json() {
        let score = sample_score(true);
        let j = Reporter::json(&score).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["benchmark_name"], "demo");
        assert!(v["overall_passed"].as_bool().unwrap());
    }

    #[test]
    fn text_contains_benchmark_name() {
        let score = sample_score(false);
        let t = Reporter::text(&score);
        assert!(t.contains("demo"));
        assert!(t.contains("FAIL"));
    }

    #[test]
    fn text_pass_shows_pass() {
        let score = sample_score(true);
        let t = Reporter::text(&score);
        assert!(t.contains("PASS"));
    }

    #[test]
    fn comparison_text_shows_direction() {
        let mut r1 = BenchmarkResult::new("bench");
        r1.add_case("a", false, 0.2, 0);
        r1.finish();
        let mut r2 = BenchmarkResult::new("bench");
        r2.add_case("a", true, 0.9, 0);
        r2.finish();

        let scorer = Scorer::new(ScorerConfig::default());
        let s1 = scorer.score(&r1);
        let s2 = scorer.score(&r2);
        let cmp = scorer.compare(&s1, &s2);
        let t = Reporter::comparison_text(&cmp);
        assert!(t.contains("improved"));
    }

    #[test]
    fn progress_bar_full() {
        assert_eq!(progress_bar(1.0, 4), "[####]");
    }

    #[test]
    fn progress_bar_empty() {
        assert_eq!(progress_bar(0.0, 4), "[....]");
    }

    #[test]
    fn progress_bar_half() {
        assert_eq!(progress_bar(0.5, 4), "[##..]");
    }
}
