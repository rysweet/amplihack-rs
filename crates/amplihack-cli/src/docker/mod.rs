//! Docker detection and command construction for launcher parity.

pub(crate) mod helpers;
mod manager;

pub(crate) use manager::DockerManager;

use std::env;
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
            std::path::Path::new("/.dockerenv").exists(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use helpers::forwarded_env_vars;
    use std::path::PathBuf;

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
            std::path::Path::new("/tmp/workspace"),
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

    #[test]
    fn invalid_api_key_format_skipped_with_warning() {
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

        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", "not-valid-key")]);
        assert!(
            !forwarded.contains_key("ANTHROPIC_API_KEY"),
            "invalid ANTHROPIC_API_KEY (no sk- prefix) must be skipped; got keys: {:?}",
            forwarded.keys().collect::<Vec<_>>()
        );

        let valid_openai_key = sample_prefixed_value(&["s", "k-", "proj-"], "abcDEF012");
        let forwarded = forwarded_env_vars([("OPENAI_API_KEY", valid_openai_key.as_str())]);
        assert!(
            forwarded.contains_key("OPENAI_API_KEY"),
            "valid sk- OPENAI_API_KEY should be forwarded"
        );

        let forwarded = forwarded_env_vars([("OPENAI_API_KEY", "bad_key_no_prefix")]);
        assert!(
            !forwarded.contains_key("OPENAI_API_KEY"),
            "invalid OPENAI_API_KEY (no sk- prefix) must be skipped"
        );

        let valid_github_token = sample_prefixed_value(&["g", "hp_"], "validtoken1234");
        let forwarded = forwarded_env_vars([("GITHUB_TOKEN", valid_github_token.as_str())]);
        assert!(
            forwarded.contains_key("GITHUB_TOKEN"),
            "valid ghp_ GITHUB_TOKEN should be forwarded"
        );

        let forwarded = forwarded_env_vars([("GITHUB_TOKEN", "invalid_prefix_token")]);
        assert!(
            !forwarded.contains_key("GITHUB_TOKEN"),
            "invalid GITHUB_TOKEN (bad prefix) must be skipped"
        );

        let valid_gh_token = sample_prefixed_value(&["g", "hs_"], "someServiceToken");
        let forwarded = forwarded_env_vars([("GH_TOKEN", valid_gh_token.as_str())]);
        assert!(
            forwarded.contains_key("GH_TOKEN"),
            "valid ghs_ GH_TOKEN should be forwarded"
        );

        let forwarded = forwarded_env_vars([("GH_TOKEN", "plaintext_bad_token")]);
        assert!(
            !forwarded.contains_key("GH_TOKEN"),
            "invalid GH_TOKEN (no recognised prefix) must be skipped"
        );

        let classic_token = "a".repeat(40);
        let forwarded = forwarded_env_vars([("GH_TOKEN", classic_token.as_str())]);
        assert!(
            forwarded.contains_key("GH_TOKEN"),
            "40-char hex classic GITHUB_TOKEN should be forwarded"
        );

        let short_token = "a".repeat(39);
        let forwarded = forwarded_env_vars([("GH_TOKEN", short_token.as_str())]);
        assert!(
            !forwarded.contains_key("GH_TOKEN"),
            "39-char hex token (too short) must be skipped"
        );

        let forwarded = forwarded_env_vars([("TERM", "xterm-256color")]);
        assert!(
            forwarded.contains_key("TERM"),
            "TERM must always be forwarded regardless of value"
        );

        let forwarded = forwarded_env_vars([("AMPLIHACK_SESSION_ID", "any-value-at-all")]);
        assert!(
            forwarded.contains_key("AMPLIHACK_SESSION_ID"),
            "AMPLIHACK_SESSION_ID must be forwarded without format validation"
        );
    }

    #[test]
    fn valid_github_pat_prefix_variants_are_forwarded() {
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
                "token with prefix '{value}' for key '{key}' should be forwarded"
            );
        }
    }

    #[test]
    fn sk_prefix_requires_non_empty_suffix() {
        let sk_prefix_only = ["s", "k-"].concat();
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", sk_prefix_only.as_str())]);
        assert!(
            !forwarded.contains_key("ANTHROPIC_API_KEY"),
            "sk- with empty suffix must be skipped"
        );

        let minimal_valid_key = sample_prefixed_value(&["s", "k-"], "a");
        let forwarded = forwarded_env_vars([("ANTHROPIC_API_KEY", minimal_valid_key.as_str())]);
        assert!(
            forwarded.contains_key("ANTHROPIC_API_KEY"),
            "sk-a (minimal valid key) must be forwarded"
        );
    }
}
