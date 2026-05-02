//! N-Version Programming orchestrator.
//!
//! Native Rust port of `patterns/n_version.py`. Generates N independent
//! implementations in parallel, runs a reviewer pass to compare them, and
//! parses the reviewer's selection (`hybrid` or `version_<n>`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::claude_process::{ProcessResult, ProcessRunner};
use crate::execution::run_parallel;
use crate::session::OrchestratorSession;

/// Diversity profile applied to one of the N implementations.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub name: &'static str,
    pub description: &'static str,
    pub traits: &'static str,
}

/// Default 5 diversity profiles, in priority order.
pub static DEFAULT_PROFILES: &[Profile] = &[
    Profile {
        name: "conservative",
        description: "Focus on proven patterns and safety",
        traits: "Use proven design patterns, comprehensive error handling, defensive programming",
    },
    Profile {
        name: "pragmatic",
        description: "Balance trade-offs for practical solutions",
        traits: "Balance simplicity and robustness, standard library solutions, practical trade-offs",
    },
    Profile {
        name: "minimalist",
        description: "Prioritize ruthless simplicity",
        traits: "Ruthless simplification, minimal dependencies, direct implementation",
    },
    Profile {
        name: "innovative",
        description: "Explore novel approaches and optimizations",
        traits: "Explore novel approaches, consider optimizations, creative solutions",
    },
    Profile {
        name: "performance_focused",
        description: "Optimize for speed and efficiency",
        traits: "Optimize for speed and efficiency, consider resource usage, benchmark-driven",
    },
];

/// Default selection criteria, in priority order.
pub static DEFAULT_CRITERIA: &[&str] = &[
    "correctness",
    "security",
    "simplicity",
    "philosophy_compliance",
    "performance",
];

/// Outcome of an N-Version run.
#[derive(Debug, Clone)]
pub struct NVersionResult {
    pub versions: Vec<ProcessResult>,
    pub comparison: Option<ProcessResult>,
    pub selected: Option<String>,
    pub rationale: String,
    pub session_id: String,
    pub success: bool,
}

/// Execute the N-version programming pattern.
#[allow(clippy::too_many_arguments)]
pub async fn run_n_version(
    task_prompt: String,
    n: usize,
    model: Option<String>,
    working_dir: Option<PathBuf>,
    selection_criteria: Option<Vec<String>>,
    diversity_profiles: Option<Vec<Profile>>,
    timeout: Option<Duration>,
    runner: Arc<dyn ProcessRunner>,
) -> NVersionResult {
    let working_dir = working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let criteria: Vec<String> = selection_criteria
        .unwrap_or_else(|| DEFAULT_CRITERIA.iter().map(|s| (*s).to_string()).collect());
    let profiles: Vec<Profile> = diversity_profiles.unwrap_or_else(|| DEFAULT_PROFILES.to_vec());

    let mut session = OrchestratorSession::builder()
        .pattern_name("n-version")
        .working_dir(working_dir)
        .runner(runner)
        .also_set_model(model.clone())
        .build()
        .expect("session build");

    session.log_info(&format!("Starting N-Version Programming with N={n}"));
    session.log_info(&format!("Selection criteria: {}", criteria.join(", ")));

    // Step 1+2: spawn N implementations in parallel.
    let mut processes = Vec::with_capacity(n);
    for i in 0..n {
        let profile = profiles[i % profiles.len()];
        let prompt = build_impl_prompt(&task_prompt, n, i + 1, &profile);
        let pid = format!("version_{}_{}", i + 1, profile.name);
        let p = session
            .create_process(&prompt, Some(&pid), None, timeout)
            .expect("create_process");
        processes.push(p);
    }
    session.log_info(&format!("Executing {n} implementations in parallel..."));
    let version_results = run_parallel(processes, None).await;

    let successful: Vec<&ProcessResult> =
        version_results.iter().filter(|r| r.is_success()).collect();
    session.log_info(&format!(
        "Completed {}/{} implementations successfully",
        successful.len(),
        n
    ));

    if successful.is_empty() {
        session.log_error("All implementations failed");
        return NVersionResult {
            versions: version_results,
            comparison: None,
            selected: None,
            rationale: "All implementations failed to complete".to_string(),
            session_id: session.session_id().to_string(),
            success: false,
        };
    }

    // Step 3: reviewer comparison.
    let comparison_prompt =
        build_comparison_prompt(&task_prompt, &criteria, &version_results, &profiles);
    let reviewer_process = session
        .create_process(
            &comparison_prompt,
            Some("reviewer_comparison"),
            None,
            timeout,
        )
        .expect("create_process");
    let comparison_result = reviewer_process.run().await;

    if !comparison_result.is_success() {
        session.log_warn("Reviewer comparison failed, selecting first successful version");
        let idx = version_results
            .iter()
            .position(|v| v.is_success())
            .expect("at least one successful version present");
        return NVersionResult {
            versions: version_results,
            comparison: Some(comparison_result),
            selected: Some(format!("version_{}", idx + 1)),
            rationale: "Reviewer failed, selected first successful implementation".to_string(),
            session_id: session.session_id().to_string(),
            success: true,
        };
    }

    // Step 4: parse selection.
    let lower = comparison_result.output.to_lowercase();
    let mut selected: Option<String> = None;
    if lower.contains("hybrid") {
        selected = Some("hybrid".to_string());
    } else {
        for i in 0..n {
            let token_a = format!("version {}", i + 1);
            let token_b = format!("v{}", i + 1);
            if (lower.contains(&token_a) || lower.contains(&token_b))
                && ["select", "chosen", "best", "recommend"]
                    .iter()
                    .any(|kw| lower.contains(kw))
            {
                selected = Some(format!("version_{}", i + 1));
                break;
            }
        }
    }

    let rationale = if let Some(idx) = lower.find("## rationale") {
        let end = (idx + 1000).min(comparison_result.output.len());
        comparison_result.output[idx..end].to_string()
    } else {
        "See comparison output for full rationale".to_string()
    };

    let selected = selected.unwrap_or_else(|| {
        session.log_warn("Could not parse selection, falling back to first successful version");
        let idx = version_results
            .iter()
            .position(|v| v.is_success())
            .expect("at least one successful version present");
        format!("version_{}", idx + 1)
    });

    session.log_info(&format!("Selected: {selected}"));

    NVersionResult {
        versions: version_results,
        comparison: Some(comparison_result),
        selected: Some(selected),
        rationale,
        session_id: session.session_id().to_string(),
        success: true,
    }
}

