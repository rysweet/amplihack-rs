//! Docker detection and command construction for launcher parity.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) const DEFAULT_IMAGE_NAME: &str = "amplihack:latest";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockerActivation {
    Flag,
    Env,
}

impl DockerActivation {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Flag => "Docker mode enabled via --docker flag",
            Self::Env => "Docker mode enabled via AMPLIHACK_USE_DOCKER",
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DockerDetector;

impl DockerDetector {
    pub(crate) fn activation_source(self, docker_flag: bool) -> Option<DockerActivation> {
        if docker_flag {
            Some(DockerActivation::Flag)
        } else if self.should_use_docker() {
            Some(DockerActivation::Env)
        } else {
            None
        }
    }

    pub(crate) fn is_available(self) -> bool {
        let Some(path) = env::var_os("PATH") else {
            return false;
        };
        env::split_paths(&path).any(|dir| dir.join("docker").is_file())
    }

    pub(crate) fn is_running(self) -> bool {
        if !self.is_available() {
            return false;
        }

        Command::new("docker")
            .arg("info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    pub(crate) fn should_use_docker(self) -> bool {
        should_use_docker_from_state(
            env::var("AMPLIHACK_USE_DOCKER").ok().as_deref(),
            self.is_in_docker(),
            self.is_running(),
        )
    }

    pub(crate) fn is_in_docker(self) -> bool {
        let cgroup = std::fs::read_to_string("/proc/1/cgroup").ok();
        is_in_docker_from_state(
            env::var("AMPLIHACK_IN_DOCKER").ok().as_deref(),
            Path::new("/.dockerenv").exists(),
            cgroup.as_deref(),
        )
    }

    pub(crate) fn check_image_exists(self, image_name: &str) -> bool {
        if !self.is_running() {
            return false;
        }

        Command::new("docker")
            .args(["images", "-q", image_name])
            .stdin(Stdio::null())
            .output()
            .map(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
            .unwrap_or(false)
    }
}

pub(crate) fn should_use_docker_from_state(
    env_value: Option<&str>,
    in_docker: bool,
    docker_running: bool,
) -> bool {
    is_truthy_env_value(env_value) && !in_docker && docker_running
}

pub(crate) fn is_in_docker_from_state(
    amplihack_in_docker: Option<&str>,
    dockerenv_exists: bool,
    cgroup_contents: Option<&str>,
) -> bool {
    amplihack_in_docker == Some("1")
        || dockerenv_exists
        || cgroup_contents.is_some_and(|contents| contents.contains("docker"))
}

fn is_truthy_env_value(value: Option<&str>) -> bool {
    matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes")
    )
}

#[derive(Debug, Clone)]
pub(crate) struct DockerManager {
    project_root: PathBuf,
    image_name: &'static str,
    detector: DockerDetector,
}

impl Default for DockerManager {
    fn default() -> Self {
        Self {
            project_root: workspace_root(),
            image_name: DEFAULT_IMAGE_NAME,
            detector: DockerDetector,
        }
    }
}

impl DockerManager {
    #[cfg(test)]
    pub(crate) fn new_for_tests(project_root: PathBuf) -> Self {
        Self {
            project_root,
            image_name: DEFAULT_IMAGE_NAME,
            detector: DockerDetector,
        }
    }

    pub(crate) fn run_command(&self, amplihack_args: &[String], cwd: &Path) -> Result<i32> {
        if !self.detector.is_running() {
            eprintln!("Docker is not running.");
            return Ok(1);
        }

        if !self.build_image()? {
            eprintln!("Failed to build Docker image.");
            return Ok(1);
        }

        let run_args = self.build_run_args(cwd, amplihack_args, env::vars_os());
        let status = Command::new("docker")
            .args(&run_args)
            .status()
            .context("failed to execute docker run")?;
        Ok(status.code().unwrap_or(1))
    }

    fn build_image(&self) -> Result<bool> {
        if self.detector.check_image_exists(self.image_name) {
            return Ok(true);
        }

        let dockerfile = self.project_root.join("Dockerfile");
        if !dockerfile.is_file() {
            eprintln!("Dockerfile not found at {}", dockerfile.display());
            return Ok(false);
        }

        println!("Building Docker image: {}", self.image_name);
        let status = Command::new("docker")
            .args(self.build_image_args(&dockerfile))
            .status()
            .context("failed to execute docker build")?;
        if !status.success() {
            eprintln!("Docker build failed.");
            return Ok(false);
        }

        println!("Successfully built Docker image: {}", self.image_name);
        Ok(true)
    }

