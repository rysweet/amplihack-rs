//! Native Rust implementation of `amplihack new`.
//!
//! Implements the full goal-agent-generator pipeline locally without delegating
//! to the Python runtime. The pipeline mirrors the Python implementation in
//! `amplihack/goal_agent_generator/` with the same CLI surface and output layout.

mod analysis;
mod bundle;
pub mod documentation;
pub mod error;
pub mod generator;
mod generator_templates;
pub mod models;
mod packaging;
mod planning;
mod skills;
mod templates;

#[cfg(test)]
mod tests;

use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use analysis::analyze_prompt;
use bundle::assemble_bundle;
use packaging::package_bundle;
use planning::generate_plan;
use skills::{get_sdk_tools, synthesize_skills};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run `amplihack new` natively.
#[allow(clippy::too_many_arguments)]
pub fn run_new(
    file: &Path,
    output: Option<&Path>,
    name: Option<&str>,
    skills_dir: Option<&Path>,
    verbose: bool,
    enable_memory: bool,
    sdk: &str,
    multi_agent: bool,
    enable_spawning: bool,
) -> Result<()> {
    // --enable-spawning auto-enables multi-agent with a warning
    let multi_agent = if enable_spawning && !multi_agent {
        eprintln!(
            "Warning: --enable-spawning has no effect without --multi-agent. \
             Adding --multi-agent automatically."
        );
        true
    } else {
        multi_agent
    };

    println!("\nGenerating goal agent from: {}", file.display());
    let start = Instant::now();

    // --- Stage 1: Analyze prompt ------------------------------------------
    println!("\n[1/4] Analyzing goal prompt...");
    let raw_prompt = fs::read_to_string(file)
        .with_context(|| format!("cannot read prompt file: {}", file.display()))?;
    let goal_def = analyze_prompt(&raw_prompt)?;

    println!("  Goal: {}", goal_def.goal);
    println!("  Domain: {}", goal_def.domain);
    println!("  Complexity: {}", goal_def.complexity);
    if verbose {
        if !goal_def.constraints.is_empty() {
            println!("  Constraints: {:?}", goal_def.constraints);
        }
        if !goal_def.success_criteria.is_empty() {
            println!("  Success criteria: {:?}", goal_def.success_criteria);
        }
    }

    // --- Stage 2: Execution plan ------------------------------------------
    println!("\n[2/4] Creating execution plan...");
    let plan = generate_plan(&goal_def);

    println!("  Phases: {}", plan.phases.len());
    println!("  Estimated duration: {}", plan.total_duration);
    println!("  Required skills: {}", plan.required_skills.join(", "));
    if verbose {
        for phase in &plan.phases {
            println!("  - {}: {}", phase.name, phase.description);
        }
    }

    // --- Stage 3: Skill synthesis & SDK tools -----------------------------
    println!("\n[3/4] Matching skills and SDK tools...");
    let skills = synthesize_skills(&plan, skills_dir);
    let sdk_tools = get_sdk_tools(sdk, &plan);

    println!("  Skills matched: {}", skills.len());
    for skill in &skills {
        println!(
            "    - {} ({}% match)",
            skill.name,
            (skill.match_score * 100.0) as u32
        );
    }
    if !sdk_tools.is_empty() {
        println!("  SDK tools ({}): {}", sdk, sdk_tools.len());
        for tool in &sdk_tools {
            println!("    - {} ({})", tool.name, tool.category);
        }
    }

    // --- Stage 4: Assemble bundle ----------------------------------------
    println!("\n[4/4] Assembling agent bundle...");
    let bundle_name = match name {
        Some(n) => sanitize_bundle_name(n, ""),
        None => generate_bundle_name(&goal_def),
    };

    let bundle = assemble_bundle(
        bundle_name,
        &goal_def,
        &plan,
        skills,
        sdk,
        enable_memory,
        multi_agent,
        enable_spawning,
        sdk_tools,
    );

    println!("  Bundle name: {}", bundle.name);
    println!("  Bundle ID: {}", bundle.id);
    println!("  SDK: {}", sdk);
    if enable_memory {
        println!("  Memory: Enabled");
    }
    if multi_agent {
        println!("  Multi-Agent: Enabled");
        println!("  Sub-agents: {}", bundle.sub_agent_configs.len());
        if enable_spawning {
            println!("  Spawning: Enabled");
        }
    }

    // --- Stage 5: Package ------------------------------------------------
    println!("\nPackaging agent...");
    let output_base = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("goal_agents"));
    let agent_dir = package_bundle(&bundle, &output_base)?;

    let elapsed = start.elapsed().as_secs_f32();
    println!("\n+ Goal agent created successfully in {elapsed:.1}s");
    println!("\nAgent directory: {}", agent_dir.display());
    println!("\nTo run the agent:");
    println!("  cd {}", agent_dir.display());
    println!("  python main.py");

    Ok(())
}

