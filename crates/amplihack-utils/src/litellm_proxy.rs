//! Secure environment adapter for an externally operated LiteLLM gateway.
//!
//! Amplihack never handles inference traffic. It validates the routing
//! contract and configures supported child CLIs to speak directly to LiteLLM.

use std::fmt;
use std::net::IpAddr;
use std::process::Command;

use semver::Version;
use thiserror::Error;
use url::Url;

pub const ENDPOINT_ENV: &str = "AMPLIHACK_LITELLM_ENDPOINT";
pub const API_KEY_ENV: &str = "AMPLIHACK_LITELLM_API_KEY";
pub const MODEL_ENV: &str = "AMPLIHACK_LITELLM_MODEL";
pub const MIN_CLAUDE_CODE_VERSION: &str = "2.1.83";

const CONFIG_ENV_VARS: [&str; 3] = [ENDPOINT_ENV, API_KEY_ENV, MODEL_ENV];

#[path = "litellm_proxy_routing.rs"]
mod routing;
pub use routing::{CliTarget, validate_launch_args};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("invalid LiteLLM gateway configuration: {0}")]
    InvalidConfig(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCodeCapability {
    Supported { version: String },
    Unsupported { version: String },
    Malformed { reported: String },
    Unknown,
}

pub fn claude_code_capability(version: Option<&str>) -> ClaudeCodeCapability {
    let Some(reported) = version else {
        return ClaudeCodeCapability::Unknown;
    };
    let Ok(version) = Version::parse(reported) else {
        return ClaudeCodeCapability::Malformed {
            reported: reported.to_string(),
        };
    };
    let minimum =
        Version::parse(MIN_CLAUDE_CODE_VERSION).expect("minimum Claude Code version is valid");
    if version >= minimum {
        ClaudeCodeCapability::Supported {
            version: reported.to_string(),
        }
    } else {
        ClaudeCodeCapability::Unsupported {
            version: reported.to_string(),
        }
    }
}

pub fn require_claude_code_capability(version: Option<&str>) -> Result<(), ProxyError> {
    match claude_code_capability(version) {
        ClaudeCodeCapability::Supported { .. } => Ok(()),
        ClaudeCodeCapability::Unsupported { version } => Err(ProxyError::InvalidConfig(format!(
            "Claude Code {version} cannot safely use external LiteLLM routing; version {MIN_CLAUDE_CODE_VERSION} or newer is required"
        ))),
        ClaudeCodeCapability::Malformed { .. } => Err(ProxyError::InvalidConfig(format!(
            "Claude Code reported a malformed version; version {MIN_CLAUDE_CODE_VERSION} or newer is required for external LiteLLM routing"
        ))),
        ClaudeCodeCapability::Unknown => Err(ProxyError::InvalidConfig(format!(
            "Claude Code version could not be verified; version {MIN_CLAUDE_CODE_VERSION} or newer is required for external LiteLLM routing"
        ))),
    }
}

struct ProxyConfig {
    endpoint: Url,
    api_key: String,
    model: String,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyConfig")
            .field("endpoint", &"[REDACTED]")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

impl ProxyConfig {
    fn from_env() -> Result<Option<Self>, ProxyError> {
        if !proxy_requested() {
            return Ok(None);
        }

        let endpoint = validate_endpoint(&required_env(ENDPOINT_ENV)?)?;
        let api_key = required_env(API_KEY_ENV)?;
        let model = validate_model(required_env(MODEL_ENV)?)?;
        if api_key.len() > 4096 {
            return Err(ProxyError::InvalidConfig(format!(
                "{API_KEY_ENV} exceeds the maximum supported length"
            )));
        }

        Ok(Some(Self {
            endpoint,
            api_key,
            model,
        }))
    }
}

struct ProxyChildEnvironment {
    target: CliTarget,
    endpoint: Url,
    api_key: String,
    model: String,
}

impl fmt::Debug for ProxyChildEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyChildEnvironment")
            .field("target", &self.target)
            .field("endpoint", &"[REDACTED]")
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

impl ProxyChildEnvironment {
    fn from_config(target: CliTarget, config: ProxyConfig) -> Self {
        Self {
            target,
            endpoint: config.endpoint,
            api_key: config.api_key,
            model: config.model,
        }
    }

    fn apply_to_command(&self, command: &mut Command) {
        remove_env(command, &CONFIG_ENV_VARS);

        match self.target {
            CliTarget::Claude | CliTarget::RustyClawd => {
                if self.target == CliTarget::Claude {
                    command.env("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1");
                } else {
                    command.env_remove("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB");
                }
                remove_env(
                    command,
                    &[
                        "ANTHROPIC_API_KEY",
                        "ANTHROPIC_CUSTOM_HEADERS",
                        "ANTHROPIC_BEDROCK_BASE_URL",
                        "ANTHROPIC_VERTEX_BASE_URL",
                        "CLAUDE_CODE_USE_BEDROCK",
                        "CLAUDE_CODE_USE_VERTEX",
                        "CLAUDE_CODE_USE_FOUNDRY",
                        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
                        "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
                        "CLAUDE_CODE_SKIP_VERTEX_AUTH",
                        "ANTHROPIC_AWS_API_KEY",
                        "ANTHROPIC_AWS_REGION",
                        "AWS_ACCESS_KEY_ID",
                        "AWS_SECRET_ACCESS_KEY",
                        "AWS_SESSION_TOKEN",
                        "AWS_PROFILE",
                        "GOOGLE_APPLICATION_CREDENTIALS",
                    ],
                );
                command.env(
                    "ANTHROPIC_BASE_URL",
                    anthropic_base_url(&self.endpoint).as_str(),
                );
                command.env("ANTHROPIC_AUTH_TOKEN", &self.api_key);
                command.env("ANTHROPIC_MODEL", &self.model);
                command.env("ANTHROPIC_SMALL_FAST_MODEL", &self.model);
                command.env("CLAUDE_CODE_SUBAGENT_MODEL", &self.model);
                command.env("ANTHROPIC_DEFAULT_OPUS_MODEL", &self.model);
                command.env("ANTHROPIC_DEFAULT_SONNET_MODEL", &self.model);
                command.env("ANTHROPIC_DEFAULT_HAIKU_MODEL", &self.model);
                command.env("ANTHROPIC_DEFAULT_FABLE_MODEL", &self.model);
            }
            CliTarget::CopilotCli => {
                remove_env(
                    command,
                    &[
                        "OPENAI_BASE_URL",
                        "OPENAI_API_KEY",
                        "COPILOT_PROVIDER_BEARER_TOKEN",
                        "COPILOT_PROVIDER_HEADERS",
                        "COPILOT_PROVIDER_WIRE_MODEL",
                        "COPILOT_PROVIDER_MODEL_ID",
                        "COPILOT_PROVIDER_GHES_HOST",
                        "COPILOT_PROVIDER_GHES_TOKEN",
                        "COPILOT_OFFLINE",
                        "COPILOT_PROVIDER_TRANSPORT",
                    ],
                );
                command.env(
                    "COPILOT_PROVIDER_BASE_URL",
                    copilot_base_url(&self.endpoint).as_str(),
                );
                command.env("COPILOT_PROVIDER_API_KEY", &self.api_key);
                command.env("COPILOT_PROVIDER_TYPE", "openai");
                command.env("COPILOT_PROVIDER_WIRE_API", "completions");
                command.env("COPILOT_MODEL", &self.model);
            }
        }

        command.env(
            "AMPLIHACK_LITELLM_TARGET",
            match self.target {
                CliTarget::Claude => "claude",
                CliTarget::CopilotCli => "copilot",
                CliTarget::RustyClawd => "rustyclawd",
            },
        );
    }
}

fn remove_env(command: &mut Command, names: &[&str]) {
    for name in names {
        command.env_remove(name);
    }
}

fn required_env(name: &str) -> Result<String, ProxyError> {
    let value = std::env::var(name).map_err(|_| {
        ProxyError::InvalidConfig(format!(
            "{name} is required when any LiteLLM gateway variable is present"
        ))
    })?;
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ProxyError::InvalidConfig(format!(
            "{name} must be non-empty and contain no surrounding whitespace or control characters"
        )));
    }
    Ok(value)
}

