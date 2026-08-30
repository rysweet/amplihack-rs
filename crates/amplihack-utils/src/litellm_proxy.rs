//! Fail-closed launch adapter for an operator-managed LiteLLM gateway.
//!
//! This module performs control-plane validation only. Inference traffic flows
//! directly between the selected child CLI and the external gateway.

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, Metadata};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use thiserror::Error;
use url::Url;

pub const ENDPOINT_ENV: &str = "AMPLIHACK_LITELLM_ENDPOINT";
pub const API_KEY_ENV: &str = "AMPLIHACK_LITELLM_API_KEY";
pub const API_KEY_FILE_ENV: &str = "AMPLIHACK_LITELLM_API_KEY_FILE";
pub const COPILOT_MODEL_ENV: &str = "AMPLIHACK_LITELLM_COPILOT_MODEL";

const CONFIG_FILE: &str = "litellm-config.toml";
const CONFIG_LIMIT: u64 = 16 * 1024;
const CREDENTIAL_LIMIT: u64 = 4 * 1024;
const READINESS_LIMIT: u64 = 8 * 1024;
const ENV_NAMES: [&str; 4] = [
    ENDPOINT_ENV,
    API_KEY_ENV,
    API_KEY_FILE_ENV,
    COPILOT_MODEL_ENV,
];

#[path = "litellm_proxy_routing.rs"]
mod routing;
pub use routing::{CliTarget, validate_launch_args};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    Auto,
    Enabled,
    Disabled,
}

fn credential_env_value(name: &str) -> Result<Option<String>, ProxyError> {
    env_value(name).map_err(|_| {
        ProxyError::Credential(format!(
            "{name} must be non-empty valid Unicode without surrounding whitespace or control characters"
        ))
    })
}

pub fn clear_config_environment(command: &mut Command) {
    remove_env(command, &ENV_NAMES);
}

