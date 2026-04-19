use super::*;
use std::fs;
use std::path::Path;

/// Builds a minimal fake amplihack repository under `root`.
pub(super) fn create_source_repo(root: &Path) {
    for dir in ESSENTIAL_DIRS {
        fs::create_dir_all(root.join(".claude").join(dir)).unwrap();
    }
    let legacy_hooks = root.join(".claude/tools/amplihack/hooks");
    fs::create_dir_all(&legacy_hooks).unwrap();
    for hook in ["pre_tool_use.py", "workflow_classification_reminder.py"] {
        fs::write(legacy_hooks.join(hook), "print(1)\n").unwrap();
    }
    fs::write(root.join(".claude/settings.json"), "{}\n").unwrap();
    fs::write(root.join(".claude/tools/statusline.sh"), "echo hi\n").unwrap();
    fs::write(root.join(".claude/AMPLIHACK.md"), "framework\n").unwrap();
    fs::write(root.join("CLAUDE.md"), "root\n").unwrap();

    // Issue #243: source repos must ship `amplifier-bundle/` so the install
    // can stage recipes + orch_helper.py for dev-orchestrator.
    let bundle = root.join("amplifier-bundle");
    fs::create_dir_all(bundle.join("recipes")).unwrap();
    fs::create_dir_all(bundle.join("tools")).unwrap();
    for recipe in [
        "smart-orchestrator.yaml",
        "default-workflow.yaml",
        "investigation-workflow.yaml",
    ] {
        fs::write(
            bundle.join("recipes").join(recipe),
            "name: test-recipe\nsteps: []\n",
        )
        .unwrap();
    }
    fs::write(bundle.join("tools/orch_helper.py"), "# stub\n").unwrap();
}

pub(super) fn create_minimal_staged_assets(root: &Path) {
    let claude_dir = root.join(".amplihack/.claude");
    for dir in ESSENTIAL_DIRS {
        fs::create_dir_all(claude_dir.join(dir)).unwrap();
    }
    fs::write(claude_dir.join("tools/statusline.sh"), "echo hi\n").unwrap();
    fs::write(claude_dir.join("AMPLIHACK.md"), "framework\n").unwrap();
    fs::write(root.join(".amplihack/CLAUDE.md"), "root\n").unwrap();

    // Issue #243: staged amplifier-bundle is now part of the presence check.
    let bundle = root.join(".amplihack/amplifier-bundle");
    fs::create_dir_all(bundle.join("recipes")).unwrap();
    fs::create_dir_all(bundle.join("tools")).unwrap();
    for recipe in [
        "smart-orchestrator.yaml",
        "default-workflow.yaml",
        "investigation-workflow.yaml",
    ] {
        fs::write(
            bundle.join("recipes").join(recipe),
            "name: test-recipe\nsteps: []\n",
        )
        .unwrap();
    }
    fs::write(bundle.join("tools/orch_helper.py"), "# stub\n").unwrap();
}

/// Creates an executable stub at `dir/name` (755 perms on Unix).
/// Content is padded to > 1024 bytes so deploy_binaries size check passes.
pub(super) fn create_exe_stub(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let content = format!("#!/usr/bin/env bash\nexit 0\n{}\n", "x".repeat(1100));
    fs::write(&path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

pub(super) fn build_canonical_native_hook_settings(hooks_bin: &Path) -> serde_json::Value {
    let mut settings = serde_json::json!({});
    let root = hooks::ensure_object(&mut settings);
    let hooks_map = root
        .entry("hooks")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let hooks_map = hooks::ensure_object(hooks_map);

    for entry in CANONICAL_AMPLIHACK_HOOK_CONTRACT
        .iter()
        .filter(|entry| entry.native_subcmd.is_some())
    {
        let wrappers = hooks_map
            .entry(entry.event)
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let wrappers = hooks::ensure_array(wrappers);
        let spec = HookSpec {
            event: entry.event,
            cmd: HookCommandKind::BinarySubcmd {
                subcmd: entry.native_subcmd.expect("filtered to native hooks"),
            },
            timeout: entry.timeout,
            matcher: entry.matcher,
        };
        wrappers.push(hooks::build_hook_wrapper(&spec, hooks_bin));
    }

    settings
}
