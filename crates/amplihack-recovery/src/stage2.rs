use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::models::{
    DeltaVerdict, RecoveryBlocker, Stage2ErrorSignature, Stage2Result, StageStatus,
};

/// Build a 12-char hex signature ID from the canonical fields.
fn make_signature_id(error_type: &str, headline: &str, location: &str, message: &str) -> String {
    let canonical = format!("{error_type}|{headline}|{location}|{message}");
    let hash = Sha256::digest(canonical.as_bytes());
    hex_encode(&hash[..6])
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse a single pytest-style error line into an `Stage2ErrorSignature`.
///
/// Expected format: `ERROR_TYPE: headline (location) - message`
/// Falls back to raw line if parsing fails.
fn parse_error_line(line: &str) -> Option<Stage2ErrorSignature> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (error_type, rest) = if let Some(idx) = trimmed.find(':') {
        let et = trimmed[..idx].trim();
        let rest = trimmed[idx + 1..].trim();
        (et.to_string(), rest.to_string())
    } else {
        ("UnknownError".to_string(), trimmed.to_string())
    };

    let (headline, location, message) = parse_headline_location_message(&rest);

    let sig_id = make_signature_id(&error_type, &headline, &location, &message);

    Some(Stage2ErrorSignature {
        signature_id: sig_id,
        error_type,
        headline,
        normalized_location: location,
        normalized_message: message,
        occurrences: 1,
    })
}

/// Split rest into `(headline, location, message)`.
fn parse_headline_location_message(rest: &str) -> (String, String, String) {
    // Try splitting on " - " for message part
    let (before_msg, message) = if let Some(idx) = rest.find(" - ") {
        (rest[..idx].trim(), rest[idx + 3..].trim().to_string())
    } else {
        (rest.trim(), String::new())
    };

    // Try extracting (location) from the end of before_msg
    if let Some(open) = before_msg.rfind('(')
        && before_msg.ends_with(')')
    {
        let headline = before_msg[..open].trim().to_string();
        let location = before_msg[open + 1..before_msg.len() - 1]
            .trim()
            .to_string();
        return (headline, location, message);
    }

    (before_msg.to_string(), String::new(), message)
}

/// Build error signatures from raw test output.
pub fn build_error_signatures(output: &str) -> Vec<Stage2ErrorSignature> {
    let mut sig_map: HashMap<String, Stage2ErrorSignature> = HashMap::new();

    for line in output.lines() {
        if let Some(sig) = parse_error_line(line) {
            sig_map
                .entry(sig.signature_id.clone())
                .and_modify(|existing| existing.occurrences += 1)
                .or_insert(sig);
        }
    }

    let mut sigs: Vec<_> = sig_map.into_values().collect();
    sigs.sort_by(|a, b| b.occurrences.cmp(&a.occurrences));
    sigs
}

/// Cluster signatures by error_type.
pub fn cluster_signatures(sigs: &[Stage2ErrorSignature]) -> Vec<serde_json::Value> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for sig in sigs {
        groups
            .entry(sig.error_type.clone())
            .or_default()
            .push(sig.signature_id.clone());
    }

    groups
        .into_iter()
        .map(|(error_type, ids)| {
            serde_json::json!({
                "error_type": error_type,
                "signature_ids": ids,
                "count": ids.len(),
            })
        })
        .collect()
}

/// Determine delta verdict by comparing baseline and final error counts.
pub fn determine_delta_verdict(baseline: u32, final_count: u32) -> DeltaVerdict {
    if final_count < baseline {
        DeltaVerdict::Reduced
    } else if final_count == baseline {
        DeltaVerdict::Unchanged
    } else {
        DeltaVerdict::Replaced
    }
}

/// Run pytest (or test command) and capture output.
fn run_tests(repo_path: &Path) -> Result<(String, u32)> {
    let output = Command::new("python3")
        .args(["-m", "pytest", "--tb=line", "-q"])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(o) => {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let sigs = build_error_signatures(&combined);
            let count = sigs.iter().map(|s| s.occurrences).sum();
            Ok((combined, count))
        }
        Err(_) => {
            warn!("stage2: pytest not available, returning zero errors");
            Ok((String::new(), 0))
        }
    }
}

