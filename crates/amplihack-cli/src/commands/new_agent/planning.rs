//! Stage 2: Execution plan generation — phases, skills, durations, and risks.

use super::{ExecutionPlan, GoalDefinition, PlanPhase};

pub(super) fn generate_plan(goal: &GoalDefinition) -> ExecutionPlan {
    let templates: &[(&str, &str, &[&str])] = match goal.domain.as_str() {
        "data-processing" => &[
            (
                "Data Collection",
                "Gather required data from sources",
                &["data-ingestion", "validation"],
            ),
            (
                "Data Transformation",
                "Transform and clean data",
                &["parsing", "transformation"],
            ),
            (
                "Data Analysis",
                "Analyze processed data",
                &["analysis", "pattern-detection"],
            ),
            (
                "Report Generation",
                "Generate results and reports",
                &["reporting", "visualization"],
            ),
        ],
        "security-analysis" => &[
            (
                "Reconnaissance",
                "Scan and identify targets",
                &["scanning", "enumeration"],
            ),
            (
                "Vulnerability Detection",
                "Detect potential vulnerabilities",
                &["vulnerability-scanning", "analysis"],
            ),
            (
                "Risk Assessment",
                "Assess and prioritize risks",
                &["risk-analysis", "scoring"],
            ),
            (
                "Reporting",
                "Generate security report",
                &["reporting", "documentation"],
            ),
        ],
        "automation" => &[
            (
                "Setup",
                "Configure automation environment",
                &["configuration", "initialization"],
            ),
            (
                "Workflow Design",
                "Design automation workflow",
                &["workflow-design", "orchestration"],
            ),
            (
                "Execution",
                "Execute automated tasks",
                &["task-execution", "monitoring"],
            ),
            (
                "Validation",
                "Validate results",
                &["validation", "quality-check"],
            ),
        ],
        "testing" => &[
            (
                "Test Planning",
                "Plan test strategy",
                &["test-design", "planning"],
            ),
            (
                "Test Implementation",
                "Implement test cases",
                &["test-coding", "framework-setup"],
            ),
            (
                "Test Execution",
                "Run test suite",
                &["test-execution", "automation"],
            ),
            (
                "Results Analysis",
                "Analyze test results",
                &["analysis", "reporting"],
            ),
        ],
        "deployment" => &[
            (
                "Pre-deployment",
                "Prepare for deployment",
                &["validation", "backup"],
            ),
            (
                "Deployment",
                "Deploy to target environment",
                &["deployment", "monitoring"],
            ),
            (
                "Verification",
                "Verify deployment success",
                &["verification", "health-check"],
            ),
            (
                "Post-deployment",
                "Complete deployment tasks",
                &["cleanup", "documentation"],
            ),
        ],
        "monitoring" => &[
            (
                "Setup Monitors",
                "Configure monitoring",
                &["configuration", "instrumentation"],
            ),
            (
                "Data Collection",
                "Collect metrics and logs",
                &["data-collection", "aggregation"],
            ),
            (
                "Analysis",
                "Analyze monitoring data",
                &["analysis", "anomaly-detection"],
            ),
            ("Alerting", "Set up alerts", &["alerting", "notification"]),
        ],
        _ => &[
            (
                "Planning",
                "Plan approach and strategy",
                &["planning", "analysis"],
            ),
            (
                "Implementation",
                "Implement solution",
                &["coding", "configuration"],
            ),
            ("Testing", "Test implementation", &["testing", "validation"]),
            (
                "Deployment",
                "Deploy solution",
                &["deployment", "verification"],
            ),
        ],
    };

    let phase_dur = match goal.complexity.as_str() {
        "simple" => "5 minutes",
        "complex" => "30 minutes",
        _ => "15 minutes",
    };

    let mut phases: Vec<PlanPhase> = Vec::new();
    for (i, (name, desc, caps)) in templates.iter().enumerate() {
        let prev = if i > 0 {
            vec![phases[i - 1].name.clone()]
        } else {
            vec![]
        };
        let indicators: Vec<String> = caps
            .iter()
            .take(2)
            .map(|c| {
                let mut s = c.replace('-', " ");
                if let Some(ch) = s.get_mut(0..1) {
                    ch.make_ascii_uppercase();
                }
                format!("{s} completed successfully")
            })
            .collect();
        phases.push(PlanPhase {
            name: name.to_string(),
            description: desc.to_string(),
            required_capabilities: caps.iter().map(|s| s.to_string()).collect(),
            estimated_duration: phase_dur.to_string(),
            dependencies: prev,
            parallel_safe: i > 0 && i < templates.len() - 1,
            success_indicators: indicators,
        });
    }
    phases.truncate(5);

    let required_skills = calculate_required_skills(&phases);
    let total_duration = estimate_total_duration(&phases, &goal.complexity);
    let risk_factors = identify_risk_factors(goal);

    ExecutionPlan {
        phases,
        total_duration,
        required_skills,
        parallel_opportunities: vec![],
        risk_factors,
    }
}

fn calculate_required_skills(phases: &[PlanPhase]) -> Vec<String> {
    let mut skills = std::collections::BTreeSet::new();
    for phase in phases {
        for cap in &phase.required_capabilities {
            if cap.contains("data") {
                skills.insert("data-processor");
            } else if cap.contains("security") || cap.contains("vulnerability") {
                skills.insert("security-analyzer");
            } else if cap.contains("test") {
                skills.insert("tester");
            } else if cap.contains("deploy") {
                skills.insert("deployer");
            } else if cap.contains("monitor") || cap.contains("alert") {
                skills.insert("monitor");
            } else if cap.contains("report") || cap.contains("document") {
                skills.insert("documenter");
            } else {
                skills.insert("generic-executor");
            }
        }
    }
    skills.into_iter().map(|s| s.to_string()).collect()
}

fn estimate_total_duration(phases: &[PlanPhase], complexity: &str) -> String {
    let per_phase = match complexity {
        "simple" => 5u32,
        "complex" => 30,
        _ => 15,
    };
    let overhead = match complexity {
        "simple" => 110u32,
        "complex" => 130,
        _ => 120,
    };
    let total = (phases.len() as u32 * per_phase * overhead) / 100;
    if total < 60 {
        format!("{total} minutes")
    } else {
        let h = total / 60;
        let m = total % 60;
        if m > 0 {
            format!("{h} hour{} {m} minutes", if h > 1 { "s" } else { "" })
        } else {
            format!("{h} hour{}", if h > 1 { "s" } else { "" })
        }
    }
}

fn identify_risk_factors(goal: &GoalDefinition) -> Vec<String> {
    let mut risks = Vec::new();
    if goal.complexity == "complex" {
        risks.push("High complexity may require extended execution time".to_string());
    }
    let domain_risk = match goal.domain.as_str() {
        "security-analysis" => {
            Some("Scan may identify critical vulnerabilities requiring immediate action")
        }
        "deployment" => Some("Deployment errors could affect production systems"),
        "data-processing" => Some("Large data volumes may cause performance issues"),
        "automation" => Some("Automated changes may have unintended side effects"),
        _ => None,
    };
    if let Some(r) = domain_risk {
        risks.push(r.to_string());
    }
    if risks.is_empty() {
        risks.push("Standard execution risks apply".to_string());
    }
    risks
}
