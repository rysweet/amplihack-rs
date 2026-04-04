//! Tests for security_log module.

use super::data;
use super::*;

#[test]
fn technique_label_known() {
    assert_eq!(
        technique_label("T1566.001"),
        "Phishing: Spearphishing Attachment"
    );
    assert_eq!(technique_label("T1486"), "Data Encrypted for Impact");
}

#[test]
fn technique_label_unknown() {
    assert_eq!(technique_label("T9999"), "Unknown Technique");
}

#[test]
fn technique_keyword_extracts_prefix() {
    let kw = technique_keyword("T1566.001");
    assert_eq!(kw, "Phishing");
}

#[test]
fn objective_keyword_formatting() {
    assert_eq!(objective_keyword("data_exfiltration"), "data exfiltration");
    assert_eq!(objective_keyword("ransomware"), "ransomware");
}

#[test]
fn actor_short_name_extraction() {
    assert_eq!(
        actor_short_name("APT-BEAR (Nation-state: Eastern European)"),
        "APT-BEAR"
    );
    assert_eq!(actor_short_name("SIMPLE"), "SIMPLE");
}

#[test]
fn grade_answer_perfect_recall() {
    let q = SecurityQuestion {
        question_id: "SEC-0001".into(),
        question: "What devices?".into(),
        category: "alert_retrieval".into(),
        ground_truth_facts: vec![],
        required_keywords: vec!["WS-FIN-001".into(), "SRV-DC-01".into()],
        campaign_ids: vec!["CAMP-2024-001".into()],
        difficulty: "easy".into(),
    };
    let result = grade_answer(&q, "The devices were WS-FIN-001 and SRV-DC-01");
    assert!((result.recall - 1.0).abs() < f64::EPSILON);
    assert!(result.score > 0.8);
}

#[test]
fn grade_answer_partial_recall() {
    let q = SecurityQuestion {
        question_id: "SEC-0002".into(),
        question: "What devices?".into(),
        category: "alert_retrieval".into(),
        ground_truth_facts: vec![],
        required_keywords: vec!["device-a".into(), "device-b".into()],
        campaign_ids: vec!["CAMP-2024-001".into()],
        difficulty: "easy".into(),
    };
    let result = grade_answer(&q, "Only device-a was found");
    assert!((result.recall - 0.5).abs() < f64::EPSILON);
}

#[test]
fn grade_answer_hallucinated_campaign() {
    let q = SecurityQuestion {
        question_id: "SEC-0003".into(),
        question: "Which campaigns?".into(),
        category: "cross_campaign".into(),
        ground_truth_facts: vec![],
        required_keywords: vec!["CAMP-2024-001".into()],
        campaign_ids: vec!["CAMP-2024-001".into()],
        difficulty: "hard".into(),
    };
    // Mentions correct campaign AND a hallucinated one
    let result = grade_answer(
        &q,
        "Found CAMP-2024-001 and also CAMP-2025-999 were involved",
    );
    assert!(result.precision < 1.0);
    assert!((result.recall - 1.0).abs() < f64::EPSILON);
}

#[test]
fn grade_answer_empty() {
    let q = SecurityQuestion {
        question_id: "SEC-0004".into(),
        question: "test".into(),
        category: "alert_retrieval".into(),
        ground_truth_facts: vec![],
        required_keywords: vec!["keyword".into()],
        campaign_ids: vec![],
        difficulty: "easy".into(),
    };
    let result = grade_answer(&q, "");
    assert!((result.recall).abs() < f64::EPSILON);
    assert!((result.score).abs() < f64::EPSILON);
}

#[test]
fn grade_answer_no_keywords_required() {
    let q = SecurityQuestion {
        question_id: "SEC-0005".into(),
        question: "test".into(),
        category: "alert_retrieval".into(),
        ground_truth_facts: vec![],
        required_keywords: vec![],
        campaign_ids: vec![],
        difficulty: "easy".into(),
    };
    let result = grade_answer(&q, "any answer");
    assert!((result.recall - 1.0).abs() < f64::EPSILON);
}

