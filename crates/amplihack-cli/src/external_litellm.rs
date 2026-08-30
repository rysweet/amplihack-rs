//! Secure client-side routing to an operator-managed LiteLLM gateway.
//!
//! This module deliberately contains no proxy lifecycle or control-plane code.
//! It validates one external route, performs one pinned readiness request, and
//! returns the final environment patch for a supported agent process.

use crate::binary_finder::BinaryInfo;
use crate::env_builder::EnvBuilder;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde::Deserializer as _;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use url::Url;

const CONFIG_LIMIT: u64 = 64 * 1024;
const CREDENTIAL_LIMIT: usize = 4 * 1024;
const RESPONSE_LIMIT: u64 = 8 * 1024;
const JSON_DEPTH_LIMIT: usize = 16;
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_TIMEOUT: Duration = Duration::from_secs(2);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_LIMIT: usize = 64 * 1024;

const KNOWN_CONTROLS: &[&str] = &[
    "AMPLIHACK_LITELLM_ENDPOINT",
    "AMPLIHACK_LITELLM_API_KEY",
    "AMPLIHACK_LITELLM_API_KEY_FILE",
    "AMPLIHACK_LITELLM_COPILOT_MODEL",
];

#[derive(Clone, Copy, Debug, Default)]
pub struct GatewayControl {
    pub enabled: bool,
    pub disabled: bool,
}

impl GatewayControl {
    pub const fn new(enabled: bool, disabled: bool) -> Self {
        Self { enabled, disabled }
    }

    pub fn may_route(self) -> bool {
        self.enabled
            || (!self.disabled
                && (KNOWN_CONTROLS
                    .iter()
                    .any(|name| std::env::var_os(name).is_some())
                    || config_path().is_some_and(|path| fs::symlink_metadata(path).is_ok())))
    }

    /// Cheap, side-effect-free startup check used to keep unrelated update and
    /// self-heal output ahead of a stable gateway diagnostic.
    pub fn startup_may_route(args: &[std::ffi::OsString]) -> bool {
        let command = args.get(1).and_then(|value| value.to_str());
        if !matches!(
            command,
            Some("launch" | "claude" | "copilot" | "codex" | "amplifier" | "RustyClawd")
        ) {
            return false;
        }
        let has_enable = args.iter().any(|value| value == "--litellm");
        let has_disable = args.iter().any(|value| value == "--no-litellm");
        if has_enable {
            return true;
        }
        if has_disable {
            return false;
        }
        GatewayControl::default().may_route()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ErrorCode {
    Config,
    Credential,
    Endpoint,
    Destination,
    Readiness,
    Protocol,
    Capability,
    Argument,
    ExecutableChanged,
    Unsupported,
}

impl ErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "AH_LITELLM_CONFIG",
            Self::Credential => "AH_LITELLM_CREDENTIAL",
            Self::Endpoint => "AH_LITELLM_ENDPOINT",
            Self::Destination => "AH_LITELLM_DESTINATION",
            Self::Readiness => "AH_LITELLM_READINESS",
            Self::Protocol => "AH_LITELLM_PROTOCOL",
            Self::Capability => "AH_LITELLM_CAPABILITY",
            Self::Argument => "AH_LITELLM_ARGUMENT",
            Self::ExecutableChanged => "AH_LITELLM_EXECUTABLE_CHANGED",
            Self::Unsupported => "AH_LITELLM_UNSUPPORTED",
        }
    }
}

#[derive(Debug)]
struct GatewayError {
    code: ErrorCode,
    message: &'static str,
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for GatewayError {}

fn failure(code: ErrorCode, message: &'static str) -> anyhow::Error {
    anyhow!(GatewayError { code, message })
}

struct Secret(String);

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug)]
struct Endpoint {
    root: Url,
    root_text: String,
    canonical_host: String,
    port: u16,
    literal_ip: Option<IpAddr>,
}

pub struct Route {
    endpoint: Endpoint,
    credential: Secret,
    adapter: Adapter,
}

#[derive(Debug)]
enum Adapter {
    Anthropic,
    Copilot { model: String },
}

pub struct PreparedRoute {
    route: Route,
    identity: ExecutableIdentity,
}

pub struct ResolvedRoute {
    route: Route,
    pinned_address: SocketAddr,
    readiness_budget: Duration,
}

/// Resolve a routed target without running it. Ordinary launcher resolution
/// intentionally performs version probes and may install tools; neither is
/// safe while gateway credentials are present in the parent environment.
pub fn resolve_executable(tool: &str) -> Result<BinaryInfo> {
    let upper = tool.to_ascii_uppercase();
    for key in [
        format!("AMPLIHACK_{upper}_BINARY_PATH"),
        format!("{upper}_BINARY_PATH"),
    ] {
        if let Some(value) = std::env::var_os(&key) {
            let path = PathBuf::from(value);
            return executable_info(tool, path);
        }
    }

    let candidates: &[&str] = match tool {
        "claude" => &["rustyclawd", "claude"],
        "copilot" => &["copilot"],
        _ => &[],
    };
    for candidate in candidates {
        for directory in amplihack_utils::launch_target::env_path_dirs() {
            let path = directory.join(candidate);
            if let Ok(info) = executable_info(tool, path) {
                return Ok(info);
            }
        }
    }
    Err(failure(
        ErrorCode::Capability,
        "target executable is unavailable",
    ))
}

