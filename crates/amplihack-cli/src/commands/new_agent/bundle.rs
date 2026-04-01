//! Stage 4: Bundle assembly — combine goal, plan, skills and config into an AgentBundle.

use serde_json::json;

use super::{
    AgentBundle, ExecutionPlan, GoalDefinition, PlanPhase, SdkTool, SkillDef, SubAgentConfig,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_bundle(
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
    let id = super::generate_uuid();

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
            yaml_content: super::templates::coordinator_yaml(agent_name),
        },
        SubAgentConfig {
            role: "memory_agent".to_string(),
            filename: "memory_agent.yaml".to_string(),
            yaml_content: super::templates::memory_agent_yaml(agent_name),
        },
    ];
    // Always write spawner yaml; the enabled flag controls runtime behaviour
    configs.push(SubAgentConfig {
        role: "spawner".to_string(),
        filename: "spawner.yaml".to_string(),
        yaml_content: super::templates::spawner_yaml(agent_name, enable_spawning),
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
