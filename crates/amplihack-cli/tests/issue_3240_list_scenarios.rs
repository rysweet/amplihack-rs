//! Outside-in scenarios for managed tool lists, using isolated installations.
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use amplihack_utils::list_selection::{DEFAULT_AGENTS, DEFAULT_SKILLS};

struct Installation {
    home: tempfile::TempDir,
    source: PathBuf,
    tools: PathBuf,
}

// Notice display requires terminal stderr. Drain it concurrently so publication
// output cannot fill the PTY and block the child.
fn terminal_output(mut command: Command) -> Output {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty initializes both descriptors on success; each is then
    // transferred to exactly one File owner.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut::<libc::termios>(),
                std::ptr::null_mut::<libc::winsize>(),
            )
        },
        0
    );
    let mut reader = unsafe { fs::File::from_raw_fd(master) };
    let terminal = unsafe { fs::File::from_raw_fd(slave) };
    let captured = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                // Linux reports the closed slave as EIO rather than EOF.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("reading terminal stderr: {error}"),
            }
        }
        bytes
    });
    let mut output = command
        .env_remove("AMPLIHACK_RECIPE_RUN_ID")
        .stderr(terminal)
        .output()
        .unwrap();
    drop(command);
    output.stderr = captured.join().unwrap();
    output
}