fn executable_info(tool: &str, path: PathBuf) -> Result<BinaryInfo> {
    if !path.is_absolute() {
        return Err(failure(
            ErrorCode::Capability,
            "target executable path is invalid",
        ));
    }
    let metadata = fs::metadata(&path)
        .map_err(|_| failure(ErrorCode::Capability, "target executable is unavailable"))?;
    if !metadata.is_file() {
        return Err(failure(
            ErrorCode::Capability,
            "target executable is unavailable",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(failure(
                ErrorCode::Capability,
                "target executable is unavailable",
            ));
        }
    }
    Ok(BinaryInfo {
        name: tool.to_string(),
        path,
        version: None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    schema_version: u32,
    endpoint: String,
    #[serde(default)]
    copilot: Option<CopilotConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CopilotConfig {
    model: String,
}

/// Resolve configuration and perform every deterministic pre-network check.
///
/// The ordering in this function is intentional and matches the documented
/// stable error precedence.
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    control: GatewayControl,
    tool: &str,
    docker: bool,
    auto: bool,
    append: bool,
    resume: bool,
    continue_session: bool,
    args: &[String],
) -> Result<Option<Route>> {
    if control.enabled && control.disabled {
        return Err(failure(
            ErrorCode::Config,
            "conflicting activation controls",
        ));
    }
    if control.disabled {
        return Ok(None);
    }

    let file_path = config_path();
    let any_known_environment = KNOWN_CONTROLS
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let file_present = file_path
        .as_deref()
        .map(config_file_present)
        .transpose()
        .or_else(|error| {
            if control.enabled || any_known_environment {
                Err(error)
            } else {
                Ok(None)
            }
        })?
        .unwrap_or(false);
    if !control.enabled && !file_present && !any_known_environment {
        return Ok(None);
    }

    reject_empty_controls()?;
    reject_unknown_controls()?;
    let file_config = match file_path {
        Some(path) if file_present => Some(read_config_file(&path)?),
        _ => None,
    };

    let endpoint_text = env_text("AMPLIHACK_LITELLM_ENDPOINT")?
        .or_else(|| file_config.as_ref().map(|config| config.endpoint.clone()))
        .ok_or_else(|| failure(ErrorCode::Config, "missing gateway endpoint"))?;

    let inline_present = std::env::var_os("AMPLIHACK_LITELLM_API_KEY").is_some();
    let file_credential_present = std::env::var_os("AMPLIHACK_LITELLM_API_KEY_FILE").is_some();
    if !inline_present && !file_credential_present {
        return Err(failure(ErrorCode::Config, "missing gateway credential"));
    }

    let model = env_text("AMPLIHACK_LITELLM_COPILOT_MODEL")?.or_else(|| {
        file_config
            .as_ref()
            .and_then(|config| config.copilot.as_ref())
            .map(|copilot| copilot.model.clone())
    });
    if let Some(model) = model.as_deref() {
        validate_model(model)?;
    }
    if tool == "copilot" && model.is_none() {
        return Err(failure(ErrorCode::Config, "missing Copilot gateway model"));
    }

    let credential = load_credential(inline_present, file_credential_present)?;
    let endpoint = parse_endpoint(&endpoint_text)?;

    let adapter = match tool {
        "claude" | "rusty" | "rustyclawd" => Adapter::Anthropic,
        "copilot" => Adapter::Copilot {
            model: model.expect("Copilot completeness checked above"),
        },
        _ => {
            return Err(failure(
                ErrorCode::Unsupported,
                "selected target does not support external LiteLLM routing",
            ));
        }
    };

    if docker || auto || append {
        return Err(failure(
            ErrorCode::Unsupported,
            "selected launch mode does not support external LiteLLM routing",
        ));
    }

    validate_arguments(&adapter, resume, continue_session, args, false)?;
    Ok(Some(Route {
        endpoint,
        credential,
        adapter,
    }))
}

impl Route {
    /// Resolve DNS once and validate the complete answer set before any
    /// executable is selected or probed.
    pub fn resolve_destination(self) -> Result<ResolvedRoute> {
        let started = Instant::now();
        let addresses = resolve_destination(&self.endpoint)?;
        let readiness_budget = TOTAL_TIMEOUT.saturating_sub(started.elapsed());
        Ok(ResolvedRoute {
            route: self,
            pinned_address: addresses[0],
            readiness_budget,
        })
    }
}

impl ResolvedRoute {
    /// Prove local executable capability and perform one pinned readiness
    /// request within the absolute readiness deadline.
    pub fn prepare(self, binary: &BinaryInfo) -> Result<PreparedRoute> {
        let identity = ExecutableIdentity::capture(&binary.path)
            .map_err(|_| failure(ErrorCode::Capability, "target executable is unavailable"))?;
        validate_capability(binary, &self.route.adapter)?;
        let readiness_deadline = Instant::now() + self.readiness_budget;
        check_readiness(
            &self.route.endpoint,
            self.pinned_address,
            readiness_deadline,
        )?;
        Ok(PreparedRoute {
            route: self.route,
            identity,
        })
    }
}

impl PreparedRoute {
    /// Apply removals first and the adapter additions last.
    pub fn apply_environment(&self, mut builder: EnvBuilder) -> EnvBuilder {
        let mut names = sensitive_environment_names();
        names.sort();
        names.dedup();
        for name in names {
            builder = builder.unset(name);
        }

        match &self.route.adapter {
            Adapter::Anthropic => builder
                .set("ANTHROPIC_BASE_URL", &self.route.endpoint.root_text)
                .set("ANTHROPIC_AUTH_TOKEN", self.route.credential.expose()),
            Adapter::Copilot { model } => builder
                .set("COPILOT_PROVIDER_TYPE", "openai")
                .set(
                    "COPILOT_PROVIDER_BASE_URL",
                    derived_url(&self.route.endpoint.root, &["v1"]),
                )
                .set("COPILOT_PROVIDER_API_KEY", self.route.credential.expose())
                .set("COPILOT_MODEL", model)
                .set("COPILOT_OFFLINE", "true"),
        }
    }

    pub fn revalidate_executable(&self) -> Result<()> {
        self.identity.revalidate().map_err(|_| {
            failure(
                ErrorCode::ExecutableChanged,
                "target executable changed after validation",
            )
        })
    }

    pub fn validate_final_command(&self, command: &Command) -> Result<()> {
        let arguments = command
            .get_args()
            .map(|argument| {
                argument.to_str().map(str::to_owned).ok_or_else(|| {
                    failure(
                        ErrorCode::Argument,
                        "agent arguments contain unsupported encoding",
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        validate_arguments(&self.route.adapter, false, false, &arguments, true)
    }
}

fn env_text(name: &str) -> Result<Option<String>> {
    std::env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| failure(ErrorCode::Config, "gateway configuration is not UTF-8"))
        })
        .transpose()
}

fn reject_empty_controls() -> Result<()> {
    for name in KNOWN_CONTROLS {
        if std::env::var_os(name).is_some()
            && std::env::var(name).is_ok_and(|value| value.is_empty())
        {
            return Err(failure(
                ErrorCode::Config,
                "gateway configuration contains an empty value",
            ));
        }
    }
    Ok(())
}

fn reject_unknown_controls() -> Result<()> {
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if name.starts_with("AMPLIHACK_LITELLM_")
            && !KNOWN_CONTROLS.iter().any(|known| *known == name)
        {
            return Err(failure(
                ErrorCode::Config,
                "gateway configuration contains an unknown control",
            ));
        }
    }
    Ok(())
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".amplihack")
            .join("litellm-config.toml")
    })
}

fn config_file_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(failure(
            ErrorCode::Config,
            "gateway config file state cannot be established",
        )),
    }
}

