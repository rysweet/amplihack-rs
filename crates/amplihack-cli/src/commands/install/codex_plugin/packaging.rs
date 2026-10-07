//! Portable package construction.
use super::*;
pub(super) fn build(source: &Path, hooks_binary: &Path, staged: &Path) -> Result<()> {
    let skills = staged.join("skills");
    let canonical = source.join("amplifier-bundle/skills").canonicalize()?;
    validate_skill_names(&canonical, &mut std::collections::BTreeSet::new(), 0)?;
    copy_tree(&canonical, &skills, &canonical, 0)?;
    expose_nested_skills(&skills, &skills, 0)?;
    instruction_skills(
        &source.join("docs/claude/commands/amplihack"),
        &skills,
        "command",
    )?;
    instruction_skills(&source.join("amplifier-bundle/agents"), &skills, "persona")?;
    fs::create_dir(staged.join("hooks"))?;
    // Empty declaration avoids duplicate execution if bundled hooks become supported.
    fs::write(staged.join("hooks/hooks.json"), "{\"hooks\":{}}\n")?;
    fs::write(
        staged.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name":"amplihack", "version":env!("CARGO_PKG_VERSION"),
            "description":"Provider-neutral Amplihack skills and reusable instructions",
            "extensions":{"com.openai":{"hooks":"./hooks/hooks.json"}}
        }))?,
    )?;
    fs::create_dir(staged.join("bin"))?;
    let wrapper = staged.join("bin/hook");
    // Copy the durable binary into the portable package; shell text has no path interpolation.
    fs::copy(hooks_binary, staged.join("bin/amplihack-hooks"))?;
    fs::write(
        &wrapper,
        "#!/bin/sh\nexport AMPLIHACK_AGENT_BINARY=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