pub fn clear_current_process_configuration() {
    for name in ENV_NAMES {
        // SAFETY: launch dispatch is single-threaded before creating workers.
        unsafe { std::env::remove_var(name) };
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProxyError {
    #[error("AH_LITELLM_CONFIG: {0}")]
    Config(String),
    #[error("AH_LITELLM_CREDENTIAL: {0}")]
    Credential(String),
    #[error("AH_LITELLM_ENDPOINT: {0}")]
    Endpoint(String),
    #[error("AH_LITELLM_DESTINATION: {0}")]
    Destination(String),
    #[error("AH_LITELLM_READINESS: {0}")]
    Readiness(String),
    #[error("AH_LITELLM_PROTOCOL: {0}")]
    Protocol(String),
    #[error("AH_LITELLM_CAPABILITY: {0}")]
    Capability(String),
    #[error("AH_LITELLM_ARGUMENT: {0}")]
    Argument(String),
    #[error("AH_LITELLM_EXECUTABLE_CHANGED: {0}")]
    ExecutableChanged(String),
    #[error("AH_LITELLM_UNSUPPORTED: {0}")]
    Unsupported(String),
}

#[derive(Clone)]
pub struct GatewayConfig {
    root: Url,
    readiness: Url,
    api_key: String,
    copilot_model: Option<String>,
}

impl fmt::Debug for GatewayConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayConfig")
            .field("root", &"[REDACTED]")
            .field("readiness", &"[REDACTED]")
            .field("api_key", &"[REDACTED]")
            .field("copilot_model", &self.copilot_model)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    schema_version: u8,
    endpoint: String,
    #[serde(default)]
    copilot: Option<CopilotConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopilotConfig {
    model: String,
}

impl GatewayConfig {
    pub fn load(activation: Activation, target: CliTarget) -> Result<Option<Self>, ProxyError> {
        Self::load_from(activation, target, config_path())
    }

    fn load_from(
        activation: Activation,
        target: CliTarget,
        path: Option<PathBuf>,
    ) -> Result<Option<Self>, ProxyError> {
        if activation == Activation::Disabled {
            return Ok(None);
        }

        reject_unknown_environment()?;
        let env_signal = ENV_NAMES
            .iter()
            .any(|name| std::env::var_os(name).is_some());
        let file_signal = path
            .as_ref()
            .is_some_and(|path| std::fs::symlink_metadata(path).is_ok());
        if activation == Activation::Auto && !env_signal && !file_signal {
            return Ok(None);
        }

        let file_config = path
            .as_deref()
            .filter(|_| file_signal)
            .map(read_config)
            .transpose()?;
        let endpoint = env_value(ENDPOINT_ENV)?
            .or_else(|| file_config.as_ref().map(|config| config.endpoint.clone()))
            .ok_or_else(|| ProxyError::Config("gateway endpoint is required".to_string()))?;

        let inline_key = credential_env_value(API_KEY_ENV)?;
        let key_path = credential_env_value(API_KEY_FILE_ENV)?;
        let api_key = match (inline_key, key_path) {
            (Some(_), Some(_)) => {
                return Err(ProxyError::Credential(
                    "configure exactly one credential source".to_string(),
                ));
            }
            (Some(key), None) => validate_credential(key)?,
            (None, Some(path)) => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(ProxyError::Credential(
                        "credential file path must be absolute".to_string(),
                    ));
                }
                validate_credential(read_protected(&path, CREDENTIAL_LIMIT, "credential")?)?
            }
            (None, None) => {
                return Err(ProxyError::Credential(
                    "configure exactly one credential source".to_string(),
                ));
            }
        };

        let file_model = file_config
            .and_then(|config| config.copilot)
            .map(|config| config.model);
        let copilot_model = env_value(COPILOT_MODEL_ENV)?.or(file_model);
        let copilot_model = copilot_model.map(validate_model).transpose()?;
        if target == CliTarget::CopilotCli && copilot_model.is_none() {
            return Err(ProxyError::Config(
                "Copilot routing requires one configured model".to_string(),
            ));
        }

        let root = validate_endpoint(&endpoint)?;
        let readiness = derived_url(&root, "health/readiness")?;
        Ok(Some(Self {
            root,
            readiness,
            api_key,
            copilot_model,
        }))
    }

    pub fn check_readiness(&self) -> Result<(), ProxyError> {
        let started = Instant::now();
        let addresses = resolve_destination(&self.readiness, Duration::from_secs(5))?;
        let remaining = Duration::from_secs(15)
            .checked_sub(started.elapsed())
            .ok_or_else(|| {
                ProxyError::Readiness("gateway readiness deadline exceeded".to_string())
            })?;
        self.check_readiness_with_deadline(addresses, remaining)
    }

    #[cfg(test)]
    fn check_readiness_with(&self, addresses: Vec<SocketAddr>) -> Result<(), ProxyError> {
        self.check_readiness_with_deadline(addresses, Duration::from_secs(15))
    }

    fn check_readiness_with_deadline(
        &self,
        mut addresses: Vec<SocketAddr>,
        deadline: Duration,
    ) -> Result<(), ProxyError> {
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(ProxyError::Destination(
                "gateway hostname returned no addresses".to_string(),
            ));
        }
        if addresses.iter().any(|address| prohibited_ip(address.ip())) {
            return Err(ProxyError::Destination(
                "gateway DNS answer contains a prohibited address".to_string(),
            ));
        }
        let selected = addresses[0];
        let agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .redirects(0)
            .max_idle_connections(0)
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(5))
            .timeout(deadline)
            .resolver(move |_netloc: &str| Ok(vec![selected]))
            .user_agent("amplihack-litellm-readiness/1")
            .build();
        let response = agent
            .get(self.readiness.as_str())
            .set("Accept", "application/json")
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|error| ProxyError::Readiness(readiness_error(error)))?;
        if !(200..300).contains(&response.status()) {
            return Err(ProxyError::Readiness(
                "gateway returned a non-success status".to_string(),
            ));
        }
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .unwrap_or_default();
        if content_type != "application/json" && !content_type.ends_with("+json") {
            return Err(ProxyError::Readiness(
                "gateway readiness response must use a JSON media type".to_string(),
            ));
        }
        if response.header("Content-Encoding").is_some() {
            return Err(ProxyError::Readiness(
                "compressed readiness responses are not accepted".to_string(),
            ));
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take(READINESS_LIMIT + 1)
            .read_to_end(&mut body)
            .map_err(|_| ProxyError::Readiness("could not read readiness response".to_string()))?;
        if body.len() as u64 > READINESS_LIMIT {
            return Err(ProxyError::Readiness(
                "gateway readiness response exceeds 8 KiB".to_string(),
            ));
        }
        validate_readiness_json(&body)
    }

    pub fn apply_to_command(&self, command: &mut Command, target: CliTarget) {
        clear_config_environment(command);
        match target {
            CliTarget::Claude | CliTarget::RustyClawd => {
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
                        "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
                        "CLAUDE_CODE_SKIP_VERTEX_AUTH",
                        "AWS_ACCESS_KEY_ID",
                        "AWS_SECRET_ACCESS_KEY",
                        "AWS_SESSION_TOKEN",
                        "AWS_PROFILE",
                        "GOOGLE_APPLICATION_CREDENTIALS",
                    ],
                );
                command.env("ANTHROPIC_BASE_URL", self.root.as_str());
                command.env("ANTHROPIC_AUTH_TOKEN", &self.api_key);
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
                        "COPILOT_PROVIDER_TRANSPORT",
                    ],
                );
                command.env(
                    "COPILOT_PROVIDER_BASE_URL",
                    derived_url(&self.root, "v1")
                        .expect("validated deployment root")
                        .as_str(),
                );
                command.env("COPILOT_PROVIDER_API_KEY", &self.api_key);
                command.env("COPILOT_PROVIDER_TYPE", "openai");
                command.env("COPILOT_PROVIDER_WIRE_API", "completions");
                command.env("COPILOT_OFFLINE", "1");
                command.env(
                    "COPILOT_MODEL",
                    self.copilot_model
                        .as_deref()
                        .expect("validated Copilot model"),
                );
            }
        }
    }

    pub fn copilot_model(&self) -> Option<&str> {
        self.copilot_model.as_deref()
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".amplihack").join(CONFIG_FILE))
}