fn read_config_file(path: &Path) -> Result<FileConfig> {
    let parent = path
        .parent()
        .ok_or_else(|| failure(ErrorCode::Config, "gateway config path is invalid"))?;
    validate_private_directory(parent)
        .map_err(|_| failure(ErrorCode::Config, "gateway config directory is insecure"))?;
    let mut file = open_private_file(path)
        .map_err(|_| failure(ErrorCode::Config, "gateway config file is insecure"))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure(ErrorCode::Config, "gateway config file cannot be read"))?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err(failure(
            ErrorCode::Config,
            "gateway config file is too large",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| failure(ErrorCode::Config, "gateway config file is not UTF-8"))?;
    let config: FileConfig = toml::from_str(text)
        .map_err(|_| failure(ErrorCode::Config, "gateway config file is malformed"))?;
    if config.schema_version != 1 {
        return Err(failure(
            ErrorCode::Config,
            "gateway config schema is unsupported",
        ));
    }
    Ok(config)
}

#[cfg(target_os = "linux")]
fn validate_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || has_posix_access_acl(&directory)?
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "insecure directory",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_private_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private ownership cannot be established",
    ))
}

#[cfg(target_os = "linux")]
fn open_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || has_posix_access_acl(&file)?
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "insecure file",
        ));
    }
    Ok(file)
}

#[cfg(not(target_os = "linux"))]
fn open_private_file(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private ownership cannot be established",
    ))
}

#[cfg(target_os = "linux")]
fn has_posix_access_acl(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    // A POSIX access ACL can grant permissions beyond the mode-bit proof. We
    // do not attempt to interpret it: any ACL is rejected. ENODATA proves that
    // no such ACL exists; lack of xattr support cannot prove privacy and is an
    // error by design.
    let name = b"system.posix_acl_access\0";
    let result = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
        )
    };
    if result >= 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENODATA) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn load_credential(inline_present: bool, file_present: bool) -> Result<Secret> {
    if inline_present && file_present {
        return Err(failure(
            ErrorCode::Credential,
            "multiple gateway credential sources are configured",
        ));
    }
    let bytes = if inline_present {
        std::env::var_os("AMPLIHACK_LITELLM_API_KEY")
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| {
                failure(
                    ErrorCode::Credential,
                    "gateway credential has invalid encoding",
                )
            })?
            .into_bytes()
    } else {
        let path_text = std::env::var_os("AMPLIHACK_LITELLM_API_KEY_FILE")
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| {
                failure(
                    ErrorCode::Credential,
                    "gateway credential path has invalid encoding",
                )
            })?;
        let path = PathBuf::from(path_text);
        if !path.is_absolute() {
            return Err(failure(
                ErrorCode::Credential,
                "gateway credential path must be absolute",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| failure(ErrorCode::Credential, "gateway credential path is invalid"))?;
        validate_private_directory(parent).map_err(|_| {
            failure(
                ErrorCode::Credential,
                "gateway credential directory is insecure",
            )
        })?;
        let mut file = open_private_file(&path)
            .map_err(|_| failure(ErrorCode::Credential, "gateway credential file is insecure"))?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take((CREDENTIAL_LIMIT + 3) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                failure(
                    ErrorCode::Credential,
                    "gateway credential file cannot be read",
                )
            })?;
        bytes
    };

    let mut value = String::from_utf8(bytes).map_err(|_| {
        failure(
            ErrorCode::Credential,
            "gateway credential has invalid encoding",
        )
    })?;
    if value.ends_with("\r\n") {
        value.truncate(value.len() - 2);
    } else if value.ends_with('\n') {
        value.pop();
    }
    if value.is_empty() || value.len() > CREDENTIAL_LIMIT || value.chars().any(char::is_control) {
        return Err(failure(
            ErrorCode::Credential,
            "gateway credential is invalid",
        ));
    }
    Ok(Secret(value))
}

