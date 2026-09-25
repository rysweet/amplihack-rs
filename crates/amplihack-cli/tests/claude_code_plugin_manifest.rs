//! The repository root is a Claude Code plugin (and a one-plugin marketplace),
//! so amplihack installs from git with no `amplihack install`:
//!
//! ```text
//! /plugin marketplace add rysweet/amplihack-rs
//! /plugin install amplihack@amplihack
//! ```
//!
//! That is the only way amplihack reaches a Claude Code cloud session, where
//! nothing has staged `~/.claude/skills` or wired `~/.claude/settings.json`.
//!
//! Claude Code validates these files at load time and drops what it rejects
//! without failing the session, so a broken manifest looks like "amplihack has
//! no skills" rather than an error. This test pins what the loader needs:
//!
//! * every component path is `./`-relative and stays inside the plugin root
//!   (symlinks or `..` escaping it are refused at load);
//! * `agents` lists files, not directories (directories are "Invalid input"),
//!   and covers exactly the Claude agents under `amplifier-bundle/agents/`
//!   so a new agent cannot be silently left out;
//! * the plugin's hooks mirror the events `amplihack install` registers;
//! * the plugin version tracks `package.json`;
//! * the hook wrapper never breaks a session and never doubles hooks that
//!   `amplihack install` already registered.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/amplihack-cli
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn read_json(rel: &str) -> Value {
    let path = repo_root().join(rel);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn plugin() -> Value {
    read_json(".claude-plugin/plugin.json")
}

/// A manifest path must be `./`-relative, free of `..`, and exist.
fn assert_contained(field: &str, rel: &str) {
    assert!(
        rel.starts_with("./"),
        "plugin.json {field} path {rel:?} must start with ./"
    );
    assert!(
        !rel.split('/').any(|part| part == ".."),
        "plugin.json {field} path {rel:?} must not contain .."
    );
    let full = repo_root().join(rel);
    assert!(
        full.exists(),
        "plugin.json {field} path {rel:?} does not exist"
    );
    let canonical = full.canonicalize().unwrap();
    assert!(
        canonical.starts_with(repo_root().canonicalize().unwrap()),
        "plugin.json {field} path {rel:?} resolves outside the plugin root"
    );
}

#[test]
fn manifest_identity_matches_the_package() {
    let plugin = plugin();
    assert_eq!(plugin["name"], "amplihack");
    let package = read_json("package.json");
    assert_eq!(
        plugin["version"], package["version"],
        "bump .claude-plugin/plugin.json version together with package.json"
    );
    // `claude plugin validate` rejects a bare-string author.
    assert!(
        plugin["author"].is_object() && plugin["author"]["name"].is_string(),
        "author must be an object with a name"
    );
    // No explicit skills: the default ./skills (a symlink to
    // amplifier-bundle/skills inside the root) already loads them, and naming
    // amplifier-bundle/skills too would load every skill twice.
    assert!(plugin.get("skills").is_none());
    let skills = repo_root().join("skills");
    assert!(skills.join("dev-orchestrator/SKILL.md").is_file());
    assert!(skills.join("default-workflow/SKILL.md").is_file());
}

#[test]
fn component_paths_stay_inside_the_plugin_root() {
    let plugin = plugin();
    let commands = plugin["commands"].as_str().expect("commands is a path");
    assert_contained("commands", commands);
    let hooks = plugin["hooks"].as_str().expect("hooks is a path");
    assert_contained("hooks", hooks);
    for agent in plugin["agents"].as_array().expect("agents is an array") {
        assert_contained("agents", agent.as_str().expect("agent entry is a path"));
    }
}

#[test]
fn agents_list_exactly_the_bundled_claude_agents() {
    const AGENT_DIRS: &[&str] = &[
        "amplifier-bundle/agents/core",
        "amplifier-bundle/agents/specialized",
        "amplifier-bundle/agents/workflows",
    ];
    let listed: BTreeSet<String> = plugin()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();

    let mut on_disk = BTreeSet::new();
    for dir in AGENT_DIRS {
        for entry in fs::read_dir(repo_root().join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let name = path.file_name().unwrap().to_string_lossy();
                on_disk.insert(format!("./{dir}/{name}"));
            }
        }
    }

    for agent in &listed {
        assert!(
            agent.ends_with(".md"),
            "{agent}: Claude Code rejects directories in plugin.json agents; list files"
        );
        assert!(
            !agent.starts_with("./agents/"),
            "{agent}: ./agents/ holds non-Claude domain agents"
        );
    }
    let missing: Vec<_> = on_disk.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(&on_disk).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "plugin.json agents is out of sync with amplifier-bundle/agents.\n\
         add to plugin.json: {missing:?}\nremove from plugin.json: {extra:?}"
    );
}