#[test]
fn security_eval_report_aggregate() {
    let questions = vec![
        SecurityQuestion {
            question_id: "Q1".into(),
            question: "q1".into(),
            category: "alert_retrieval".into(),
            ground_truth_facts: vec![],
            required_keywords: vec!["kw1".into()],
            campaign_ids: vec![],
            difficulty: "easy".into(),
        },
        SecurityQuestion {
            question_id: "Q2".into(),
            question: "q2".into(),
            category: "temporal".into(),
            ground_truth_facts: vec![],
            required_keywords: vec!["kw2".into()],
            campaign_ids: vec![],
            difficulty: "hard".into(),
        },
    ];
    let results = vec![
        grade_answer(&questions[0], "kw1 found"),
        grade_answer(&questions[1], "no match"),
    ];
    let report = SecurityEvalReport::aggregate(&results, &questions, 100, 2, 1.0, 0.5);
    assert_eq!(report.num_questions, 2);
    assert!(report.category_scores.contains_key("alert_retrieval"));
    assert!(report.category_scores.contains_key("temporal"));
    assert!(report.difficulty_scores.contains_key("easy"));
    assert!(report.difficulty_scores.contains_key("hard"));
}

#[test]
fn security_eval_report_empty() {
    let report = SecurityEvalReport::aggregate(&[], &[], 0, 0, 0.0, 0.0);
    assert!((report.overall_score).abs() < f64::EPSILON);
}

#[test]
fn generate_questions_from_campaigns() {
    let campaigns = data::generate_campaigns(42, 3);
    let questions = generate_questions(&campaigns, 20);
    assert!(!questions.is_empty());
    // Should have at least 5 questions per campaign (5 types × 3 campaigns = 15)
    assert!(questions.len() >= 15);
}

#[test]
fn generate_questions_respects_max() {
    let campaigns = data::generate_campaigns(42, 12);
    let questions = generate_questions(&campaigns, 10);
    assert!(questions.len() <= 10);
}

#[test]
fn attack_campaign_serde() {
    let campaigns = data::generate_campaigns(42, 1);
    let json = serde_json::to_string(&campaigns[0]).unwrap();
    let camp: AttackCampaign = serde_json::from_str(&json).unwrap();
    assert_eq!(camp.campaign_id, campaigns[0].campaign_id);
}

#[test]
fn security_question_serde() {
    let q = SecurityQuestion {
        question_id: "SEC-0001".into(),
        question: "test question".into(),
        category: "temporal".into(),
        ground_truth_facts: vec!["fact1".into()],
        required_keywords: vec!["keyword".into()],
        campaign_ids: vec!["CAMP-2024-001".into()],
        difficulty: "hard".into(),
    };
    let json = serde_json::to_string(&q).unwrap();
    let q2: SecurityQuestion = serde_json::from_str(&json).unwrap();
    assert_eq!(q2.question_id, "SEC-0001");
    assert_eq!(q2.difficulty, "hard");
}

#[test]
fn f1_score_calculation() {
    let q = SecurityQuestion {
        question_id: "Q".into(),
        question: "test".into(),
        category: "test".into(),
        ground_truth_facts: vec![],
        required_keywords: vec!["a".into(), "b".into()],
        campaign_ids: vec!["CAMP-2024-001".into()],
        difficulty: "easy".into(),
    };
    let result = grade_answer(&q, "a is found, also CAMP-2024-001");
    // recall = 0.5, precision = 1.0 (correct campaign mentioned)
    // f1 = 2 * 0.5 * 1.0 / 1.5 ≈ 0.667
    assert!(result.f1 > 0.6);
    assert!(result.f1 < 0.7);
}

#[test]
fn category_metrics_default() {
    let m = CategoryMetrics::default();
    assert!((m.score).abs() < f64::EPSILON);
    assert_eq!(m.count, 0);
}