fn validate_model(model: &str) -> Result<()> {
    if model.is_empty() || model.chars().any(char::is_control) {
        return Err(failure(
            ErrorCode::Config,
            "Copilot gateway model is invalid",
        ));
    }
    Ok(())
}

fn parse_endpoint(input: &str) -> Result<Endpoint> {
    if input.is_empty() || input.chars().any(char::is_control) || input.trim() != input {
        return Err(failure(ErrorCode::Endpoint, "gateway endpoint is invalid"));
    }
    let (scheme, remainder) = input
        .split_once("://")
        .ok_or_else(|| failure(ErrorCode::Endpoint, "gateway endpoint is invalid"))?;
    if !matches!(scheme, "http" | "https") {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway endpoint scheme is unsupported",
        ));
    }
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let raw_path = &remainder[authority_end..];
    if authority.is_empty()
        || authority.contains('@')
        || raw_path.contains('?')
        || raw_path.contains('#')
        || raw_path.contains('\\')
    {
        return Err(failure(ErrorCode::Endpoint, "gateway endpoint is invalid"));
    }

    let (raw_host, explicit_port, bracketed) = split_authority(authority)?;
    if raw_host.ends_with('.') || raw_host.is_empty() {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway hostname is not canonical",
        ));
    }
    let literal_ip = raw_host.parse::<IpAddr>().ok();
    if literal_ip.is_none()
        && (raw_host
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, '.' | 'x' | 'X'))
            || raw_host
                .split('.')
                .next_back()
                .is_some_and(|label| label.chars().all(|character| character.is_ascii_digit())))
    {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway numeric hostname is not canonical",
        ));
    }
    let canonical_host = match literal_ip {
        Some(ip) => ip.to_string(),
        None => idna::domain_to_ascii_strict(raw_host)
            .map_err(|_| failure(ErrorCode::Endpoint, "gateway hostname is invalid"))?
            .to_ascii_lowercase(),
    };
    if canonical_host.is_empty() || canonical_host.split('.').any(str::is_empty) {
        return Err(failure(ErrorCode::Endpoint, "gateway hostname is invalid"));
    }
    if bracketed != matches!(literal_ip, Some(IpAddr::V6(_))) {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway authority is ambiguous",
        ));
    }
    let port = explicit_port.unwrap_or(if scheme == "https" { 443 } else { 80 });
    if port == 0 {
        return Err(failure(ErrorCode::Endpoint, "gateway port is invalid"));
    }
    if scheme == "http" && !literal_ip.is_some_and(|ip| ip.is_loopback()) {
        return Err(failure(
            ErrorCode::Endpoint,
            "plain HTTP requires a literal loopback address",
        ));
    }

    validate_endpoint_path(raw_path)?;
    let authority = match literal_ip {
        Some(IpAddr::V6(_)) => format!("[{canonical_host}]"),
        _ => canonical_host.clone(),
    };
    let authority = if explicit_port.is_some() {
        format!("{authority}:{port}")
    } else {
        authority
    };
    let path = if raw_path.is_empty() { "/" } else { raw_path };
    let mut root = Url::parse(&format!("{scheme}://{authority}{path}"))
        .map_err(|_| failure(ErrorCode::Endpoint, "gateway endpoint is invalid"))?;
    if root.host().is_none_or(|host| match (literal_ip, host) {
        (Some(IpAddr::V4(expected)), url::Host::Ipv4(actual)) => expected != actual,
        (Some(IpAddr::V6(expected)), url::Host::Ipv6(actual)) => expected != actual,
        (None, url::Host::Domain(actual)) => actual != canonical_host,
        _ => true,
    }) {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway hostname normalization is inconsistent",
        ));
    }
    if root.path() != "/" {
        let trimmed = root.path().trim_end_matches('/').to_string();
        root.set_path(&trimmed);
    }
    Ok(Endpoint {
        root_text: root.as_str().trim_end_matches('/').to_string(),
        root,
        canonical_host,
        port,
        literal_ip,
    })
}

fn split_authority(authority: &str) -> Result<(&str, Option<u16>, bool)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| failure(ErrorCode::Endpoint, "gateway authority is invalid"))?;
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            let value = suffix
                .strip_prefix(':')
                .ok_or_else(|| failure(ErrorCode::Endpoint, "gateway authority is invalid"))?;
            Some(
                value
                    .parse()
                    .map_err(|_| failure(ErrorCode::Endpoint, "gateway port is invalid"))?,
            )
        };
        return Ok((host, port, true));
    }
    if authority.matches(':').count() > 1 {
        return Err(failure(
            ErrorCode::Endpoint,
            "IPv6 gateway addresses must be bracketed",
        ));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse()
            .map_err(|_| failure(ErrorCode::Endpoint, "gateway port is invalid"))?;
        Ok((host, Some(port), false))
    } else {
        Ok((authority, None, false))
    }
}

fn validate_endpoint_path(path: &str) -> Result<()> {
    if path.is_empty() || path == "/" {
        return Ok(());
    }
    if !path.starts_with('/') || path.ends_with('/') || path.contains("//") || path.contains('%') {
        return Err(failure(
            ErrorCode::Endpoint,
            "gateway endpoint path is ambiguous",
        ));
    }
    let governed = [
        "health",
        "readiness",
        "v1",
        "completions",
        "completion",
        "chat",
    ];
    for segment in path.trim_start_matches('/').split('/') {
        let lower = segment.to_ascii_lowercase();
        if segment.is_empty() || matches!(segment, "." | "..") || governed.contains(&lower.as_str())
        {
            return Err(failure(
                ErrorCode::Endpoint,
                "gateway endpoint is not a deployment root",
            ));
        }
    }
    Ok(())
}

