//! Outside-in scenarios for managed tool lists, using isolated installations.
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use amplihack_utils::list_selection::{DEFAULT_AGENTS, DEFAULT_SKILLS};

struct Installation {
    home: tempfile::TempDir,
    source: PathBuf,
    tools: PathBuf,
}

impl Installation {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let tools = home.path().join("tools");
        fs::create_dir(&tools).unwrap();
        // Only these system utilities are available. Client probes and package
        // managers are local fixtures, so no account or network is required.
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
            let output = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            assert!(output.status.success());
            std::os::unix::fs::symlink(
                String::from_utf8(output.stdout).unwrap().trim(),
                tools.join(tool),
            )
            .unwrap();
        }
        let executable = |name: &str, body: &str| {
            crate::support::exec_stub::write_executable(&tools.join(name), body, 0o700);
        };
        executable("node", "#!/bin/sh\necho v24.0.0\n");
        executable("codex-code-mode-host", "#!/bin/sh\nexit 0\n");
        executable("npm", "#!/bin/sh\necho 1.0.0\n");
        executable("mmdc", "#!/bin/sh\necho 1.0.0\n");
        executable(
            "claude",
            "#!/bin/sh\ncase \"$1\" in\n --version) echo '2.1.331 (Claude Code)';;\n *) echo launched > \"$HOME/client-launched\";;\nesac\n",
        );
        executable(
            "recipe-runner-rs",
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\",\"durable_recovery_v1\"]}'\n",
        );
        executable(
            "amplihack-hooks",
            &format!("#!/bin/sh\necho '{{}}'\n#{}\n", "x".repeat(1100)),
        );
        executable(
            "codex",
            r#"#!/bin/sh
case "$*" in
 *--version*) echo 'codex-cli 0.160.0';;
 'plugin list --json')
  if [ -f "$HOME/registered" ]; then
   printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
  else printf '{"installed":[]}\n'; fi;;
 'plugin marketplace add '*)
  mkdir -p "$CODEX_HOME"
  if [ ! -f "$HOME/marketplace-written" ]; then
   [ ! -s "$CODEX_HOME/config.toml" ] || printf '\n' >> "$CODEX_HOME/config.toml"
   printf '[marketplaces.amplihack-local]\nsource_type = "local"\nsource = "%s/.amplihack/codex/market"\n' "$HOME" >> "$CODEX_HOME/config.toml"
   echo yes > "$HOME/marketplace-written"
  fi
  echo '{}';;
 'plugin add '*)
  if [ ! -f "$HOME/registered" ]; then
   printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$CODEX_HOME/config.toml"
  fi
  echo yes > "$HOME/registered"
  echo '{}';;
 'plugin remove '*) rm -f "$HOME/registered"; echo '{}';;
 *) exit 1;;
