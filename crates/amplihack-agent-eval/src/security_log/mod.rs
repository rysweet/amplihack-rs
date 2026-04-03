//! Security log evaluation — MDE-style distributed threat detection.
//!
//! Ports Python `amplihack/eval/security_log_eval.py`:
//! - MITRE ATT&CK technique definitions
//! - Attack campaign generation
//! - Question generation with ground truth
//! - Precision/Recall/F1 grading
//!
//! Event generation is in the companion `security_log_data` module.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod data;

// ---------------------------------------------------------------------------
// MITRE ATT&CK technique catalog
// ---------------------------------------------------------------------------

/// Return the human-readable label for a MITRE technique ID.
pub fn technique_label(id: &str) -> &'static str {
    match id {
        "T1566.001" => "Phishing: Spearphishing Attachment",
        "T1059.001" => "Command and Scripting: PowerShell",
        "T1059.003" => "Command and Scripting: Windows Command Shell",
        "T1053.005" => "Scheduled Task",
        "T1547.001" => "Registry Run Keys / Startup Folder",
        "T1003.001" => "OS Credential Dumping: LSASS Memory",
        "T1021.001" => "Remote Services: RDP",
        "T1021.002" => "Remote Services: SMB/Windows Admin Shares",
        "T1021.006" => "Remote Services: Windows Remote Management",
        "T1070.001" => "Indicator Removal: Clear Windows Event Logs",
        "T1070.004" => "Indicator Removal: File Deletion",
        "T1105" => "Ingress Tool Transfer",
        "T1027" => "Obfuscated Files or Information",
        "T1569.002" => "System Services: Service Execution",
        "T1486" => "Data Encrypted for Impact",
        "T1048.003" => "Exfiltration Over Alternative Protocol",
        "T1071.001" => "Application Layer Protocol: Web Protocols",
        "T1082" => "System Information Discovery",
        "T1083" => "File and Directory Discovery",
        "T1057" => "Process Discovery",
        "T1018" => "Remote System Discovery",
        "T1087.002" => "Account Discovery: Domain Account",
        "T1560.001" => "Archive Collected Data: Archive via Utility",
        "T1036.005" => "Masquerading: Match Legitimate Name",
        "T1055.001" => "Process Injection: DLL Injection",
        "T1140" => "Deobfuscate/Decode Files or Information",
        "T1218.011" => "Rundll32",
        "T1543.003" => "Create or Modify System Process: Windows Service",
        "T1562.001" => "Impair Defenses: Disable or Modify Tools",
        "T1490" => "Inhibit System Recovery",
        _ => "Unknown Technique",
    }
}

/// Return the short keyword (before the colon) for a technique.
pub fn technique_keyword(id: &str) -> String {
    let label = technique_label(id);
    label.split(':').next().unwrap_or(label).trim().to_string()
}

/// Convert an objective slug to human-readable form.
pub fn objective_keyword(objective: &str) -> String {
    objective.replace('_', " ")
}

// ---------------------------------------------------------------------------
// Attack campaign
// ---------------------------------------------------------------------------

/// A multi-stage attack campaign with ground truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttackCampaign {
    pub campaign_id: String,
    pub name: String,
    pub threat_actor: String,
    pub start_day: u32,
    pub duration_days: u32,
    pub initial_access: String,
    pub techniques: Vec<String>,
    pub target_devices: Vec<String>,
    pub target_users: Vec<String>,
    pub c2_domains: Vec<String>,
    pub malware_hashes: Vec<String>,
    pub objective: String,
    pub iocs: HashMap<String, Vec<String>>,
    pub lateral_movement_path: Vec<String>,
    pub data_exfil_gb: f64,
    pub detected: bool,
    pub detection_delay_hours: u32,
}

/// Extract the actor short-name (before the parenthetical description).
pub fn actor_short_name(threat_actor: &str) -> &str {
    threat_actor
        .split('(')
        .next()
        .unwrap_or(threat_actor)
        .trim()
}

// ---------------------------------------------------------------------------
// Security question
// ---------------------------------------------------------------------------

/// A question with ground truth for grading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityQuestion {
    pub question_id: String,
    pub question: String,
    /// alert_retrieval, attack_chain, ioc_correlation, temporal, cross_campaign
    pub category: String,
    pub ground_truth_facts: Vec<String>,
    pub required_keywords: Vec<String>,
    pub campaign_ids: Vec<String>,
    /// easy, medium, hard
    pub difficulty: String,
}

// ---------------------------------------------------------------------------
// Grading
// ---------------------------------------------------------------------------

/// Grading result for a single question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityGradeResult {
    pub question_id: String,
    pub category: String,
    pub score: f64,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub matched_keywords: Vec<String>,
    pub missing_keywords: Vec<String>,
    pub answer_excerpt: String,
}