fn derived_url(root: &Url, segments: &[&str]) -> String {
    let mut url = root.clone();
    {
        let mut path = url
            .path_segments_mut()
            .expect("validated HTTP URLs are path bases");
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    url.to_string()
}

fn validate_arguments(
    adapter: &Adapter,
    resume: bool,
    continue_session: bool,
    args: &[String],
    final_command: bool,
) -> Result<()> {
    if resume || continue_session {
        return Err(failure(
            ErrorCode::Argument,
            "session resume and continuation are unavailable for routed launches",
        ));
    }
    let blocked = [
        "--api-key",
        "--auth-token",
        "--base-url",
        "--endpoint",
        "--provider",
        "--remote",
        "--remote-export",
        "--remote-control",
        "--cloud",
        "--environment",
        "--teleport",
        "--connect",
        "--share",
        "--share-gist",
        "--export",
        "--from-pr",
        "--fork-session",
        "--resume",
        "--resume-session",
        "--continue",
        "--continue-session",
        "--session-id",
        "--config",
        "--settings",
        "--env",
        "--env-file",
        "--exec",
        "--command",
        "--litellm",
        "--no-litellm",
        "-r",
        "-c",
    ];
    let mut model_values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let name = argument
            .split_once('=')
            .map_or(argument.as_str(), |(name, _)| name);
        if blocked.contains(&name) || (!final_command && name == "--setting-sources") {
            return Err(failure(
                ErrorCode::Argument,
                "agent arguments conflict with external routing",
            ));
        }
        if name == "--setting-sources" {
            let value = match argument.split_once('=') {
                Some((_, value)) => value,
                None => {
                    index += 1;
                    args.get(index).map(String::as_str).ok_or_else(|| {
                        failure(
                            ErrorCode::Argument,
                            "routed setting sources argument is missing",
                        )
                    })?
                }
            };
            if !value.is_empty() {
                return Err(failure(
                    ErrorCode::Argument,
                    "routed setting sources must remain disabled",
                ));
            }
        }
        if name == "--model" {
            let value = match argument.split_once('=') {
                Some((_, value)) if !value.is_empty() => value,
                Some(_) => {
                    return Err(failure(
                        ErrorCode::Argument,
                        "routed model argument is invalid",
                    ));
                }
                None => {
                    index += 1;
                    args.get(index).map(String::as_str).ok_or_else(|| {
                        failure(ErrorCode::Argument, "routed model argument is missing")
                    })?
                }
            };
            model_values.push(value);
        }
        index += 1;
    }

    if let Adapter::Copilot { model } = adapter {
        if model_values.len() > 1
            || model_values
                .first()
                .is_some_and(|argument| *argument != model)
        {
            return Err(failure(
                ErrorCode::Argument,
                "Copilot model arguments conflict with the configured route",
            ));
        }
        if args.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "login"
                    | "logout"
                    | "auth"
                    | "config"
                    | "update"
                    | "version"
                    | "help"
                    | "plugin"
                    | "plugins"
                    | "mcp"
            )
        }) {
            return Err(failure(
                ErrorCode::Argument,
                "Copilot nested commands are unavailable for routed launches",
            ));
        }
    }
    Ok(())
}

fn resolve_destination(endpoint: &Endpoint) -> Result<Vec<SocketAddr>> {
    let mut addresses = if let Some(ip) = endpoint.literal_ip {
        vec![SocketAddr::new(ip, endpoint.port)]
    } else {
        let host = endpoint.canonical_host.clone();
        let port = endpoint.port;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>());
            let _ = sender.send(result);
        });
        receiver
            .recv_timeout(DNS_TIMEOUT)
            .map_err(|_| failure(ErrorCode::Destination, "gateway DNS resolution failed"))?
            .map_err(|_| failure(ErrorCode::Destination, "gateway DNS resolution failed"))?
    };
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() || addresses.iter().any(|address| prohibited_ip(address.ip())) {
        return Err(failure(
            ErrorCode::Destination,
            "gateway destination is prohibited",
        ));
    }
    Ok(addresses)
}

fn prohibited_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => prohibited_ipv4(ip),
        IpAddr::V6(ip) => {
            if ip.is_unspecified()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ipv6_in_prefix(ip, "100::".parse().expect("fixed IPv6"), 64)
                || ipv6_in_prefix(ip, "2001::".parse().expect("fixed IPv6"), 23)
                || ipv6_in_prefix(ip, "2001:db8::".parse().expect("fixed IPv6"), 32)
                || ip == "fd00:ec2::254".parse::<Ipv6Addr>().expect("fixed IPv6")
            {
                return true;
            }
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return prohibited_ipv4(mapped);
            }
            ip.segments()[0..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback()
        }
    }
}

fn prohibited_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 0
        || ip.is_link_local()
        || ip.is_multicast()
        || octets[0] >= 240
        || octets[0] == 192 && octets[1] == 0 && octets[2] == 0
        || octets[0] == 192 && octets[1] == 0 && octets[2] == 2
        || octets[0] == 198 && matches!(octets[1], 18 | 19)
        || octets[0] == 198 && octets[1] == 51 && octets[2] == 100
        || octets[0] == 203 && octets[1] == 0 && octets[2] == 113
        || ip == Ipv4Addr::BROADCAST
        || ip == Ipv4Addr::new(100, 100, 100, 200)
}

