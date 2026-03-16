//! Native Rust implementation of `amplihack new`.
//!
//! Implements the full goal-agent-generator pipeline locally without delegating
//! to the Python runtime. The pipeline mirrors the Python implementation in
//! `amplihack/goal_agent_generator/` with the same CLI surface and output layout.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
// Stage 1: Prompt analysis
// ---------------------------------------------------------------------------

fn analyze_prompt(prompt: &str) -> Result<GoalDefinition> {
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

fn classify_domain(prompt: &str) -> String {
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

fn determine_complexity(prompt: &str) -> String {
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

// ---------------------------------------------------------------------------
// Stage 2: Execution plan
// ---------------------------------------------------------------------------

fn generate_plan(goal: &GoalDefinition) -> ExecutionPlan {
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

// ---------------------------------------------------------------------------
// Stage 3: Skill synthesis & SDK tools
// ---------------------------------------------------------------------------

fn synthesize_skills(plan: &ExecutionPlan, skills_dir: Option<&Path>) -> Vec<SkillDef> {
    let mut out = Vec::new();

    // Try to load real skill files from the skills directory
    let dir = skills_dir
        .map(|p| p.to_path_buf())
        .or_else(find_skills_directory);

    for skill_name in &plan.required_skills {
        let skill = if let Some(ref d) = dir {
            load_skill_from_dir(d, skill_name)
        } else {
            None
        };
        out.push(skill.unwrap_or_else(|| generate_generic_skill(skill_name)));
    }

    if out.is_empty() {
        out.push(generate_generic_skill("generic-executor"));
    }

    out
}

fn find_skills_directory() -> Option<PathBuf> {
    // Walk up looking for .claude/agents/amplihack
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".claude").join("agents").join("amplihack");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    // Also try ~/.amplihack/.claude/agents/amplihack
    dirs_home().map(|h| {
        h.join(".amplihack")
            .join(".claude")
            .join("agents")
            .join("amplihack")
    })
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn load_skill_from_dir(dir: &Path, skill_name: &str) -> Option<SkillDef> {
    let path = dir.join(format!("{skill_name}.md"));
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    Some(SkillDef {
        name: skill_name.to_string(),
        capabilities: vec![skill_name.to_string()],
        description: format!("Skill loaded from {}", path.display()),
        content,
        match_score: 0.9,
    })
}

fn generate_generic_skill(name: &str) -> SkillDef {
    let content = format!(
        "# {name}\n\n\
         Generic skill for {name} operations.\n\n\
         ## Capabilities\n\
         - Execute tasks autonomously\n\
         - Adapt to goal requirements\n\
         - Report progress and results\n"
    );
    SkillDef {
        name: name.to_string(),
        capabilities: vec![name.to_string()],
        description: format!("Generic {name} skill"),
        content,
        match_score: 0.5,
    }
}

// SDK tool mapping
fn get_sdk_tools(sdk: &str, plan: &ExecutionPlan) -> Vec<SdkTool> {
    let all_caps: Vec<&str> = plan
        .phases
        .iter()
        .flat_map(|p| p.required_capabilities.iter().map(|s| s.as_str()))
        .collect();

    let native: &[(&str, &str, &str)] = match sdk.to_lowercase().as_str() {
        "claude" => &[
            ("bash", "Execute shell commands", "system"),
            ("read_file", "Read file contents", "file_ops"),
            ("write_file", "Create/overwrite files", "file_ops"),
            ("edit_file", "Modify files", "file_ops"),
            ("glob", "Find files by pattern", "file_ops"),
            ("grep", "Search file contents", "search"),
        ],
        "copilot" => &[
            ("file_system", "File operations", "file_ops"),
            ("git", "Git version control", "vcs"),
            ("web_requests", "HTTP requests", "network"),
        ],
        "microsoft" => &[("ai_function", "Agent Framework AI functions", "ai")],
        _ => &[], // mini: no native tools
    };

    // Categories needed by the plan capabilities
    let needed_cats = caps_to_categories(&all_caps);

    native
        .iter()
        .filter(|(_, _, cat)| needed_cats.is_empty() || needed_cats.contains(*cat))
        .map(|(name, desc, cat)| SdkTool {
            name: name.to_string(),
            description: desc.to_string(),
            category: cat.to_string(),
        })
        .collect()
}

fn caps_to_categories(caps: &[&str]) -> std::collections::HashSet<&'static str> {
    let mut cats = std::collections::HashSet::new();
    for cap in caps {
        let lower = cap.to_lowercase();
        if lower.contains("file")
            || lower.contains("read")
            || lower.contains("write")
            || lower.contains("edit")
            || lower.contains("glob")
        {
            cats.insert("file_ops");
        }
        if lower.contains("exec")
            || lower.contains("shell")
            || lower.contains("bash")
            || lower.contains("run")
            || lower.contains("command")
        {
            cats.insert("system");
        }
        if lower.contains("search")
            || lower.contains("grep")
            || lower.contains("scan")
            || lower.contains("detect")
            || lower.contains("pattern")
        {
            cats.insert("search");
        }
        if lower.contains("git") || lower.contains("version") || lower.contains("commit") {
            cats.insert("vcs");
        }
        if lower.contains("http")
            || lower.contains("api")
            || lower.contains("web")
            || lower.contains("fetch")
            || lower.contains("webhook")
        {
            cats.insert("network");
        }
        if lower.contains("ai") || lower.contains("llm") || lower.contains("generate") {
            cats.insert("ai");
        }
    }
    cats
}

// ---------------------------------------------------------------------------
// Stage 4: Bundle assembly
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn assemble_bundle(
    name: String,
    goal: &GoalDefinition,
    plan: &ExecutionPlan,
    skills: Vec<SkillDef>,
    sdk: &str,
    enable_memory: bool,
    multi_agent: bool,
    enable_spawning: bool,
    sdk_tools: Vec<SdkTool>,
) -> AgentBundle {
    let id = generate_uuid();

    let complexity_turns: u32 = match goal.complexity.as_str() {
        "simple" => 5,
        "complex" => 15,
        _ => 10,
    };
    let phase_mul = 1.0 + (plan.phases.len() as f32 - 3.0).max(0.0) * 0.2;
    let mut max_turns = (complexity_turns as f32 * phase_mul) as u32;
    if multi_agent {
        max_turns = (max_turns as f32 * 1.5) as u32;
    }

    let auto_mode_config = json!({
        "max_turns": max_turns,
        "initial_prompt": build_initial_prompt(goal, plan),
        "working_dir": ".",
        "sdk": sdk,
        "ui_mode": false,
        "success_criteria": goal.success_criteria,
        "constraints": goal.constraints,
    });

    let all_caps: Vec<String> = plan
        .phases
        .iter()
        .flat_map(|p| p.required_capabilities.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let mut metadata = json!({
        "domain": goal.domain,
        "complexity": goal.complexity,
        "phase_count": plan.phases.len(),
        "skill_count": skills.len(),
        "estimated_duration": plan.total_duration,
        "required_capabilities": all_caps,
        "skill_names": skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
        "parallel_opportunities": plan.parallel_opportunities.len(),
        "risk_factors": plan.risk_factors,
    });

    if enable_memory {
        metadata["memory_enabled"] = json!(true);
        metadata["memory_storage_path"] = json!("./memory");
    }
    if multi_agent {
        metadata["multi_agent"] = json!(true);
        metadata["enable_spawning"] = json!(enable_spawning);
    }

    let sub_agent_configs = if multi_agent {
        build_sub_agent_configs(&name, enable_spawning)
    } else {
        vec![]
    };

    AgentBundle {
        id,
        name,
        goal_def: GoalDefinition {
            raw_prompt: goal.raw_prompt.clone(),
            goal: goal.goal.clone(),
            domain: goal.domain.clone(),
            complexity: goal.complexity.clone(),
            constraints: goal.constraints.clone(),
            success_criteria: goal.success_criteria.clone(),
            context: goal.context.clone(),
        },
        plan: ExecutionPlan {
            phases: plan
                .phases
                .iter()
                .map(|p| PlanPhase {
                    name: p.name.clone(),
                    description: p.description.clone(),
                    required_capabilities: p.required_capabilities.clone(),
                    estimated_duration: p.estimated_duration.clone(),
                    dependencies: p.dependencies.clone(),
                    parallel_safe: p.parallel_safe,
                    success_indicators: p.success_indicators.clone(),
                })
                .collect(),
            total_duration: plan.total_duration.clone(),
            required_skills: plan.required_skills.clone(),
            parallel_opportunities: plan.parallel_opportunities.clone(),
            risk_factors: plan.risk_factors.clone(),
        },
        skills,
        sdk: sdk.to_string(),
        sdk_tools,
        sub_agent_configs,
        memory_enabled: enable_memory,
        multi_agent,
        enable_spawning,
        auto_mode_config,
        metadata,
    }
}

fn build_sub_agent_configs(agent_name: &str, enable_spawning: bool) -> Vec<SubAgentConfig> {
    let mut configs = vec![
        SubAgentConfig {
            role: "coordinator".to_string(),
            filename: "coordinator.yaml".to_string(),
            yaml_content: coordinator_yaml(agent_name),
        },
        SubAgentConfig {
            role: "memory_agent".to_string(),
            filename: "memory_agent.yaml".to_string(),
            yaml_content: memory_agent_yaml(agent_name),
        },
    ];
    // Always write spawner yaml; the enabled flag controls runtime behaviour
    configs.push(SubAgentConfig {
        role: "spawner".to_string(),
        filename: "spawner.yaml".to_string(),
        yaml_content: spawner_yaml(agent_name, enable_spawning),
    });
    configs
}

fn build_initial_prompt(goal: &GoalDefinition, plan: &ExecutionPlan) -> String {
    let mut parts = vec![
        format!("# Goal: {}", goal.goal),
        String::new(),
        "## Objective".to_string(),
        goal.raw_prompt.clone(),
        String::new(),
        "## Execution Plan".to_string(),
    ];
    for (i, phase) in plan.phases.iter().enumerate() {
        parts.push(format!("\n### Phase {}: {}", i + 1, phase.name));
        parts.push(phase.description.clone());
        parts.push(format!(
            "**Estimated Duration**: {}",
            phase.estimated_duration
        ));
        parts.push(format!(
            "**Required Capabilities**: {}",
            phase.required_capabilities.join(", ")
        ));
        if !phase.dependencies.is_empty() {
            parts.push(format!(
                "**Dependencies**: {}",
                phase.dependencies.join(", ")
            ));
        }
    }
    if !goal.success_criteria.is_empty() {
        parts.push("\n## Success Criteria".to_string());
        for c in &goal.success_criteria {
            parts.push(format!("- {c}"));
        }
    }
    if !goal.constraints.is_empty() {
        parts.push("\n## Constraints".to_string());
        for c in &goal.constraints {
            parts.push(format!("- {c}"));
        }
    }
    parts.push("\n## Instructions".to_string());
    parts.push("Execute the plan above autonomously:".to_string());
    parts.push("1. Follow each phase in sequence".to_string());
    parts.push("2. Use available skills and tools".to_string());
    parts.push("3. Verify success criteria are met".to_string());
    parts.push("4. Report progress and completion".to_string());
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// Stage 5: Packaging
// ---------------------------------------------------------------------------

fn package_bundle(bundle: &AgentBundle, output_base: &Path) -> Result<PathBuf> {
    let agent_dir = output_base.join(&bundle.name);
    fs::create_dir_all(&agent_dir)
        .with_context(|| format!("cannot create agent directory: {}", agent_dir.display()))?;

    // Directory structure
    fs::create_dir_all(agent_dir.join(".claude").join("agents"))?;
    fs::create_dir_all(agent_dir.join(".claude").join("context"))?;
    fs::create_dir_all(agent_dir.join("logs"))?;
    if bundle.multi_agent {
        fs::create_dir_all(agent_dir.join("sub_agents"))?;
    }

    // prompt.md
    write_file(&agent_dir.join("prompt.md"), &bundle.goal_def.raw_prompt)?;

    // .claude/context/goal.json
    let goal_json = json!({
        "goal": bundle.goal_def.goal,
        "domain": bundle.goal_def.domain,
        "complexity": bundle.goal_def.complexity,
        "constraints": bundle.goal_def.constraints,
        "success_criteria": bundle.goal_def.success_criteria,
        "context": bundle.goal_def.context,
    });
    write_json(
        &agent_dir.join(".claude").join("context").join("goal.json"),
        &goal_json,
    )?;

    // .claude/context/execution_plan.json
    let plan_json = json!({
        "total_duration": bundle.plan.total_duration,
        "required_skills": bundle.plan.required_skills,
        "parallel_opportunities": bundle.plan.parallel_opportunities,
        "risk_factors": bundle.plan.risk_factors,
        "phases": bundle.plan.phases.iter().map(|p| json!({
            "name": p.name,
            "description": p.description,
            "required_capabilities": p.required_capabilities,
            "estimated_duration": p.estimated_duration,
            "dependencies": p.dependencies,
            "parallel_safe": p.parallel_safe,
            "success_indicators": p.success_indicators,
        })).collect::<Vec<_>>(),
    });
    write_json(
        &agent_dir
            .join(".claude")
            .join("context")
            .join("execution_plan.json"),
        &plan_json,
    )?;

    // .claude/agents/*.md  (skills)
    for skill in &bundle.skills {
        write_file(
            &agent_dir
                .join(".claude")
                .join("agents")
                .join(format!("{}.md", skill.name)),
            &skill.content,
        )?;
    }

    // main.py
    write_file(&agent_dir.join("main.py"), &generate_main_py(bundle))?;

    // README.md
    write_file(&agent_dir.join("README.md"), &generate_readme(bundle))?;

    // agent_config.json
    let mut config = json!({
        "bundle_id": bundle.id,
        "name": bundle.name,
        "version": "1.0.0",
        "metadata": bundle.metadata,
        "auto_mode_config": bundle.auto_mode_config,
    });
    if !bundle.sdk_tools.is_empty() {
        config["sdk_tools"] = json!(
            bundle
                .sdk_tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "category": t.category,
                }))
                .collect::<Vec<_>>()
        );
    }
    if !bundle.sub_agent_configs.is_empty() {
        config["sub_agents"] = json!(
            bundle
                .sub_agent_configs
                .iter()
                .map(|s| json!({
                    "role": s.role,
                    "filename": s.filename,
                }))
                .collect::<Vec<_>>()
        );
    }
    write_json(&agent_dir.join("agent_config.json"), &config)?;

    // requirements.txt
    write_file(
        &agent_dir.join("requirements.txt"),
        &generate_requirements(bundle),
    )?;

    // Memory artifacts
    if bundle.memory_enabled {
        write_file(
            &agent_dir.join("memory_config.yaml"),
            &memory_config_yaml(&bundle.name),
        )?;
        let memory_dir = agent_dir.join("memory");
        fs::create_dir_all(&memory_dir)?;
        write_file(&memory_dir.join(".gitignore"), "*.sqlite\n*.db\n*.log\n")?;
    }

    // Multi-agent artifacts
    if bundle.multi_agent {
        for sa in &bundle.sub_agent_configs {
            write_file(
                &agent_dir.join("sub_agents").join(&sa.filename),
                &sa.yaml_content,
            )?;
        }
        write_file(
            &agent_dir.join("sub_agents").join("__init__.py"),
            &multi_agent_init_py(&bundle.name),
        )?;
    }

    // SDK tools config
    if !bundle.sdk_tools.is_empty() {
        let tools_json = json!({
            "sdk": bundle.sdk,
            "tools": bundle.sdk_tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "category": t.category,
            })).collect::<Vec<_>>(),
        });
        write_json(
            &agent_dir
                .join(".claude")
                .join("context")
                .join("sdk_tools.json"),
            &tools_json,
        )?;
    }

    Ok(agent_dir)
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("cannot write {}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    write_file(path, &s)
}