fn validate_model(model: String) -> Result<String, ProxyError> {
    let valid = !model.is_empty()
        && model.len() <= 128
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte));
    if !valid {
        return Err(ProxyError::InvalidConfig(format!(
            "{MODEL_ENV} must contain 1-128 model-name characters"
        )));
    }
    Ok(model)
}

fn validate_endpoint(value: &str) -> Result<Url, ProxyError> {
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(ProxyError::InvalidConfig(format!(
            "{ENDPOINT_ENV} must not contain whitespace or control characters"
        )));
    }
    let endpoint = Url::parse(value).map_err(|_| {
        ProxyError::InvalidConfig(format!("{ENDPOINT_ENV} must be an absolute URL"))
    })?;
    if !value.starts_with(&format!("{}://", endpoint.scheme()))
        || endpoint.cannot_be_a_base()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ProxyError::InvalidConfig(format!(
            "{ENDPOINT_ENV} must be an unambiguous absolute URL without credentials, query, or fragment"
        )));
    }
    let host = endpoint
        .host_str()
        .ok_or_else(|| ProxyError::InvalidConfig(format!("{ENDPOINT_ENV} must include a host")))?;
    if !matches!(endpoint.path(), "/" | "/v1" | "/v1/") {
        return Err(ProxyError::InvalidConfig(format!(
            "{ENDPOINT_ENV} path must be empty or /v1"
        )));
    }

    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let literal_loopback = host.parse::<IpAddr>().is_ok_and(address_is_loopback);
    if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && literal_loopback) {
        return Err(ProxyError::InvalidConfig(format!(
            "{ENDPOINT_ENV} must use HTTPS; HTTP is allowed only for literal loopback addresses"
        )));
    }
    Ok(endpoint)
}

