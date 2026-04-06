//! Stage 3: Skill synthesis and SDK tool mapping.

use std::fs;
use std::path::{Path, PathBuf};

use super::{ExecutionPlan, SdkTool, SkillDef};

pub(super) fn synthesize_skills(plan: &ExecutionPlan, skills_dir: Option<&Path>) -> Vec<SkillDef> {
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
        _capabilities: vec![skill_name.to_string()],
        _description: format!("Skill loaded from {}", path.display()),
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
        _capabilities: vec![name.to_string()],
        _description: format!("Generic {name} skill"),
        content,
        match_score: 0.5,
    }
}

// SDK tool mapping
pub(super) fn get_sdk_tools(sdk: &str, plan: &ExecutionPlan) -> Vec<SdkTool> {
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