/// Grade an answer using keyword matching with precision/recall/F1.
///
/// Score weighting: 60% recall + 20% precision + 20% F1.
/// Precision penalizes hallucinated campaign IDs.
pub fn grade_answer(question: &SecurityQuestion, answer: &str) -> SecurityGradeResult {
    let answer_lower = answer.to_lowercase();
    let mut matched = Vec::new();
    let mut missing = Vec::new();

    for kw in &question.required_keywords {
        if answer_lower.contains(&kw.to_lowercase()) {
            matched.push(kw.clone());
        } else {
            missing.push(kw.clone());
        }
    }

    let total_required = question.required_keywords.len();
    let recall = if total_required == 0 {
        1.0
    } else {
        matched.len() as f64 / total_required as f64
    };

    // Precision: penalize hallucinated campaign IDs
    let mut mentioned_camps = Vec::new();
    let mut start = 0;
    while let Some(pos) = answer[start..].find("CAMP-") {
        let abs_pos = start + pos;
        let end = (abs_pos + 16).min(answer.len());
        mentioned_camps.push(&answer[abs_pos..end]);
        start = abs_pos + 1;
    }

    let precision = if !mentioned_camps.is_empty() {
        let correct = mentioned_camps
            .iter()
            .filter(|m| {
                question
                    .campaign_ids
                    .iter()
                    .any(|c| m.starts_with(c.as_str()))
            })
            .count();
        correct as f64 / mentioned_camps.len() as f64
    } else if recall > 0.0 {
        1.0
    } else {
        0.0
    };

    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    let score = 0.6 * recall + 0.2 * precision + 0.2 * f1;

    SecurityGradeResult {
        question_id: question.question_id.clone(),
        category: question.category.clone(),
        score,
        precision,
        recall,
        f1,
        matched_keywords: matched,
        missing_keywords: missing,
        answer_excerpt: answer.chars().take(200).collect(),
    }
}

// ---------------------------------------------------------------------------
// Eval report
// ---------------------------------------------------------------------------

/// Complete security evaluation report.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityEvalReport {
    pub overall_score: f64,
    pub overall_precision: f64,
    pub overall_recall: f64,
    pub overall_f1: f64,
    pub category_scores: HashMap<String, CategoryMetrics>,
    pub difficulty_scores: HashMap<String, f64>,
    pub num_questions: usize,
    pub num_turns: usize,
    pub num_campaigns: usize,
    pub learning_time_s: f64,
    pub grading_time_s: f64,
    #[serde(default)]
    pub results: Vec<SecurityGradeResult>,
}

/// Per-category aggregate metrics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CategoryMetrics {
    pub score: f64,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub count: usize,
}

impl SecurityEvalReport {
    /// Aggregate individual grade results into a report.
    pub fn aggregate(
        results: &[SecurityGradeResult],
        questions: &[SecurityQuestion],
        num_turns: usize,
        num_campaigns: usize,
        learning_time_s: f64,
        grading_time_s: f64,
    ) -> Self {
        let mut report = Self {
            num_questions: results.len(),
            num_turns,
            num_campaigns,
            learning_time_s,
            grading_time_s,
            results: results.to_vec(),
            ..Default::default()
        };

        if results.is_empty() {
            return report;
        }

        let n = results.len() as f64;
        report.overall_score = results.iter().map(|r| r.score).sum::<f64>() / n;
        report.overall_precision = results.iter().map(|r| r.precision).sum::<f64>() / n;
        report.overall_recall = results.iter().map(|r| r.recall).sum::<f64>() / n;
        report.overall_f1 = results.iter().map(|r| r.f1).sum::<f64>() / n;

        // By category
        let mut by_cat: HashMap<String, Vec<&SecurityGradeResult>> = HashMap::new();
        for r in results {
            by_cat.entry(r.category.clone()).or_default().push(r);
        }
        for (cat, cat_results) in &by_cat {
            let cn = cat_results.len() as f64;
            report.category_scores.insert(
                cat.clone(),
                CategoryMetrics {
                    score: cat_results.iter().map(|r| r.score).sum::<f64>() / cn,
                    precision: cat_results.iter().map(|r| r.precision).sum::<f64>() / cn,
                    recall: cat_results.iter().map(|r| r.recall).sum::<f64>() / cn,
                    f1: cat_results.iter().map(|r| r.f1).sum::<f64>() / cn,
                    count: cat_results.len(),
                },
            );
        }

        // By difficulty
        let mut by_diff: HashMap<String, Vec<&SecurityGradeResult>> = HashMap::new();
        for (q, r) in questions.iter().zip(results.iter()) {
            by_diff.entry(q.difficulty.clone()).or_default().push(r);
        }
        for (diff, diff_results) in &by_diff {
            report.difficulty_scores.insert(
                diff.clone(),
                diff_results.iter().map(|r| r.score).sum::<f64>() / diff_results.len() as f64,
            );
        }

        report
    }
}

// ---------------------------------------------------------------------------
// Question generation
// ---------------------------------------------------------------------------

pub mod questions;
pub use questions::generate_questions;

#[cfg(test)]
#[path = "../tests/security_log_tests.rs"]
mod tests;