fn address_is_loopback(address: IpAddr) -> bool {
    address.is_loopback()
        || match address {
            IpAddr::V4(_) => false,
            IpAddr::V6(address) => address
                .to_ipv4_mapped()
                .is_some_and(|address| address.is_loopback()),
        }
}

fn copilot_base_url(endpoint: &Url) -> Url {
    let mut endpoint = endpoint.clone();
    endpoint.set_path("/v1");
    endpoint
}

fn anthropic_base_url(endpoint: &Url) -> Url {
    let mut endpoint = endpoint.clone();
    endpoint.set_path("/");
    endpoint
}

pub fn endpoint_is_loopback(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|endpoint| endpoint.host_str().map(str::to_owned))
        .is_some_and(|host| {
            let host = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(&host);
            host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(address_is_loopback)
        })
}

/// True when any required gateway variable is present, including an empty or
/// non-Unicode value. Callers must then validate and fail closed.
pub fn proxy_requested() -> bool {
    CONFIG_ENV_VARS
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

pub fn validate_environment() -> Result<bool, ProxyError> {
    ProxyConfig::from_env().map(|config| config.is_some())
}

pub fn apply_proxy_to_command(
    command: &mut Command,
    target: CliTarget,
) -> Result<bool, ProxyError> {
    let Some(config) = ProxyConfig::from_env()? else {
        return Ok(false);
    };
    ProxyChildEnvironment::from_config(target, config).apply_to_command(command);
    Ok(true)
}

pub(crate) fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn with_gateway_env<T>(
        endpoint: Option<&str>,
        key: Option<&str>,
        model: Option<&str>,
        test: impl FnOnce() -> T,
    ) -> T {
        let _guard = crate::test_serial::acquire();
        let previous = CONFIG_ENV_VARS.map(std::env::var_os);
        for (name, value) in CONFIG_ENV_VARS.into_iter().zip([endpoint, key, model]) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        let result = test();
        for (name, value) in CONFIG_ENV_VARS.into_iter().zip(previous) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        result
    }

    fn command_env(command: &Command, name: &str) -> Option<Option<String>> {
        command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(name))
            .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    }

    #[test]
    fn no_configuration_preserves_default_behavior() {
        with_gateway_env(None, None, None, || {
            assert!(!proxy_requested());
            assert_eq!(validate_environment(), Ok(false));
            let mut command = Command::new("claude");
            assert!(!apply_proxy_to_command(&mut command, CliTarget::Claude).unwrap());
            assert_eq!(command.get_envs().count(), 0);
        });
    }

    #[test]
    fn claude_code_capability_accepts_minimum_and_newer_semver() {
        for version in ["2.1.83", "2.1.84", "3.0.0", "2.1.84-beta.1"] {
            assert_eq!(
                claude_code_capability(Some(version)),
                ClaudeCodeCapability::Supported {
                    version: version.to_string()
                },
                "{version}"
            );
            assert!(require_claude_code_capability(Some(version)).is_ok());
        }
    }

    #[test]
    fn claude_code_capability_rejects_old_prerelease_unknown_and_malformed_versions() {
        for version in ["2.1.82", "2.1.83-beta.1", "not-a-version", "", "2.1"] {
            let capability = claude_code_capability(Some(version));
            assert!(
                !matches!(capability, ClaudeCodeCapability::Supported { .. }),
                "{version} must not prove subprocess environment scrubbing"
            );
            let error = require_claude_code_capability(Some(version))
                .expect_err("unsupported capability must fail closed");
            assert!(
                !format!("{error}").contains("gateway-secret"),
                "capability errors must not disclose credentials"
            );
        }

        assert_eq!(claude_code_capability(None), ClaudeCodeCapability::Unknown);
        assert!(require_claude_code_capability(None).is_err());
    }

    #[test]
    fn partial_empty_and_invalid_configuration_fails_closed() {
        for values in [
            (Some("https://gateway.example.com"), None, None),
            (None, Some("secret"), None),
            (None, None, Some("gateway-model")),
            (Some(""), None, None),
            (Some("http://localhost:4000"), Some("secret"), Some("model")),
        ] {
            with_gateway_env(values.0, values.1, values.2, || {
                assert!(proxy_requested());
                assert!(validate_environment().is_err(), "{values:?}");
            });
        }
    }

    #[test]
    fn endpoint_requires_https_except_literal_loopback() {
        for accepted in [
            "https://gateway.example.com",
            "https://gateway.example.com/v1",
            "http://127.0.0.1:4000",
            "http://127.1:4000",
            "http://[::1]:4000",
        ] {
            assert!(validate_endpoint(accepted).is_ok(), "{accepted}");
        }
        for rejected in [
            "http://gateway.example.com",
            "http://localhost:4000",
            "https://user:pass@gateway.example.com",
            "https://gateway.example.com/models",
            "https://gateway.example.com////",
            "https://gateway.example.com/v1////",
            "https://gateway.example.com?secret=value",
            "https://gateway.example.com#fragment",
            "https:gateway.example.com",
            "file:///tmp/socket",
        ] {
            assert!(validate_endpoint(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn child_environments_remove_only_routing_credentials_and_redact_debug() {
        for target in [
            CliTarget::Claude,
            CliTarget::RustyClawd,
            CliTarget::CopilotCli,
        ] {
            with_gateway_env(
                Some("https://gateway.example.com"),
                Some("gateway-secret"),
                Some("gateway-model"),
                || {
                    let config = ProxyConfig::from_env().unwrap().unwrap();
                    assert!(!format!("{config:?}").contains("gateway-secret"));
                    let mut command = Command::new("child");
                    command.env("ANTHROPIC_API_KEY", "bypass");
                    command.env("CLAUDE_CODE_USE_ANTHROPIC_AWS", "1");
                    command.env("ANTHROPIC_AWS_API_KEY", "bypass");
                    command.env("ANTHROPIC_DEFAULT_SONNET_MODEL", "bypass-model");
                    command.env("OPENAI_API_KEY", "bypass");
                    command.env("GITHUB_TOKEN", "bypass");
                    command.env("DATABASE_PASSWORD", "bypass");
                    apply_proxy_to_command(&mut command, target).unwrap();
                    for name in CONFIG_ENV_VARS {
                        assert_eq!(command_env(&command, name), Some(None));
                    }
                    assert_eq!(
                        command_env(&command, "GITHUB_TOKEN"),
                        Some(Some("bypass".to_string()))
                    );
                    assert_eq!(
                        command_env(&command, "DATABASE_PASSWORD"),
                        Some(Some("bypass".to_string()))
                    );
                    match target {
                        CliTarget::Claude | CliTarget::RustyClawd => {
                            assert_eq!(
                                command_env(&command, "ANTHROPIC_AUTH_TOKEN"),
                                Some(Some("gateway-secret".to_string()))
                            );
                            let expected_scrub = if target == CliTarget::Claude {
                                Some(Some("1".to_string()))
                            } else {
                                Some(None)
                            };
                            assert_eq!(
                                command_env(&command, "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"),
                                expected_scrub
                            );
                            assert_eq!(command_env(&command, "ANTHROPIC_API_KEY"), Some(None));
                            assert_eq!(
                                command_env(&command, "OPENAI_API_KEY"),
                                Some(Some("bypass".to_string()))
                            );
                            assert_eq!(
                                command_env(&command, "CLAUDE_CODE_USE_ANTHROPIC_AWS"),
                                Some(None)
                            );
                            assert_eq!(command_env(&command, "ANTHROPIC_AWS_API_KEY"), Some(None));
                            assert_eq!(
                                command_env(&command, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
                                Some(Some("gateway-model".to_string()))
                            );
                        }
                        CliTarget::CopilotCli => {
                            assert_eq!(
                                command_env(&command, "COPILOT_PROVIDER_BASE_URL"),
                                Some(Some("https://gateway.example.com/v1".to_string()))
                            );
                            assert_eq!(command_env(&command, "OPENAI_API_KEY"), Some(None));
                            assert_eq!(
                                command_env(&command, "ANTHROPIC_API_KEY"),
                                Some(Some("bypass".to_string()))
                            );
                            assert_eq!(
                                command_env(&command, "ANTHROPIC_AWS_API_KEY"),
                                Some(Some("bypass".to_string()))
                            );
                        }
                    }
                },
            );
        }
    }

    #[test]
    fn endpoint_forms_are_normalized_for_each_protocol() {
        for endpoint in [
            "https://gateway.example.com",
            "https://gateway.example.com/",
            "https://gateway.example.com/v1",
            "https://gateway.example.com/v1/",
        ] {
            with_gateway_env(
                Some(endpoint),
                Some("gateway-secret"),
                Some("gateway-model"),
                || {
                    let mut claude = Command::new("claude");
                    apply_proxy_to_command(&mut claude, CliTarget::Claude).unwrap();
                    assert_eq!(
                        command_env(&claude, "ANTHROPIC_BASE_URL"),
                        Some(Some("https://gateway.example.com/".to_string()))
                    );

                    let mut copilot = Command::new("copilot");
                    apply_proxy_to_command(&mut copilot, CliTarget::CopilotCli).unwrap();
                    assert_eq!(
                        command_env(&copilot, "COPILOT_PROVIDER_BASE_URL"),
                        Some(Some("https://gateway.example.com/v1".to_string()))
                    );
                },
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn direct_child_streaming_and_failure_status_are_preserved() {
        with_gateway_env(
            Some("https://gateway.example.com"),
            Some("gateway-secret"),
            Some("gateway-model"),
            || {
                let mut streaming = Command::new("sh");
                streaming.args([
                    "-c",
                    "printf first; sleep 0.01; printf second >&2; printf third",
                ]);
                apply_proxy_to_command(&mut streaming, CliTarget::Claude).unwrap();
                assert!(
                    !streaming
                        .get_args()
                        .any(|arg| arg.to_string_lossy().contains("gateway-secret"))
                );
                let output = streaming.output().unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout, b"firstthird");
                assert_eq!(output.stderr, b"second");

                let mut failing = Command::new("sh");
                failing.args(["-c", "exit 23"]);
                apply_proxy_to_command(&mut failing, CliTarget::Claude).unwrap();
                assert_eq!(failing.status().unwrap().code(), Some(23));
            },
        );
    }

    #[test]
    fn model_rejects_shell_metacharacters() {
        assert_eq!(
            validate_model("openai/gpt-5.4".to_string()).unwrap(),
            "openai/gpt-5.4"
        );
        assert!(validate_model("model;env".to_string()).is_err());
        assert!(validate_model(String::new()).is_err());
    }
}
