//! Per-child roots and observable clients; no native service or credentials.
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Resource {
    mode: u32,
    device: u64,
    inode: u64,
    size: u64,
    data: Vec<u8>,
}

type Snapshot = BTreeMap<PathBuf, Resource>;

pub struct Fixture {
    _root: TempDir,
    pub home: PathBuf,
    pub source: PathBuf,
    tools: PathBuf,
    inherited: String,
}

fn executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("{body}\n#{}\n", "fixture padding ".repeat(100)),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn snapshot(root: &Path) -> Snapshot {
    fn walk(root: &Path, path: &Path, result: &mut Snapshot) {
        let meta = fs::symlink_metadata(path).unwrap();
        let data = if meta.is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if meta.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        result.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            Resource {
                mode: meta.permissions().mode(),
                device: meta.dev(),
                inode: meta.ino(),
                size: meta.len(),
                data,
            },
        );
        if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

impl Fixture {
    pub fn new(inherited: &str, unsupported: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let tools = root.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".amplihack/codex")).unwrap();
        fs::write(
            home.join(".amplihack/codex/ownership.json"),
            json!({
                "schema_version":1,"codex_home":home.join(".codex"),
                "package_digest":"legacy-unframed-digest","hooks":{}
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            home.join(".amplihack/codex/foreign.txt"),
            b"foreign package bytes\n",
        )
        .unwrap();
        fs::write(home.join(".codex/config.toml"), b"model = \"user-owned\"\n").unwrap();
        fs::write(home.join(".codex/hooks.json"), b"{\"foreign\":true}\n").unwrap();
        for (provider, version) in [("claude", "2.1.0 (Claude Code)"), ("copilot", "0.0.400")] {
            executable(
                &tools.join(provider),
                &format!(
                    r#"#!/bin/sh
case "$*" in
 *--version*) printf '%s\n' '{version}';;
 *plugin*) printf '%s\n' '{provider}:plugin' >> "$HOME/events"; printf '%s\n' '{{}}';;
 *) printf '%s\n' '{provider}:launch' >> "$HOME/events"; exit 0;;
esac"#
                ),
            );
        }
        let reply = if unsupported {
            "exit 23"
        } else {
            "printf '%s\\n' '{\"installed\":[]}'"
        };
        executable(
            &tools.join("codex"),
            &format!(
                r#"#!/bin/sh
case "$*" in
 *--version*) printf '%s\n' 'codex-cli 0.160.0';;
 plugin*) printf '%s\n' 'codex:plugin' >> "$HOME/events"; {reply};;
 *) printf '%s\n' 'codex:launch' >> "$HOME/events"; exit 0;;
esac"#
            ),
        );
        executable(&tools.join("node"), "#!/bin/sh\nprintf '%s\\n' 'v24.1.0'");
        executable(
            &tools.join("amplihack-hooks"),
            "#!/bin/sh\nprintf '%s\\n' '{}'",
        );
        executable(
            &tools.join("recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'",
        );
        for name in [
            ".cargo",
            ".config",
            ".cache",
            ".local/share",
            ".local/state",
            ".claude",
            "claude-data",
            ".copilot",
            ".config/gh",
            "tmp",
        ] {
            fs::create_dir_all(home.join(name)).unwrap();
        }
        Self {
            _root: root,
            home,
            tools,
            inherited: inherited.to_string(),
            source: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap(),
        }
    }

    pub fn protected_state(&self) -> Vec<Snapshot> {
        [".amplihack/codex", ".codex"]
            .iter()
            .map(|p| snapshot(&self.home.join(p)))
            .collect()
    }

    pub fn events(&self) -> Vec<String> {
        fs::read_to_string(self.home.join("events"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    pub fn diagnostic(out: &Output) -> String {
        format!(
            "exit={}\nstdout={}\nstderr={}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_amplihack"));
        command
            .env_clear()
            .args(args)
            .current_dir(&self.source)
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()))
            .env("AMPLIHACK_HOME", &self.source)
            .env("AMPLIHACK_AGENT_BINARY", &self.inherited)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("IS_SANDBOX", "1")
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.tools.join("amplihack-hooks"),
            )
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"));
        for (key, name) in [
            ("CARGO_HOME", ".cargo"),
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_CACHE_HOME", ".cache"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_STATE_HOME", ".local/state"),
            ("CLAUDE_CONFIG_DIR", ".claude"),
            ("CLAUDE_PLUGIN_DATA", "claude-data"),
            ("COPILOT_HOME", ".copilot"),
            ("COPILOT_CONFIG_DIR", ".copilot"),
            ("GH_CONFIG_DIR", ".config/gh"),
            ("TMPDIR", "tmp"),
        ] {
            command.env(key, self.home.join(name));
        }
        // env_clear keeps startup repair enabled and excludes all ambient credentials.
        command.output().unwrap()
    }
}