// ---------------------------------------------------------------------------
// Generated file content
// ---------------------------------------------------------------------------

fn generate_main_py(bundle: &AgentBundle) -> String {
    let memory_import = if bundle.memory_enabled {
        "\ntry:\n    from amplihack_memory import MemoryConnector, ExperienceStore\nexcept ImportError:\n    print(\"Warning: amplihack-memory-lib not found. Memory features disabled.\")\n    MemoryConnector = None\n"
    } else {
        ""
    };

    format!(
        r#"#!/usr/bin/env python3
"""
{name} - Autonomous Goal-Seeking Agent

Generated by Amplihack Goal Agent Generator (Rust)
"""

import sys
from pathlib import Path
from typing import Any
{memory_import}
def main() -> int:
    """Run the goal-seeking agent."""
    print(f"Starting agent: {name}")
    print(f"SDK: {sdk}")
    print("Executing goal autonomously...")
    return 0


if __name__ == "__main__":
    sys.exit(main())
"#,
        name = bundle.name,
        sdk = bundle.sdk,
        memory_import = memory_import,
    )
}

fn generate_readme(bundle: &AgentBundle) -> String {
    let memory_section = if bundle.memory_enabled {
        "\n## Memory\n\nThis agent has memory capabilities enabled.\n\
         Experiences are stored in the `./memory` directory.\n"
    } else {
        ""
    };

    let multi_agent_section = if bundle.multi_agent {
        "\n## Multi-Agent Architecture\n\nThis bundle uses a multi-agent setup:\n\
         - **coordinator**: Routes tasks to appropriate sub-agents\n\
         - **memory_agent**: Manages knowledge retrieval\n\
         - **spawner**: Dynamically creates specialist sub-agents\n\
         \nSee `sub_agents/` for configuration details.\n"
    } else {
        ""
    };

    format!(
        "# {name}\n\n\
         Goal: {goal}\n\n\
         Domain: {domain} | Complexity: {complexity}\n\n\
         ## Quick Start\n\n\
         ```bash\ncd {name}\npython main.py\n```\n\
         {memory_section}\
         {multi_agent_section}\n\
         ## Generated Metadata\n\n\
         - Bundle ID: {id}\n\
         - SDK: {sdk}\n\n\
         ---\n\nGenerated by Amplihack Goal Agent Generator (Rust)\n",
        name = bundle.name,
        goal = bundle.goal_def.goal,
        domain = bundle.goal_def.domain,
        complexity = bundle.goal_def.complexity,
        id = bundle.id,
        sdk = bundle.sdk,
    )
}