    fn build_image_args(&self, dockerfile: &Path) -> Vec<String> {
        vec![
            "build".to_string(),
            "-t".to_string(),
            self.image_name.to_string(),
            "-f".to_string(),
            dockerfile.display().to_string(),
            self.project_root.display().to_string(),
        ]
    }

    pub(crate) fn build_run_args<I, K, V>(
        &self,
        cwd: &Path,
        amplihack_args: &[String],
        env_vars: I,
    ) -> Vec<String>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<std::ffi::OsString>,
        V: Into<std::ffi::OsString>,
    {
        let workspace_dir = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--interactive".to_string(),
        ];
        if std::io::stdin().is_terminal() {
            args.push("--tty".to_string());
        }
        args.extend([
            "--security-opt".to_string(),
            "no-new-privileges".to_string(),
            "--memory".to_string(),
            "4g".to_string(),
            "--cpus".to_string(),
            "2".to_string(),
        ]);
        #[cfg(unix)]
        {
            args.extend(["--user".to_string(), format!("{}:{}", nix_uid(), nix_gid())]);
        }
        args.extend([
            "-v".to_string(),
            format!("{}:/workspace", workspace_dir.display()),
            "-w".to_string(),
            "/workspace".to_string(),
        ]);

        for (key, value) in forwarded_env_vars(env_vars) {
            args.extend(["-e".to_string(), format!("{key}={value}")]);
        }

        args.push(self.image_name.to_string());
        args.extend(amplihack_args.iter().cloned());
        args
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn validate_api_key(key: &str, value: &str) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;

    static SK_RE: OnceLock<Regex> = OnceLock::new();
    static GH_RE: OnceLock<Regex> = OnceLock::new();

