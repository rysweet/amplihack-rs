//! Stage 1: Prompt analysis — extract goal, domain, constraints, and complexity.

use anyhow::{Result, bail};
use std::collections::HashMap;

use super::GoalDefinition;

pub(super) fn analyze_prompt(prompt: &str) -> Result<GoalDefinition> {
    if prompt.trim().is_empty() {
        bail!("Prompt cannot be empty");
    }

    let goal = extract_goal(prompt);
    let domain = classify_domain(prompt);
    let constraints = extract_constraints(prompt);
    let success_criteria = extract_success_criteria(prompt, &goal);
    let complexity = determine_complexity(prompt);
    let context = extract_context(prompt);

    Ok(GoalDefinition {
        raw_prompt: prompt.to_string(),
        goal,
        domain,
        complexity,
        constraints,
        success_criteria,
        context,
    })
}

fn extract_goal(prompt: &str) -> String {
    // Look for explicit markers first
    for line in prompt.lines() {
        let lower = line.to_lowercase();
        for prefix in &["goal:", "objective:", "aim:", "purpose:"] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                let s = rest.trim().to_string();
                if s.len() > 10 {
                    // Preserve original casing
                    let offset = line.len() - rest.len();
                    return line[offset..].trim().to_string();
                }
            }
        }
        // Markdown heading
        if let Some(heading) = line.strip_prefix("# ") {
            let s = heading.trim().to_string();
            if s.len() > 10 {
                return s;
            }
        }
    }
    // First sentence
    for punct in &['.', '!', '?'] {
        if let Some(idx) = prompt.find(*punct) {
            let s = prompt[..idx].trim().to_string();
            if s.len() > 10 {
                return s;
            }
        }
    }
    // First non-empty line
    prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .chars()
        .take(100)
        .collect()
}

pub(super) fn classify_domain(prompt: &str) -> String {
    let lower = prompt.to_lowercase();
    let domain_keywords: &[(&str, &[&str])] = &[
        (
            "data-processing",
            &[
                "data",
                "process",
                "transform",
                "analyze",
                "parse",
                "extract",
            ],
        ),
        (
            "security-analysis",
            &["security", "vulnerab", "audit", "scan", "threat", "exploit"],
        ),
        (
            "automation",
            &["automate", "schedule", "workflow", "trigger", "monitor"],
        ),
        (
            "testing",
            &["test", "validate", "verify", "check", "qa", "quality"],
        ),
        (
            "deployment",
            &["deploy", "release", "ship", "publish", "distribute"],
        ),
        (
            "monitoring",
            &["monitor", "alert", "track", "observe", "log"],
        ),
        (
            "integration",
            &["integrate", "connect", "api", "webhook", "sync"],
        ),
        (
            "reporting",
            &["report", "dashboard", "metric", "visualize", "summary"],
        ),
    ];

    let mut best = ("general", 0usize);
    for (domain, keywords) in domain_keywords {
        let score = keywords.iter().filter(|kw| lower.contains(**kw)).count();
        if score > best.1 {
            best = (domain, score);
        }
    }
    best.0.to_string()
}

fn extract_constraints(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in prompt.lines() {
        let lower = line.to_lowercase();
        for prefix in &[
            "constraint:",
            "requirement:",
            "must:",
            "must not ",
            "should not ",
            "cannot ",
        ] {
            if let Some(idx) = lower.find(prefix) {
                let rest = line[idx + prefix.len()..].trim().to_string();
                if rest.len() > 5 {
                    out.push(rest);
                    break;
                }
            }
        }
    }
    out.truncate(5);
    out
}

fn extract_success_criteria(prompt: &str, goal: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in prompt.lines() {
        let lower = line.to_lowercase();
        for prefix in &["output:", "result:", "outcome:"] {
            if lower.starts_with(prefix) {
                let rest = line[prefix.len()..].trim().to_string();
                if rest.len() > 5 {
                    out.push(rest);
                    break;
                }
            }
        }
    }
    if out.is_empty() {
        let snippet: String = goal.chars().take(50).collect();
        out.push(format!("Goal '{snippet}...' is achieved"));
    }
    out.truncate(5);
    out
}

pub(super) fn determine_complexity(prompt: &str) -> String {
    let lower = prompt.to_lowercase();
    let mut scores = [0i32; 3]; // simple, moderate, complex

    for kw in &["single", "one", "simple", "basic", "quick"] {
        if lower.contains(kw) {
            scores[0] += 1;
        }
    }
    for kw in &["multiple", "several", "coordinate", "orchestrate"] {
        if lower.contains(kw) {
            scores[1] += 1;
        }
    }
    for kw in &[
        "complex",
        "distributed",
        "multi-stage",
        "advanced",
        "sophisticated",
    ] {
        if lower.contains(kw) {
            scores[2] += 1;
        }
    }

    let words = lower.split_whitespace().count();
    if words < 50 {
        scores[0] += 2;
    } else if words < 150 {
        scores[1] += 2;
    } else {
        scores[2] += 2;
    }

    // phase/step heuristic
    if lower.contains("step ") || lower.contains("phase ") || lower.contains("stage ") {
        scores[2] += 1;
    }

    let max = *scores.iter().max().unwrap_or(&0);
    if max == 0 {
        return "moderate".to_string();
    }
    if scores[2] >= scores[1] && scores[2] >= scores[0] {
        "complex".to_string()
    } else if scores[0] > scores[1] {
        "simple".to_string()
    } else {
        "moderate".to_string()
    }
}

fn extract_context(prompt: &str) -> HashMap<String, String> {
    let lower = prompt.to_lowercase();
    let mut ctx = HashMap::new();

    if lower.contains("urgent") || lower.contains("asap") || lower.contains("critical") {
        ctx.insert("priority".to_string(), "high".to_string());
    } else if lower.contains("eventually") || lower.contains("someday") {
        ctx.insert("priority".to_string(), "low".to_string());
    } else {
        ctx.insert("priority".to_string(), "normal".to_string());
    }

    if lower.contains("large") || lower.contains("enterprise") || lower.contains("production") {
        ctx.insert("scale".to_string(), "large".to_string());
    } else if lower.contains("small") || lower.contains("minimal") {
        ctx.insert("scale".to_string(), "small".to_string());
    } else {
        ctx.insert("scale".to_string(), "medium".to_string());
    }

    ctx
}