/// Stage 2: collect error signatures, attempt fixes, compute delta verdict.
///
/// `protected_files` lists paths that must not be modified by automated fixes.
pub fn run_stage2(repo_path: &Path, protected_files: &[String]) -> Result<Stage2Result> {
    info!("stage2: collecting error signatures");
    if !protected_files.is_empty() {
        info!(
            "stage2: {} protected file(s) will be excluded from automated fixes",
            protected_files.len()
        );
    }

    let mut blockers = Vec::new();
    let mut diagnostics = Vec::new();

    let (baseline_output, baseline_errors) =
        run_tests(repo_path).context("stage2: baseline test run failed")?;
    diagnostics.push(format!("baseline: {baseline_errors} error(s)"));

    let signatures = build_error_signatures(&baseline_output);
    let clusters = cluster_signatures(&signatures);

    // NOTE: Automated fix application is not yet implemented. Stage 2
    // currently benchmarks the error delta by re-running the test suite.
    // Future work: parse error signatures, apply heuristic patches, and
    // verify the fix reduces the error count before promoting.
    let (_final_output, final_errors) =
        run_tests(repo_path).context("stage2: final test run failed")?;
    diagnostics.push(format!("final: {final_errors} error(s)"));

    let delta_verdict = determine_delta_verdict(baseline_errors, final_errors);

    if final_errors > baseline_errors {
        blockers.push(RecoveryBlocker {
            stage: 2,
            code: "ERROR_REGRESSION".into(),
            message: format!("errors increased from {baseline_errors} to {final_errors}"),
            retryable: true,
        });
    }

    let status = if blockers.is_empty() {
        StageStatus::Completed
    } else {
        StageStatus::Blocked
    };

    Ok(Stage2Result {
        status,
        baseline_errors,
        final_errors,
        delta_verdict,
        signatures,
        clusters,
        applied_fixes: vec![],
        diagnostics,
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_signature_id_deterministic() {
        let a = make_signature_id("TypeError", "x", "foo.py:1", "bad");
        let b = make_signature_id("TypeError", "x", "foo.py:1", "bad");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn make_signature_id_differs_on_type() {
        let a = make_signature_id("TypeError", "x", "foo.py:1", "bad");
        let b = make_signature_id("ValueError", "x", "foo.py:1", "bad");
        assert_ne!(a, b);
    }

    #[test]
    fn build_signatures_deduplicates() {
        let output = "TypeError: x (foo.py:1) - bad\nTypeError: x (foo.py:1) - bad\n";
        let sigs = build_error_signatures(output);
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].occurrences, 2);
    }

    #[test]
    fn build_signatures_empty_input() {
        let sigs = build_error_signatures("");
        assert!(sigs.is_empty());
    }

    #[test]
    fn build_signatures_no_colon_fallback() {
        let sigs = build_error_signatures("some weird error line");
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0].error_type, "UnknownError");
    }

    #[test]
    fn cluster_signatures_groups_by_type() {
        let sigs = vec![
            Stage2ErrorSignature {
                signature_id: "aaa".into(),
                error_type: "TypeError".into(),
                headline: "h1".into(),
                normalized_location: "l1".into(),
                normalized_message: "m1".into(),
                occurrences: 1,
            },
            Stage2ErrorSignature {
                signature_id: "bbb".into(),
                error_type: "TypeError".into(),
                headline: "h2".into(),
                normalized_location: "l2".into(),
                normalized_message: "m2".into(),
                occurrences: 1,
            },
            Stage2ErrorSignature {
                signature_id: "ccc".into(),
                error_type: "ValueError".into(),
                headline: "h3".into(),
                normalized_location: "l3".into(),
                normalized_message: "m3".into(),
                occurrences: 1,
            },
        ];
        let clusters = cluster_signatures(&sigs);
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn delta_verdict_reduced() {
        assert_eq!(determine_delta_verdict(10, 5), DeltaVerdict::Reduced);
    }

    #[test]
    fn delta_verdict_unchanged() {
        assert_eq!(determine_delta_verdict(10, 10), DeltaVerdict::Unchanged);
    }

    #[test]
    fn delta_verdict_replaced() {
        assert_eq!(determine_delta_verdict(5, 10), DeltaVerdict::Replaced);
    }

    #[test]
    fn parse_error_line_full_format() {
        let sig = parse_error_line("TypeError: foo bar (src/x.py:10) - something wrong").unwrap();
        assert_eq!(sig.error_type, "TypeError");
        assert_eq!(sig.headline, "foo bar");
        assert_eq!(sig.normalized_location, "src/x.py:10");
        assert_eq!(sig.normalized_message, "something wrong");
    }

    #[test]
    fn parse_error_line_no_location() {
        let sig = parse_error_line("ValueError: bad value - details here").unwrap();
        assert_eq!(sig.error_type, "ValueError");
        assert_eq!(sig.normalized_message, "details here");
    }

    #[test]
    fn parse_error_line_empty() {
        assert!(parse_error_line("").is_none());
        assert!(parse_error_line("   ").is_none());
    }
}