fn env_value(name: &str) -> Result<Option<String>, ProxyError> {
    match std::env::var(name) {
        Ok(value)
            if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) =>
        {
            Err(ProxyError::Config(format!(
                "{name} must be non-empty and contain no surrounding whitespace or control characters"
            )))
        }
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(ProxyError::Config(format!(
            "{name} must contain valid Unicode"
        ))),
    }
}

fn reject_unknown_environment() -> Result<(), ProxyError> {
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if name.starts_with("AMPLIHACK_LITELLM_") && !ENV_NAMES.contains(&name.as_ref()) {
            return Err(ProxyError::Config(
                "unknown AMPLIHACK_LITELLM_* variable".to_string(),
            ));
        }
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<FileConfig, ProxyError> {
    let text = read_protected(path, CONFIG_LIMIT, "configuration")
        .map_err(|error| ProxyError::Config(error.to_string()))?;
    let config: FileConfig = toml::from_str(&text)
        .map_err(|_| ProxyError::Config("configuration file is malformed".to_string()))?;
    if config.schema_version != 1 {
        return Err(ProxyError::Config(
            "configuration schema_version must be 1".to_string(),
        ));
    }
    if config.endpoint.is_empty()
        || config
            .copilot
            .as_ref()
            .is_some_and(|copilot| copilot.model.is_empty())
    {
        return Err(ProxyError::Config(
            "configuration values must not be empty".to_string(),
        ));
    }
    Ok(config)
}

fn read_protected(path: &Path, limit: u64, kind: &str) -> Result<String, ProxyError> {
    let before = std::fs::symlink_metadata(path)
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be inspected")))?;
    validate_file_metadata(&before, kind)?;
    let mut file = File::open(path)
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be opened")))?;
    let opened = file
        .metadata()
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be inspected")))?;
    validate_file_metadata(&opened, kind)?;
    if !same_file(&before, &opened) {
        return Err(ProxyError::Credential(format!(
            "{kind} file changed while it was opened"
        )));
    }
    if opened.len() > limit {
        return Err(ProxyError::Credential(format!(
            "{kind} file exceeds the size limit"
        )));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be read")))?;
    if bytes.len() as u64 > limit {
        return Err(ProxyError::Credential(format!(
            "{kind} file exceeds the size limit"
        )));
    }
    let after_opened = file
        .metadata()
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be revalidated")))?;
    let after = std::fs::metadata(path)
        .map_err(|_| ProxyError::Credential(format!("{kind} file cannot be revalidated")))?;
    if !metadata_unchanged(&opened, &after_opened) || !metadata_unchanged(&opened, &after) {
        return Err(ProxyError::Credential(format!(
            "{kind} file changed while it was read"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| ProxyError::Credential(format!("{kind} file must contain valid UTF-8")))
}

#[cfg(unix)]
fn validate_file_metadata(metadata: &Metadata, kind: &str) -> Result<(), ProxyError> {
    use std::os::unix::fs::MetadataExt;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(ProxyError::Credential(format!(
            "{kind} file must be a private, singly-linked regular file owned by the current user"
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_file_metadata(_metadata: &Metadata, kind: &str) -> Result<(), ProxyError> {
    Err(ProxyError::Credential(format!(
        "{kind} file safeguards cannot be verified on this platform"
    )))
}

#[cfg(unix)]
fn same_file(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn metadata_unchanged(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    same_file(left, right)
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_file(_left: &Metadata, _right: &Metadata) -> bool {
    false
}

#[cfg(not(unix))]
fn metadata_unchanged(_left: &Metadata, _right: &Metadata) -> bool {
    false
}

fn validate_credential(mut value: String) -> Result<String, ProxyError> {
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.is_empty()
        || value.len() > CREDENTIAL_LIMIT as usize
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ProxyError::Credential(
            "credential must be non-empty, bounded, and contain no whitespace padding or control characters"
                .to_string(),
        ));
    }
    Ok(value)
}

fn validate_model(model: String) -> Result<String, ProxyError> {
    if model.is_empty()
        || model.len() > 128
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return Err(ProxyError::Config(
            "Copilot model must contain 1-128 model-name characters".to_string(),
        ));
    }
    Ok(model)
}

fn validate_endpoint(value: &str) -> Result<Url, ProxyError> {
    if value.chars().any(char::is_control)
        || value.contains('\\')
        || contains_encoded_separator(value)
        || value.contains("/../")
        || value.contains("/./")
        || value.ends_with("/..")
        || value.ends_with("/.")
    {
        return Err(ProxyError::Endpoint(
            "gateway endpoint contains ambiguous characters".to_string(),
        ));
    }
    let mut endpoint = Url::parse(value).map_err(|_| {
        ProxyError::Endpoint("gateway endpoint must be an absolute URL".to_string())
    })?;
    if endpoint.cannot_be_a_base()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.host_str().is_none()
    {
        return Err(ProxyError::Endpoint(
            "gateway endpoint must be an unambiguous deployment root".to_string(),
        ));
    }
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(ProxyError::Endpoint(
            "gateway endpoint must use HTTP or HTTPS".to_string(),
        ));
    }
    let lower_path = endpoint.path().to_ascii_lowercase();
    let segments = lower_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| {
        matches!(
            *segment,
            "." | ".."
                | "v1"
                | "health"
                | "readiness"
                | "chat"
                | "messages"
                | "responses"
                | "completions"
                | "completion"
        )
    }) {
        return Err(ProxyError::Endpoint(
            "gateway endpoint must name a deployment root, not an API or readiness endpoint"
                .to_string(),
        ));
    }
    let host = endpoint.host_str().expect("checked");
    if endpoint.scheme() == "http" {
        let ip = unbracketed_host(host).parse::<IpAddr>().map_err(|_| {
            ProxyError::Endpoint("cleartext HTTP requires a literal loopback address".to_string())
        })?;
        if !literal_http_loopback(ip) {
            return Err(ProxyError::Endpoint(
                "cleartext HTTP requires 127.0.0.0/8 or ::1".to_string(),
            ));
        }
    }
    let trimmed = endpoint.path().trim_end_matches('/').to_string();
    endpoint.set_path(if trimmed.is_empty() { "/" } else { &trimmed });
    Ok(endpoint)
}

fn contains_encoded_separator(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("%2f") || lower.contains("%5c") || lower.contains("%2e")
}

fn literal_http_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.octets()[0] == 127,
        IpAddr::V6(ip) => ip == Ipv6Addr::LOCALHOST,
    }
}

fn derived_url(root: &Url, suffix: &str) -> Result<Url, ProxyError> {
    let mut url = root.clone();
    let path = format!("{}/{}", root.path().trim_end_matches('/'), suffix);
    url.set_path(&path);
    Ok(url)
}

fn resolve_destination(url: &Url, timeout: Duration) -> Result<Vec<SocketAddr>, ProxyError> {
    let host = url
        .host_str()
        .ok_or_else(|| ProxyError::Destination("gateway host is missing".to_string()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ProxyError::Destination("gateway port cannot be determined".to_string()))?;
    let host = unbracketed_host(host).to_string();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("litellm-dns".to_string())
        .spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>());
            let _ = sender.send(result);
        })
        .map_err(|_| ProxyError::Destination("gateway DNS resolver cannot start".to_string()))?;
    receiver
        .recv_timeout(timeout)
        .map_err(|_| {
            ProxyError::Destination("gateway DNS resolution exceeded its deadline".to_string())
        })?
        .map_err(|_| ProxyError::Destination("gateway hostname cannot be resolved".to_string()))
}

fn unbracketed_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host)
}