fn ipv6_in_prefix(address: Ipv6Addr, prefix: Ipv6Addr, bits: u32) -> bool {
    let shift = 128 - bits;
    (u128::from(address) >> shift) == (u128::from(prefix) >> shift)
}

fn check_readiness(endpoint: &Endpoint, pinned: SocketAddr, deadline: Instant) -> Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness operation timed out",
        ));
    }
    let header_budget = remaining.min(HEADER_TIMEOUT);
    let resolver = move |_authority: &str| Ok(vec![pinned]);
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .max_idle_connections(0)
        .max_idle_connections_per_host(0)
        .timeout_connect(CONNECT_TIMEOUT.min(header_budget))
        .timeout_read(header_budget)
        .timeout_write(header_budget)
        .timeout(header_budget)
        .resolver(resolver)
        .build();
    let response = agent
        .get(&derived_url(&endpoint.root, &["health", "readiness"]))
        .set("Accept", "application/json")
        .set("Accept-Encoding", "identity")
        .call()
        .map_err(|_| failure(ErrorCode::Readiness, "gateway readiness request failed"))?;
    if !(200..300).contains(&response.status()) {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness status is unsuccessful",
        ));
    }
    let content_types = response.all("content-type");
    if content_types.len() != 1 || !is_json_media_type(content_types[0]) {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness response is not JSON",
        ));
    }
    if response
        .header("content-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness response uses unsupported encoding",
        ));
    }
    if let Some(value) = response.header("content-length") {
        let length = value.parse::<u64>().map_err(|_| {
            failure(
                ErrorCode::Readiness,
                "gateway readiness content length is invalid",
            )
        })?;
        if length > RESPONSE_LIMIT {
            return Err(failure(
                ErrorCode::Readiness,
                "gateway readiness response is too large",
            ));
        }
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut body = Vec::new();
        let result = response
            .into_reader()
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut body)
            .map(|_| body);
        let _ = sender.send(result);
    });
    let body_budget = BODY_TIMEOUT.min(deadline.saturating_duration_since(Instant::now()));
    if body_budget.is_zero() {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness operation timed out",
        ));
    }
    let body = receiver
        .recv_timeout(body_budget)
        .map_err(|_| {
            failure(
                ErrorCode::Readiness,
                "gateway readiness body read timed out",
            )
        })?
        .map_err(|_| failure(ErrorCode::Readiness, "gateway readiness body read failed"))?;
    if body.len() as u64 > RESPONSE_LIMIT {
        return Err(failure(
            ErrorCode::Readiness,
            "gateway readiness response is too large",
        ));
    }
    validate_readiness_json(&body)
}

fn is_json_media_type(value: &str) -> bool {
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence == "application/json"
        || essence
            .strip_prefix("application/")
            .is_some_and(|subtype| subtype.ends_with("+json"))
}

fn validate_readiness_json(body: &[u8]) -> Result<()> {
    validate_json_depth(body)?;
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let readiness = deserializer
        .deserialize_map(ReadinessVisitor)
        .map_err(|_| failure(ErrorCode::Protocol, "gateway readiness JSON is invalid"))?;
    deserializer
        .end()
        .map_err(|_| failure(ErrorCode::Protocol, "gateway readiness JSON is invalid"))?;
    if readiness.status.as_deref() != Some("healthy")
        || !matches!(
            readiness.db.as_deref(),
            None | Some("connected") | Some("Not connected")
        )
    {
        return Err(failure(
            ErrorCode::Protocol,
            "gateway readiness capability is unacceptable",
        ));
    }
    Ok(())
}

fn validate_json_depth(body: &[u8]) -> Result<()> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in body {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > JSON_DEPTH_LIMIT {
                    return Err(failure(
                        ErrorCode::Protocol,
                        "gateway readiness JSON is too deeply nested",
                    ));
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Readiness {
    status: Option<String>,
    db: Option<String>,
}

struct ReadinessVisitor;

impl<'de> Visitor<'de> for ReadinessVisitor {
    type Value = Readiness;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a readiness object")
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut status = None;
        let mut db = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "status" => {
                    if status.is_some() {
                        return Err(serde::de::Error::duplicate_field("status"));
                    }
                    status = Some(map.next_value::<String>()?);
                }
                "db" => {
                    if db.is_some() {
                        return Err(serde::de::Error::duplicate_field("db"));
                    }
                    db = Some(map.next_value::<String>()?);
                }
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                    return Err(serde::de::Error::unknown_field(&key, &["status", "db"]));
                }
            }
        }
        Ok(Readiness { status, db })
    }
}

fn validate_capability(binary: &BinaryInfo, adapter: &Adapter) -> Result<()> {
    let version = run_probe(&binary.path, "--version")?;
    if !version.status.success() || (version.stdout.is_empty() && version.stderr.is_empty()) {
        return Err(failure(
            ErrorCode::Capability,
            "target executable did not prove a usable version",
        ));
    }
    if matches!(adapter, Adapter::Anthropic) {
        let help = run_probe(&binary.path, "--help")?;
        if !help.status.success() {
            return Err(failure(
                ErrorCode::Capability,
                "target executable cannot isolate gateway settings",
            ));
        }
        let mut bytes = help.stdout;
        bytes.extend_from_slice(&help.stderr);
        if !String::from_utf8_lossy(&bytes).contains("--setting-sources") {
            return Err(failure(
                ErrorCode::Capability,
                "target executable cannot isolate gateway settings",
            ));
        }
    } else {
        let help = run_probe(&binary.path, "--help")?;
        if !help.status.success() {
            return Err(failure(
                ErrorCode::Capability,
                "Copilot executable did not prove external provider support",
            ));
        }
        let mut bytes = help.stdout;
        bytes.extend_from_slice(&help.stderr);
        let output = String::from_utf8_lossy(&bytes);
        for capability in [
            "COPILOT_PROVIDER_TYPE",
            "COPILOT_PROVIDER_BASE_URL",
            "COPILOT_PROVIDER_API_KEY",
            "COPILOT_MODEL",
            "COPILOT_OFFLINE",
        ] {
            if !output.contains(capability) {
                return Err(failure(
                    ErrorCode::Capability,
                    "Copilot executable did not prove external provider support",
                ));
            }
        }
    }
    Ok(())
}

