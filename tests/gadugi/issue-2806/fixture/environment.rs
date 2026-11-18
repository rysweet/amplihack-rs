//! Private environments shared by fixture unit tests and the CI executable.
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail, ensure};

#[derive(Debug)]
pub struct IsolatedCase {
    pub root: PathBuf,
    pub home: PathBuf,
    pub work: PathBuf,
    pub tmp: PathBuf,
    pub evidence: PathBuf,
}

pub fn private_dir(path: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    Ok(())
}

impl IsolatedCase {
    pub fn create(root: &Path) -> Result<Self> {
        ensure!(
            !root.exists(),
            "case root already exists: {}",
            root.display()
        );
        private_dir(root)?;
        let root = root.canonicalize()?;
        let case = Self {
            home: root.join("home"),
            work: root.join("work"),
            tmp: root.join("tmp"),
            evidence: root.join("evidence"),
            root,
        };
        for dir in [
            &case.home,
            &case.work,
            &case.tmp,
            &case.evidence,
            &case.root.join("bin"),
        ] {
            private_dir(dir)?;
        }
        for key in [".codex", ".claude", ".config", ".amplihack", ".cache"] {
            private_dir(&case.home.join(key))?;
        }
        Ok(case)
    }

    pub fn open(root: &Path) -> Result<Self> {
        ensure!(
            root.is_absolute() && root.is_dir(),
            "private case root missing"
        );
        Ok(Self {
            root: root.into(),
            home: root.join("home"),
            work: root.join("work"),
            tmp: root.join("tmp"),
            evidence: root.join("evidence"),
        })
    }

    fn environment(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        for (key, value) in [
            ("HOME", self.home.clone()),
            ("CODEX_HOME", self.home.join(".codex")),
            ("CLAUDE_CONFIG_DIR", self.home.join(".claude")),
            ("XDG_CONFIG_HOME", self.home.join(".config")),
            ("XDG_CACHE_HOME", self.home.join(".cache")),
            ("AMPLIHACK_HOME", self.home.join(".amplihack")),
            ("AMPLIHACK_WORK_ROOT", self.work.clone()),
            ("TMPDIR", self.tmp.clone()),
            ("AMPLIHACK_2806_ROOT", self.root.clone()),
        ] {
            vars.insert(key.into(), value.to_string_lossy().into_owned());
        }
        for (key, value) in [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ALLOW_PROTOCOL", "file"),
            ("GIT_AUTHOR_NAME", "Issue 2806"),
            ("GIT_AUTHOR_EMAIL", "qa@example.invalid"),
            ("GIT_COMMITTER_NAME", "Issue 2806"),
            ("GIT_COMMITTER_EMAIL", "qa@example.invalid"),
            ("GIT_PAGER", "cat"),
            ("GH_PAGER", "cat"),
            ("PAGER", "cat"),
            ("LC_ALL", "C.UTF-8"),
        ] {
            vars.insert(key.into(), value.into());
        }
        vars.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
        );
        vars
    }

    pub fn allowed_environment_keys(&self) -> Vec<String> {
        self.environment().into_keys().collect()
    }

    /// Use an explicit env -i boundary even when called from an already scrubbed child.
    pub fn command(&self, executable: &Path) -> Result<Command> {
        ensure!(
            executable.is_absolute(),
            "executable must be absolute: {}",
            executable.display()
        );
        let mut command = Command::new("/usr/bin/env");
        command.env_clear().arg("-i");
        for (key, value) in self.environment() {
            command.arg(format!("{key}={value}"));
        }
        // CI stages a private global config containing only a local URL rewrite.
        // Ordinary unit fixtures retain /dev/null and do not receive extra state.
        let config = self.root.join("gitconfig");
        if config.exists() {
            command.arg(format!("GIT_CONFIG_GLOBAL={}", config.display()));
        }
        let fixture_config = self.root.join("config.json");
        if fixture_config.is_file() {
            let config: serde_json::Value = serde_json::from_slice(&fs::read(fixture_config)?)?;
            for (key, value) in config["environment"]
                .as_object()
                .context("fixture environment missing")?
            {
                ensure!(
                    key.starts_with("AMPLIHACK_")
                        || ["RECIPE_RUNNER_RS_PATH", "CARGO_HOME", "PRE_COMMIT_HOME"]
                            .contains(&key.as_str()),
                    "unsupported fixture environment key {key}"
                );
                command.arg(format!(
                    "{key}={}",
                    value.as_str().context("environment value must be text")?
                ));
            }
        }
        command.arg(executable).current_dir(&self.root);
        Ok(command)
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut command = self.command(Path::new("/usr/bin/git"))?;
        // amplihack-git can use the PATH name; resolve the known system Git explicitly.
        command
            .args(["-c", "commit.gpgsign=false", "-c", "init.templateDir="])
            .args(args)
            .current_dir(cwd);
        checked(command.output()?, "Git")
    }
}

/// Reject traversal and existing symlink components, including dangling links.
pub fn confined_path(root: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.is_absolute(),
        "absolute path is outside relative contract: {}",
        path.display()
    );
    let mut result = root.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                result.push(part);
                match fs::symlink_metadata(&result) {
                    Ok(meta) => ensure!(
                        !meta.file_type().is_symlink(),
                        "symlink in confined path: {}",
                        result.display()
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("inspect confined path"),
                }
            }
            _ => bail!("invalid confined path: {}", path.display()),
        }
    }
    Ok(result)
}

pub fn checked(output: Output, operation: &str) -> Result<String> {
    ensure!(
        output.status.success(),
        "{operation} exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Read-only Git observations with no inherited configuration or credentials.
pub fn observe_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut command = amplihack_git::command_in(cwd);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(args);
    checked(command.output()?, "Git observation")
}

/// Parse the current process's network namespace view, not a pre-existing sysfs mount.
pub fn network_interfaces(devices: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for line in devices.lines().skip(2) {
        let (name, counts) = line
            .split_once(':')
            .context("malformed /proc/self/net/dev record")?;
        let name = name.trim();
        ensure!(
            !name.is_empty() && counts.split_whitespace().count() == 16,
            "incomplete network-interface record"
        );
        names.push(name.to_owned());
    }
    ensure!(!names.is_empty(), "no network interfaces observed");
    names.sort();
    names.dedup();
    Ok(names)
}