fn build_impl_prompt(task: &str, n: usize, version_idx: usize, profile: &Profile) -> String {
    format!(
        "You are implementing a task using the N-Version Programming pattern.\n\n\
         TASK SPECIFICATION:\n{task}\n\n\
         CRITICAL REQUIREMENTS:\n\
         1. You are one of {n} independent implementations (Version {version_idx})\n\
         2. DO NOT consult or share context with other implementations\n\
         3. Produce a COMPLETE, WORKING implementation\n\
         4. Include tests that verify correctness\n\
         5. Document your approach and design decisions\n\
         6. Follow project philosophy: ruthless simplicity, zero-BS implementation\n\n\
         Your implementation approach should follow the \"{name}\" profile:\n{traits}\n\n\
         Deliver a complete implementation with:\n\
         - All code files needed\n\
         - Test files proving correctness\n\
         - Brief explanation of your approach\n\n\
         Begin implementation now.\n",
        task = task,
        n = n,
        version_idx = version_idx,
        name = profile.name,
        traits = profile.traits,
    )
}

fn build_comparison_prompt(
    task: &str,
    criteria: &[String],
    versions: &[ProcessResult],
    profiles: &[Profile],
) -> String {
    let mut summary = String::new();
    for (i, r) in versions.iter().enumerate() {
        let p = profiles[i % profiles.len()];
        let status = if r.is_success() { "SUCCESS" } else { "FAILED" };
        summary.push_str(&format!(
            "Version {} ({}): {}\nDuration: {:.1}s\nOutput length: {} chars\n\n",
            i + 1,
            p.name,
            status,
            r.duration.as_secs_f32(),
            r.output.len(),
        ));
    }

    let mut criteria_list = String::new();
    for (i, c) in criteria.iter().enumerate() {
        criteria_list.push_str(&format!("{}. {}\n", i + 1, c));
    }

    let mut prompt = format!(
        "You are a reviewer agent analyzing multiple implementations from N-Version Programming.\n\n\
         ORIGINAL TASK:\n{task}\n\n\
         SELECTION CRITERIA (in priority order):\n{criteria_list}\n\
         IMPLEMENTATIONS GENERATED:\n{summary}\nFULL OUTPUTS:\n",
    );

    for (i, r) in versions.iter().enumerate() {
        let p = profiles[i % profiles.len()];
        let bar = "=".repeat(80);
        let truncated = if r.output.len() > 5000 {
            "...(truncated)"
        } else {
            ""
        };
        let body = if r.output.len() > 5000 {
            &r.output[..5000]
        } else {
            r.output.as_str()
        };
        prompt.push_str(&format!(
            "\n\n{bar}\nVERSION {} ({}) - Exit Code: {}\n{bar}\n\n{}{}\n",
            i + 1,
            p.name,
            r.exit_code,
            body,
            truncated,
        ));
    }

    prompt.push_str(
        "\n\nYOUR TASK:\n\n\
         Analyze all implementations according to the selection criteria above.\n\n\
         Provide:\n\
         1. **Comparison Matrix** - Score each version on each criterion\n\
         2. **Analysis** - Detailed evaluation of each implementation\n\
         3. **Selection** - Which version to use (or \"HYBRID\" if synthesizing best parts)\n\
         4. **Rationale** - Clear explanation of why this selection\n\n\
         ## Selection\n[Version number or \"HYBRID\"]\n\n\
         ## Rationale\n[Clear explanation of selection decision]\n",
    );
    prompt
}

// Helper trait so n_version can pass an Option<String> model into the
// builder without an awkward branch. Implemented inline via extension.
trait BuilderModelExt: Sized {
    fn also_set_model(self, model: Option<String>) -> Self;
}

impl BuilderModelExt for crate::session::OrchestratorSessionBuilder {
    fn also_set_model(self, model: Option<String>) -> Self {
        match model {
            Some(m) => self.model(m),
            None => self,
        }
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn profiles_have_5_entries() {
        assert_eq!(DEFAULT_PROFILES.len(), 5);
    }

    #[test]
    fn criteria_have_5_entries() {
        assert_eq!(DEFAULT_CRITERIA.len(), 5);
    }

    #[test]
    fn impl_prompt_mentions_version_number_and_profile() {
        let p = build_impl_prompt("task", 3, 2, &DEFAULT_PROFILES[0]);
        assert!(p.contains("Version 2"));
        assert!(p.contains("conservative"));
    }
}