use anyhow::Context;

// ---------------------------------------------------------------------------
// Domain data structures
// ---------------------------------------------------------------------------

struct GoalDefinition {
    raw_prompt: String,
    goal: String,
    domain: String,
    complexity: String,
    constraints: Vec<String>,
    success_criteria: Vec<String>,
    context: HashMap<String, String>,
}

struct PlanPhase {
    name: String,
    description: String,
    required_capabilities: Vec<String>,
    estimated_duration: String,
    dependencies: Vec<String>,
    parallel_safe: bool,
    success_indicators: Vec<String>,
}

struct ExecutionPlan {
    phases: Vec<PlanPhase>,
    total_duration: String,
    required_skills: Vec<String>,
    parallel_opportunities: Vec<Vec<String>>,
    risk_factors: Vec<String>,
}

struct SkillDef {
    name: String,
    #[allow(dead_code)]
    capabilities: Vec<String>,
    #[allow(dead_code)]
    description: String,
    content: String,
    match_score: f32,
}

struct SdkTool {
    name: String,
    description: String,
    category: String,
}

struct SubAgentConfig {
    role: String,
    filename: String,
    yaml_content: String,
}

struct AgentBundle {
    id: String,
    name: String,
    goal_def: GoalDefinition,
    plan: ExecutionPlan,
    skills: Vec<SkillDef>,
    sdk: String,
    sdk_tools: Vec<SdkTool>,
    sub_agent_configs: Vec<SubAgentConfig>,
    memory_enabled: bool,
    multi_agent: bool,
    #[allow(dead_code)]
    enable_spawning: bool,
    auto_mode_config: Value,
    metadata: Value,
}

// ---------------------------------------------------------------------------
// Name generation / sanitization
// ---------------------------------------------------------------------------

fn generate_bundle_name(goal: &GoalDefinition) -> String {
    let stop_words = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
    ];

    let key_words: Vec<String> = goal
        .goal
        .to_lowercase()
        .split_whitespace()
        .filter(|w| !stop_words.contains(w) && w.len() > 2)
        .take(3)
        .map(|w| w.to_string())
        .collect();

    let domain_prefix = goal
        .domain
        .split('-')
        .next()
        .unwrap_or("general")
        .to_string();
    let mut words = vec![domain_prefix];
    for w in key_words {
        if !words.contains(&w) {
            words.push(w);
        }
    }
    let raw = words.join("-");
    sanitize_bundle_name(&raw, "-agent")
}

/// Sanitize a bundle name to meet requirements (3-50 chars, alphanumeric + hyphens).
pub fn sanitize_bundle_name(name: &str, suffix: &str) -> String {
    if name.is_empty() {
        return format!("agent{suffix}");
    }

    // lowercase, spaces/underscores -> hyphens, strip invalid chars
    let mut s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c == '_' || c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    // collapse multiple hyphens, trim leading/trailing hyphens
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_string();

    if s.is_empty() {
        s = "agent".to_string();
    }

    let suffix_len = suffix.len();
    let available = 50usize.saturating_sub(suffix_len);

    if s.len() > available {
        // truncate at word boundary if possible
        let trunc: String = s.chars().take(available).collect();
        if let Some(pos) = trunc.rfind('-') {
            if pos >= 3 {
                s = trunc[..pos].to_string();
            } else {
                s = trunc;
            }
        } else {
            s = trunc;
        }
    }

    s = format!("{s}{suffix}");

    // ensure minimum length of 3
    if s.len() < 3 {
        s = format!("{s}-agent");
    }
    if s.len() < 3 {
        s = format!("goal-{s}");
    }

    // ensure starts with alphanumeric
    if s.chars()
        .next()
        .map(|c| !c.is_alphanumeric())
        .unwrap_or(true)
    {
        s = format!("a{}", &s[1..]);
    }

    s
}

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

fn generate_uuid() -> String {
    // Determinism is not required here; use timestamp + pid for uniqueness
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{ts:032x}-{pid:08x}")
}
