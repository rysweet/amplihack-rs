//! Runtime evaluation, quality metrics, and scoring.
//!
//! Port of Python `continuous_eval.py` — provides grading logic, default eval
//! content/questions, and result types for single-agent continuous evaluation.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration for a continuous eval run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalConfig {
    /// Number of content turns to feed.
    pub turns: usize,
    /// Number of eval repetitions (results averaged).
    pub repeats: usize,
    /// Optional path to write JSON results.
    pub output: Option<String>,
    /// Content pool to feed the agent.
    pub content_pool: Vec<String>,
    /// Evaluation questions: `(question, expected_answer)`.
    pub questions: Vec<EvalQuestion>,
}

/// A single evaluation question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalQuestion {
    pub question: String,
    pub expected: String,
}

/// Result for a single question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionResult {
    pub question: String,
    pub expected: String,
    pub answer: String,
    pub score: f64,
    pub elapsed_s: f64,
}

/// Result for one eval repetition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatResult {
    pub repeat: usize,
    pub turns: usize,
    pub learn_elapsed_s: f64,
    pub learn_throughput: f64,
    pub questions: Vec<QuestionResult>,
    pub avg_score: f64,
}

/// Aggregate eval summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    pub eval_name: String,
    pub turns: usize,
    pub repeats: usize,
    pub results: Vec<RepeatResult>,
    pub aggregate_avg_score: f64,
    pub aggregate_avg_throughput: f64,
}

// ---------------------------------------------------------------------------
// Grading
// ---------------------------------------------------------------------------

/// Simple keyword-based grading. Returns a score in `[0.0, 1.0]`.
///
/// Splits the expected answer into keywords (≥4 chars) and checks how many
/// appear in the actual answer.
pub fn grade_answer(_question: &str, expected: &str, actual: &str) -> f64 {
    if actual.is_empty() {
        return 0.0;
    }
    let actual_lower = actual.to_lowercase();
    let expected_lower = expected.to_lowercase();

    let keywords: Vec<&str> = expected_lower
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();

    if keywords.is_empty() {
        return if actual_lower.contains(&expected_lower) {
            1.0
        } else {
            0.0
        };
    }

    let hits = keywords
        .iter()
        .filter(|kw| actual_lower.contains(**kw))
        .count();
    hits as f64 / keywords.len() as f64
}