esac
"#,
        );
        fs::create_dir(home.path().join(".copilot")).unwrap();
        Self {
            home,
            source: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap(),
            tools,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        command
            .args(args)
            .current_dir(self.home.path())
            .env("HOME", self.home.path())
            .env("PATH", &self.tools)
            .env("CODEX_HOME", self.path(".codex"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_SOURCE_ROOT", &self.source)
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.tools.join("amplihack-hooks"),
            )
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"));
        command
    }

    fn run(&self, args: &[&str]) {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if args.first() == Some(&"install") {
            for required in [
                ".claude/skills",
                ".copilot/skills",
                ".amplihack/codex/market/plugin/skills",
            ] {
                assert!(
                    self.path(required).is_dir(),
                    "missing {required}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    fn install(&self) {
        self.run(&["install", "--local", self.source.to_str().unwrap()]);
    }

    fn defaults(&self) {
        let expected: BTreeSet<_> = DEFAULT_SKILLS.map(str::to_owned).into();
        assert_eq!(skill_names(&self.path(".claude/skills")), expected);
        assert_eq!(skill_names(&self.path(".copilot/skills")), expected);
        let codex = skill_names(&self.path(".amplihack/codex/market/plugin/skills"));
        let personas: BTreeSet<_> = DEFAULT_AGENTS
            .iter()
            .map(|name| format!("amplihack-persona-{}", name.replace('/', "-")))
            .collect();
        assert_eq!(codex, expected.union(&personas).cloned().collect());
        assert_eq!(
            markdown_files(&self.path(".claude/skills/amplihack/agents")).len(),
            12
        );
        assert_eq!(
            markdown_files(&self.path(".copilot/agents/amplihack")).len(),
            12
        );
        assert!(
            self.path(".amplihack/codex/market/plugin/bin/hook")
                .is_file()
        );
    }

    fn report_inventory(&self, mode: &str) {
        let codex = skill_names(&self.path(".amplihack/codex/market/plugin/skills"));
        let personas = codex
            .iter()
            .filter(|name| name.starts_with("amplihack-persona-"))
            .count();
        let commands = codex
            .iter()
            .filter(|name| name.starts_with("amplihack-command-"))
            .count();
        eprintln!(
            "inventory {mode}: Claude regular={} native_agents={}; Codex regular={} personas={personas} commands={commands} native_agents=0; Copilot regular={} native_agents={}",
            skill_names(&self.path(".claude/skills")).len(),
            markdown_files(&self.path(".claude/skills/amplihack/agents")).len(),
            codex.len() - personas - commands,
            skill_names(&self.path(".copilot/skills")).len(),
            markdown_files(&self.path(".copilot/agents/amplihack")).len()
        );
    }

    fn readable_assets(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut assets = BTreeMap::new();
        for relative in [
            ".claude/skills",
            ".copilot/skills",
            ".copilot/agents",
            ".amplihack/codex/market/plugin",
        ] {
            for (path, bytes) in snapshot(&self.path(relative)) {
                assets.insert(Path::new(relative).join(path), bytes);
            }
        }
        assets
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap().map(Result::unwrap) {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn skill_names(root: &Path) -> BTreeSet<String> {
    fs::read_dir(root)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn assert_same_assets(actual: &BTreeMap<PathBuf, Vec<u8>>, expected: &BTreeMap<PathBuf, Vec<u8>>) {
    let changed: BTreeSet<_> = actual
        .keys()
        .chain(expected.keys())
        .filter(|path| actual.get(*path) != expected.get(*path))
        .collect();
    assert!(changed.is_empty(), "changed assets: {changed:?}");
}

fn markdown_files(root: &Path) -> BTreeSet<PathBuf> {
    snapshot(root)
        .into_keys()
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect()
}

#[test]
fn fresh_default_install_keeps_complete_framework_and_index() {
    let install = Installation::new();
    install.install();
    install.report_inventory("default");
    install.defaults();
    assert!(!install.path(".copilot/instructions.md").exists());
    assert!(
        !install
            .path(".amplihack/.claude/install/list-changes.txt")
            .exists()
    );
    let staged = install.path(".amplihack/.claude");
    let agents = markdown_files(&staged.join("agents"));
    assert_eq!(
        agents,
        markdown_files(&install.source.join("amplifier-bundle/agents"))
    );
    let index = fs::read_to_string(staged.join("install/skills-and-agents-index.tsv")).unwrap();
    let mut skills = 0;
    for line in index.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        assert!(Path::new(fields[2]).is_file(), "{line}");
        skills += usize::from(fields[0] == "skill");
    }
    assert!(skills > 32);
    for resource in [
        "docx/ooxml",
        "pptx/ooxml",
        "outside-in-testing/scripts",
        "outside-in-testing/tests",
        "outside-in-testing/examples",
    ] {
        assert!(staged.join("skills").join(resource).is_dir(), "{resource}");
    }
    assert!(staged.join("skills/outside-in-testing/README.md").is_file());
}

#[test]
fn enable_disable_and_single_command_restoration_update_every_installed_tool() {
    let install = Installation::new();
    install.install();
    let config = install.path(".amplihack/config.toml");
    fs::write(&config, "# preserve\n[recipe]\nverbose = true\n").unwrap();
    let original_config = fs::read(&config).unwrap();
    let original = install.readable_assets();
    let index = install.path(".amplihack/.claude/install/skills-and-agents-index.tsv");
    let original_index = fs::read(&index).unwrap();
    for name in ["skill:code-visualizer", "agent:core/guide"] {
        for _ in 0..2 {
            install.run(&["directory", "enable", name]);
        }
    }
    for relative in [
        ".claude/skills/code-visualizer/SKILL.md",
        ".copilot/skills/code-visualizer/SKILL.md",
        ".amplihack/codex/market/plugin/skills/code-visualizer/SKILL.md",
        ".claude/skills/amplihack/agents/core/guide.md",
        ".copilot/agents/amplihack/guide.md",
        ".amplihack/codex/market/plugin/skills/amplihack-persona-core-guide/SKILL.md",
    ] {
        assert!(install.path(relative).is_file(), "{relative}");
    }
    install.install();
    for name in ["skill:code-visualizer", "agent:core/guide"] {
        for _ in 0..2 {
            install.run(&["directory", "disable", name]);
        }
    }
    assert_same_assets(&install.readable_assets(), &original);
    assert_eq!(fs::read(config).unwrap(), original_config);
    assert_eq!(fs::read(index).unwrap(), original_index);
    install.run(&["config", "set", "lists.set", "all"]);
    assert!(skill_names(&install.path(".claude/skills")).len() > 32);
    assert!(skill_names(&install.path(".copilot/skills")).len() > 32);
    assert!(
        skill_names(&install.path(".amplihack/codex/market/plugin/skills"))
            .iter()
            .any(|name| name.starts_with("amplihack-command-"))
    );
    let restored = install.readable_assets();
    install.run(&["config", "set", "lists.set", "all"]);
    assert_same_assets(&install.readable_assets(), &restored);
}

#[test]
fn update_from_full_install_preserves_edits_and_unowned_skills() {
    let install = Installation::new();
    install.run(&["config", "set", "lists.set", "all"]);
    install.install();
    install.report_inventory("all");
    let full_framework = snapshot(&install.path(".amplihack/.claude/agents"));
    for tool in [".claude", ".copilot"] {
        fs::write(
            install.path(tool).join("skills/code-visualizer/SKILL.md"),
            "user edit\n",
        )
        .unwrap();
        let unowned = install.path(tool).join("skills/independent");
        fs::create_dir_all(&unowned).unwrap();
        fs::write(unowned.join("SKILL.md"), "independent\n").unwrap();
    }
    fs::write(install.path(".copilot/instructions.md"), "before\n<!-- AMPLIHACK_INSTRUCTIONS_START -->\nold agents\n<!-- AMPLIHACK_INSTRUCTIONS_END -->\nafter\n").unwrap();
    install.run(&["config", "unset", "lists.set"]);
    install.install();
    for tool in [".claude", ".copilot"] {
        assert_eq!(
            fs::read(install.path(tool).join("skills/code-visualizer/SKILL.md")).unwrap(),
            b"user edit\n"
        );
        assert_eq!(
            fs::read(install.path(tool).join("skills/independent/SKILL.md")).unwrap(),
            b"independent\n"
        );
        assert!(
            !install
                .path(tool)
                .join("skills/context-management")
                .exists()
        );
    }
    assert_eq!(
        snapshot(&install.path(".amplihack/.claude/agents")),
        full_framework
    );
    assert_eq!(
        fs::read(install.path(".copilot/instructions.md")).unwrap(),
        b"before\n\nafter\n"
    );
    let notice = install.path(".amplihack/.claude/install/list-changes.txt");
    let saved = fs::read(&notice).unwrap();
    assert!(!saved.is_empty());
    install.install();
    assert_eq!(fs::read(notice).unwrap(), saved);
}

#[test]
fn automatic_launch_repair_narrows_lists_and_saves_the_notice() {
    let install = Installation::new();
    install.run(&["config", "set", "lists.set", "all"]);
    install.install();
    // Reproduce an older full installation without asking a config command
    // to publish the new selection before startup repair runs.
    fs::write(install.path(".amplihack/config.toml"), "").unwrap();
    fs::write(install.path(".amplihack/.installed-version"), "0.0.0").unwrap();
    let agents = snapshot(&install.path(".amplihack/.claude/agents"));
    let settings = fs::read(install.path(".claude/settings.json")).unwrap();
    let launch = || {
        let output = install
            .command(&["claude", "--subprocess-safe", "--", "-p", "fixture"])
            .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", install.tools.join("claude"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "launch: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    launch();
    assert!(install.path("client-launched").is_file());
    let expected: BTreeSet<_> = DEFAULT_SKILLS.map(str::to_owned).into();
    assert_eq!(skill_names(&install.path(".claude/skills")), expected);
    assert_eq!(skill_names(&install.path(".copilot/skills")), expected);
    assert_eq!(
        markdown_files(&install.path(".claude/skills/amplihack/agents")).len(),
        12
    );
    assert_eq!(
        markdown_files(&install.path(".copilot/agents/amplihack")).len(),
        12
    );
    assert_eq!(snapshot(&install.path(".amplihack/.claude/agents")), agents);
    assert_eq!(
        fs::read(install.path(".claude/settings.json")).unwrap(),
        settings
    );
    let notice = install.path(".amplihack/.claude/install/list-changes.txt");
    let saved = fs::read(&notice).unwrap();
    assert!(!saved.is_empty());
    assert!(
        !install
            .path(".amplihack/.claude/install/list-changes-shown.json")
            .exists()
    );
    launch();
    assert_eq!(fs::read(notice).unwrap(), saved);
}
