//! Portable package construction.
use super::*;
pub(super) const HOOK_WRAPPER: &str = "#!/bin/sh\nexport AMPLIHACK_HOOK_PROTOCOL=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n";
pub(super) const LEGACY_HOOK_WRAPPER: &str = "#!/bin/sh\nexport AMPLIHACK_AGENT_BINARY=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n";
pub(super) fn build(source: &Path, hooks_binary: &Path, staged: &Path) -> Result<()> {
    build_with_selection(
        source,
        hooks_binary,
        staged,
        &crate::user_config::list_selection(),
    )
}

pub(super) fn build_with_selection(
    source: &Path,
    hooks_binary: &Path,
    staged: &Path,
    selection: &ListSelection,
) -> Result<()> {
    let skills = staged.join("skills");
    let canonical = source.join("amplifier-bundle/skills").canonicalize()?;
    validate_skill_names(
        &canonical,
        &mut std::collections::BTreeSet::new(),
        0,
        selection,
    )?;
    copy_tree(&canonical, &skills, &canonical, 0, selection)?;
    expose_nested_skills(&skills, &skills, 0, selection)?;
    if selection.set == ListSet::All {
        instruction_skills(
            &source.join("docs/claude/commands/amplihack"),
            &skills,
            "command",
            selection,
        )?;
    }
    instruction_skills(
        &source.join("amplifier-bundle/agents"),
        &skills,
        "persona",
        selection,
    )?;
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
    fs::write(&wrapper, HOOK_WRAPPER)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(test)]
mod hook_wrapper_contract_tests {
    use super::HOOK_WRAPPER;

    #[test]
    fn wrapper_exports_protocol_then_execs_with_quoted_args() {
        let export = HOOK_WRAPPER
            .find("export AMPLIHACK_HOOK_PROTOCOL=codex")
            .unwrap();
        let exec = HOOK_WRAPPER
            .find("exec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"")
            .unwrap();
        assert!(export < exec);
        assert!(!HOOK_WRAPPER.contains("eval"));
    }
}

#[cfg(test)]
mod analyst_skill_tests {
    use super::*;

    #[test]
    fn package_leaves_out_analyst_skills_and_source_keeps_both() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let skills = source.join("amplifier-bundle/skills");
        for name in ["economist-analyst", "code-review"] {
            fs::create_dir_all(skills.join(name)).unwrap();
            fs::write(
                skills.join(name).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: test skill\n---\n"),
            )
            .unwrap();
        }
        let hooks_binary = dir.path().join("amplihack-hooks");
        fs::write(&hooks_binary, b"hooks").unwrap();
        let staged = dir.path().join("staged");

        build_with_selection(
            &source,
            &hooks_binary,
            &staged,
            &ListSelection::new(ListSet::All, []),
        )
        .unwrap();

        assert!(staged.join("skills/code-review/SKILL.md").is_file());
        assert!(!staged.join("skills/economist-analyst").exists());
        assert!(skills.join("code-review/SKILL.md").is_file());
        assert!(skills.join("economist-analyst/SKILL.md").is_file());
    }
}
