//! Fallback cascade orchestrator.
//!
//! Native Rust port of `patterns/cascade.py`. Attempts primary → secondary →
//! tertiary levels, returning at the first success and reporting degradation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use thiserror::Error;

use crate::claude_process::{ProcessResult, ProcessRunner};
use crate::session::OrchestratorSession;

/// Per-level timeout (seconds) for a cascade strategy.
#[derive(Debug, Clone, Copy)]
pub struct TimeoutSet {
    pub primary: u64,
    pub secondary: u64,
    pub tertiary: u64,
}

/// Per-level constraint string template.
#[derive(Debug, Clone, Copy)]
pub struct FallbackTemplate {
    pub primary: &'static str,
    pub secondary: &'static str,
    pub tertiary: &'static str,
}

pub static TIMEOUT_STRATEGIES: Lazy<HashMap<&'static str, TimeoutSet>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "aggressive",
        TimeoutSet {
            primary: 5,
            secondary: 2,
            tertiary: 1,
        },
    );
    m.insert(
        "balanced",
        TimeoutSet {
            primary: 30,
            secondary: 10,
            tertiary: 5,
        },
    );
    m.insert(
        "patient",
        TimeoutSet {
            primary: 120,
            secondary: 30,
            tertiary: 10,
        },
    );
    m
});

pub static FALLBACK_TEMPLATES: Lazy<HashMap<&'static str, FallbackTemplate>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "quality",
        FallbackTemplate {
            primary: "comprehensive and thorough analysis with all details",
            secondary: "standard analysis covering main points",
            tertiary: "minimal quick analysis of essential points only",
        },
    );
    m.insert(
        "service",
        FallbackTemplate {
            primary: "using optimal external service or API",
            secondary: "using cached or alternative service",
            tertiary: "using local defaults or fallback data",
        },
    );
    m.insert(
        "freshness",
        FallbackTemplate {
            primary: "with real-time current data",
            secondary: "with recent cached data (< 1 hour old)",
            tertiary: "with historical or default data",
        },
    );
    m.insert(
        "completeness",
        FallbackTemplate {
            primary: "processing full dataset completely",
            secondary: "processing representative sample (10-20%)",
            tertiary: "using precomputed summary statistics",
        },
    );
    m.insert(
        "accuracy",
        FallbackTemplate {
            primary: "with precise calculations and exact results",
            secondary: "with approximate results and estimations",
            tertiary: "with rough estimates and order-of-magnitude",
        },
    );
    m
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeLevel {
    Primary,
    Secondary,
    Tertiary,
    Failed,
    /// Custom level produced by `create_custom_cascade`.
    Custom,
}

#[derive(Debug, Error)]
pub enum CascadeError {
    #[error(
        "Unknown fallback_strategy '{0}'. Known: quality, service, freshness, completeness, accuracy"
    )]
    UnknownFallbackStrategy(String),
    #[error("Unknown timeout_strategy '{0}'. Known: aggressive, balanced, patient")]
    UnknownTimeoutStrategy(String),
}

