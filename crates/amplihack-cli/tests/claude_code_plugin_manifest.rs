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
//! * the plugin carries no `version`, so installs follow the commit;
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
    // Claude Code keys the plugin cache by `version` when one is set, and an
    // unchanged version means no update reaches installed users. Releases
    // advance by tag while package.json and Cargo.toml stay at the workspace
    // base version, so any version written here would freeze every install at
    // the commit it first saw. With none, the cache follows the commit.
    assert!(
        plugin.get("version").is_none(),
        "plugin.json must not pin a version; see the comment above"
    );
    let marketplace = read_json(".claude-plugin/marketplace.json");
    assert!(marketplace["plugins"][0].get("version").is_none());
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
    // And the other direction: a hook the installer gains must reach the plugin.
    let specs = types
        .split("AMPLIHACK_HOOK_SPECS: &[HookSpec] = &[")
        .nth(1)
        .and_then(|rest| rest.split("\n];").next())
        .expect("AMPLIHACK_HOOK_SPECS in install/types.rs");
    let plugin_subcommands: BTreeSet<&str> = EXPECTED_HOOKS
        .iter()
        .flat_map(|(_, _, subs)| subs.iter().copied())
        .collect();
    for sub in specs.split("subcmd: \"").skip(1) {
        let sub = sub.split('"').next().unwrap();
        assert!(
            plugin_subcommands.contains(sub),
            "the installer registers {sub}; add it to claude-plugin/hooks.json and EXPECTED_HOOKS"
        );
    }
}

