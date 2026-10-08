//! Private roots for CLI children testing recipe conditions, not startup repair.
use std::{fs, path::Path, path::PathBuf, process::Command};

fn selected_tool(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{name} required for terminal-gate test"))
}

pub(super) fn isolate(command: &mut Command, root: &Path) {
    let runner = std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| selected_tool("recipe-runner-rs"));
    let runner = runner.canonicalize().expect("selected recipe runner");
    let bin = root.join("tools");
    fs::create_dir_all(&bin).expect("private tools");
    for name in ["bash", "sh", "jq", "git"] {
        let selected = selected_tool(name).canonicalize().expect("selected tool");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&selected, bin.join(name)).expect("private tool link");
        #[cfg(not(unix))]
        fs::copy(&selected, bin.join(name)).expect("private tool copy");
    }
    command.env_clear();
    for (key, name) in [
        ("HOME", "home"),
        ("CODEX_HOME", "codex"),
        ("CARGO_HOME", "cargo"),
        ("XDG_CONFIG_HOME", "xdg/config"),
        ("XDG_CACHE_HOME", "xdg/cache"),
        ("XDG_DATA_HOME", "xdg/data"),
        ("XDG_STATE_HOME", "xdg/state"),
        ("CLAUDE_CONFIG_DIR", "claude"),
        ("CLAUDE_PLUGIN_DATA", "claude-data"),
        ("COPILOT_HOME", "copilot"),
        ("COPILOT_CONFIG_DIR", "copilot-config"),
        ("GH_CONFIG_DIR", "gh"),
        ("TMPDIR", "tmp"),
    ] {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("private child root");
        command.env(key, path);
    }
    command
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("RECIPE_RUNNER_RS_PATH", runner)
        // This test checks skip conditions. Provider startup repair has its
        // own regressions with self-heal enabled.
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1");
}