/// Result of a cascade attempt.
#[derive(Debug, Clone)]
pub struct CascadeResult {
    pub result: Option<ProcessResult>,
    pub cascade_level: CascadeLevel,
    pub level_name: String,
    pub degradation: Option<String>,
    pub attempts: Vec<ProcessResult>,
    pub session_id: String,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct CustomLevel {
    pub name: String,
    pub timeout: Duration,
    pub constraint: String,
    pub model: Option<String>,
}

/// Internal step descriptor used by the unified executor — abstracts over
/// both the predefined three-level cascade and arbitrary custom levels.
struct CascadeStep {
    name: String,
    pid: String,
    prompt: String,
    timeout: Duration,
    model: Option<String>,
    /// `CascadeLevel` to attach to the result when this step succeeds.
    level_enum: CascadeLevel,
    /// Degradation string to record when this step succeeds. `None` for the
    /// first (non-degraded) step.
    degradation_msg: Option<String>,
}

/// Execute the cascading fallback pattern with predefined strategies.
#[allow(clippy::too_many_arguments)]
pub async fn run_cascade(
    task_prompt: String,
    fallback_strategy: String,
    timeout_strategy: String,
    models: Option<Vec<String>>,
    working_dir: Option<PathBuf>,
    notification_level: String,
    custom_timeouts: Option<TimeoutSet>,
    custom_constraints: Option<FallbackTemplate>,
    runner: Arc<dyn ProcessRunner>,
) -> Result<CascadeResult, CascadeError> {
    let template = match custom_constraints {
        Some(t) => t,
        None => *FALLBACK_TEMPLATES
            .get(fallback_strategy.as_str())
            .ok_or_else(|| CascadeError::UnknownFallbackStrategy(fallback_strategy.clone()))?,
    };
    let timeouts = match custom_timeouts {
        Some(t) => t,
        None => *TIMEOUT_STRATEGIES
            .get(timeout_strategy.as_str())
            .ok_or_else(|| CascadeError::UnknownTimeoutStrategy(timeout_strategy.clone()))?,
    };

    let working_dir = working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut builder = OrchestratorSession::builder()
        .pattern_name("cascade")
        .working_dir(working_dir);
    if let Some(m) = models.as_ref().and_then(|v| v.first()).cloned() {
        builder = builder.model(m);
    }
    let mut session = builder.runner(runner).build().expect("session build");

    session.log_info(&format!(
        "Starting Cascade Workflow with fallback strategy: {fallback_strategy}"
    ));
    session.log_info(&format!("Timeout strategy: {timeout_strategy}"));

    let levels: [(&str, u64, &str, CascadeLevel); 3] = [
        (
            "primary",
            timeouts.primary,
            template.primary,
            CascadeLevel::Primary,
        ),
        (
            "secondary",
            timeouts.secondary,
            template.secondary,
            CascadeLevel::Secondary,
        ),
        (
            "tertiary",
            timeouts.tertiary,
            template.tertiary,
            CascadeLevel::Tertiary,
        ),
    ];

    let steps: Vec<CascadeStep> = levels
        .iter()
        .enumerate()
        .map(|(i, (name, t_secs, constraint, level_enum))| {
            let degradation_msg = match *name {
                "secondary" => Some(format!(
                    "Degraded from primary to secondary: {}",
                    template.secondary
                )),
                "tertiary" => Some(format!(
                    "Degraded to tertiary (minimal): {}",
                    template.tertiary
                )),
                _ => None,
            };
            CascadeStep {
                name: (*name).to_string(),
                pid: format!("cascade_{name}"),
                prompt: build_cascade_prompt(&task_prompt, name, constraint, *name == "tertiary"),
                timeout: Duration::from_secs(*t_secs),
                model: models.as_ref().and_then(|m| m.get(i)).cloned(),
                level_enum: *level_enum,
                degradation_msg,
            }
        })
        .collect();

    Ok(execute_cascade_steps(&mut session, steps, &notification_level).await)
}

/// Execute a cascade with arbitrary user-defined levels.
pub async fn create_custom_cascade(
    task_prompt: String,
    levels: Vec<CustomLevel>,
    working_dir: Option<PathBuf>,
    notification_level: String,
    runner: Arc<dyn ProcessRunner>,
) -> Result<CascadeResult, CascadeError> {
    let working_dir = working_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut builder = OrchestratorSession::builder()
        .pattern_name("cascade-custom")
        .working_dir(working_dir);
    if let Some(m) = levels.first().and_then(|l| l.model.clone()) {
        builder = builder.model(m);
    }
    let mut session = builder.runner(runner).build().expect("session build");
    session.log_info(&format!(
        "Starting Custom Cascade with {} levels",
        levels.len()
    ));

    let total = levels.len();
    let steps: Vec<CascadeStep> = levels
        .iter()
        .enumerate()
        .map(|(i, level)| {
            let degradation_msg = if i > 0 {
                Some(format!(
                    "Degraded to level {} ({}): {}",
                    i + 1,
                    level.name,
                    level.constraint
                ))
            } else {
                None
            };
            CascadeStep {
                name: level.name.clone(),
                pid: format!("cascade_{}", level.name),
                prompt: build_custom_cascade_prompt(&task_prompt, level, i + 1, total),
                timeout: level.timeout,
                model: level.model.clone(),
                level_enum: CascadeLevel::Custom,
                degradation_msg,
            }
        })
        .collect();

    Ok(execute_cascade_steps(&mut session, steps, &notification_level).await)
}

/// Drive a sequence of cascade steps: stop at first success, collect all
/// attempts, surface degradation. Shared by both `run_cascade` and
/// `create_custom_cascade`.
async fn execute_cascade_steps(
    session: &mut OrchestratorSession,
    steps: Vec<CascadeStep>,
    notification_level: &str,
) -> CascadeResult {
    let mut attempts = Vec::new();

    for step in steps {
        let process = session
            .create_process(
                &step.prompt,
                Some(&step.pid),
                step.model.as_deref(),
                Some(step.timeout),
            )
            .expect("create_process");
        session.log_info(&format!(
            "Attempting {} level (timeout: {:?})",
            step.name.to_uppercase(),
            step.timeout
        ));
        let result = process.run().await;
        let succeeded = result.is_success();
        attempts.push(result.clone());

        if succeeded {
            session.log_info(&format!("{} level succeeded!", step.name.to_uppercase()));
            if let Some(d) = &step.degradation_msg
                && notification_level != "silent"
            {
                session.log_warn(&format!("Degradation: {d}"));
            }
            return CascadeResult {
                result: Some(result),
                cascade_level: step.level_enum,
                level_name: step.name,
                degradation: step.degradation_msg,
                attempts,
                session_id: session.session_id().to_string(),
                success: true,
            };
        }

        if result.exit_code == -1 {
            session.log_warn(&format!("{} level timed out", step.name.to_uppercase()));
        } else {
            session.log_warn(&format!(
                "{} level failed with exit code {}",
                step.name.to_uppercase(),
                result.exit_code
            ));
        }
    }

    session.log_error("All cascade levels failed");
    let last = attempts.last().cloned();
    CascadeResult {
        result: last,
        cascade_level: CascadeLevel::Failed,
        level_name: "failed".to_string(),
        degradation: Some("All cascade levels failed".to_string()),
        attempts,
        session_id: session.session_id().to_string(),
        success: false,
    }
}

fn build_cascade_prompt(task: &str, level: &str, constraint: &str, is_final: bool) -> String {
    let final_msg = if is_final {
        "This is the FINAL fallback - you MUST complete successfully"
    } else {
        "If you cannot complete in time, a fallback will be attempted"
    };
    format!(
        "You are executing a task with cascading fallback support.\n\n\
         TASK:\n{task}\n\n\
         CASCADE LEVEL: {level_upper}\nCONSTRAINT: {constraint}\n\n\
         IMPORTANT:\n\
         - This is the {level_upper} attempt in a cascade\n\
         - You should aim for {constraint}\n\
         - Focus on completing within the time constraint\n\
         - {final_msg}\n\n\
         Execute the task now with the {level} approach.\n",
        level = level,
        level_upper = level.to_uppercase(),
        constraint = constraint,
        final_msg = final_msg,
    )
}

fn build_custom_cascade_prompt(
    task: &str,
    level: &CustomLevel,
    idx: usize,
    total: usize,
) -> String {
    let final_msg = if idx == total {
        "This is the FINAL fallback - you MUST complete successfully"
    } else {
        "If you cannot complete in time, a fallback will be attempted"
    };
    format!(
        "You are executing a task with cascading fallback support.\n\n\
         TASK:\n{task}\n\n\
         CASCADE LEVEL: {level_upper}\nCONSTRAINT: {constraint}\n\n\
         IMPORTANT:\n\
         - This is level {idx} of {total} in the cascade\n\
         - You should aim for {constraint}\n\
         - Focus on completing within the time constraint\n\
         - {final_msg}\n\n\
         Execute the task now with the {level_name} approach.\n",
        level_name = level.name,
        level_upper = level.name.to_uppercase(),
        constraint = level.constraint,
        final_msg = final_msg,
    )
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn timeout_strategies_have_three() {
        assert_eq!(TIMEOUT_STRATEGIES.len(), 3);
    }
    #[test]
    fn templates_have_five() {
        assert_eq!(FALLBACK_TEMPLATES.len(), 5);
    }
    #[test]
    fn cascade_prompt_contains_uppercase_level() {
        let p = build_cascade_prompt("t", "primary", "c", false);
        assert!(p.contains("PRIMARY"));
    }
    #[test]
    fn custom_prompt_contains_uppercase_name() {
        let l = CustomLevel {
            name: "quick".into(),
            timeout: Duration::from_secs(1),
            constraint: "fast".into(),
            model: None,
        };
        let p = build_custom_cascade_prompt("t", &l, 2, 3);
        assert!(p.contains("QUICK"));
    }
}