fn generate_requirements(bundle: &AgentBundle) -> String {
    let mut reqs = vec!["amplihack>=0.9.0".to_string()];
    if bundle.memory_enabled {
        reqs.push("amplihack-memory-lib>=0.1.0".to_string());
    }
    if bundle.multi_agent {
        reqs.push("pyyaml>=6.0".to_string());
    }
    let mut s = reqs.join("\n");
    s.push('\n');
    s
}

// ---------------------------------------------------------------------------
// YAML template helpers
// ---------------------------------------------------------------------------

fn coordinator_yaml(agent_name: &str) -> String {
    format!(
        "# Coordinator Configuration for {agent_name}\n\
         # Routes tasks to appropriate sub-agents based on classification\n\n\
         role: task_classifier\n\
         agent_name: \"{agent_name}\"\n\n\
         strategies:\n\
         \x20 - entity_centric\n\
         \x20 - temporal\n\
         \x20 - aggregation\n\
         \x20 - full_text\n\
         \x20 - simple_all\n\
         \x20 - two_phase\n\n\
         classification:\n\
         \x20 default_strategy: simple_all\n\
         \x20 confidence_threshold: 0.6\n"
    )
}

fn memory_agent_yaml(agent_name: &str) -> String {
    format!(
        "# Memory Agent Configuration for {agent_name}\n\
         # Specializes in knowledge retrieval and fact management\n\n\
         role: retrieval_specialist\n\
         agent_name: \"{agent_name}\"\n\n\
         max_facts: 300\n\
         summarization_threshold: 1000\n\n\
         retrieval:\n\
         \x20 strategies:\n\
         \x20   - semantic_search\n\
         \x20   - keyword_match\n\
         \x20   - recency_weighted\n\
         \x20 max_results: 20\n\
         \x20 min_relevance: 0.3\n\n\
         memory_sharing:\n\
         \x20 read_access: true\n\
         \x20 write_access: false\n\
         \x20 namespace: \"{agent_name}-shared\"\n"
    )
}