    let result = match key {
        "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" => {
            let re = SK_RE.get_or_init(|| Regex::new(r"^sk-[a-zA-Z0-9\-_]+$").unwrap());
            re.is_match(value)
        }
        "GITHUB_TOKEN" | "GH_TOKEN" => {
            let re = GH_RE.get_or_init(|| {
                Regex::new(r"^(ghp_|ghs_|gho_|ghu_|github_pat_).+$|^[0-9a-fA-F]{40}$").unwrap()
            });
            re.is_match(value)
        }
        _ => true, // No format requirement for other keys
    };
    if !result {
        eprintln!("Warning: {key} has an unexpected format, skipping.");
    }
    result
}

fn forwarded_env_vars<I, K, V>(env_vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<std::ffi::OsString>,
    V: Into<std::ffi::OsString>,
{
    let mut forwarded = BTreeMap::new();
    for (key, value) in env_vars {
        let key = key.into();
        let value = value.into();
        let key = key.to_string_lossy();
        let value = value.to_string_lossy();
        let should_forward = (matches!(
            key.as_ref(),
            "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" | "GITHUB_TOKEN" | "GH_TOKEN" | "TERM"
        ) || (key.starts_with("AMPLIHACK_")
            && key != "AMPLIHACK_USE_DOCKER"))
            && validate_api_key(&key, &value);
        if should_forward {
            forwarded.insert(key.into_owned(), sanitize_env_value(&value));
        }
    }
    forwarded.insert("AMPLIHACK_IN_DOCKER".to_string(), "1".to_string());
    forwarded
}

fn sanitize_env_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect()
}

#[cfg(unix)]
fn nix_uid() -> u32 {
    // SAFETY: libc getter has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn nix_gid() -> u32 {
    // SAFETY: libc getter has no preconditions.
    unsafe { libc::getegid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_prefixed_value(parts: &[&str], suffix: &str) -> String {
        let mut value = parts.concat();
        value.push_str(suffix);
        value
    }

    #[test]
    fn should_use_docker_requires_truthy_env_not_in_docker_and_running_daemon() {
        assert!(should_use_docker_from_state(Some("true"), false, true));
        assert!(!should_use_docker_from_state(Some("true"), true, true));
        assert!(!should_use_docker_from_state(Some("true"), false, false));
        assert!(!should_use_docker_from_state(Some("0"), false, true));
    }

    #[test]
    fn is_in_docker_detects_explicit_env_and_runtime_markers() {
        assert!(is_in_docker_from_state(Some("1"), false, None));
        assert!(is_in_docker_from_state(None, true, None));
        assert!(is_in_docker_from_state(
            None,
            false,
            Some("12:devices:/docker/abc123")
        ));
        assert!(!is_in_docker_from_state(
            None,
            false,
            Some("12:devices:/user.slice")
        ));
    }

    #[test]
    fn build_run_args_mounts_workspace_and_forwards_selected_env() {
        let manager = DockerManager::new_for_tests(PathBuf::from("/repo"));
        let anthropic_api_key = sample_prefixed_value(&["s", "k-"], "test");
        let args = manager.build_run_args(
            Path::new("/tmp/workspace"),
            &[
                "launch".to_string(),
                "--".to_string(),
                "-p".to_string(),
                "hi".to_string(),
            ],
            [
                ("AMPLIHACK_USE_DOCKER", "1"),
                ("AMPLIHACK_SESSION_ID", "abc123"),
                ("ANTHROPIC_API_KEY", anthropic_api_key.as_str()),
                ("TERM", "xterm-256color"),
            ],
        );

        assert!(args.windows(2).any(|window| window == ["-w", "/workspace"]));
        assert!(
            args.windows(2)
                .any(|window| { window[0] == "-e" && window[1] == "AMPLIHACK_SESSION_ID=abc123" })
        );
        assert!(
            args.windows(2)
                .any(|window| { window[0] == "-e" && window[1] == "AMPLIHACK_IN_DOCKER=1" })
        );
        assert!(!args.iter().any(|arg| arg.contains("AMPLIHACK_USE_DOCKER")));
        assert!(args.ends_with(&[
            "amplihack:latest".to_string(),
            "launch".to_string(),
            "--".to_string(),
            "-p".to_string(),
            "hi".to_string()
        ]));
    }

    #[test]
    fn build_image_args_target_repo_root_dockerfile() {
        let project_root = PathBuf::from("/tmp/amplihack-rs");
        let manager = DockerManager::new_for_tests(project_root.clone());
        let dockerfile = project_root.join("Dockerfile");
        assert_eq!(
            manager.build_image_args(&dockerfile),
            vec![
                "build",
                "-t",
                "amplihack:latest",
                "-f",
                "/tmp/amplihack-rs/Dockerfile",
                "/tmp/amplihack-rs",
            ]
        );
    }

    // --- Gap 3: API key format validation ---
    //
    // FAILING TEST: `forwarded_env_vars` must validate ANTHROPIC_API_KEY, OPENAI_API_KEY,
    // GITHUB_TOKEN, and GH_TOKEN against known format patterns. Keys with invalid formats
    // must be SKIPPED (with a warning to stderr) rather than forwarded into the container.
    //
    // Currently `forwarded_env_vars` forwards any value for these key names without format
    // validation, so the assertions that check invalid keys are NOT forwarded will fail.
    //
    // These tests will pass once `validate_api_key()` is implemented and wired into
    // `forwarded_env_vars()`.

    #[test]
    fn invalid_api_key_format_skipped_with_warning() {
        // ANTHROPIC_API_KEY: valid sk- prefix → must be forwarded.
        let valid_anthropic_key = sample_prefixed_value(&["s", "k-"], "validKey123");
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", valid_anthropic_key.as_str())]);
        assert!(
            forwarded.contains_key("ANTHROPIC_API_KEY"),
            "valid sk- key should be forwarded; got keys: {:?}",
            forwarded.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            forwarded.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some(valid_anthropic_key.as_str()),
            "value must be preserved for valid key"
        );

        // ANTHROPIC_API_KEY: no sk- prefix → must NOT be forwarded.
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", "not-valid-key")]);
        assert!(
            !forwarded.contains_key("ANTHROPIC_API_KEY"),
            "invalid ANTHROPIC_API_KEY (no sk- prefix) must be skipped; got keys: {:?}",
            forwarded.keys().collect::<Vec<_>>()
        );

        // OPENAI_API_KEY: valid sk- prefix → must be forwarded.
        let valid_openai_key = sample_prefixed_value(&["s", "k-", "proj-"], "abcDEF012");
        let forwarded = forwarded_env_vars([("OPENAI_API_KEY", valid_openai_key.as_str())]);
        assert!(
            forwarded.contains_key("OPENAI_API_KEY"),
            "valid sk- OPENAI_API_KEY should be forwarded"
        );

        // OPENAI_API_KEY: missing sk- prefix → must NOT be forwarded.
        let forwarded = forwarded_env_vars([("OPENAI_API_KEY", "bad_key_no_prefix")]);
        assert!(
            !forwarded.contains_key("OPENAI_API_KEY"),
            "invalid OPENAI_API_KEY (no sk- prefix) must be skipped"
        );

        // GITHUB_TOKEN: valid ghp_ prefix → must be forwarded.
        let valid_github_token = sample_prefixed_value(&["g", "hp_"], "validtoken1234");
        let forwarded = forwarded_env_vars([("GITHUB_TOKEN", valid_github_token.as_str())]);
        assert!(
            forwarded.contains_key("GITHUB_TOKEN"),
            "valid ghp_ GITHUB_TOKEN should be forwarded"
        );

        // GITHUB_TOKEN: invalid prefix → must NOT be forwarded.
        let forwarded = forwarded_env_vars([("GITHUB_TOKEN", "invalid_prefix_token")]);
        assert!(
            !forwarded.contains_key("GITHUB_TOKEN"),
            "invalid GITHUB_TOKEN (bad prefix) must be skipped"
        );

        // GH_TOKEN: valid ghs_ prefix → must be forwarded.
        let valid_gh_token = sample_prefixed_value(&["g", "hs_"], "someServiceToken");
        let forwarded = forwarded_env_vars([("GH_TOKEN", valid_gh_token.as_str())]);
        assert!(
            forwarded.contains_key("GH_TOKEN"),
            "valid ghs_ GH_TOKEN should be forwarded"
        );

        // GH_TOKEN: invalid prefix → must NOT be forwarded.
        let forwarded = forwarded_env_vars([("GH_TOKEN", "plaintext_bad_token")]);
        assert!(
            !forwarded.contains_key("GH_TOKEN"),
            "invalid GH_TOKEN (no recognised prefix) must be skipped"
        );

        // GH_TOKEN: 40-char lowercase hex classic token → must be forwarded.
        let classic_token = "a".repeat(40); // 40-char hex string
        let forwarded = forwarded_env_vars([("GH_TOKEN", classic_token.as_str())]);
        assert!(
            forwarded.contains_key("GH_TOKEN"),
            "40-char hex classic GITHUB_TOKEN should be forwarded"
        );

        // GH_TOKEN: 39-char hex (too short) → must NOT be forwarded.
        let short_token = "a".repeat(39);
        let forwarded = forwarded_env_vars([("GH_TOKEN", short_token.as_str())]);
        assert!(
            !forwarded.contains_key("GH_TOKEN"),
            "39-char hex token (too short) must be skipped"
        );

        // TERM has no format validation — all values must pass through unchanged.
        let forwarded = forwarded_env_vars([("TERM", "xterm-256color")]);
        assert!(
            forwarded.contains_key("TERM"),
            "TERM must always be forwarded regardless of value"
        );

        // AMPLIHACK_* variables (except AMPLIHACK_USE_DOCKER) must not be format-validated.
        let forwarded = forwarded_env_vars([("AMPLIHACK_SESSION_ID", "any-value-at-all")]);
        assert!(
            forwarded.contains_key("AMPLIHACK_SESSION_ID"),
            "AMPLIHACK_SESSION_ID must be forwarded without format validation"
        );
    }

    #[test]
    fn valid_github_pat_prefix_variants_are_forwarded() {
        // All recognised GitHub token prefix variants must be accepted.
        for (key, value) in [
            (
                "GITHUB_TOKEN",
                sample_prefixed_value(&["g", "hp_"], "abc123"),
            ),
            (
                "GITHUB_TOKEN",
                sample_prefixed_value(&["g", "hs_"], "abc123"),
            ),
            (
                "GITHUB_TOKEN",
                sample_prefixed_value(&["g", "ho_"], "abc123"),
            ),
            (
                "GITHUB_TOKEN",
                sample_prefixed_value(&["g", "hu_"], "abc123"),
            ),
            (
                "GITHUB_TOKEN",
                sample_prefixed_value(&["github", "_pat_"], "abc123"),
            ),
            ("GH_TOKEN", sample_prefixed_value(&["g", "hp_"], "abc123")),
        ] {
            let forwarded = forwarded_env_vars([(key, value.as_str())]);
            assert!(
                forwarded.contains_key(key),
                "token with prefix '{}' for key '{}' should be forwarded",
                value,
                key
            );
        }
    }

    #[test]
    fn sk_prefix_requires_non_empty_suffix() {
        // "sk-" alone (empty suffix) must NOT be forwarded — the + quantifier requires ≥1 char.
        let sk_prefix_only = ["s", "k-"].concat();
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", sk_prefix_only.as_str())]);
        assert!(
            !forwarded.contains_key("ANTHROPIC_API_KEY"),
            "sk- with empty suffix must be skipped"
        );

        // "sk-a" (minimal valid key) must be forwarded.
        let minimal_valid_key = sample_prefixed_value(&["s", "k-"], "a");
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", minimal_valid_key.as_str())]);
        assert!(
            forwarded.contains_key("ANTHROPIC_API_KEY"),
            "sk-a (minimal valid key) must be forwarded"
        );
    }
}