#[test]
fn damaged_notice_does_not_block_terminal_inspection_or_full_list_restoration() {
    for (damage, diagnostic) in [
        ("malformed", "cannot read the list notice display position"),
        ("shortened", "list notice history was shortened"),
    ] {
        let install = Installation::new();
        install.run(&["config", "set", "lists.set", "all"]);
        install.install();
        let lists = [
            ".claude/skills",
            ".copilot/skills",
            ".copilot/agents/amplihack",
            ".amplihack/codex/market/plugin/skills",
        ];
        let full = lists.map(|path| snapshot(&install.path(path)));
        install.run(&["config", "set", "lists.set", "default"]);
        install.defaults();
        let history = install.path(".amplihack/.claude/install/list-changes.txt");
        let shown = install.path(".amplihack/.claude/install/list-changes-shown.json");
        if damage == "shortened" {
            let position = fs::read(&history).unwrap().len();
            fs::write(&shown, serde_json::to_vec(&position).unwrap()).unwrap();
            fs::write(&history, "shortened notice\n").unwrap();
        } else {
            fs::write(&shown, "not JSON\n").unwrap();
        }
        let mut pending = fs::read(&history).unwrap();
        let position = fs::read(&shown).unwrap();
        for (args, expected) in [
            (vec!["config", "get", "lists.set"], Some("default\n")),
            (vec!["config", "set", "lists.set", "all"], None),
            (vec!["config", "get", "lists.set"], Some("all\n")),
        ] {
            let output = terminal_output(install.command(&args));
            assert!(output.status.success(), "{damage}: {args:?}: {output:?}");
            let warning = String::from_utf8_lossy(&output.stderr);
            assert!(warning.contains("Warning:"), "{warning}");
            assert!(warning.contains(diagnostic), "{warning}");
            if let Some(expected) = expected {
                assert_eq!(output.stdout, expected.as_bytes());
            }
            let saved = fs::read(&history).unwrap();
            if args[1] == "set" {
                // Publication may append a migration (Copilot restores its
                // flat agent filenames); it must retain the pending history.
                assert!(saved.starts_with(&pending));
                pending = saved;
            } else {
                assert_eq!(saved, pending);
            }
            assert_eq!(fs::read(&shown).unwrap(), position);
        }
        for (path, expected) in lists.into_iter().zip(full) {
            assert_eq!(snapshot(&install.path(path)), expected, "{damage}: {path}");
        }
    }
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

// Verbatim Markdown payloads from PR base 439a20122d52d08abe727c27d43e9fe8bbc32bfc.
// All three SKILL.md definitions change in this release; compare destinations
// with the old staged payload, rather than with the incoming bundle.
const LEGACY_COPILOT_FILES: [(&str, &str); 7] = [
    (
        "lsp-setup/README.md",
        include_str!("fixtures/issue_3240_legacy_copilot/lsp-setup/README.md"),
    ),
    (
        "lsp-setup/SKILL.md",
        include_str!("fixtures/issue_3240_legacy_copilot/lsp-setup/SKILL.md"),
    ),
    (
        "context-management/QUICK_START.md",
        include_str!("fixtures/issue_3240_legacy_copilot/context-management/QUICK_START.md"),
    ),
    (
        "context-management/README.md",
        include_str!("fixtures/issue_3240_legacy_copilot/context-management/README.md"),
    ),
    (
        "context-management/SKILL.md",
        include_str!("fixtures/issue_3240_legacy_copilot/context-management/SKILL.md"),
    ),
    (
        "code-visualizer/README.md",
        include_str!("fixtures/issue_3240_legacy_copilot/code-visualizer/README.md"),
    ),
    (
        "code-visualizer/SKILL.md",
        include_str!("fixtures/issue_3240_legacy_copilot/code-visualizer/SKILL.md"),
    ),
];

#[test]
fn base_version_copilot_upgrade_without_ownership_refreshes_and_narrows() {
    // Exercise both writers that replace framework definitions: install and a
    // list change against an older version. Do not use the new publisher to
    // create the starting installation or its ownership manifest.
    for list_change in [false, true] {
        let install = Installation::new();
        let staged = install.path(".amplihack/.claude");
        for (relative, body) in LEGACY_COPILOT_FILES {
            if relative.ends_with("SKILL.md") {
                assert_ne!(
                    fs::read(
                        install
                            .source
                            .join("amplifier-bundle/skills")
                            .join(relative)
                    )
                    .unwrap(),
                    body.as_bytes(),
                    "{relative} must exercise a changed release payload"
                );
            }
            // These skills have no nested Markdown; this is exactly the old
            // Copilot publisher's flattened, Markdown-only installation.
            for root in [staged.join("skills"), install.path(".copilot/skills")] {
                let file = root.join(relative);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, body).unwrap();
            }
        }
        fs::write(install.path(".amplihack/.installed-version"), "0.0.0").unwrap();
        fs::write(
            install.path(".copilot/config.json"),
            r#"{"installedPlugins":[{"name":"amplihack"}]}"#,
        )
        .unwrap();
        let edited = install.path(".copilot/skills/code-visualizer/SKILL.md");
        fs::write(&edited, "user edit\n").unwrap();
        let independent = install.path(".copilot/skills/independent");
        fs::create_dir_all(&independent).unwrap();
        fs::write(independent.join("SKILL.md"), "independent\n").unwrap();
        let record = install.path(".copilot/copilot-skills-manifest.json");
        assert!(!record.exists());

        let upgrade = || {
            if list_change {
                install.run(&["config", "set", "lists.set", "default"]);
            } else {
                install.install();
            }
        };
        upgrade();
        let expected: BTreeSet<_> = DEFAULT_SKILLS
            .iter()
            .copied()
            .chain(["code-visualizer", "independent"])
            .map(str::to_owned)
            .collect();
        assert_eq!(skill_names(&install.path(".copilot/skills")), expected);
        assert_eq!(
            fs::read(install.path(".copilot/skills/lsp-setup/SKILL.md")).unwrap(),
            fs::read(
                install
                    .source
                    .join("amplifier-bundle/skills/lsp-setup/SKILL.md")
            )
            .unwrap()
        );
        assert!(!install.path(".copilot/skills/context-management").exists());
        assert_eq!(fs::read(&edited).unwrap(), b"user edit\n");
        assert_eq!(
            fs::read(independent.join("SKILL.md")).unwrap(),
            b"independent\n"
        );
        let ownership: serde_json::Value =
            serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        let owned: BTreeSet<_> = ownership["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(owned, DEFAULT_SKILLS.map(str::to_owned).into());
        let published = snapshot(&install.path(".copilot/skills"));
        upgrade();
        assert_eq!(snapshot(&install.path(".copilot/skills")), published);
    }
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