fn prohibited_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => prohibited_ipv4(ip),
        IpAddr::V6(ip) => {
            ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unicast_link_local()
                || ip == "fd00:ec2::254".parse::<Ipv6Addr>().expect("literal")
                || ip.to_ipv4_mapped().is_some_and(prohibited_ipv4)
        }
    }
}

fn prohibited_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_link_local()
        || ip.is_multicast()
        || octets[0] == 0
        || ip == Ipv4Addr::new(169, 254, 169, 254)
        || ip == Ipv4Addr::new(100, 100, 100, 200)
}

fn readiness_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(_, _) => "gateway returned a non-success status".to_string(),
        ureq::Error::Transport(_) => {
            "gateway readiness connection failed or exceeded its deadline".to_string()
        }
    }
}

fn validate_readiness_json(bytes: &[u8]) -> Result<(), ProxyError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    ReadinessSeed
        .deserialize(&mut deserializer)
        .map_err(|_| ProxyError::Protocol("invalid readiness JSON contract".to_string()))?;
    deserializer
        .end()
        .map_err(|_| ProxyError::Protocol("readiness JSON has trailing data".to_string()))
}

struct ReadinessSeed;

impl<'de> DeserializeSeed<'de> for ReadinessSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(ReadinessVisitor)
    }
}