#[test]
fn recipe_runner_is_pinned_to_a_commit() {
    // install-runtime builds recipe-runner-rs without asking, so the plugin
    // commit pins exactly what it builds rather than following a branch.
    let rev = fs::read_to_string(repo_root().join("claude-plugin/recipe-runner.rev")).unwrap();
    let rev = rev.trim();
    assert_eq!(rev.len(), 40, "full commit sha expected, got {rev:?}");
    assert!(rev.chars().all(|c| c.is_ascii_hexdigit()), "{rev:?}");
    let script = fs::read_to_string(repo_root().join("claude-plugin/bin/install-runtime")).unwrap();
    assert!(script.contains("--rev \"$rev\""));
    assert!(
        !script.contains("--branch"),
        "install-runtime must not follow a branch"
    );
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
        run_with_input(script, home, stub, args, envs, b"{\"hook\":\"payload\"}")
    }

    fn run_with_input(
        script: &Path,
        home: &Path,
        stub: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        input: &[u8],
    ) -> Output {
        let mut cmd = Command::new(script);
        cmd.args(args)
            .env_clear()
            // Hermetic project scope: CLAUDE_PROJECT_DIR defaults to the cwd.
            .current_dir(home)
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
        let _ = child.stdin.take().unwrap().write_all(input);
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
        let registered = stub.path().join("amplihack-hooks");
        fs::write(
            home.path().join(".claude/settings.json"),
            format!(
                r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"\"{}\" stop"}}]}}]}}}}"#,
                registered.display()
            ),
        )
        .unwrap();
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &[]);
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!home.path().join("called").exists(), "binary must not run");
    }

    #[test]
    fn a_registration_whose_binary_is_gone_does_not_disable_the_plugin_hooks() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        stub_hooks(stub.path());
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"/gone/amplihack-hooks\" stop"}]}]}}"#,
        )
        .unwrap();
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &[]);
        assert!(out.status.success());
        assert!(home.path().join("called").exists(), "binary must run");
    }

    #[test]
    fn hook_points_amplihack_hooks_at_the_plugin_root() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        write_exe(
            &stub.path().join("amplihack-hooks"),
            "#!/bin/sh\nprintf '%s' \"$AMPLIHACK_HOME\" > \"$HOME/home-seen\"\n",
        );
        let envs = [("CLAUDE_PLUGIN_ROOT", "/plugin/root")];
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &envs);
        assert!(out.status.success());
        let seen = fs::read_to_string(home.path().join("home-seen")).unwrap();
        assert_eq!(seen, "/plugin/root");
    }

    #[test]
    fn hook_defers_to_hooks_registered_in_the_project_scope() {
        // `amplihack install --interactive` can register hooks repo-locally.
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        stub_hooks(stub.path());
        fs::create_dir_all(project.path().join(".claude")).unwrap();
        let registered = stub.path().join("amplihack-hooks");
        fs::write(
            project.path().join(".claude/settings.json"),
            format!(
                "{{\n  \"hooks\": {{\"Stop\": [{{\"hooks\": [{{\n    \"type\": \"command\",\n    \"command\": \"\\\"{}\\\" stop\"\n  }}]}}]}}\n}}\n",
                registered.display()
            ),
        )
        .unwrap();
        let envs = [("CLAUDE_PROJECT_DIR", project.path().to_str().unwrap())];
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &envs);
        assert!(out.status.success());
        assert!(!home.path().join("called").exists(), "binary must not run");
    }

    #[test]
    fn a_mere_mention_of_amplihack_hooks_does_not_disable_the_plugin_hooks() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        stub_hooks(stub.path());
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        fs::write(
            home.path().join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(amplihack-hooks:*)"]}}"#,
        )
        .unwrap();
        let out = run(&hook_script(), home.path(), stub.path(), &["stop"], &[]);
        assert!(out.status.success());
        assert!(home.path().join("called").exists(), "binary must run");
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
        fs::copy(
            repo_root().join("claude-plugin/bin/plugin-lock.sh"),
            bin.path().join("plugin-lock.sh"),
        )
        .unwrap();
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

    fn wait_for(path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        path.exists()
    }

    /// Stubs for all three runtime binaries, so nothing reads as missing.
    fn stub_runtime(stub: &Path) {
        for tool in ["amplihack", "amplihack-hooks", "recipe-runner-rs"] {
            write_exe(&stub.join(tool), "#!/bin/sh\n");
        }
    }

    #[test]
    fn bootstrap_reinstalls_when_the_plugin_version_changes() {
        // The runtime must follow plugin updates, not stay at the first install.
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        stub_runtime(stub.path());
        fs::write(data.path().join("runtime.stamp"), "oldcommit\n").unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_ROOT", "/cache/amplihack/newcommit"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success());
        assert!(
            out.stdout.is_empty(),
            "nothing is missing, so nothing to tell Claude"
        );
        assert!(
            wait_for(&home.path().join("installer-ran")),
            "stale runtime not reconciled"
        );
    }

    #[test]
    fn bootstrap_does_nothing_when_the_runtime_matches_the_plugin() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        stub_runtime(stub.path());
        fs::write(data.path().join("runtime.stamp"), "samecommit\n").unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_ROOT", "/cache/amplihack/samecommit"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success() && out.stdout.is_empty());
        std::thread::sleep(Duration::from_millis(300));
        assert!(!home.path().join("installer-ran").exists());
        assert!(!data.path().join("install.lock").exists());
    }

    #[test]
    fn bootstrap_backs_off_after_a_failed_install() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        fs::write(data.path().join("install.failed"), "thiscommit\n").unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_ROOT", "/cache/amplihack/thiscommit"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success());
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("failed"), "{context}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !home.path().join("installer-ran").exists(),
            "must not retry yet"
        );
    }

    #[test]
    fn bootstrap_never_starts_an_install_on_compaction() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run_with_input(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
            br#"{"hook_event_name":"SessionStart","source": "compact"}"#,
        );
        assert!(out.status.success());
        std::thread::sleep(Duration::from_millis(300));
        assert!(!home.path().join("installer-ran").exists());
    }

    /// Age a path with POSIX `touch -t` (GNU `-d` is not portable).
    fn age(path: &Path) {
        let aged = Command::new("touch")
            .args(["-t", "202001010000"])
            .arg(path)
            .status()
            .unwrap();
        assert!(aged.success());
    }

    #[test]
    fn bootstrap_leaves_a_live_installers_lock_alone_however_old() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let lock = data.path().join("install.lock");
        fs::create_dir(&lock).unwrap();
        // A process whose command line names install-runtime stands in for a
        // live installer.
        let mut installer = Command::new("sh")
            .args(["-c", "sleep 30", "install-runtime"])
            .spawn()
            .unwrap();
        fs::write(lock.join("pid"), format!("{}\n", installer.id())).unwrap();
        age(&lock);
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        std::thread::sleep(Duration::from_millis(300));
        let _ = installer.kill();
        let _ = installer.wait();
        assert!(out.status.success());
        assert!(
            !home.path().join("installer-ran").exists(),
            "second installer started"
        );
    }

    #[test]
    fn bootstrap_reclaims_a_lock_whose_pid_now_belongs_to_something_else() {
        // After a SIGKILL or a reboot the recorded pid can be reused.
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let lock = data.path().join("install.lock");
        fs::create_dir(&lock).unwrap();
        // This test process is alive but is not an installer.
        fs::write(lock.join("pid"), format!("{}\n", std::process::id())).unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success());
        assert!(
            wait_for(&home.path().join("installer-ran")),
            "stale lock not reclaimed"
        );
    }

    #[test]
    fn bootstrap_retries_once_the_backoff_expires() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let failed = data.path().join("install.failed");
        fs::write(&failed, "thiscommit\n").unwrap();
        age(&failed);
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_ROOT", "/cache/amplihack/thiscommit"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success());
        assert!(
            wait_for(&home.path().join("installer-ran")),
            "backoff never expired"
        );
    }

    #[test]
    fn bootstrap_exports_env_even_when_stdin_never_closes() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let bin = bootstrap_fixture();
        let env_file = home.path().join("env.sh");
        let mut cmd = Command::new(bin.path().join("bootstrap"));
        cmd.env_clear()
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", stub.path().display()))
            .env("CLAUDE_ENV_FILE", &env_file)
            .env("CLAUDE_CODE_REMOTE", "true")
            .env("CLAUDE_PLUGIN_DATA", home.path().join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = {
            let _guard = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            cmd.spawn().unwrap()
        };
        let open_stdin = child.stdin.take();
        let exported = wait_for(&env_file);
        drop(open_stdin);
        let _ = child.wait();
        assert!(exported, "no environment exported while stdin stayed open");
        assert!(
            fs::read_to_string(&env_file)
                .unwrap()
                .contains("AMPLIHACK_HOME")
        );
    }

    #[test]
    fn bootstrap_marks_root_cloud_sessions_as_sandboxed() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        stub_runtime(stub.path());
        let bin = bootstrap_fixture();
        let env_file = home.path().join("env.sh");
        let envs = [
            ("CLAUDE_ENV_FILE", env_file.to_str().unwrap()),
            ("CLAUDE_CODE_REMOTE", "true"),
            ("AMPLIHACK_PLUGIN_AUTO_INSTALL", "0"),
        ];
        run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        let exported = fs::read_to_string(&env_file).unwrap();
        let uid = Command::new("id").arg("-u").output().unwrap();
        let is_root = String::from_utf8_lossy(&uid.stdout).trim() == "0";
        assert_eq!(
            exported.contains("export IS_SANDBOX=1"),
            is_root,
            "{exported}"
        );

        // Never outside a cloud session.
        let local = tempfile::tempdir().unwrap();
        let env_file = local.path().join("env.sh");
        let envs = [
            ("CLAUDE_ENV_FILE", env_file.to_str().unwrap()),
            ("AMPLIHACK_PLUGIN_AUTO_INSTALL", "0"),
        ];
        run(
            &bin.path().join("bootstrap"),
            local.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(
            !fs::read_to_string(&env_file)
                .unwrap()
                .contains("IS_SANDBOX")
        );
    }

    struct InstallRun {
        out: Output,
        home: tempfile::TempDir,
        data: tempfile::TempDir,
    }

    impl InstallRun {
        fn log(&self) -> String {
            String::from_utf8_lossy(&self.out.stdout).into_owned()
        }
    }

    /// Run install-runtime hermetically. `amplihack` and `amplihack-hooks`
    /// stubs report `installed` from ~/.local/bin, recorded as plugin-owned
    /// when `owned`. `curl` and `node` always fail, so an attempted download is visible
    /// in the log and never touches the network. `cargo` records its argv and
    /// installs a recipe-runner-rs stub unless `cargo_ok` is false.
    fn install_runtime(installed: &str, owned: bool, cargo_ok: bool, want: &str) -> InstallRun {
        install_runtime_with(installed, true, owned, cargo_ok, want, &[])
    }

    /// As `install_runtime`, optionally without amplihack-hooks, and with
    /// `extra` stubs (name, script) shadowing system tools.
    fn install_runtime_with(
        installed: &str,
        with_hooks: bool,
        owned: bool,
        cargo_ok: bool,
        want: &str,
        extra: &[(&str, &str)],
    ) -> InstallRun {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let dest = home.path().join(".local/bin");
        fs::create_dir_all(&dest).unwrap();
        let mut bins = vec![(
            "amplihack",
            format!("#!/bin/sh\necho 'amplihack {installed}'\n"),
        )];
        if with_hooks {
            bins.push(("amplihack-hooks", "#!/bin/sh\n".to_owned()));
        }
        let mut record = String::new();
        for (name, body) in &bins {
            write_exe(&dest.join(name), body);
            let sum = Command::new("sha256sum")
                .arg(dest.join(name))
                .output()
                .unwrap();
            let sum = String::from_utf8_lossy(&sum.stdout);
            record.push_str(&format!(
                "{name} {}\n",
                sum.split_whitespace().next().unwrap()
            ));
        }
        if owned {
            fs::write(data.path().join("owned-binaries"), record).unwrap();
        }
        // No network: curl and node stubs shadow any real ones in /usr/bin.
        write_exe(&stub.path().join("curl"), "#!/bin/sh\nexit 7\n");
        write_exe(
            &stub.path().join("node"),
            "#!/bin/sh\necho 'node stub: no network' >&2\nexit 1\n",
        );
        let cargo = if cargo_ok {
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/cargo-args\"\nmkdir -p \"$HOME/.cargo/bin\"\nprintf '#!/bin/sh\\n' > \"$HOME/.cargo/bin/recipe-runner-rs\"\nchmod +x \"$HOME/.cargo/bin/recipe-runner-rs\"\n"
        } else {
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/cargo-args\"\nexit 101\n"
        };
        write_exe(&stub.path().join("cargo"), cargo);
        for (name, body) in extra {
            write_exe(&stub.path().join(name), body);
        }
        let root = repo_root();
        let envs = [
            ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
            ("AMPLIHACK_NPM_VERSION", want),
        ];
        let out = run(
            &root.join("claude-plugin/bin/install-runtime"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        InstallRun { out, home, data }
    }

    fn plugin_id() -> String {
        repo_root()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn install_runtime_builds_the_pinned_runner_and_stamps_the_plugin_version() {
        let run = install_runtime("1.2.3", true, true, "1.2.3");
        let log = run.log();
        assert!(run.out.status.success(), "{log}");
        assert!(log.contains("amplihack 1.2.3 already installed"), "{log}");
        let rev = fs::read_to_string(repo_root().join("claude-plugin/recipe-runner.rev")).unwrap();
        let args = fs::read_to_string(run.home.path().join("cargo-args")).unwrap();
        assert!(args.contains(&format!("--rev {}", rev.trim())), "{args}");
        let stamp = fs::read_to_string(run.data.path().join("runtime.stamp")).unwrap();
        assert_eq!(stamp.trim(), plugin_id());
        assert!(!run.data.path().join("install.failed").exists());
        assert!(
            !run.data.path().join("install.lock").exists(),
            "lock not released"
        );
    }

    #[test]
    fn install_runtime_replaces_its_own_binaries_from_another_release() {
        let run = install_runtime("1.0.0", true, true, "1.2.3");
        let log = run.log();
        assert!(
            log.contains("fetching amplihack release binaries (1.2.3; installed: 1.0.0)"),
            "{log}"
        );
        // The stub download fails, so 1.0.0 remains: that is a failure, not
        // a reconciled runtime.
        assert!(!run.out.status.success(), "{log}");
        assert!(!run.data.path().join("runtime.stamp").exists());
        assert!(run.data.path().join("install.failed").exists());
    }

    #[test]
    fn install_runtime_leaves_binaries_it_did_not_install_alone() {
        // e.g. `amplihack install` or a developer's own build in ~/.local/bin.
        let run = install_runtime("1.0.0", false, true, "1.2.3");
        let log = run.log();
        assert!(run.out.status.success(), "{log}");
        assert!(
            log.contains("was not installed by the plugin; leaving it as is"),
            "{log}"
        );
        assert!(!log.contains("fetching amplihack"), "{log}");
        let seen = Command::new(run.home.path().join(".local/bin/amplihack"))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&seen.stdout).trim(),
            "amplihack 1.0.0"
        );
        assert!(run.data.path().join("runtime.stamp").exists());
    }

    #[test]
    fn install_runtime_does_not_stamp_when_the_release_is_unknown() {
        let run = install_runtime("1.0.0", true, true, "");
        let log = run.log();
        assert!(run.out.status.success(), "{log}");
        assert!(!run.data.path().join("runtime.stamp").exists(), "{log}");
    }

    #[test]
    fn install_runtime_records_a_failure_for_bootstrap_to_back_off() {
        let run = install_runtime("1.2.3", true, false, "1.2.3");
        assert!(!run.out.status.success());
        let failed = fs::read_to_string(run.data.path().join("install.failed")).unwrap();
        assert_eq!(failed.trim(), plugin_id());
        assert!(!run.data.path().join("runtime.stamp").exists());
    }

    #[test]
    fn install_runtime_defers_to_an_install_already_running() {
        let home_data = tempfile::tempdir().unwrap();
        let lock = home_data.path().join("install.lock");
        fs::create_dir(&lock).unwrap();
        let mut installer = Command::new("sh")
            .args(["-c", "sleep 30", "install-runtime"])
            .spawn()
            .unwrap();
        fs::write(lock.join("pid"), format!("{}\n", installer.id())).unwrap();
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let root = repo_root();
        let envs = [
            ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
            ("CLAUDE_PLUGIN_DATA", home_data.path().to_str().unwrap()),
            ("AMPLIHACK_NPM_VERSION", "1.2.3"),
        ];
        let out = run(
            &root.join("claude-plugin/bin/install-runtime"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        let _ = installer.kill();
        let _ = installer.wait();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("another install is running"));
        assert!(lock.exists(), "a running installer's lock was removed");
    }

    #[test]
    fn install_runtime_does_not_shadow_a_lone_user_amplihack() {
        // A user's amplihack without amplihack-hooks is still the user's:
        // placing the plugin's pair in ~/.local/bin would shadow it.
        let run = install_runtime_with("1.0.0", false, false, true, "1.2.3", &[]);
        let log = run.log();
        assert!(log.contains("was not installed by the plugin"), "{log}");
        assert!(!log.contains("fetching amplihack"), "{log}");
        assert!(!run.home.path().join(".local/bin/amplihack-hooks").exists());
        // A settled user-managed state, not a failed install to retry.
        assert!(run.out.status.success(), "{log}");
        assert!(run.data.path().join("user-managed").exists());
        assert!(!run.data.path().join("install.failed").exists());
        assert!(!run.data.path().join("runtime.stamp").exists());
    }

    #[test]
    fn bootstrap_explains_an_incomplete_user_managed_runtime_without_retrying() {
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        for tool in ["amplihack", "recipe-runner-rs"] {
            write_exe(&stub.path().join(tool), "#!/bin/sh\n");
        }
        fs::write(data.path().join("user-managed"), "thiscommit\n").unwrap();
        let bin = bootstrap_fixture();
        let envs = [
            ("CLAUDE_CODE_REMOTE", "true"),
            ("CLAUDE_PLUGIN_ROOT", "/cache/amplihack/thiscommit"),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
        ];
        let out = run(
            &bin.path().join("bootstrap"),
            home.path(),
            stub.path(),
            &[],
            &envs,
        );
        assert!(out.status.success());
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let context = json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            context.contains("user-managed") && context.contains("amplihack-hooks"),
            "{context}"
        );
        assert!(
            !context.contains("rm "),
            "must not suggest a retry: {context}"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(!home.path().join("installer-ran").exists());
    }

    #[test]
    fn install_runtime_keeps_state_under_a_relocated_plugins_root() {
        // CLAUDE_CODE_PLUGIN_CACHE_DIR moves installed_plugins.json and
        // data/<id>/; a manual run (no CLAUDE_PLUGIN_DATA) must follow it.
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        let plugins = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("curl", "#!/bin/sh\nexit 7\n"),
            ("node", "#!/bin/sh\nexit 1\n"),
            ("cargo", "#!/bin/sh\nexit 101\n"),
        ] {
            write_exe(&stub.path().join(name), body);
        }
        let root = repo_root();
        let envs = [
            ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
            (
                "CLAUDE_CODE_PLUGIN_CACHE_DIR",
                plugins.path().to_str().unwrap(),
            ),
            ("AMPLIHACK_NPM_VERSION", "1.2.3"),
        ];
        let script = root.join("claude-plugin/bin/install-runtime");
        run(&script, home.path(), stub.path(), &[], &envs);
        let data = plugins.path().join("data/amplihack-amplihack");
        assert!(
            data.join("install.failed").exists(),
            "state not under the relocated root"
        );
        assert!(!home.path().join(".claude/plugins/data").exists());
    }

    #[test]
    fn install_runtime_treats_binaries_as_the_users_when_it_cannot_checksum() {
        // The owned record is correct, but with no working sha256sum/shasum
        // there is no evidence the binaries are still the plugin's.
        let broken = "#!/bin/sh\nexit 1\n";
        let run = install_runtime_with(
            "1.0.0",
            true,
            true,
            true,
            "1.2.3",
            &[("sha256sum", broken), ("shasum", broken)],
        );
        let log = run.log();
        assert!(log.contains("was not installed by the plugin"), "{log}");
        assert!(!log.contains("fetching amplihack"), "{log}");
    }

    #[test]
    fn install_runtime_tells_source_builds_which_release_they_stand_for() {
        let node = "#!/bin/sh\nprintf '%s' \"$AMPLIHACK_RELEASE_VERSION\" > \"$HOME/release-seen\"\nexit 1\n";
        let run = install_runtime_with("1.0.0", true, true, true, "1.2.3", &[("node", node)]);
        let seen = fs::read_to_string(run.home.path().join("release-seen")).unwrap();
        assert_eq!(seen, "1.2.3", "{}", run.log());
    }

    /// A manual install-runtime run against a lock prepared by `lock_setup`.
    fn manual_install_against(
        lock_setup: impl FnOnce(&Path),
        extra: &[(&str, &str)],
    ) -> (Output, tempfile::TempDir) {
        let data = tempfile::tempdir().unwrap();
        lock_setup(&data.path().join("install.lock"));
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        for (name, body) in extra {
            write_exe(&stub.path().join(name), body);
        }
        let root = repo_root();
        let envs = [
            ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
            ("CLAUDE_PLUGIN_DATA", data.path().to_str().unwrap()),
            ("AMPLIHACK_NPM_VERSION", "1.2.3"),
        ];
        let script = root.join("claude-plugin/bin/install-runtime");
        let out = run(&script, home.path(), stub.path(), &[], &envs);
        (out, data)
    }

    #[test]
    fn a_manual_run_leaves_a_lock_that_has_no_pid_yet() {
        // bootstrap has just created it and its installer is starting.
        let (out, data) = manual_install_against(|lock| fs::create_dir(lock).unwrap(), &[]);
        assert!(String::from_utf8_lossy(&out.stdout).contains("another install is running"));
        assert!(data.path().join("install.lock").exists());
    }

    #[test]
    fn without_a_usable_ps_a_live_installers_lock_is_kept() {
        let mut installer = Command::new("sh")
            .args(["-c", "sleep 30", "install-runtime"])
            .spawn()
            .unwrap();
        let pid = installer.id();
        let (out, data) = manual_install_against(
            |lock| {
                fs::create_dir(lock).unwrap();
                fs::write(lock.join("pid"), format!("{pid}\n")).unwrap();
            },
            &[("ps", "#!/bin/sh\nexit 1\n")],
        );
        let _ = installer.kill();
        let _ = installer.wait();
        assert!(String::from_utf8_lossy(&out.stdout).contains("another install is running"));
        assert!(data.path().join("install.lock").exists());
    }

    #[test]
    fn concurrent_manual_runs_never_install_at_the_same_time() {
        // Several runs reclaim the same dead-pid lock at once; at most one
        // may hold it, so the builds never overlap.
        let mut dead = Command::new("true").spawn().unwrap();
        let dead_pid = dead.id();
        dead.wait().unwrap();
        let data = tempfile::tempdir().unwrap();
        let lock = data.path().join("install.lock");
        fs::create_dir(&lock).unwrap();
        fs::write(lock.join("pid"), format!("{dead_pid}\n")).unwrap();
        let home = tempfile::tempdir().unwrap();
        let stub = tempfile::tempdir().unwrap();
        write_exe(
            &stub.path().join("cargo"),
            "#!/bin/sh\nif mkdir \"$HOME/building\" 2>/dev/null; then sleep 1; rmdir \"$HOME/building\"; else echo overlap >> \"$HOME/overlaps\"; fi\necho ran >> \"$HOME/cargo-runs\"\n",
        );
        write_exe(&stub.path().join("curl"), "#!/bin/sh\nexit 7\n");
        write_exe(&stub.path().join("node"), "#!/bin/sh\nexit 1\n");
        let root = repo_root();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let script = root.join("claude-plugin/bin/install-runtime");
                let (home, stub) = (home.path().to_path_buf(), stub.path().to_path_buf());
                let (data, root) = (data.path().to_path_buf(), root.clone());
                std::thread::spawn(move || {
                    let envs = [
                        ("CLAUDE_PLUGIN_ROOT", root.to_str().unwrap()),
                        ("CLAUDE_PLUGIN_DATA", data.to_str().unwrap()),
                        ("AMPLIHACK_NPM_VERSION", "1.2.3"),
                    ];
                    run(&script, &home, &stub, &[], &envs)
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(
            !home.path().join("overlaps").exists(),
            "two installs ran at once"
        );
        assert!(home.path().join("cargo-runs").exists(), "no install ran");
    }
}
