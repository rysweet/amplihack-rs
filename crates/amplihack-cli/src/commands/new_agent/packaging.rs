//! Stage 5: Packaging — write the agent bundle to the filesystem.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::AgentBundle;
use super::templates;

pub(super) fn package_bundle(bundle: &AgentBundle, output_base: &Path) -> Result<PathBuf> {
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
    write_file(
        &agent_dir.join("main.py"),
        &templates::generate_main_py(bundle),
    )?;

    // README.md
    write_file(
        &agent_dir.join("README.md"),
        &templates::generate_readme(bundle),
    )?;

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
        &templates::generate_requirements(bundle),
    )?;

    // Memory artifacts
    if bundle.memory_enabled {
        write_file(
            &agent_dir.join("memory_config.yaml"),
            &templates::memory_config_yaml(&bundle.name),
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
            &templates::multi_agent_init_py(&bundle.name),
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
