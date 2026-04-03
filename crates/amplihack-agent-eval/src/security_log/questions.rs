//! Question generation for security log evaluations.

use super::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Question generation
// ---------------------------------------------------------------------------

/// Generate questions testing retrieval capabilities across campaigns.
pub fn generate_questions(campaigns: &[AttackCampaign], max_questions: usize) -> Vec<SecurityQuestion> {
    let mut questions: Vec<SecurityQuestion> = Vec::new();
    let mut qid = 0u32;

    for camp in campaigns {
        // Alert retrieval (easy)
        qid += 1;
        questions.push(SecurityQuestion {
            question_id: format!("SEC-{qid:04}"),
            question: format!("What devices were targeted in campaign {}?", camp.campaign_id),
            category: "alert_retrieval".into(),
            ground_truth_facts: camp
                .target_devices
                .iter()
                .map(|d| format!("{} {d}", camp.campaign_id))
                .collect(),
            required_keywords: camp.target_devices.iter().take(3).cloned().collect(),
            campaign_ids: vec![camp.campaign_id.clone()],
            difficulty: "easy".into(),
        });

        // Attack chain reconstruction (medium)
        qid += 1;
        questions.push(SecurityQuestion {
            question_id: format!("SEC-{qid:04}"),
            question: format!(
                "Describe the lateral movement path in campaign {}. Which devices were compromised in order?",
                camp.campaign_id
            ),
            category: "attack_chain".into(),
            ground_truth_facts: camp.lateral_movement_path[1..]
                .iter()
                .map(|d| format!("{} lateral movement to {d}", camp.campaign_id))
                .collect(),
            required_keywords: camp.lateral_movement_path.iter().take(3).cloned().collect(),
            campaign_ids: vec![camp.campaign_id.clone()],
            difficulty: "medium".into(),
        });

        // IOC correlation (medium)
        qid += 1;
        let mut ioc_kw: Vec<String> = camp
            .iocs
            .get("ip")
            .map(|ips| ips.iter().take(2).cloned().collect())
            .unwrap_or_default();
        if let Some(h) = camp.malware_hashes.first() {
            ioc_kw.push(h.chars().take(16).collect());
        }
        questions.push(SecurityQuestion {
            question_id: format!("SEC-{qid:04}"),
            question: format!(
                "What are the IOCs (IP addresses and file hashes) associated with campaign {}?",
                camp.campaign_id
            ),
            category: "ioc_correlation".into(),
            ground_truth_facts: camp
                .iocs
                .get("ip")
                .map(|ips| {
                    ips.iter()
                        .take(2)
                        .map(|ip| format!("{} C2 connection to {ip}", camp.campaign_id))
                        .collect()
                })
                .unwrap_or_default(),
            required_keywords: ioc_kw,
            campaign_ids: vec![camp.campaign_id.clone()],
            difficulty: "medium".into(),
        });

        // Temporal reasoning (hard)
        qid += 1;
        questions.push(SecurityQuestion {
            question_id: format!("SEC-{qid:04}"),
            question: format!(
                "What was the sequence of MITRE ATT&CK techniques used in campaign {}? List in chronological order.",
                camp.campaign_id
            ),
            category: "temporal".into(),
            ground_truth_facts: camp.techniques.iter().take(4)
                .map(|t| format!("{} technique {t}", camp.campaign_id))
                .collect(),
            required_keywords: camp.techniques.iter().take(3)
                .map(|t| technique_keyword(t))
                .collect(),
            campaign_ids: vec![camp.campaign_id.clone()],
            difficulty: "hard".into(),
        });

        // Objective (easy)
        qid += 1;
        questions.push(SecurityQuestion {
            question_id: format!("SEC-{qid:04}"),
            question: format!(
                "What was the objective of campaign {}?",
                camp.campaign_id
            ),
            category: "alert_retrieval".into(),
            ground_truth_facts: vec![format!("{} objective: {}", camp.campaign_id, camp.objective)],
            required_keywords: vec![objective_keyword(&camp.objective)],
            campaign_ids: vec![camp.campaign_id.clone()],
            difficulty: "easy".into(),
        });

        if questions.len() >= max_questions {
            break;
        }
    }

    // Cross-campaign questions
    let mut actors: HashMap<String, Vec<&AttackCampaign>> = HashMap::new();
    for camp in campaigns {
        let actor = actor_short_name(&camp.threat_actor).to_string();
        actors.entry(actor).or_default().push(camp);
    }
    for (actor, actor_camps) in &actors {
        if actor_camps.len() >= 2 && questions.len() < max_questions {
            qid += 1;
            let camp_ids: Vec<String> = actor_camps.iter().take(3).map(|c| c.campaign_id.clone()).collect();
            questions.push(SecurityQuestion {
                question_id: format!("SEC-{qid:04}"),
                question: format!(
                    "Which campaigns are attributed to {actor}? What common techniques did they use?"
                ),
                category: "cross_campaign".into(),
                ground_truth_facts: camp_ids.iter()
                    .map(|cid| format!("{cid} threat actor {actor}"))
                    .collect(),
                required_keywords: camp_ids.iter().take(2).cloned().collect(),
                campaign_ids: camp_ids,
                difficulty: "hard".into(),
            });
        }
    }

    questions.truncate(max_questions);
    questions
}