/// Compute the average score across question results.
pub fn average_score(results: &[QuestionResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    let sum: f64 = results.iter().map(|r| r.score).sum();
    (sum / results.len() as f64 * 1000.0).round() / 1000.0
}

/// Compute the aggregate summary from repeat results.
pub fn aggregate_summary(eval_name: &str, turns: usize, results: &[RepeatResult]) -> EvalSummary {
    let n = results.len().max(1) as f64;
    let avg_score: f64 = results.iter().map(|r| r.avg_score).sum::<f64>() / n;
    let avg_tp: f64 = results.iter().map(|r| r.learn_throughput).sum::<f64>() / n;

    EvalSummary {
        eval_name: eval_name.to_string(),
        turns,
        repeats: results.len(),
        results: results.to_vec(),
        aggregate_avg_score: (avg_score * 1000.0).round() / 1000.0,
        aggregate_avg_throughput: (avg_tp * 10.0).round() / 10.0,
    }
}

// ---------------------------------------------------------------------------
// Default eval content (security analyst domain)
// ---------------------------------------------------------------------------

/// Default content pool for eval runs.
pub fn default_content_pool() -> Vec<String> {
    vec![
        "Log4Shell (CVE-2021-44228) is a critical RCE vulnerability in Apache Log4j 2.x with a CVSS score of 10.0.".into(),
        "The Midnight Blizzard (APT29) threat actor is linked to the Russian SVR intelligence service.".into(),
        "Incident INC-2024-001: Ransomware encrypted 500 files on corp-server-01. Encrypted files restored from backup.".into(),
        "The insider threat incident involved jsmith downloading 2,847 documents before account suspension.".into(),
        "CVE-2021-44228 affects Apache Log4j versions 2.0-beta9 through 2.14.1.".into(),
        "A malicious npm package 'event-stream' was used in a supply chain attack targeting cryptocurrency wallets.".into(),
        "Incident INC-2024-002: C2 beacon to 185.220.101.45 detected from workstation WS-047.".into(),
        "DNS tunneling uses DNS protocol to exfiltrate data by encoding payloads in DNS queries.".into(),
        "Security improvement after INC-2024-001: mandatory MFA enforced for all privileged accounts.".into(),
        "APT29 uses spearphishing and DNS tunneling for initial access and C2 communications.".into(),
        "Zero-day exploit CVE-2023-23397 targets Microsoft Outlook with no user interaction required.".into(),
        "Lateral movement via pass-the-hash attack detected using Mimikatz credential dumping tool.".into(),
        "The MITRE ATT&CK framework documents 14 tactics used by adversaries in cyber attacks.".into(),
        "Ransomware operators increasingly use double extortion: encrypt data AND threaten to leak it.".into(),
        "SIEM correlation rule triggered: 50+ failed logins followed by successful login from new IP.".into(),
    ]
}

/// Default evaluation questions: `(question, expected_substring)`.
pub fn default_questions() -> Vec<EvalQuestion> {
    vec![
        EvalQuestion {
            question: "What CVE is associated with the Log4Shell vulnerability?".into(),
            expected: "CVE-2021-44228".into(),
        },
        EvalQuestion {
            question: "Which threat actor is associated with APT29?".into(),
            expected: "Midnight Blizzard".into(),
        },
        EvalQuestion {
            question: "What happened in incident INC-2024-001?".into(),
            expected: "Ransomware encrypted 500 files".into(),
        },
        EvalQuestion {
            question: "How many documents did jsmith download?".into(),
            expected: "2,847".into(),
        },
        EvalQuestion {
            question: "What was the CVSS score of CVE-2021-44228?".into(),
            expected: "10.0".into(),
        },
        EvalQuestion {
            question: "Which malicious npm package was used in the supply chain attack?".into(),
            expected: "event-stream".into(),
        },
        EvalQuestion {
            question: "What IP address was the C2 server in INC-2024-002?".into(),
            expected: "185.220.101.45".into(),
        },
        EvalQuestion {
            question: "How were the encrypted files restored after INC-2024-001?".into(),
            expected: "restored from backup".into(),
        },
        EvalQuestion {
            question: "What is DNS tunneling used for in the APT29 campaign?".into(),
            expected: "exfiltrate data".into(),
        },
        EvalQuestion {
            question: "What security improvement was enforced after INC-2024-001?".into(),
            expected: "MFA".into(),
        },
    ]
}

/// Build a default `EvalConfig`.
pub fn default_eval_config(turns: usize, repeats: usize) -> EvalConfig {
    EvalConfig {
        turns,
        repeats,
        output: None,
        content_pool: default_content_pool(),
        questions: default_questions(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grade_exact_match() {
        let s = grade_answer("q", "CVE-2021-44228", "The CVE is CVE-2021-44228.");
        assert!((s - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn grade_partial_match() {
        let s = grade_answer("q", "Ransomware encrypted 500 files", "Ransomware attack");
        assert!(s > 0.0);
        assert!(s < 1.0);
    }

    #[test]
    fn grade_empty_actual() {
        assert_eq!(grade_answer("q", "expected", ""), 0.0);
    }

    #[test]
    fn grade_short_expected() {
        // Keywords shorter than 4 chars get filtered; fallback to substring.
        assert_eq!(grade_answer("q", "MFA", "MFA was enforced"), 1.0);
        assert_eq!(grade_answer("q", "MFA", "nothing here"), 0.0);
    }

    #[test]
    fn average_score_empty() {
        assert_eq!(average_score(&[]), 0.0);
    }

    #[test]
    fn average_score_values() {
        let qrs = vec![
            QuestionResult {
                question: "q1".into(),
                expected: "e1".into(),
                answer: "a1".into(),
                score: 1.0,
                elapsed_s: 0.1,
            },
            QuestionResult {
                question: "q2".into(),
                expected: "e2".into(),
                answer: "a2".into(),
                score: 0.5,
                elapsed_s: 0.2,
            },
        ];
        let avg = average_score(&qrs);
        assert!((avg - 0.75).abs() < 0.01);
    }

    #[test]
    fn aggregate_summary_basic() {
        let rr = RepeatResult {
            repeat: 1,
            turns: 10,
            learn_elapsed_s: 1.0,
            learn_throughput: 10.0,
            questions: Vec::new(),
            avg_score: 0.8,
        };
        let s = aggregate_summary("test_eval", 10, &[rr]);
        assert_eq!(s.repeats, 1);
        assert!((s.aggregate_avg_score - 0.8).abs() < 0.01);
    }

    #[test]
    fn default_pool_non_empty() {
        assert!(!default_content_pool().is_empty());
    }

    #[test]
    fn default_questions_non_empty() {
        assert!(!default_questions().is_empty());
    }

    #[test]
    fn default_config() {
        let cfg = default_eval_config(100, 3);
        assert_eq!(cfg.turns, 100);
        assert_eq!(cfg.repeats, 3);
    }
}