fn spawner_yaml(agent_name: &str, enabled: bool) -> String {
    format!(
        "# Spawner Configuration for {agent_name}\n\
         # Manages dynamic creation of specialist sub-agents\n\n\
         enabled: {enabled}\n\
         agent_name: \"{agent_name}\"\n\n\
         specialist_types:\n\
         \x20 - retrieval\n\
         \x20 - analysis\n\
         \x20 - synthesis\n\
         \x20 - code_generation\n\
         \x20 - research\n\n\
         max_concurrent: 3\n\
         timeout: 60\n\n\
         lifecycle:\n\
         \x20 auto_cleanup: true\n\
         \x20 idle_timeout: 30\n\
         \x20 max_memory_mb: 512\n\
         \x20 max_tokens_per_turn: 4096\n"
    )
}

fn memory_config_yaml(agent_name: &str) -> String {
    format!(
        "# Memory Configuration for {agent_name}\n\n\
         agent_name: \"{agent_name}\"\n\
         storage_path: \"./memory\"\n\
         max_experiences: 1000\n\
         auto_compress: true\n"
    )
}

fn multi_agent_init_py(agent_name: &str) -> String {
    format!(
        "\"\"\"Multi-agent initialization for {agent_name}.\"\"\"\n\n\
         AGENT_NAME = \"{agent_name}\"\n"
    )
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_prompt(dir: &Path, content: &str) -> PathBuf {
        let p = dir.join("prompt.md");
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn test_sanitize_bundle_name_basic() {
        assert_eq!(sanitize_bundle_name("my agent", "-agent"), "my-agent-agent");
        assert_eq!(sanitize_bundle_name("TEST_NAME", ""), "test-name");
        assert_eq!(sanitize_bundle_name("", "-agent"), "agent-agent");
    }

    #[test]
    fn test_sanitize_bundle_name_long() {
        let long = "a".repeat(60);
        let result = sanitize_bundle_name(&long, "");
        assert!(result.len() <= 50, "got len={}", result.len());
        assert!(result.len() >= 3);
    }

    #[test]
    fn test_sanitize_bundle_name_special_chars() {
        let result = sanitize_bundle_name("Test@#$Name!!!", "-agent");
        assert!(!result.contains('@'));
        assert!(!result.contains('#'));
        assert!(!result.contains('$'));
        assert!(!result.contains('!'));
    }

    #[test]
    fn test_analyze_prompt_basic() {
        let prompt = "# Build a data pipeline\n\nProcess CSV files and generate reports.";
        let goal = analyze_prompt(prompt).unwrap();
        assert!(!goal.goal.is_empty());
        assert!(!goal.domain.is_empty());
        assert!(!goal.complexity.is_empty());
    }

    #[test]
    fn test_analyze_prompt_empty_fails() {
        assert!(analyze_prompt("").is_err());
        assert!(analyze_prompt("   ").is_err());
    }

    #[test]
    fn test_domain_classification() {
        assert_eq!(
            classify_domain("scan for security vulnerabilities"),
            "security-analysis"
        );
        assert_eq!(
            classify_domain("deploy the application to production"),
            "deployment"
        );
        assert_eq!(classify_domain("test the API endpoints"), "testing");
        assert_eq!(
            classify_domain("process and transform data"),
            "data-processing"
        );
        assert_eq!(classify_domain("something completely generic"), "general");
    }

    #[test]
    fn test_complexity_detection() {
        assert_eq!(determine_complexity("simple one step task"), "simple");
        assert_eq!(
            determine_complexity("complex distributed multi-stage pipeline"),
            "complex"
        );
        // word count heuristic
        let long_prompt = "word ".repeat(200);
        assert_eq!(determine_complexity(&long_prompt), "complex");
    }

    #[test]
    fn test_e2e_creates_expected_files() {
        let tmp = TempDir::new().unwrap();
        let prompt_path =
            write_prompt(tmp.path(), "# Automate deployment\n\nDeploy to production.");
        let out = tmp.path().join("out");

        run_new(
            &prompt_path,
            Some(&out),
            None,
            None,
            false,
            false,
            "copilot",
            false,
            false,
        )
        .unwrap();

        // find the agent dir (name is auto-generated)
        let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
        assert_eq!(entries.len(), 1, "expected exactly one agent directory");
        let agent_dir = entries[0].as_ref().unwrap().path();

        assert!(agent_dir.join("prompt.md").exists(), "prompt.md missing");
        assert!(agent_dir.join("main.py").exists(), "main.py missing");
        assert!(agent_dir.join("README.md").exists(), "README.md missing");
        assert!(
            agent_dir.join("agent_config.json").exists(),
            "agent_config.json missing"
        );
        assert!(
            agent_dir.join("requirements.txt").exists(),
            "requirements.txt missing"
        );
        assert!(
            agent_dir
                .join(".claude")
                .join("context")
                .join("goal.json")
                .exists()
        );
        assert!(
            agent_dir
                .join(".claude")
                .join("context")
                .join("execution_plan.json")
                .exists()
        );
    }

    #[test]
    fn test_memory_mode_writes_artifacts() {
        let tmp = TempDir::new().unwrap();
        let prompt_path = write_prompt(tmp.path(), "# Process data\n\nAnalyze CSV files.");
        let out = tmp.path().join("out");

        run_new(
            &prompt_path,
            Some(&out),
            Some("mem-agent"),
            None,
            false,
            true,
            "copilot",
            false,
            false,
        )
        .unwrap();

        let agent_dir = out.join("mem-agent");
        assert!(
            agent_dir.join("memory_config.yaml").exists(),
            "memory_config.yaml missing"
        );
        assert!(
            agent_dir.join("memory").join(".gitignore").exists(),
            "memory/.gitignore missing"
        );
        let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
        assert!(
            reqs.contains("amplihack-memory-lib"),
            "memory dep missing from requirements.txt"
        );
    }

    #[test]
    fn test_multi_agent_mode_writes_sub_agent_configs() {
        let tmp = TempDir::new().unwrap();
        let prompt_path = write_prompt(
            tmp.path(),
            "# Orchestrate multiple services\n\nCoordinate deployment.",
        );
        let out = tmp.path().join("out");

        run_new(
            &prompt_path,
            Some(&out),
            Some("multi-agent"),
            None,
            false,
            false,
            "claude",
            true,
            false,
        )
        .unwrap();

        let agent_dir = out.join("multi-agent");
        assert!(
            agent_dir
                .join("sub_agents")
                .join("coordinator.yaml")
                .exists()
        );
        assert!(
            agent_dir
                .join("sub_agents")
                .join("memory_agent.yaml")
                .exists()
        );
        assert!(agent_dir.join("sub_agents").join("spawner.yaml").exists());
        assert!(agent_dir.join("sub_agents").join("__init__.py").exists());
        let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
        assert!(
            reqs.contains("pyyaml"),
            "pyyaml missing from requirements.txt"
        );
    }

    #[test]
    fn test_enable_spawning_implies_multi_agent() {
        let tmp = TempDir::new().unwrap();
        let prompt_path = write_prompt(tmp.path(), "# Spawn workers\n\nDynamic task spawning.");
        let out = tmp.path().join("out");

        // enable_spawning=true, multi_agent=false — should auto-enable multi-agent
        run_new(
            &prompt_path,
            Some(&out),
            Some("spawner-test"),
            None,
            false,
            false,
            "copilot",
            false,
            true,
        )
        .unwrap();

        let agent_dir = out.join("spawner-test");
        // sub_agents directory should exist (multi-agent was auto-enabled)
        assert!(agent_dir.join("sub_agents").exists());
        // spawner.yaml should have enabled: true
        let spawner =
            fs::read_to_string(agent_dir.join("sub_agents").join("spawner.yaml")).unwrap();
        assert!(spawner.contains("enabled: true"));
    }

    #[test]
    fn test_missing_skills_dir_falls_back_to_generic() {
        let tmp = TempDir::new().unwrap();
        let prompt_path = write_prompt(tmp.path(), "# Process data\n\nTransform and analyze.");
        let out = tmp.path().join("out");

        // Pass a non-existent skills dir
        let nonexistent = tmp.path().join("no_skills_here");
        run_new(
            &prompt_path,
            Some(&out),
            Some("fallback-test"),
            Some(&nonexistent),
            false,
            false,
            "copilot",
            false,
            false,
        )
        .unwrap();

        let agents_dir = out.join("fallback-test").join(".claude").join("agents");
        let skill_files: Vec<_> = fs::read_dir(&agents_dir).unwrap().collect();
        assert!(
            !skill_files.is_empty(),
            "expected at least one generic skill file"
        );
    }

    #[test]
    fn test_custom_name_is_sanitized() {
        let tmp = TempDir::new().unwrap();
        let prompt_path = write_prompt(tmp.path(), "Deploy to staging.");
        let out = tmp.path().join("out");

        run_new(
            &prompt_path,
            Some(&out),
            Some("My Custom Name!!!"),
            None,
            false,
            false,
            "copilot",
            false,
            false,
        )
        .unwrap();

        // Sanitized name should exist (special chars stripped)
        let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let dir_name = entries[0].as_ref().unwrap().file_name();
        let dir_str = dir_name.to_string_lossy();
        assert!(!dir_str.contains('!'), "name should be sanitized");
    }
}
