//! `amplihack eval` subcommands.
//!
//! Wires the three public bricks in `amplihack-eval` to the CLI:
//!
//! * `run`    — execute a benchmark from a JSON config and print results.
//! * `compare`— diff two benchmark result files (baseline vs. candidate).
//! * `report` — pretty-print or re-serialise a saved result file.

use amplihack_eval::{BenchmarkResult, Reporter, RunScore, Scorer, ScorerConfig};
use anyhow::{Context, Result};
use std::io::Read;

// ─── public entry points ────────────────────────────────────────────────────

/// `amplihack eval run <config> [--format text|json] [--threshold N]`
pub fn run_eval_run(config_path: &str, format: &str, threshold: Option<f64>) -> Result<()> {
    let raw = read_source(config_path).context("reading benchmark config")?;

    // The config file is a BenchmarkResult in JSON form (cases pre-populated).
    // This mirrors the Python eval framework where the runner records outcomes
    // into a result object that is then passed to the scorer.
    let mut result: BenchmarkResult =
        serde_json::from_str(&raw).context("parsing benchmark config JSON")?;

    // Ensure the result is finished (may already have finished_at set).
    if result.finished_at.is_none() {
        result.finish();
    }

    let config = match threshold {
        Some(t) => ScorerConfig::with_threshold(t),
        None => ScorerConfig::default(),
    };
    let score = Scorer::new(config).score(&result);

    print_score(&score, format)?;

    // Exit non-zero when the benchmark fails so scripts can detect failures.
    if score.failed() {
        std::process::exit(1);
    }
    Ok(())
}

/// `amplihack eval compare <baseline> <candidate> [--format text|json]`
pub fn run_eval_compare(baseline_path: &str, candidate_path: &str, format: &str) -> Result<()> {
    let baseline_score = load_score(baseline_path).context("loading baseline")?;
    let candidate_score = load_score(candidate_path).context("loading candidate")?;

    let scorer = Scorer::new(ScorerConfig::default());
    let cmp = scorer.compare(&baseline_score, &candidate_score);

    match format {
        "json" => println!("{}", Reporter::comparison_json(&cmp)?),
        _ => println!("{}", Reporter::comparison_text(&cmp)),
    }
    Ok(())
}

/// `amplihack eval report <result> [--format text|json]`
pub fn run_eval_report(result_path: &str, format: &str) -> Result<()> {
    let score = load_score(result_path).context("loading result")?;
    print_score(&score, format)?;
    Ok(())
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Read a file path or "-" for stdin.
fn read_source(path: &str) -> Result<String> {
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading stdin")?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading file '{path}'"))
    }
}

/// Load a [`RunScore`] from a JSON file (or stdin with "-").
fn load_score(path: &str) -> Result<RunScore> {
    let raw = read_source(path)?;
    serde_json::from_str(&raw).context("parsing RunScore JSON")
}

/// Print a [`RunScore`] in the requested format.
fn print_score(score: &RunScore, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", Reporter::json(score)?),
        _ => println!("{}", Reporter::text(score)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use amplihack_eval::{BenchmarkResult, Scorer, ScorerConfig};
    use std::io::Write;

    fn write_temp_result(cases: &[(&str, bool, f64, u64)]) -> tempfile::NamedTempFile {
        let mut r = BenchmarkResult::new("cli-test");
        for &(id, passed, score, ms) in cases {
            r.add_case(id, passed, score, ms);
        }
        r.finish();
        let scorer = Scorer::new(ScorerConfig::default());
        let score = scorer.score(&r);
        let json = Reporter::json(&score).unwrap();
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn report_text_reads_file() {
        let f = write_temp_result(&[("c1", true, 0.9, 5)]);
        run_eval_report(f.path().to_str().unwrap(), "text").unwrap();
    }

    #[test]
    fn report_json_reads_file() {
        let f = write_temp_result(&[("c1", true, 0.9, 5)]);
        run_eval_report(f.path().to_str().unwrap(), "json").unwrap();
    }

    #[test]
    fn compare_two_files() {
        let base = write_temp_result(&[("c1", false, 0.2, 5)]);
        let cand = write_temp_result(&[("c1", true, 0.9, 5)]);
        run_eval_compare(
            base.path().to_str().unwrap(),
            cand.path().to_str().unwrap(),
            "text",
        )
        .unwrap();
    }

    #[test]
    fn load_score_missing_file_errors() {
        let err = load_score("/no/such/file.json");
        assert!(err.is_err());
    }
}