fn run_probe(path: &Path, argument: &str) -> Result<Output> {
    let mut command = Command::new(path);
    command.arg(argument).env_clear().stdin(Stdio::null());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    crate::util::run_output_with_timeout_limited(command, PROBE_TIMEOUT, PROBE_LIMIT)
        .map_err(|_| failure(ErrorCode::Capability, "target executable probe failed"))
}

#[derive(Debug)]
struct ExecutableIdentity {
    launch_path: PathBuf,
    canonical_path: PathBuf,
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl ExecutableIdentity {
    fn capture(path: &Path) -> io::Result<Self> {
        let launch_path = path.to_path_buf();
        let canonical_path = path.canonicalize()?;
        let metadata = fs::metadata(&canonical_path)?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a regular executable",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "not executable",
                ));
            }
            Ok(Self {
                launch_path,
                canonical_path,
                len: metadata.len(),
                modified: metadata.modified()?,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                launch_path,
                canonical_path,
                len: metadata.len(),
                modified: metadata.modified()?,
            })
        }
    }

    fn revalidate(&self) -> io::Result<()> {
        let current = Self::capture(&self.launch_path)?;
        if current.canonical_path != self.canonical_path
            || current.len != self.len
            || current.modified != self.modified
        {
            return Err(io::Error::other("executable identity changed"));
        }
        #[cfg(unix)]
        if current.device != self.device || current.inode != self.inode {
            return Err(io::Error::other("executable identity changed"));
        }
        Ok(())
    }
}

