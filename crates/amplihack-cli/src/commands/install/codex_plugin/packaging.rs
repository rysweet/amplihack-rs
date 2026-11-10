//! Portable package construction.
use super::*;
pub(super) const HOOK_WRAPPER: &str = "#!/bin/sh\nexport AMPLIHACK_HOOK_PROTOCOL=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n";
pub(super) const LEGACY_HOOK_WRAPPER: &str = "#!/bin/sh\nexport AMPLIHACK_AGENT_BINARY=codex\nexec \"$(dirname -- \"$0\")/amplihack-hooks\" \"$@\"\n";
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

    // Issue #1878 item 1: the Codex wrapper neither sets nor defaults any
    // XPIA variable, and passes the user's own XPIA_ENABLED through.

    #[test]
    fn xpia_codex_wrappers_do_not_mention_xpia() {
        for (name, wrapper) in [
            ("HOOK_WRAPPER", HOOK_WRAPPER),
            ("LEGACY_HOOK_WRAPPER", super::LEGACY_HOOK_WRAPPER),
        ] {
            assert!(
                !wrapper.to_ascii_uppercase().contains("XPIA"),
                "{name} must not set or forward XPIA settings: {wrapper:?}"
            );
        }
    }

    #[test]
    fn xpia_codex_hook_declarations_and_config_do_not_mention_xpia() {
        let package = std::path::Path::new("/opt/amplihack/codex-plugin");
        let declarations = super::super::declarations(package).unwrap().to_string();
        assert!(
            !declarations.to_ascii_uppercase().contains("XPIA"),
            "{declarations}"
        );
        let market = std::path::Path::new("/opt/amplihack/codex-market");
        for state in super::super::config::config_states(None, market, false).unwrap() {
            let text = String::from_utf8(state).unwrap();
            assert!(!text.to_ascii_uppercase().contains("XPIA"), "{text}");
        }
    }

    /// Build the real package from this repository with a stub
    /// `amplihack-hooks` that echoes `$XPIA_ENABLED|$AMPLIHACK_HOOK_PROTOCOL`,
    /// then run the staged `bin/hook` with a cleared environment.
    #[cfg(unix)]
    #[test]
    fn xpia_codex_wrapper_passes_xpia_enabled_through_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let stub = temp.path().join("stub-hooks");
        std::fs::write(
            &stub,
            "#!/bin/sh\nprintf '%s|%s' \"$XPIA_ENABLED\" \"$AMPLIHACK_HOOK_PROTOCOL\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let staged = temp.path().join("package");
        std::fs::create_dir(&staged).unwrap();
        super::build(&source, &stub, &staged).unwrap();

        for file in ["hooks/hooks.json", "plugin.json", "bin/hook"] {
            let body = std::fs::read_to_string(staged.join(file)).unwrap();
            assert!(
                !body.to_ascii_uppercase().contains("XPIA"),
                "staged {file} must not mention XPIA: {body}"
            );
        }
        let bin = staged.join("bin/amplihack-hooks");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let run = |value: Option<&str>| {
            let mut command = Command::new(staged.join("bin/hook"));
            command
                .arg("pre-tool-use")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", temp.path())
                .current_dir(temp.path());
            if let Some(value) = value {
                command.env("XPIA_ENABLED", value);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(run(None), "|codex", "unset must stay unset");
        assert_eq!(run(Some("true")), "true|codex", "opt-in must pass through");
        assert_eq!(
            run(Some("false")),
            "false|codex",
            "opt-out must pass through"
        );
    }
}