struct ReadinessVisitor;

impl<'de> Visitor<'de> for ReadinessVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the LiteLLM readiness object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut seen = BTreeSet::new();
        let mut healthy = false;
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom("duplicate readiness field"));
            }
            match key.as_str() {
                "status" => healthy = map.next_value::<String>()? == "healthy",
                "db" => {
                    let db = map.next_value::<String>()?;
                    if !matches!(db.as_str(), "connected" | "Not connected") {
                        return Err(de::Error::custom("invalid db readiness value"));
                    }
                }
                _ => return Err(de::Error::custom("unknown readiness field")),
            }
        }
        if !healthy || !seen.contains("status") {
            return Err(de::Error::custom("gateway is not healthy"));
        }
        Ok(())
    }
}

fn remove_env(command: &mut Command, names: &[&str]) {
    for name in names {
        command.env_remove(name);
    }
}

pub fn proxy_requested() -> bool {
    ENV_NAMES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
        || std::env::vars_os()
            .any(|(name, _)| name.to_string_lossy().starts_with("AMPLIHACK_LITELLM_"))
        || config_path().is_some_and(|path| std::fs::symlink_metadata(path).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn with_env<T>(values: &[(&str, Option<&str>)], test: impl FnOnce() -> T) -> T {
        let _guard = crate::test_serial::acquire();
        let previous = ENV_NAMES.map(std::env::var_os);
        let previous_home = std::env::var_os("HOME");
        let home = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", home.path()) };
        for name in ENV_NAMES {
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in values {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        let result = test();
        for (name, value) in ENV_NAMES.into_iter().zip(previous) {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        result
    }

    #[test]
    fn explicit_disable_ignores_invalid_configuration() {
        with_env(&[(ENDPOINT_ENV, Some("not a URL"))], || {
            assert!(
                GatewayConfig::load(Activation::Disabled, CliTarget::Claude)
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn implicit_partial_configuration_fails_closed() {
        with_env(&[(ENDPOINT_ENV, Some("https://gateway.example"))], || {
            assert!(matches!(
                GatewayConfig::load(Activation::Auto, CliTarget::Claude),
                Err(ProxyError::Credential(_))
            ));
        });
    }

    #[test]
    fn endpoint_accepts_roots_and_rejects_api_or_ambiguous_paths() {
        for accepted in [
            "https://gateway.example",
            "https://gateway.example/team-a",
            "http://127.42.0.1:4000",
            "http://[::1]:4000",
        ] {
            assert!(validate_endpoint(accepted).is_ok(), "{accepted}");
        }
        for rejected in [
            "http://localhost:4000",
            "http://192.168.1.2:4000",
            "https://gateway.example/v1",
            "https://gateway.example/health/readiness",
            "https://gateway.example/a/%2e%2e/v1",
            "https://gateway.example/a%2fb",
            "https://user@gateway.example",
            "https://gateway.example?x=1",
        ] {
            assert!(validate_endpoint(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn readiness_schema_is_exact_and_rejects_duplicates_or_trailing_data() {
        for accepted in [
            br#"{"status":"healthy"}"#.as_slice(),
            br#"{"status":"healthy","db":"connected"}"#.as_slice(),
            br#"{"db":"Not connected","status":"healthy"}"#.as_slice(),
        ] {
            assert!(validate_readiness_json(accepted).is_ok());
        }
        for rejected in [
            br#"{"status":"unhealthy"}"#.as_slice(),
            br#"{"status":"healthy","extra":true}"#.as_slice(),
            br#"{"status":"healthy","status":"healthy"}"#.as_slice(),
            br#"{"status":"healthy"} {}"#.as_slice(),
            br#"[{"status":"healthy"}]"#.as_slice(),
        ] {
            assert!(validate_readiness_json(rejected).is_err());
        }
    }

    #[test]
    fn destination_policy_rejects_special_addresses_but_allows_private_https_targets() {
        for rejected in [
            "0.0.0.0",
            "255.255.255.255",
            "169.254.1.1",
            "169.254.169.254",
            "224.0.0.1",
            "::",
            "fe80::1",
            "ff02::1",
            "::ffff:169.254.169.254",
        ] {
            assert!(prohibited_ip(rejected.parse().unwrap()), "{rejected}");
        }
        for accepted in ["127.0.0.1", "10.0.0.1", "192.168.1.2", "::1", "fd00::1"] {
            assert!(!prohibited_ip(accepted.parse().unwrap()), "{accepted}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn protected_file_rejects_symlinks_and_public_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private");
        fs::write(&private, "secret\n").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_protected(&private, CREDENTIAL_LIMIT, "credential").unwrap(),
            "secret\n"
        );
        let public = directory.path().join("public");
        fs::write(&public, "secret").unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_protected(&public, CREDENTIAL_LIMIT, "credential").is_err());
        let link = directory.path().join("link");
        symlink(&private, &link).unwrap();
        assert!(read_protected(&link, CREDENTIAL_LIMIT, "credential").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn file_schema_and_environment_precedence_are_strict() {
        with_env(&[(API_KEY_ENV, Some("key"))], || {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(CONFIG_FILE);
            fs::write(
                &path,
                "schema_version = 1\nendpoint = \"https://file.example\"\n[copilot]\nmodel = \"file-model\"\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            unsafe {
                std::env::set_var(ENDPOINT_ENV, "https://env.example");
                std::env::set_var(COPILOT_MODEL_ENV, "env-model");
            }
            let config =
                GatewayConfig::load_from(Activation::Enabled, CliTarget::CopilotCli, Some(path))
                    .unwrap()
                    .unwrap();
            assert_eq!(config.root.as_str(), "https://env.example/");
            assert_eq!(config.copilot_model(), Some("env-model"));
        });
    }

    #[test]
    #[cfg(unix)]
    fn unknown_toml_fields_are_rejected() {
        with_env(&[(API_KEY_ENV, Some("key"))], || {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(CONFIG_FILE);
            fs::write(
                &path,
                "schema_version = 1\nendpoint = \"https://gateway.example\"\napi_key = \"forbidden\"\n",
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(
                GatewayConfig::load_from(Activation::Enabled, CliTarget::Claude, Some(path))
                    .is_err()
            );
        });
    }

    #[test]
    #[cfg(not(unix))]
    fn protected_files_fail_closed_when_safeguards_cannot_be_proven() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential");
        fs::write(&path, "secret").unwrap();
        assert!(read_protected(&path, CREDENTIAL_LIMIT, "credential").is_err());
    }

    #[test]
    fn debug_output_redacts_endpoint_and_key() {
        with_env(
            &[
                (ENDPOINT_ENV, Some("https://secret-host.example")),
                (API_KEY_ENV, Some("super-secret-key")),
            ],
            || {
                let config = GatewayConfig::load(Activation::Enabled, CliTarget::Claude)
                    .unwrap()
                    .unwrap();
                let debug = format!("{config:?}");
                assert!(!debug.contains("secret-host"));
                assert!(!debug.contains("super-secret"));
            },
        );
    }

    #[test]
    fn child_projection_uses_environment_only_and_removes_configuration_sources() {
        let config = test_config(Url::parse("https://gateway.example/root").unwrap());
        let mut command = Command::new("claude");
        command.arg("--print");
        config.apply_to_command(&mut command, CliTarget::Claude);
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
                .and_then(|(_, value)| value)
                .and_then(|value| value.to_str()),
            Some("https://gateway.example/root")
        );
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == std::ffi::OsStr::new(API_KEY_ENV) && value.is_none())
        );
        assert!(
            command
                .get_args()
                .all(|argument| !argument.to_string_lossy().contains("never-send-this-key"))
        );
    }

    fn readiness_response(response: &'static str, test: impl FnOnce(SocketAddr)) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /health/readiness HTTP/1.1\r\n"));
            assert!(request.contains("\r\nAccept: application/json\r\n"));
            assert!(request.contains("\r\nAccept-Encoding: identity\r\n"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            stream.write_all(response.as_bytes()).unwrap();
        });
        test(address);
        server.join().unwrap();
    }

    fn test_config(root: Url) -> GatewayConfig {
        GatewayConfig {
            readiness: derived_url(&root, "health/readiness").unwrap(),
            root,
            api_key: "never-send-this-key".to_string(),
            copilot_model: None,
        }
    }

    #[test]
    fn readiness_is_one_unauthenticated_bounded_request() {
        readiness_response(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"status\":\"healthy\"}",
            |address| {
                let root = Url::parse(&format!("http://127.0.0.1:{}", address.port())).unwrap();
                test_config(root)
                    .check_readiness_with(vec![address])
                    .unwrap();
            },
        );
    }

    #[test]
    fn readiness_rejects_redirects_and_invalid_protocol() {
        readiness_response(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            |address| {
                let root = Url::parse(&format!("http://127.0.0.1:{}", address.port())).unwrap();
                assert!(matches!(
                    test_config(root).check_readiness_with(vec![address]),
                    Err(ProxyError::Readiness(_))
                ));
            },
        );
        readiness_response(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 22\r\nConnection: close\r\n\r\n{\"status\":\"unhealthy\"}",
            |address| {
                let root = Url::parse(&format!("http://127.0.0.1:{}", address.port())).unwrap();
                assert!(matches!(
                    test_config(root).check_readiness_with(vec![address]),
                    Err(ProxyError::Protocol(_))
                ));
            },
        );
    }
}