fn sensitive_environment_names() -> Vec<String> {
    let exact: HashSet<&'static str> = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "COPILOT_GITHUB_TOKEN",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_ORG_ID",
        "OPENAI_ORGANIZATION",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_ENDPOINT",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "AWS_WEB_IDENTITY_TOKEN_FILE",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "AZURE_CLIENT_ID",
        "AZURE_CLIENT_SECRET",
        "AZURE_TENANT_ID",
        "COPILOT_PROVIDER_TYPE",
        "COPILOT_PROVIDER_BASE_URL",
        "COPILOT_PROVIDER_API_KEY",
        "COPILOT_MODEL",
        "COPILOT_OFFLINE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "DYLD_FRAMEWORK_PATH",
        "NODE_OPTIONS",
        "NODE_PATH",
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONSTARTUP",
        "RUBYOPT",
        "PERL5OPT",
        "BASH_ENV",
        "ENV",
        "JAVA_TOOL_OPTIONS",
        "_JAVA_OPTIONS",
        "DOTNET_STARTUP_HOOKS",
        "DOTNET_ADDITIONAL_DEPS",
        "RUSTC_WRAPPER",
        "SSLKEYLOGFILE",
        "GIT_SSH_COMMAND",
    ]
    .into_iter()
    .collect();
    let prefixes = [
        "AMPLIHACK_LITELLM_",
        "ANTHROPIC_",
        "OPENAI_",
        "COPILOT_PROVIDER_",
        "CLAUDE_CODE_OAUTH_",
        "AWS_",
        "AZURE_",
        "GOOGLE_",
        "GCP_",
        "GCLOUD_",
        "GEMINI_",
        "MISTRAL_",
        "COHERE_",
        "GROQ_",
        "TOGETHER_",
        "BEDROCK_",
        "OCI_",
        "IBM_",
        "DIGITALOCEAN_",
        "CLOUDFLARE_",
        "HUGGINGFACE_",
        "HF_",
        "OPENROUTER_",
        "DEEPSEEK_",
        "XAI_",
        "PERPLEXITY_",
        "CEREBRAS_",
        "FIREWORKS_",
        "VOYAGE_",
        "AI21_",
        "REPLICATE_",
        "VERTEX_",
        "VERTEXAI_",
    ];
    let mut names = exact
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    names.extend(KNOWN_CONTROLS.iter().map(|name| (*name).to_string()));
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if exact.contains(name.as_ref()) || prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            names.push(name.into_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_formatting_is_constant_redaction() {
        let secret = Secret("do-not-print".to_string());
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }

    #[test]
    fn endpoint_policy_accepts_only_safe_roots() {
        for endpoint in [
            "https://gateway.example",
            "https://gateway.example/team-a",
            "http://127.0.0.1:4000",
            "http://[::1]:4000",
        ] {
            parse_endpoint(endpoint).unwrap_or_else(|error| panic!("{endpoint}: {error}"));
        }
        for endpoint in [
            "http://gateway.example",
            "http://localhost:4000",
            "ftp://127.0.0.1",
            "https://user@gateway.example",
            "https://gateway.example.",
            "https://gateway.example/v1",
            "https://gateway.example/%2e%2e/admin",
            "https://gateway.example/a//b",
        ] {
            let error = parse_endpoint(endpoint).expect_err(endpoint);
            assert!(error.to_string().starts_with("AH_LITELLM_ENDPOINT"));
            assert!(!error.to_string().contains(endpoint));
        }
    }

    #[test]
    fn destination_policy_rejects_non_gateway_address_classes() {
        for ip in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::BROADCAST),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V6("fe80::1".parse().unwrap()),
            IpAddr::V6("ff02::1".parse().unwrap()),
            IpAddr::V6("::ffff:0.1.2.3".parse().unwrap()),
            IpAddr::V6("::ffff:240.0.0.1".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            IpAddr::V6("100::1".parse().unwrap()),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
        ] {
            assert!(prohibited_ip(ip), "{ip} must be prohibited");
        }
        assert!(!prohibited_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!prohibited_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn readiness_protocol_is_exact_and_duplicate_aware() {
        for accepted in [
            br#"{"status":"healthy"}"#.as_slice(),
            br#"{"status":"healthy","db":"connected"}"#.as_slice(),
            br#"{"status":"healthy","db":"Not connected"}"#.as_slice(),
        ] {
            validate_readiness_json(accepted).expect("accepted readiness shape");
        }
        for rejected in [
            br#"{"status":"unhealthy"}"#.as_slice(),
            br#"{"status":"healthy","status":"healthy"}"#.as_slice(),
            br#"{"status":"healthy","db":"waiting"}"#.as_slice(),
            br#"{"status":"healthy","unexpected":true}"#.as_slice(),
            br#"{"status":"healthy"} trailing"#.as_slice(),
            br#"[]"#.as_slice(),
        ] {
            let error = validate_readiness_json(rejected).expect_err("invalid readiness shape");
            assert!(error.to_string().starts_with("AH_LITELLM_PROTOCOL"));
        }
    }

    #[test]
    fn readiness_nesting_is_bounded_even_in_unknown_values() {
        let body = format!(
            r#"{{"status":"healthy","extra":{}}}"#,
            "[".repeat(JSON_DEPTH_LIMIT + 1) + &"]".repeat(JSON_DEPTH_LIMIT + 1)
        );
        assert!(validate_readiness_json(body.as_bytes()).is_err());
    }

    #[test]
    fn json_media_type_matching_is_strict() {
        assert!(is_json_media_type("application/json"));
        assert!(is_json_media_type(
            "Application/Problem+Json; charset=utf-8"
        ));
        assert!(!is_json_media_type("text/json"));
        assert!(!is_json_media_type("application/jsonp"));
    }

    #[test]
    fn argument_policy_catches_equals_forms_duplicates_and_bypasses() {
        let adapter = Adapter::Copilot {
            model: "gateway-coding".to_string(),
        };
        validate_arguments(
            &adapter,
            false,
            false,
            &["--model=gateway-coding".to_string()],
            false,
        )
        .unwrap();
        for arguments in [
            vec!["--remote".to_string()],
            vec!["--remote-export".to_string()],
            vec!["--remote-control".to_string()],
            vec!["--environment=cloud-session".to_string()],
            vec!["--teleport".to_string()],
            vec!["--from-pr=123".to_string()],
            vec!["--fork-session".to_string()],
            vec!["--share-gist".to_string()],
            vec!["-r".to_string()],
            vec!["--provider=openai".to_string()],
            vec!["--model=other".to_string()],
            vec!["login".to_string()],
            vec![
                "--model".to_string(),
                "gateway-coding".to_string(),
                "--model=gateway-coding".to_string(),
            ],
        ] {
            let error = validate_arguments(&adapter, false, false, &arguments, false)
                .expect_err("unsafe args");
            assert!(error.to_string().starts_with("AH_LITELLM_ARGUMENT"));
        }
        for arguments in [
            vec!["--settings".to_string(), "route.json".to_string()],
            vec!["--setting-sources=user".to_string()],
        ] {
            let error = validate_arguments(&Adapter::Anthropic, false, false, &arguments, false)
                .expect_err("settings overrides must be rejected");
            assert!(error.to_string().starts_with("AH_LITELLM_ARGUMENT"));
        }
        validate_arguments(
            &Adapter::Anthropic,
            false,
            false,
            &["--setting-sources".to_string(), String::new()],
            true,
        )
        .expect("the internally generated settings isolation must validate");
    }

    #[test]
    fn derived_urls_append_segments_without_string_concatenation() {
        let endpoint = parse_endpoint("https://gateway.example/team-a").unwrap();
        assert_eq!(
            derived_url(&endpoint.root, &["health", "readiness"]),
            "https://gateway.example/team-a/health/readiness"
        );
        assert_eq!(
            derived_url(&endpoint.root, &["v1"]),
            "https://gateway.example/team-a/v1"
        );
    }

    #[test]
    fn sensitive_environment_inventory_covers_gateway_provider_and_injection_inputs() {
        let names = sensitive_environment_names();
        for required in [
            "AMPLIHACK_LITELLM_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "COPILOT_PROVIDER_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "HTTPS_PROXY",
            "LD_PRELOAD",
            "NODE_OPTIONS",
            "DOTNET_STARTUP_HOOKS",
        ] {
            assert!(
                names.iter().any(|name| name == required),
                "{required} must be removed from routed children"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn executable_identity_detects_symlink_retargeting() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let launch = directory.path().join("agent");
        for path in [&first, &second] {
            fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        symlink(&first, &launch).unwrap();
        let identity = ExecutableIdentity::capture(&launch).unwrap();
        fs::remove_file(&launch).unwrap();
        symlink(&second, &launch).unwrap();

        assert!(identity.revalidate().is_err());
    }
}
