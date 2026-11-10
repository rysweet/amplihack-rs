//! Issue #1878 item 1 (R3, D2 approach (a)): nothing `amplihack install`
//! writes to `~/.claude/settings.json` turns XPIA on or registers an XPIA
//! shell hook, with or without the optional
//! `~/.amplihack/.claude/tools/xpia/hooks` asset directory.
//!
//! The binary runs through `isolated(..)` with a fresh temp HOME per case, a
//! PATH of stub tools only, and every `XPIA_*` variable removed. Each case
//! runs `install --local` twice in the same HOME: the second run is the
//! re-registration `amplihack update` performs after it replaces the binary.
//! `update` itself needs the GitHub releases API and replaces the running
//! binary, so it cannot run offline in a test; the profile-refresh writer it
//! shares is covered by
//! `install::tests::profile_refresh_install::xpia_register_hooks_with_xpia_assets_writes_no_xpia_enablement`.
#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

const XPIA_ENV_KEYS: &[&str] = &[
    "XPIA_ENABLED",
    "XPIA_SECURITY_LEVEL",
    "XPIA_VERBOSE_FEEDBACK",
    "XPIA_BLOCK_HIGH_RISK",
    "XPIA_BLOCK_CRITICAL",
    "XPIA_VALIDATE_WEBFETCH",
    "XPIA_VALIDATE_BASH",
    "XPIA_VALIDATE_AGENTS",
    "XPIA_LOG_EVENTS",
    "XPIA_LOG_FILE",
    "XPIA_MAX_PROMPT_LENGTH",
    "XPIA_MAX_URL_LENGTH",
    "XPIA_WHITELIST_DOMAINS",
    "XPIA_WHITELIST_FILE",
    "XPIA_BLACKLIST_DOMAINS",
    "XPIA_BLACKLIST_FILE",
];

fn executable(path: &Path, script: &str) {
    crate::support::exec_stub::write_executable(path, script, 0o700);
}

fn link_host_tools(tools: &Path) {
    for tool in [
        "sh",
        "mkdir",
        "cat",
        "cp",
        "chmod",
        "dirname",
        "rm",
        "mv",
        "git",
        "uname",
        "head",
        "sed",
        "date",
        "cut",
        "sha256sum",
    ] {
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        assert!(out.status.success(), "{tool} not on the host PATH");
        std::os::unix::fs::symlink(
            String::from_utf8(out.stdout).unwrap().trim(),
            tools.join(tool),
        )
        .unwrap();
    }
}

fn install(home: &Path, tools: &Path) -> std::process::Output {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = crate::isolated_amplihack::isolated(amplihack_bin());
    for key in XPIA_ENV_KEYS {
        command.env_remove(key);
    }
    command
        .args(["install", "--local", source.to_str().unwrap()])
        .env("HOME", home)
        .env("PATH", tools)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_SOURCE_ROOT", &source)
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env(
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
            tools.join("amplihack-hooks"),
        )
        .env("RECIPE_RUNNER_RS_PATH", tools.join("recipe-runner-rs"))
        .env_remove("AMPLIHACK_ORCHESTRATION_BOOTSTRAP")
        .env_remove("AMPLIHACK_DEFAULT_MODEL")
        .env_remove("AMPLIHACK_HOME")
        .env_remove("AMPLIHACK_ASSET_RESOLVER")
        .env_remove("AMPLIHACK_SESSION_ID")
        .env_remove("AMPLIHACK_SESSION_DEPTH")
        .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
        .env_remove("AMPLIHACK_CLAUDE_BINARY_PATH")
        .env_remove("AMPLIHACK_CLAUDE_PROFILE_ROOT")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("COPILOT_HOME")
        .current_dir(home)
        .output()
        .unwrap()
}

/// Every hook command string in `settings`, by event.
fn commands(settings: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(hooks) = settings["hooks"].as_object() {
        for (event, wrappers) in hooks {
            for wrapper in wrappers.as_array().into_iter().flatten() {
                for hook in wrapper["hooks"].as_array().into_iter().flatten() {
                    if let Some(c) = hook["command"].as_str() {
                        out.push((event.clone(), c.to_string()));
                    }
                }
            }
        }
    }
    out
}

fn run_case(with_xpia_assets: bool) {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir(&tools).unwrap();
    link_host_tools(&tools);
    executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n");
    executable(&tools.join("npm"), "#!/bin/sh\necho 10.9.0\n");
    executable(&tools.join("amplihack-hooks"), "#!/bin/sh\necho '{}'\n");
    executable(
        &tools.join("recipe-runner-rs"),
        "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[]}'\n",
    );
    executable(
        &tools.join("claude"),
        "#!/bin/sh\ncase \"$*\" in\n *--version*) echo '2.1.306 (Claude Code)';;\n *) printf '%s\\n' \"$*\" >> \"$HOME/claude-calls\";;\nesac\n",
    );
    let claude_dir = home.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    if with_xpia_assets {
        let xpia = home.path().join(".amplihack/.claude/tools/xpia/hooks");
        fs::create_dir_all(&xpia).unwrap();
        // A legacy settings.json from before the shared hook binary: the
        // install must replace this entry, not keep or duplicate it.
        fs::write(
            claude_dir.join("settings.json"),
            json!({"hooks": {"PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": format!("python3 {}/pre_tool_use.py", xpia.display())
                }]
            }]}})
            .to_string(),
        )
        .unwrap();
    }

    for run in ["install", "re-install (update)"] {
        let out = install(home.path(), &tools);
        assert!(
            out.status.success(),
            "{run} failed (xpia assets: {with_xpia_assets}):\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let text = fs::read_to_string(claude_dir.join("settings.json")).unwrap();
        let settings: Value = serde_json::from_str(&text).unwrap();
        let ctx = format!("after {run}, xpia assets: {with_xpia_assets}");

        assert!(
            !text.contains("XPIA_ENABLED"),
            "{ctx}: settings.json sets XPIA_ENABLED:\n{settings:#}"
        );
        for (event, command) in commands(&settings) {
            assert!(
                !command.contains("tools/xpia/"),
                "{ctx}: {event} runs an XPIA shell hook: {command}"
            );
        }
        if let Some(env) = settings["env"].as_object() {
            for key in env.keys() {
                assert!(
                    !key.starts_with("XPIA_"),
                    "{ctx}: env sets {key}:\n{settings:#}"
                );
            }
        }
        for (event, subcommand) in [
            ("SessionStart", "session-start"),
            ("PreToolUse", "pre-tool-use"),
            ("PostToolUse", "post-tool-use"),
        ] {
            let count = commands(&settings)
                .iter()
                .filter(|(e, c)| {
                    e == event && c.contains("amplihack-hooks") && c.ends_with(subcommand)
                })
                .count();
            assert_eq!(
                count, 1,
                "{ctx}: {event} {subcommand} registered {count} times:\n{settings:#}"
            );
        }
        let pre: Vec<String> = commands(&settings)
            .into_iter()
            .filter(|(e, _)| e == "PreToolUse")
            .map(|(_, c)| c)
            .collect();
        let unique: std::collections::BTreeSet<&String> = pre.iter().collect();
        assert_eq!(
            unique.len(),
            pre.len(),
            "{ctx}: duplicate PreToolUse entry: {pre:?}"
        );
    }
}

#[test]
fn xpia_install_without_xpia_assets_writes_no_xpia() {
    run_case(false);
}

#[test]
fn xpia_install_with_xpia_assets_writes_no_xpia() {
    run_case(true);
}