#[test]
fn marketplace_publishes_the_repo_root() {
    let marketplace = read_json(".claude-plugin/marketplace.json");
    assert_eq!(marketplace["name"], "amplihack");
    assert!(marketplace["owner"]["name"].is_string());
    let plugins = marketplace["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "amplihack");
    assert_eq!(plugins[0]["source"], "./");
}

/// (event, matcher, subcommands in order). Mirrors `AMPLIHACK_HOOK_SPECS` in
/// `commands/install/types.rs`, which is not visible to integration tests.
const EXPECTED_HOOKS: &[(&str, Option<&str>, &[&str])] = &[
    ("SessionStart", None, &["bootstrap", "session-start"]),
    (
        "UserPromptSubmit",
        None,
        &["workflow-classification-reminder", "user-prompt-submit"],
    ),
    ("PreToolUse", Some("*"), &["pre-tool-use"]),
    ("PostToolUse", Some("*"), &["post-tool-use"]),
    ("Stop", None, &["stop"]),
    ("PreCompact", None, &["pre-compact"]),
    ("SessionEnd", None, &["session-stop-event"]),
];

#[test]
fn plugin_hooks_mirror_the_installer() {
    let hooks = read_json("claude-plugin/hooks.json");
    let events = hooks["hooks"]
        .as_object()
        .expect("hooks.hooks is an object");
    let expected_events: BTreeSet<&str> = EXPECTED_HOOKS.iter().map(|(e, _, _)| *e).collect();
    let actual_events: BTreeSet<&str> = events.keys().map(String::as_str).collect();
    assert_eq!(actual_events, expected_events);

    for (event, matcher, subcommands) in EXPECTED_HOOKS {
        let groups = events[*event].as_array().unwrap();
        assert_eq!(groups.len(), 1, "{event}: one matcher group");
        assert_eq!(
            groups[0].get("matcher").and_then(Value::as_str),
            *matcher,
            "{event}: matcher"
        );
        let commands: Vec<&str> = groups[0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert_eq!(commands.len(), subcommands.len(), "{event}: hook count");
        for (command, sub) in commands.iter().zip(subcommands.iter()) {
            assert!(
                command.starts_with("\"${CLAUDE_PLUGIN_ROOT}/claude-plugin/bin/"),
                "{event}: {command} must run a script from the plugin"
            );
            let expected_tail = if *sub == "bootstrap" {
                "/bootstrap\"".to_owned()
            } else {
                format!("/amplihack-hook\" {sub}")
            };
            assert!(
                command.ends_with(&expected_tail),
                "{event}: expected {sub}, got {command}"
            );
        }
    }

    // Cheap drift check against the installer's own list.
    let types =
        fs::read_to_string(repo_root().join("crates/amplihack-cli/src/commands/install/types.rs"))
            .unwrap();
    for (event, _, subcommands) in EXPECTED_HOOKS {
        assert!(types.contains(&format!("event: \"{event}\"")), "{event}");
        for sub in subcommands.iter().filter(|s| **s != "bootstrap") {
            assert!(
                types.contains(&format!("subcmd: \"{sub}\"")),
                "installer no longer registers {sub}; update claude-plugin/hooks.json"
            );
        }
    }
}

#[test]
fn install_runtime_never_runs_amplihack_install() {
    // `amplihack install` would publish every skill a second time into
    // ~/.claude/skills and hooks into settings.json, duplicating the plugin.
    let script = fs::read_to_string(repo_root().join("claude-plugin/bin/install-runtime")).unwrap();
    for line in script.lines() {
        let code = line.trim_start();
        assert!(
            code.starts_with('#') || !code.contains("amplihack install"),
            "install-runtime must not run `amplihack install`: {line}"
        );
    }
}

#[cfg(unix)]
mod shell {
    use super::repo_root;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Held while writing an executable and while forking. A thread that forks
    /// while another still has a script open for writing hands that fd to its
    /// child until exec, and exec of the script then fails with ETXTBSY.
    static EXEC_LOCK: Mutex<()> = Mutex::new(());

    const SCRIPTS: &[&str] = &["amplihack-hook", "bootstrap", "install-runtime"];

    #[test]
    fn plugin_scripts_are_executable() {
        for name in SCRIPTS {
            let path = repo_root().join("claude-plugin/bin").join(name);
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{} is not executable", path.display());
        }
    }

    fn write_exe(path: &Path, body: &str) {
        let _guard = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Run a plugin script with a hermetic HOME/PATH: only `stub` and the
    /// system directories are on PATH, so no real amplihack binary is found.
    fn run(
        script: &Path,
        home: &Path,
        stub: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Output {
        let mut cmd = Command::new(script);
        cmd.args(args)
            .env_clear()
            .env("HOME", home)
            .env("PATH", format!("{}:/usr/bin:/bin", stub.display()))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = {
            let _guard = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            cmd.spawn().unwrap()
        };
        // A no-op path exits without reading stdin, so the write can race a
        // closed pipe; that is the behavior under test, not a failure.
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"hook\":\"payload\"}");
        child.wait_with_output().unwrap()
    }

    fn hook_script() -> std::path::PathBuf {
        repo_root().join("claude-plugin/bin/amplihack-hook")
    }

    /// A stub amplihack-hooks that records its argv and stdin.
    fn stub_hooks(stub: &Path) {
        write_exe(
            &stub.join("amplihack-hooks"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$HOME/called\"\ncat >> \"$HOME/called\"\necho '{\"ok\":true}'\n",
        );
    }

    #[test]
    fn hook_is_a_silent_noop_without_the_binary() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &[]);
        assert!(out.status.success());
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }

    #[test]
    fn hook_defers_to_hooks_registered_by_amplihack_install() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        stub_hooks(stub.path());
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"/x/amplihack-hooks\" stop"}]}]}}"#,
        )
        .unwrap();
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &[]);
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!home.path().join("called").exists(), "binary must not run");
    }

    #[test]
    fn hook_forwards_subcommand_and_stdin() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        stub_hooks(stub.path());
        let out = run(
            &hook_script(),
            home.path(),
            stub.path(),
            &["pre-tool-use"],
            &[],
        );
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "{\"ok\":true}");
        let called = fs::read_to_string(home.path().join("called")).unwrap();
        assert_eq!(called, "pre-tool-use\n{\"hook\":\"payload\"}");
    }

    /// bootstrap next to a stub install-runtime that just leaves a marker.
    fn bootstrap_fixture() -> tempfile::TempDir {
        let bin = tempfile::tempdir().unwrap();
        let bootstrap =
            fs::read_to_string(repo_root().join("claude-plugin/bin/bootstrap")).unwrap();
        write_exe(&bin.path().join("bootstrap"), &bootstrap);
        write_exe(
            &bin.path().join("install-runtime"),
            "#!/bin/sh\ntouch \"$HOME/installer-ran\"\nrm -rf \"$AMPLIHACK_PLUGIN_INSTALL_LOCK\"\n",
        );
        bin
    }

    #[test]
    fn bootstrap_exports_env_and_stays_quiet_when_runtime_present() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        for tool in ["amplihack", "amplihack-hooks", "recipe-runner-rs"] {
            write_exe(&stub.path().join(tool), "#!/bin/sh\n");
        }
        let bin = bootstrap_fixture();
        let env_file = home.path().join("env.sh");
        let env_file = env_file.to_str().unwrap();
        let envs = [
            ("CLAUDE_ENV_FILE", env_file),
            ("CLAUDE_PLUGIN_ROOT", "/plugin/root"),
        ];
        let script = bin.path().join("bootstrap");
        let out = run(&script, home.path(), stub.path(), &[], &envs);
        assert!(out.status.success());
        assert!(
            out.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        // Idempotent: a second session start adds nothing.
        run(&script, home.path(), stub.path(), &[], &envs);
        let exported = fs::read_to_string(env_file).unwrap();
        assert_eq!(
            exported.matches("AMPLIHACK_HOME=\"/plugin/root\"").count(),
            1
        );
        assert_eq!(exported.matches("export PATH=").count(), 1);
        assert!(exported.contains("AMPLIHACK_AGENT_BINARY"));
    }

    #[test]
    fn bootstrap_explains_how_to_install_when_auto_install_is_off() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let bin = bootstrap_fixture();
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &[
                ("AMPLIHACK_PLUGIN_AUTO_INSTALL", "0"),
                ("CLAUDE_CODE_REMOTE", "true"),
            ],
        );
        assert!(out.status.success());
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(json["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(context.contains("install-runtime"), "{context}");
        assert!(!home.path().join("installer-ran").exists());
    }

    #[test]
    fn bootstrap_starts_the_installer_in_the_background_in_cloud_sessions() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let bin = bootstrap_fixture();
        let started = Instant::now();
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &[
                ("CLAUDE_CODE_REMOTE", "true"),
                ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
            ],
        );
        assert!(out.status.success());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "bootstrap must not block"
        );
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("being installed"), "{context}");
        assert!(data.path().join("install-runtime.log").exists());

        let marker = home.path().join("installer-ran");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(marker.exists(), "install-runtime was not started");
    }
}
