//! Docker detection and command construction for launcher parity.

pub(crate) mod helpers;
mod manager;

pub(crate) use manager::DockerManager;

use std::env;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::util::{run_output_with_timeout, run_with_timeout};

pub(crate) const DEFAULT_IMAGE_NAME: &str = "amplihack:latest";
pub(crate) const LITELLM_ROUTING_LABEL: &str = "org.amplihack.litellm-routing";
pub(crate) const LITELLM_ROUTING_REVISION: &str = "2";
pub(crate) const VERSION_LABEL: &str = "org.amplihack.version";
const DOCKER_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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
    pub(crate) fn requested_outside_container(self, docker_flag: bool) -> bool {
        docker_flag
            || (is_truthy_env_value(env::var("AMPLIHACK_USE_DOCKER").ok().as_deref())
                && !self.is_in_docker())
    }

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

        let mut command = Command::new("docker");
        command
            .arg("info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_with_timeout(command, DOCKER_PROBE_TIMEOUT)
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Whether this launch should be redirected into a container.
    ///
    /// Delegates to [`should_use_docker_lazily`], which is the same rule as
    /// [`should_use_docker_from_state`] but does not compute an operand until
    /// the operands before it have failed to decide the answer.
    pub(crate) fn should_use_docker(self) -> bool {
        should_use_docker_lazily(
            env::var("AMPLIHACK_USE_DOCKER").ok().as_deref(),
            || self.is_in_docker(),
            || self.is_running(),
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

        let mut command = Command::new("docker");
        command
            .args(["images", "-q", image_name])
            .stdin(Stdio::null());
        run_output_with_timeout(command, DOCKER_PROBE_TIMEOUT)
            .map(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty())
            .unwrap_or(false)
    }

    pub(crate) fn image_supports_litellm(self, image_name: &str) -> bool {
        let format = format!(
            "{{{{ index .Config.Labels \"{LITELLM_ROUTING_LABEL}\" }}}}|{{{{ index .Config.Labels \"{VERSION_LABEL}\" }}}}"
        );
        let mut command = Command::new("docker");
        command
            .args(["image", "inspect", "--format", &format, image_name])
            .stdin(Stdio::null());
        run_output_with_timeout(command, DOCKER_PROBE_TIMEOUT)
            .map(|output| {
                String::from_utf8_lossy(&output.stdout).trim()
                    == format!("{LITELLM_ROUTING_REVISION}|{}", crate::VERSION)
            })
            .unwrap_or(false)
    }
}

/// [`should_use_docker_from_state`], but lazy in its two probes.
///
/// # Why this exists
///
/// The rule is `env && !in_docker && running`, and expressing it by calling
/// `should_use_docker_from_state` with the probes as arguments looked
/// equivalent but was not: Rust evaluates arguments eagerly, so **both** probes
/// ran before the env gate could reject them. [`DockerDetector::is_running`]
/// scans `$PATH` for `docker` and then spawns `docker info`, which talks to the
/// daemon socket — routinely 200 ms-1 s, and up to the full
/// [`DOCKER_PROBE_TIMEOUT`] of 5 s in the common case where a docker *client*
/// is installed but no daemon is running.
///
/// That ran on every `amplihack claude` launch, so every user with docker on
/// `$PATH` paid a subprocess, and potentially five seconds of startup latency,
/// to compute an operand of a conjunction whose first term was already `false`.
///
/// Short-circuiting cannot change the answer: each early return reproduces the
/// conjunct it stands for, so any input that returns early would have produced
/// `false` from the state function too. `should_use_docker_from_state` remains
/// the single source of truth for the decision and remains testable against
/// synthetic state; this adds only the evaluation order.
///
/// Taking the probes as closures rather than `bool`s is the point — it is what
/// makes "the probes are not run" a property the type system enforces at the
/// call site and `a_falsy_env_never_probes_for_docker` can assert directly.
/// The ordering is stated in full above and needs no external referent; that
/// this form and the state function agree is proved by
/// `the_lazy_form_agrees_with_the_state_function_everywhere`, which crosses all
/// 20 input combinations.
fn should_use_docker_lazily(
    env_value: Option<&str>,
    in_docker: impl FnOnce() -> bool,
    docker_running: impl FnOnce() -> bool,
) -> bool {
    if !is_truthy_env_value(env_value) {
        return false;
    }
    // Cheapest-first: `is_in_docker` is a small file read, `is_running` is a
    // subprocess.
    let in_docker = in_docker();
    if in_docker {
        return false;
    }
    should_use_docker_from_state(env_value, in_docker, docker_running())
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

    /// The probes must not run when the environment has already decided.
    ///
    /// This is the regression that motivated [`should_use_docker_lazily`]:
    /// `is_running()` spawns `docker info` (up to a 5 s daemon timeout) and it
    /// was being evaluated as an eager argument on every launch, including the
    /// overwhelmingly common one where `AMPLIHACK_USE_DOCKER` is unset. The
    /// assertion is on the *call counts*, not on the return value, because the
    /// return value was already correct — it was the cost of computing it that
    /// was wrong.
    #[test]
    fn a_falsy_env_never_probes_for_docker() {
        for env_value in [None, Some("0"), Some("false"), Some("")] {
            let in_docker_calls = std::cell::Cell::new(0);
            let running_calls = std::cell::Cell::new(0);

            let decision = should_use_docker_lazily(
                env_value,
                || {
                    in_docker_calls.set(in_docker_calls.get() + 1);
                    false
                },
                || {
                    running_calls.set(running_calls.get() + 1);
                    true
                },
            );

            assert!(!decision, "env {env_value:?} must not select docker");
            assert_eq!(
                (in_docker_calls.get(), running_calls.get()),
                (0, 0),
                "env {env_value:?} decides the answer alone; neither probe may run"
            );
        }
    }

    /// Being inside a container decides the answer before the subprocess.
    #[test]
    fn being_in_docker_short_circuits_the_daemon_probe() {
        let running_calls = std::cell::Cell::new(0);
        let decision = should_use_docker_lazily(
            Some("true"),
            || true,
            || {
                running_calls.set(running_calls.get() + 1);
                true
            },
        );
        assert!(!decision);
        assert_eq!(
            running_calls.get(),
            0,
            "already inside docker; `docker info` must not be spawned"
        );
    }

    /// Laziness must not have changed the decision itself: the lazy form and
    /// the state function agree on every combination of inputs.
    #[test]
    fn the_lazy_form_agrees_with_the_state_function_everywhere() {
        for env_value in [None, Some("0"), Some("true"), Some("1"), Some("yes")] {
            for in_docker in [false, true] {
                for running in [false, true] {
                    assert_eq!(
                        should_use_docker_lazily(env_value, || in_docker, || running),
                        should_use_docker_from_state(env_value, in_docker, running),
                        "disagreement at {env_value:?}/{in_docker}/{running}"
                    );
                }
            }
        }
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
                ("AMPLIHACK_LITELLM_API_KEY", "gateway-secret"),
                ("AMPLIHACK_LITELLM_ENDPOINT", "http://127.0.0.1:4000"),
                (
                    "AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION",
                    "Bearer telemetry-secret",
                ),
                ("ANTHROPIC_API_KEY", anthropic_api_key.as_str()),
                ("OPENAI_API_KEY", anthropic_api_key.as_str()),
                ("GITHUB_TOKEN", "ghp_gatewayMustNotForward"),
                ("GH_TOKEN", "ghs_gatewayMustNotForward"),
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
        assert!(
            args.windows(2)
                .any(|window| { window[0] == "-e" && window[1] == "AMPLIHACK_LITELLM_API_KEY" })
        );
        assert!(!args.iter().any(|arg| matches!(
            arg.as_str(),
            "ANTHROPIC_API_KEY" | "OPENAI_API_KEY" | "GITHUB_TOKEN" | "GH_TOKEN"
        )));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION"))
        );
        assert!(args.windows(2).any(|window| {
            window[0] == "-e" && window[1] == "AMPLIHACK_LITELLM_ENDPOINT=http://127.0.0.1:4000"
        }));
        assert!(!args.iter().any(|arg| arg == "--network"));
        assert!(!args.iter().any(|arg| arg == "--add-host"));
        assert!(!args.iter().any(|arg| arg.contains("gateway-secret")));
        assert!(!args.iter().any(|arg| arg.contains("telemetry-secret")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains(anthropic_api_key.as_str()))
        );
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
                "build".to_string(),
                "-t".to_string(),
                "amplihack:latest".to_string(),
                "--label".to_string(),
                format!("org.amplihack.litellm-routing={}", LITELLM_ROUTING_REVISION),
                "--label".to_string(),
                format!("org.amplihack.version={}", crate::VERSION),
                "-f".to_string(),
                "/tmp/amplihack-rs/Dockerfile".to_string(),
                "/tmp/amplihack-rs".to_string(),
            ]
        );
    }

    #[test]
    fn docker_network_isolation_is_preserved_without_a_loopback_gateway() {
        let manager = DockerManager::new_for_tests(PathBuf::from("/repo"));
        for env in [
            vec![("AMPLIHACK_SESSION_ID", "abc123")],
            vec![
                ("AMPLIHACK_SESSION_ID", "abc123"),
                ("AMPLIHACK_LITELLM_ENDPOINT", "https://gateway.example.com"),
            ],
        ] {
            let args = manager.build_run_args(
                std::path::Path::new("/tmp/workspace"),
                &["launch".to_string()],
                env,
            );
            assert!(!args.iter().any(|arg| arg == "--network"));
            assert!(!args.iter().any(|arg| arg == "--add-host"));
        }
    }

    #[test]
    fn docker_launcher_rejects_loopback_gateway_without_weakening_isolation() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os(amplihack_utils::litellm_proxy::ENDPOINT_ENV);
        for endpoint in [
            "http://127.0.0.1:4000",
            "http://127.0.0.2:4000",
            "https://127.1:4000",
            "http://localhost:4000",
            "http://[::1]:4000",
            "https://[::ffff:127.0.0.1]:4000",
        ] {
            unsafe { std::env::set_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV, endpoint) };
            assert!(
                super::manager::validate_docker_gateway_env().is_err(),
                "{endpoint} must be rejected for Docker launches"
            );
        }
        match previous {
            Some(value) => unsafe {
                std::env::set_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV, value)
            },
            None => unsafe { std::env::remove_var(amplihack_utils::litellm_proxy::ENDPOINT_ENV) },
        }
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

        let forwarded = forwarded_env_vars([
            ("ANTHROPIC_API_KEY", "sk-provider-must-not-forward"),
            ("GITHUB_TOKEN", "ghp_providerMustNotForward"),
            ("AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION", "telemetry-secret"),
            ("AMPLIHACK_DATABASE_PASSWORD", "database-secret"),
            ("AMPLIHACK_LITELLM_API_KEY", "gateway-secret"),
        ]);
        assert!(!forwarded.contains_key("ANTHROPIC_API_KEY"));
        assert!(!forwarded.contains_key("GITHUB_TOKEN"));
        assert!(!forwarded.contains_key("AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION"));
        assert!(!forwarded.contains_key("AMPLIHACK_DATABASE_PASSWORD"));
        assert_eq!(
            forwarded
                .get(amplihack_utils::litellm_proxy::API_KEY_ENV)
                .map(String::as_str),
            Some("gateway-secret")
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
