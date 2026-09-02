use super::evidence;
use super::types::{ClientSummaryV1, GatewayTelemetryV1, RunStatus, RunSummaryV1};
use crate::{LiveClient, VerifyLiveArgs};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const RUSTYCLAWD_PACKAGE: &str = "rustyclawd-cli";
const RUSTYCLAWD_VERSION: &str = "0.1.1";
const RUSTYCLAWD_SOURCE: &str = "https://github.com/rysweet/RustyClawd";
const RUSTYCLAWD_REVISION: &str = "2825862711a4bd1367022c62ed6cd2efae9f4998";

pub(crate) struct VerifyFailure {
    pub exit: u8,
    id: &'static str,
    stage: &'static str,
    message: String,
    remediation: &'static str,
    pub summary: Option<Box<RunSummaryV1>>,
}

impl VerifyFailure {
    fn new(
        exit: u8,
        id: &'static str,
        stage: &'static str,
        message: impl Into<String>,
        remediation: &'static str,
    ) -> Self {
        Self {
            exit,
            id,
            stage,
            message: message.into(),
            remediation,
            summary: None,
        }
    }

    pub fn emit(&self) {
        eprintln!(
            "{} stage={}: {}\n{}",
            self.id, self.stage, self.message, self.remediation
        );
    }
}

struct ClientAttestation {
    id: &'static str,
    version: String,
    digest: String,
    path: PathBuf,
}

struct LiveConfig {
    endpoint: String,
    key: String,
    model: String,
    expected_provider: String,
    expected_model: String,
    telemetry_file: PathBuf,
    telemetry_hmac_key: String,
}

struct ClientRun {
    correlation_id: String,
    result_sha256: String,
    telemetry: GatewayTelemetryV1,
}

pub(crate) fn run(args: &VerifyLiveArgs) -> Result<RunSummaryV1, VerifyFailure> {
    if super::startup_guard::ci_environment_present() {
        return Err(VerifyFailure::new(
            78,
            "AH-LIVE-CI-001",
            "host-eligibility",
            "host-only LiteLLM verification cannot run in CI",
            "Run it from a trusted host against the exact PR head.",
        ));
    }

    let repo = attest_repository(args)?;
    let evidence_dir = validate_evidence_directory(args.evidence_dir.as_deref(), &repo)?;
    attest_pull_request(args, &repo)?;

    let selected = selected_clients(args)?;
    let mut clients = Vec::new();
    for client in selected {
        let attestation = match client {
            LiveClient::Claude => attest_client(
                "claude",
                "claude",
                amplihack_utils::litellm_proxy::VERIFIED_CLAUDE_CODE_VERSIONS[0],
            )?,
            LiveClient::Copilot => attest_client(
                "copilot",
                "copilot",
                amplihack_utils::litellm_proxy::VERIFIED_COPILOT_CLI_VERSIONS[0],
            )?,
            LiveClient::Rustyclawd => {
                let attestation = attest_client("rustyclawd", "rusty", RUSTYCLAWD_VERSION)?;
                attest_rustyclawd_receipt(&attestation.path)?;
                attestation
            }
            LiveClient::All => unreachable!("all is expanded by selected_clients"),
        };
        clients.push(attestation);
    }

    let (context_digest, context) = repository_context(args, &repo)?;
    let config = LiveConfig::from_env(&repo)?;
    let mut summaries = Vec::new();
    let mut negative_cases_passed = 0_u16;

    for client in &clients {
        let run = run_positive(
            client,
            &config,
            &repo,
            &context,
            &context_digest,
            args.timeout_seconds,
        )?;
        ensure_repository_unchanged(&repo, client.id, "positive-postcondition")?;
        negative_cases_passed +=
            run_negative_cases(client, &config, &repo, args.timeout_seconds.min(45))?;
        ensure_repository_unchanged(&repo, client.id, "negative-postcondition")?;
        summaries.push(client_summary(client, &config, run));
    }

    let mut summary = RunSummaryV1 {
        schema: "amplihack.litellm.run-summary",
        schema_version: 1,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        run_id: Uuid::new_v4().to_string(),
        execution_context: "trusted-host",
        repository_commit: args.expected_head.clone(),
        repository_context_sha256: context_digest,
        pr_number: args.pr,
        status: RunStatus::Passed,
        exit_code: 0,
        clients: summaries,
        negative_cases_passed,
        negative_cases_failed: 0,
        evidence_path: None,
        evidence_sha256: None,
        credentials_read: true,
    };
    let committed = evidence::commit(&evidence_dir, &summary).map_err(|message| {
        VerifyFailure::new(
            70,
            "AH-LIVE-EVIDENCE-001",
            "evidence-commit",
            message,
            "Use an owner-only external evidence directory and rerun the complete verification.",
        )
    })?;
    summary.evidence_path = Some(committed.path.display().to_string());
    summary.evidence_sha256 = Some(committed.sha256);
    Ok(summary)
}

impl LiveConfig {
    fn from_env(repo: &Path) -> Result<Self, VerifyFailure> {
        amplihack_utils::litellm_proxy::validate_environment()
            .map_err(|error| config_failure("configuration", error.to_string()))?;
        let endpoint = required_live_env(amplihack_utils::litellm_proxy::ENDPOINT_ENV)?;
        let key = required_live_env(amplihack_utils::litellm_proxy::API_KEY_ENV)?;
        let model = required_live_env(amplihack_utils::litellm_proxy::MODEL_ENV)?;
        let expected_provider = required_live_env("AMPLIHACK_LITELLM_EXPECTED_PROVIDER")?;
        let expected_model = required_live_env("AMPLIHACK_LITELLM_EXPECTED_MODEL")?;
        let telemetry_file = PathBuf::from(required_live_env("AMPLIHACK_LITELLM_TELEMETRY_FILE")?);
        if !telemetry_file.is_absolute() {
            return Err(config_failure(
                "configuration",
                "AMPLIHACK_LITELLM_TELEMETRY_FILE must be absolute",
            ));
        }
        let existing_parent = telemetry_file.parent().ok_or_else(|| {
            config_failure(
                "configuration",
                "telemetry file must have an existing external parent",
            )
        })?;
        let parent = fs::canonicalize(existing_parent).map_err(|error| {
            config_failure(
                "configuration",
                format!("cannot resolve telemetry directory: {}", error.kind()),
            )
        })?;
        if parent.starts_with(repo) {
            return Err(config_failure(
                "configuration",
                "telemetry file must be outside the repository",
            ));
        }
        let telemetry_metadata = fs::symlink_metadata(&telemetry_file).map_err(|error| {
            config_failure(
                "configuration",
                format!("cannot inspect telemetry file: {}", error.kind()),
            )
        })?;
        if !telemetry_metadata.file_type().is_file() || telemetry_metadata.file_type().is_symlink()
        {
            return Err(config_failure(
                "configuration",
                "telemetry path must be an existing regular non-symlink file",
            ));
        }
        let telemetry_hmac_key = required_live_env("AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY")?;
        if telemetry_hmac_key.len() < 32 {
            return Err(config_failure(
                "configuration",
                "AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY must contain at least 32 bytes",
            ));
        }
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            key,
            model,
            expected_provider,
            expected_model,
            telemetry_file,
            telemetry_hmac_key,
        })
    }
}

fn required_live_env(name: &str) -> Result<String, VerifyFailure> {
    let value = std::env::var(name)
        .map_err(|_| config_failure("configuration", format!("{name} is required")))?;
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(config_failure(
            "configuration",
            format!("{name} must be non-empty without whitespace or control characters"),
        ));
    }
    Ok(value)
}

fn selected_clients(args: &VerifyLiveArgs) -> Result<Vec<LiveClient>, VerifyFailure> {
    let requested = if args.client.is_empty() {
        vec![LiveClient::All]
    } else {
        args.client.clone()
    };
    if requested.contains(&LiveClient::All) && requested.len() != 1 {
        return Err(config_failure(
            "arguments",
            "--client all cannot be combined with another --client",
        ));
    }
    if requested == [LiveClient::All] {
        return Ok(vec![
            LiveClient::Claude,
            LiveClient::Copilot,
            LiveClient::Rustyclawd,
        ]);
    }
    Ok(requested
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn repository_context(
    args: &VerifyLiveArgs,
    repo: &Path,
) -> Result<(String, String), VerifyFailure> {
    let mut entries = args
        .context_paths
        .iter()
        .map(|path| {
            fs::read(repo.join(path))
                .map(|bytes| (path.to_string_lossy().into_owned(), bytes))
                .map_err(|error| {
                    config_failure(
                        "repository-context",
                        format!("cannot read context file: {}", error.kind()),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    let mut prompt_context = String::new();
    for (path, bytes) in entries {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
        hasher.update([0]);
        let text = String::from_utf8(bytes).map_err(|_| {
            config_failure(
                "repository-context",
                "context files must contain valid UTF-8",
            )
        })?;
        prompt_context.push_str(&format!("\n--- {path} ---\n{text}\n"));
    }
    if prompt_context.len() > 512 * 1024 {
        return Err(config_failure(
            "repository-context",
            "combined repository context exceeds 512 KiB",
        ));
    }
    Ok((evidence::hex(&hasher.finalize()), prompt_context))
}

fn run_positive(
    client: &ClientAttestation,
    config: &LiveConfig,
    repo: &Path,
    context: &str,
    context_digest: &str,
    timeout_seconds: u64,
) -> Result<ClientRun, VerifyFailure> {
    let correlation_id = Uuid::new_v4().to_string();
    let nonce = Uuid::new_v4().to_string();
    let nonce_digest = evidence::hex(&Sha256::digest(nonce.as_bytes()));
    let prompt = format!(
        "Perform a substantive read-only cross-file analysis of the supplied repository context. \
         Do not use tools and do not modify files. Return exactly one ASCII line beginning \
         AMPLIHACK_RESULT_V1|{correlation_id}|{context_digest}|{nonce_digest}|left_path= and finish \
         this exact pipe-delimited schema: left_path, left_symbol, right_path, right_symbol, \
         relationship, risk. Select two identifier symbols that actually occur in two different \
         supplied files. The relationship must name both symbols and explain their concrete \
         cross-file interaction. The risk must describe a bounded correctness failure. Do not use \
         a pipe inside a value. Repository context follows:{context}"
    );
    let telemetry_offset = fs::metadata(&config.telemetry_file)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let output = execute_client(
        client,
        config,
        repo,
        &prompt,
        timeout_seconds,
        "positive-live",
    )?;
    if output.contains(&config.key) {
        return Err(client_failure(
            client.id,
            70,
            "credential-leakage",
            "client output contained the gateway credential",
            "Ensure the client credential is marked secret and never included in diagnostics.",
        ));
    }
    let result = extract_result_line(&output).ok_or_else(|| {
        client_failure(
            client.id,
            70,
            "result-validation",
            "client did not return the required fresh structured repository analysis",
            "Check the requested model's instruction following and rerun with a fresh request.",
        )
    })?;
    validate_substantive_result(
        result,
        &correlation_id,
        context_digest,
        &nonce_digest,
        context,
    )
    .map_err(|message| {
        client_failure(
            client.id,
            70,
            "result-validation",
            message,
            "Return the exact schema with symbols and paths present in two supplied files.",
        )
    })?;
    let result_sha256 = evidence::hex(&Sha256::digest(result.as_bytes()));
    let telemetry = read_telemetry(
        config,
        telemetry_offset,
        &correlation_id,
        &result_sha256,
        client.id,
    )?;
    Ok(ClientRun {
        correlation_id,
        result_sha256,
        telemetry,
    })
}

fn extract_result_line(output: &str) -> Option<&str> {
    let start = output.find("AMPLIHACK_RESULT_V1|")?;
    let tail = &output[start..];
    let end = tail
        .find(['\n', '\r', '"'])
        .or_else(|| tail.find("\\n"))
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

fn validate_substantive_result(
    result: &str,
    correlation_id: &str,
    context_digest: &str,
    nonce_digest: &str,
    context: &str,
) -> Result<(), &'static str> {
    let fields = result.split('|').collect::<Vec<_>>();
    if fields.len() != 10
        || fields[0] != "AMPLIHACK_RESULT_V1"
        || fields[1] != correlation_id
        || fields[2] != context_digest
        || fields[3] != nonce_digest
    {
        return Err("client result did not match the exact fresh-result schema");
    }
    let left_path = field_value(fields[4], "left_path")?;
    let left_symbol = field_value(fields[5], "left_symbol")?;
    let right_path = field_value(fields[6], "right_path")?;
    let right_symbol = field_value(fields[7], "right_symbol")?;
    let relationship = field_value(fields[8], "relationship")?;
    let risk = field_value(fields[9], "risk")?;
    if left_path == right_path
        || !valid_identifier(left_symbol)
        || !valid_identifier(right_symbol)
        || !context_file_contains(context, left_path, left_symbol)
        || !context_file_contains(context, right_path, right_symbol)
        || relationship.len() < 60
        || !relationship.contains(left_symbol)
        || !relationship.contains(right_symbol)
        || risk.len() < 40
    {
        return Err("client result did not prove a substantive cross-file relationship and risk");
    }
    Ok(())
}

fn field_value<'a>(field: &'a str, name: &str) -> Result<&'a str, &'static str> {
    field
        .strip_prefix(name)
        .and_then(|value| value.strip_prefix('='))
        .filter(|value| !value.is_empty())
        .ok_or("client result contained a missing or malformed field")
}

fn valid_identifier(value: &str) -> bool {
    value.len() >= 3
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn context_file_contains(context: &str, path: &str, symbol: &str) -> bool {
    let marker = format!("\n--- {path} ---\n");
    let Some(start) = context.find(&marker) else {
        return false;
    };
    let content = &context[start + marker.len()..];
    let end = content.find("\n--- ").unwrap_or(content.len());
    let content = &content[..end];
    content.match_indices(symbol).any(|(index, _)| {
        let before = content[..index].bytes().next_back();
        let after = content[index + symbol.len()..].bytes().next();
        !before.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !after.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    })
}

fn attest_repository(args: &VerifyLiveArgs) -> Result<PathBuf, VerifyFailure> {
    let repo = fs::canonicalize(&args.repo).map_err(|error| {
        config_failure(
            "repository-preflight",
            format!("cannot resolve repository: {}", error.kind()),
        )
    })?;
    let root = git_output(&repo, &["rev-parse", "--show-toplevel"])?;
    let root = fs::canonicalize(root.trim()).map_err(|error| {
        config_failure(
            "repository-preflight",
            format!("cannot resolve Git root: {}", error.kind()),
        )
    })?;
    if root != repo {
        return Err(config_failure(
            "repository-preflight",
            "--repo must name the Git worktree root",
        ));
    }

    let head = git_output(&repo, &["rev-parse", "HEAD"])?;
    if head.trim() != args.expected_head {
        return Err(config_failure(
            "repository-preflight",
            format!(
                "local HEAD {} does not equal expected head {}",
                head.trim(),
                args.expected_head
            ),
        ));
    }
    let status = git_output(
        &repo,
        &["status", "--porcelain=v2", "--untracked-files=all"],
    )?;
    if !status.is_empty() {
        return Err(config_failure(
            "repository-preflight",
            "repository must be clean, including untracked files",
        ));
    }
    for marker in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REBASE_HEAD"] {
        let marker_path = git_output(&repo, &["rev-parse", "--git-path", marker])?;
        let marker_path = PathBuf::from(marker_path);
        let marker_path = if marker_path.is_absolute() {
            marker_path
        } else {
            repo.join(marker_path)
        };
        if marker_path.exists() {
            return Err(config_failure(
                "repository-preflight",
                "merge, rebase, or cherry-pick state is not allowed",
            ));
        }
    }

    validate_context_paths(args, &repo)?;
    Ok(repo)
}

fn validate_context_paths(args: &VerifyLiveArgs, repo: &Path) -> Result<(), VerifyFailure> {
    if !(2..=24).contains(&args.context_paths.len()) {
        return Err(config_failure(
            "arguments",
            "--context must be supplied for 2-24 distinct tracked files",
        ));
    }
    let mut distinct = BTreeSet::new();
    for path in &args.context_paths {
        if path.is_absolute()
            || path.components().any(|component| {
                !matches!(component, Component::Normal(_))
                    || component.as_os_str().to_str().is_none()
            })
        {
            return Err(config_failure(
                "arguments",
                "context paths must be UTF-8 repository-relative paths without traversal",
            ));
        }
        let text = path
            .to_str()
            .ok_or_else(|| config_failure("arguments", "context paths must contain valid UTF-8"))?;
        if text.contains('\\') || !distinct.insert(text.to_string()) {
            return Err(config_failure(
                "arguments",
                "context paths must be distinct and use '/' separators",
            ));
        }
        let candidate = repo.join(path);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            config_failure(
                "repository-preflight",
                format!("cannot inspect context path {text}: {}", error.kind()),
            )
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(config_failure(
                "repository-preflight",
                format!("context path {text} must be a regular non-symlink file"),
            ));
        }
        git_output(repo, &["ls-files", "--error-unmatch", "--", text])?;
    }
    Ok(())
}

fn attest_pull_request(args: &VerifyLiveArgs, repo: &Path) -> Result<(), VerifyFailure> {
    let mut command = Command::new("gh");
    apply_control_environment(&mut command);
    let output = safe_command(
        command.current_dir(repo).args([
            "pr",
            "view",
            &args.pr.to_string(),
            "--json",
            "state,headRefOid",
            "--jq",
            "[.state,.headRefOid]|@tsv",
        ]),
        "pull-request-attestation",
    )?;
    let mut fields = output.trim().split('\t');
    let state = fields.next().unwrap_or_default();
    let head = fields.next().unwrap_or_default();
    if state != "OPEN" || head != args.expected_head {
        return Err(config_failure(
            "pull-request-attestation",
            "pull request must be open and its head must equal --expected-head",
        ));
    }
    Ok(())
}

fn attest_client(
    id: &'static str,
    binary_name: &str,
    expected_version: &str,
) -> Result<ClientAttestation, VerifyFailure> {
    let path = resolve_unique_binary(binary_name)?;
    let isolated_home = tempfile::tempdir().map_err(|error| {
        identity_failure(format!("cannot isolate client HOME: {}", error.kind()))
    })?;
    let output = safe_command(
        Command::new(&path)
            .env_clear()
            .env("PATH", restricted_path())
            .env("HOME", isolated_home.path())
            .env("LC_ALL", "C")
            .arg("--version"),
        "client-resolution",
    )?;
    let observed = output
        .split(|character: char| character.is_ascii_whitespace())
        .any(|word| version_word_matches(word, expected_version));
    if !observed {
        return Err(identity_failure(format!(
            "{id} did not report exact required version {expected_version}"
        )));
    }
    let digest = sha256_file(&path)?;
    Ok(ClientAttestation {
        id,
        version: expected_version.to_string(),
        digest,
        path,
    })
}

fn version_word_matches(word: &str, expected: &str) -> bool {
    word == expected || word.strip_suffix('.') == Some(expected)
}

fn resolve_unique_binary(name: &str) -> Result<PathBuf, VerifyFailure> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| identity_failure("PATH is unavailable for client resolution"))?;
    let mut matches = BTreeSet::new();
    for directory in std::env::split_paths(&path) {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(name);
        if is_executable(&candidate)
            && let Ok(canonical) = fs::canonicalize(candidate)
        {
            matches.insert(canonical);
        }
    }
    if matches.len() != 1 {
        return Err(identity_failure(format!(
            "{name} must resolve to exactly one distinct executable; found {}",
            matches.len()
        )));
    }
    Ok(matches.into_iter().next().expect("one match"))
}

fn attest_rustyclawd_receipt(executable: &Path) -> Result<(), VerifyFailure> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .ok_or_else(|| identity_failure("cannot locate Cargo installation receipt"))?;
    let receipt = cargo_home.join(".crates2.json");
    let metadata = fs::symlink_metadata(&receipt).map_err(|error| {
        identity_failure(format!("cannot inspect Cargo receipt: {}", error.kind()))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(identity_failure(
            "Cargo receipt must be a regular non-symlink file",
        ));
    }
    let bytes = fs::read(&receipt).map_err(|error| {
        identity_failure(format!("cannot read Cargo receipt: {}", error.kind()))
    })?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| identity_failure("Cargo receipt is malformed"))?;
    let installs = document
        .get("installs")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| identity_failure("Cargo receipt has no installs object"))?;
    let matches = installs
        .iter()
        .filter(|(package, value)| {
            package.starts_with(&format!("{RUSTYCLAWD_PACKAGE} {RUSTYCLAWD_VERSION} "))
                && package.contains(&format!("git+{RUSTYCLAWD_SOURCE}"))
                && package.contains(RUSTYCLAWD_REVISION)
                && value
                    .get("bins")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|bins| {
                        bins.as_slice() == [serde_json::Value::String("rusty".to_string())]
                    })
        })
        .count();
    if matches != 1 {
        return Err(identity_failure(format!(
            "RustyClawd must have one authoritative git receipt for {RUSTYCLAWD_SOURCE} at revision {RUSTYCLAWD_REVISION}; registry, path, tag-only, branch-only, and ambiguous receipts are rejected"
        )));
    }
    let expected = fs::canonicalize(cargo_home.join("bin/rusty")).map_err(|error| {
        identity_failure(format!(
            "cannot resolve Cargo-installed RustyClawd binary: {}",
            error.kind()
        ))
    })?;
    if executable != expected {
        return Err(identity_failure(
            "RustyClawd executable is not the canonical Cargo-installed binary",
        ));
    }
    Ok(())
}

fn execute_client(
    client: &ClientAttestation,
    config: &LiveConfig,
    repo: &Path,
    prompt: &str,
    timeout_seconds: u64,
    failure_case: &'static str,
) -> Result<String, VerifyFailure> {
    #[cfg(not(target_os = "linux"))]
    return Err(client_failure(
        client.id,
        78,
        failure_case,
        "read-only repository isolation is unavailable on this host",
        "Run live verification on Linux with bubblewrap installed.",
    ));

    #[cfg(target_os = "linux")]
    let mut command = {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("bwrap");
        command
            .process_group(0)
            .args([
                "--die-with-parent",
                "--new-session",
                "--ro-bind",
                "/",
                "/",
                "--dev-bind",
                "/dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/tmp",
                "--dir",
                "/tmp/amplihack-home",
                "--chdir",
            ])
            .arg(repo)
            .arg("--")
            .arg(&client.path);
        command
    };
    command
        .env_clear()
        .env("PATH", restricted_path())
        .env("HOME", "/tmp/amplihack-home")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("NO_COLOR", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1");
    for name in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    match client.id {
        "claude" => {
            command
                .env("ANTHROPIC_BASE_URL", &config.endpoint)
                .env("ANTHROPIC_AUTH_TOKEN", &config.key)
                .env("ANTHROPIC_MODEL", &config.model)
                .env("ANTHROPIC_SMALL_FAST_MODEL", &config.model)
                .env("CLAUDE_CODE_SUBAGENT_MODEL", &config.model)
                .env("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1")
                .args([
                    "--print",
                    "--model",
                    &config.model,
                    "--tools",
                    "",
                    "--setting-sources",
                    "",
                    "--strict-mcp-config",
                    "--mcp-config",
                    r#"{"mcpServers":{}}"#,
                    "--no-session-persistence",
                    "--output-format",
                    "text",
                    prompt,
                ]);
        }
        "copilot" => {
            command
                .env(
                    "COPILOT_PROVIDER_BASE_URL",
                    format!("{}/v1", config.endpoint),
                )
                .env("COPILOT_PROVIDER_API_KEY", &config.key)
                .env("COPILOT_PROVIDER_TYPE", "openai")
                .env("COPILOT_PROVIDER_WIRE_API", "completions")
                .env("COPILOT_MODEL", &config.model)
                .args([
                    "--model",
                    &config.model,
                    "--disable-builtin-mcps",
                    "--no-custom-instructions",
                    "--no-remote",
                    "--no-remote-export",
                    "--no-auto-update",
                    "--no-color",
                    "--no-experimental",
                    "--no-ask-user",
                    "--no-bash-env",
                    "--secret-env-vars=COPILOT_PROVIDER_API_KEY",
                    "--deny-tool=shell",
                    "--deny-tool=write",
                    "--deny-tool=web_fetch",
                    "--output-format",
                    "text",
                    "-p",
                    prompt,
                ]);
        }
        "rustyclawd" => {
            command
                .env("GITHUB_TOKEN", &config.key)
                .env("GITHUB_COPILOT_ENDPOINT", format!("{}/v1", config.endpoint))
                .args([
                    "--provider",
                    "copilot",
                    "--model",
                    &config.model,
                    "--print",
                    "--max-turns",
                    "1",
                    "--output-format",
                    "json",
                    "--disallowedTools",
                    "Bash",
                    "--disallowedTools",
                    "Write",
                    "--disallowedTools",
                    "Edit",
                    "--disallowedTools",
                    "WebFetch",
                    "--disallowedTools",
                    "WebSearch",
                    prompt,
                ]);
        }
        _ => unreachable!("attested client identifier"),
    }
    let stdout_file = tempfile::tempfile().map_err(|error| {
        client_failure(
            client.id,
            78,
            failure_case,
            format!("cannot create private stdout capture: {}", error.kind()),
            "Ensure the host temporary directory is writable and owner-only.",
        )
    })?;
    let stderr_file = tempfile::tempfile().map_err(|error| {
        client_failure(
            client.id,
            78,
            failure_case,
            format!("cannot create private stderr capture: {}", error.kind()),
            "Ensure the host temporary directory is writable and owner-only.",
        )
    })?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file.try_clone().map_err(|error| {
            client_failure(
                client.id,
                78,
                failure_case,
                format!("cannot clone stdout capture: {}", error.kind()),
                "Check host file descriptor limits and rerun verification.",
            )
        })?))
        .stderr(Stdio::from(stderr_file.try_clone().map_err(|error| {
            client_failure(
                client.id,
                78,
                failure_case,
                format!("cannot clone stderr capture: {}", error.kind()),
                "Check host file descriptor limits and rerun verification.",
            )
        })?));
    let mut child = command.spawn().map_err(|error| {
        let exit = if error.kind() == std::io::ErrorKind::NotFound {
            78
        } else {
            77
        };
        client_failure(
            client.id,
            exit,
            failure_case,
            format!("client failed to start: {}", error.kind()),
            "Install the exact pinned client and ensure its executable is available.",
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                return Err(client_failure(
                    client.id,
                    70,
                    failure_case,
                    "client execution timed out",
                    "Check gateway reachability and retry with a larger --timeout-seconds value.",
                ));
            }

            Err(error) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                return Err(client_failure(
                    client.id,
                    70,
                    failure_case,
                    format!("cannot monitor client: {}", error.kind()),
                    "Check host process limits and rerun verification.",
                ));
            }
        }
    }
    let status = child.wait().map_err(|error| {
        client_failure(
            client.id,
            70,
            failure_case,
            format!("cannot collect client status: {}", error.kind()),
            "Check host process limits and rerun verification.",
        )
    })?;
    if !status.success() {
        return Err(client_failure(
            client.id,
            69,
            failure_case,
            format!("client exited nonzero ({})", status.code().unwrap_or(1)),
            "Verify the gateway endpoint, key, model alias, and upstream route, then rerun.",
        ));
    }
    let mut output = Vec::new();
    let mut stdout_file = stdout_file;
    let mut stderr_file = stderr_file;
    use std::io::{Seek, SeekFrom};
    stdout_file.seek(SeekFrom::Start(0)).map_err(|error| {
        client_failure(
            client.id,
            70,
            failure_case,
            format!("cannot rewind stdout capture: {}", error.kind()),
            "Check the host temporary filesystem and rerun verification.",
        )
    })?;
    stderr_file.seek(SeekFrom::Start(0)).map_err(|error| {
        client_failure(
            client.id,
            70,
            failure_case,
            format!("cannot rewind stderr capture: {}", error.kind()),
            "Check the host temporary filesystem and rerun verification.",
        )
    })?;
    stdout_file
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|error| {
            client_failure(
                client.id,
                70,
                failure_case,
                format!("cannot read stdout capture: {}", error.kind()),
                "Check the host temporary filesystem and rerun verification.",
            )
        })?;
    stderr_file
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|error| {
            client_failure(
                client.id,
                70,
                failure_case,
                format!("cannot read stderr capture: {}", error.kind()),
                "Check the host temporary filesystem and rerun verification.",
            )
        })?;
    if output.len() > 2 * 1024 * 1024 {
        return Err(client_failure(
            client.id,
            70,
            failure_case,
            "client output exceeded the 2 MiB safety limit",
            "Use a bounded model response and rerun verification.",
        ));
    }
    let output = String::from_utf8(output).map_err(|_| {
        client_failure(
            client.id,
            70,
            failure_case,
            "client output was not UTF-8",
            "Configure the client for UTF-8 text output.",
        )
    })?;
    Ok(output)
}

#[cfg(target_os = "linux")]
fn terminate_process_tree(child: &mut std::process::Child) {
    let process_group = -(child.id() as i32);
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
}

#[cfg(not(target_os = "linux"))]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn run_negative_cases(
    client: &ClientAttestation,
    config: &LiveConfig,
    repo: &Path,
    timeout_seconds: u64,
) -> Result<u16, VerifyFailure> {
    let mut passed = 0_u16;
    for (case, endpoint, key, model) in [
        ("missing-endpoint", None, Some("key"), Some("model")),
        (
            "missing-key",
            Some("https://gateway.invalid"),
            None,
            Some("model"),
        ),
        (
            "missing-model",
            Some("https://gateway.invalid"),
            Some("key"),
            None,
        ),
    ] {
        if validate_negative_route(endpoint, key, model).is_ok() {
            return Err(client_failure(
                client.id,
                1,
                case,
                "incomplete route unexpectedly passed validation",
                "Require endpoint, key, and model before launching every client.",
            ));
        }
        passed += 1;
    }
    let prompt = format!(
        "Negative route verification {}. Return no sensitive values.",
        Uuid::new_v4()
    );
    let invalid = LiveConfig {
        endpoint: config.endpoint.clone(),
        key: format!("invalid-{}", Uuid::new_v4()),
        model: config.model.clone(),
        expected_provider: config.expected_provider.clone(),
        expected_model: config.expected_model.clone(),
        telemetry_file: config.telemetry_file.clone(),
        telemetry_hmac_key: config.telemetry_hmac_key.clone(),
    };
    expect_client_failure(
        client,
        &invalid,
        repo,
        &prompt,
        timeout_seconds,
        "invalid-credential",
    )?;
    passed += 1;

    let unavailable = LiveConfig {
        endpoint: "http://127.0.0.1:9".to_string(),
        key: config.key.clone(),
        model: config.model.clone(),
        expected_provider: config.expected_provider.clone(),
        expected_model: config.expected_model.clone(),
        telemetry_file: config.telemetry_file.clone(),
        telemetry_hmac_key: config.telemetry_hmac_key.clone(),
    };
    expect_client_failure(
        client,
        &unavailable,
        repo,
        &prompt,
        timeout_seconds,
        "unavailable-gateway",
    )?;
    passed += 1;

    for scenario in [FixtureScenario::UpstreamFailure, FixtureScenario::Malformed] {
        let fixture = FailureFixture::start(scenario, &config.model)?;
        let fixture_config = LiveConfig {
            endpoint: format!("http://127.0.0.1:{}", fixture.port),
            key: config.key.clone(),
            model: config.model.clone(),
            expected_provider: config.expected_provider.clone(),
            expected_model: config.expected_model.clone(),
            telemetry_file: config.telemetry_file.clone(),
            telemetry_hmac_key: config.telemetry_hmac_key.clone(),
        };
        let case = match scenario {
            FixtureScenario::UpstreamFailure => "upstream-failure",
            FixtureScenario::Malformed => "malformed-response",
        };
        expect_client_failure(
            client,
            &fixture_config,
            repo,
            &prompt,
            timeout_seconds,
            case,
        )?;
        passed += 1;
    }
    Ok(passed)
}

fn validate_negative_route(
    endpoint: Option<&str>,
    key: Option<&str>,
    model: Option<&str>,
) -> Result<(), ()> {
    if endpoint.is_some_and(|value| !value.is_empty())
        && key.is_some_and(|value| !value.is_empty())
        && model.is_some_and(|value| !value.is_empty())
    {
        Ok(())
    } else {
        Err(())
    }
}

#[derive(Clone, Copy)]
enum FixtureScenario {
    UpstreamFailure,
    Malformed,
}

struct FailureFixture {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FailureFixture {
    fn start(scenario: FixtureScenario, model: &str) -> Result<Self, VerifyFailure> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
            config_failure(
                "negative-fixture",
                format!("cannot bind loopback fixture: {}", error.kind()),
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                config_failure(
                    "negative-fixture",
                    format!("cannot inspect loopback fixture: {}", error.kind()),
                )
            })?
            .port();
        let model = model.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            let deadline = Instant::now() + Duration::from_secs(50);
            while Instant::now() < deadline && !thread_stop.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut request = [0_u8; 16 * 1024];
                let count = stream.read(&mut request).unwrap_or(0);
                let is_get = request[..count].starts_with(b"GET ");
                let (status, body) = if is_get {
                    (
                        "200 OK",
                        format!(
                            r#"{{"object":"list","data":[{{"id":"{model}","object":"model"}}]}}"#
                        ),
                    )
                } else {
                    match scenario {
                        FixtureScenario::UpstreamFailure => (
                            "502 Bad Gateway",
                            r#"{"error":{"type":"upstream_error","message":"fixture"}}"#
                                .to_string(),
                        ),
                        FixtureScenario::Malformed => ("200 OK", "{".to_string()),
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                if !is_get {
                    break;
                }
            }
        });
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for FailureFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn expect_client_failure(
    client: &ClientAttestation,
    config: &LiveConfig,
    repo: &Path,
    prompt: &str,
    timeout_seconds: u64,
    failure_case: &'static str,
) -> Result<(), VerifyFailure> {
    match execute_client(client, config, repo, prompt, timeout_seconds, failure_case) {
        Err(error) if error.exit == 69 || error.exit == 70 => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(client_failure(
            client.id,
            1,
            failure_case,
            "client unexpectedly succeeded",
            "Remove fallback credentials/routes and ensure this case fails closed.",
        )),
    }
}

fn read_telemetry(
    config: &LiveConfig,
    offset: u64,
    correlation_id: &str,
    result_sha256: &str,
    client: &'static str,
) -> Result<GatewayTelemetryV1, VerifyFailure> {
    use std::io::{Seek, SeekFrom};

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut matches = Vec::new();
    while Instant::now() < deadline {
        let metadata = fs::symlink_metadata(&config.telemetry_file).map_err(|error| {
            client_failure(
                client,
                70,
                "gateway-telemetry",
                format!("cannot inspect gateway telemetry: {}", error.kind()),
                "Keep telemetry as one append-only regular non-symlink file.",
            )
        })?;
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() < offset
        {
            return Err(client_failure(
                client,
                70,
                "gateway-telemetry",
                "gateway telemetry file changed identity or was truncated",
                "Keep telemetry as one append-only regular non-symlink file.",
            ));
        }
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&config.telemetry_file).map_err(|error| {
            client_failure(
                client,
                70,
                "gateway-telemetry",
                format!("cannot open gateway telemetry: {}", error.kind()),
                "Configure LiteLLM's signed amplihack telemetry callback and retry.",
            )
        })?;
        file.seek(SeekFrom::Start(offset)).map_err(|error| {
            client_failure(
                client,
                70,
                "gateway-telemetry",
                format!("cannot seek gateway telemetry: {}", error.kind()),
                "Use an append-only regular telemetry file.",
            )
        })?;
        let mut appended = String::new();
        file.read_to_string(&mut appended).map_err(|error| {
            client_failure(
                client,
                70,
                "gateway-telemetry",
                format!("cannot read gateway telemetry: {}", error.kind()),
                "Use an append-only UTF-8 JSONL telemetry file.",
            )
        })?;
        matches = appended
            .lines()
            .filter_map(|line| serde_json::from_str::<GatewayTelemetryV1>(line).ok())
            .filter(|record| record.correlation_id == correlation_id)
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if matches.len() != 1 {
        return Err(client_failure(
            client,
            70,
            "gateway-telemetry",
            format!(
                "expected one fresh correlated gateway record, found {}",
                matches.len()
            ),
            "Enable signed per-request telemetry and rerun with a fresh correlation ID.",
        ));
    }
    let record = matches.pop().expect("one telemetry record");
    if record.schema_version != 1
        || record.signature_sha256 != telemetry_signature(&record, &config.telemetry_hmac_key)
        || record.requested_alias != config.model
        || record.observed_provider != config.expected_provider
        || record.observed_model != config.expected_model
        || record.cache_status != "miss"
        || record.backend_dispatch_id.is_empty()
        || record.observed_provider.is_empty()
        || record.observed_model.is_empty()
        || record.gateway_identity.is_empty()
        || !constant_time_eq(record.result_sha256.as_bytes(), result_sha256.as_bytes())
        || !record
            .result_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(client_failure(
            client,
            70,
            "gateway-telemetry",
            "gateway telemetry failed signature, model, dispatch, or cache-miss validation",
            "Disable caches and fallbacks, select the exact alias, and verify the telemetry HMAC key.",
        ));
    }
    Ok(record)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn telemetry_signature(record: &GatewayTelemetryV1, key: &str) -> String {
    let body = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        record.schema_version,
        record.correlation_id,
        record.requested_alias,
        record.observed_provider,
        record.observed_model,
        record.gateway_identity,
        record.cache_status,
        record.backend_dispatch_id,
        record.result_sha256
    );
    hmac_sha256(key.as_bytes(), body.as_bytes())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        normalized[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    evidence::hex(&outer.finalize())
}

fn client_summary(
    client: &ClientAttestation,
    config: &LiveConfig,
    run: ClientRun,
) -> ClientSummaryV1 {
    let rustyclawd = client.id == "rustyclawd";
    ClientSummaryV1 {
        client: client.id,
        version: Some(client.version.clone()),
        binary_sha256: Some(client.digest.clone()),
        status: "passed",
        correlation_id: Some(run.correlation_id),
        requested_alias: Some(config.model.clone()),
        observed_provider: Some(run.telemetry.observed_provider),
        observed_model: Some(run.telemetry.observed_model),
        gateway_identity: Some(run.telemetry.gateway_identity),
        cache_status: Some(run.telemetry.cache_status),
        backend_dispatch_id: Some(run.telemetry.backend_dispatch_id),
        result_sha256: Some(run.result_sha256),
        failure_case: None,
        failure_stage: None,
        rustyclawd_source: rustyclawd.then_some(RUSTYCLAWD_SOURCE),
        rustyclawd_revision: rustyclawd.then_some(RUSTYCLAWD_REVISION),
        rustyclawd_package: rustyclawd.then_some(RUSTYCLAWD_PACKAGE),
        executable_path: Some(client.path.display().to_string()),
        tools_disabled: Some(true),
    }
}

fn ensure_repository_unchanged(
    repo: &Path,
    client: &'static str,
    stage: &'static str,
) -> Result<(), VerifyFailure> {
    let status = git_output(repo, &["status", "--porcelain=v2", "--untracked-files=all"])?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(client_failure(
            client,
            70,
            stage,
            "repository changed during verification",
            "Discard the run, inspect client behavior, restore cleanliness, and rerun.",
        ))
    }
}

fn client_failure(
    client: &'static str,
    exit: u8,
    stage: &'static str,
    message: impl Into<String>,
    remediation: &'static str,
) -> VerifyFailure {
    VerifyFailure::new(
        exit,
        "AH-LIVE-CLIENT-001",
        stage,
        format!("client={client}: {}", message.into()),
        remediation,
    )
}

fn validate_evidence_directory(
    requested: Option<&Path>,
    repo: &Path,
) -> Result<PathBuf, VerifyFailure> {
    let directory = match requested {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(_) => {
            return Err(config_failure(
                "arguments",
                "--evidence-dir must be absolute",
            ));
        }
        None => {
            if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
                PathBuf::from(state).join("amplihack/litellm-evidence")
            } else {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| config_failure("arguments", "HOME is unavailable"))?;
                PathBuf::from(home).join(".local/state/amplihack/litellm-evidence")
            }
        }
    };
    let mut parent = directory.as_path();
    while !parent.exists() {
        parent = parent.parent().ok_or_else(|| {
            config_failure("arguments", "evidence directory has no existing parent")
        })?;
    }
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        config_failure(
            "arguments",
            format!("cannot resolve evidence parent: {}", error.kind()),
        )
    })?;
    if canonical_parent.starts_with(repo) {
        return Err(config_failure(
            "arguments",
            "evidence directory must be outside the repository",
        ));
    }
    Ok(directory)
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String, VerifyFailure> {
    let mut command = amplihack_git::command_in(repo);
    apply_control_environment(&mut command);
    safe_command(command.args(args), "repository-preflight")
}

fn apply_control_environment(command: &mut Command) {
    command.env_clear().env("PATH", restricted_path());
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "GH_CONFIG_DIR",
        "GH_HOST",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn safe_command(command: &mut Command, stage: &'static str) -> Result<String, VerifyFailure> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        config_failure(
            stage,
            format!("required command failed to start: {}", error.kind()),
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                return Err(config_failure(stage, "required command timed out"));
            }
            Err(error) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                return Err(config_failure(
                    stage,
                    format!("cannot monitor required command: {}", error.kind()),
                ));
            }
        }
    }
    let output = child.wait_with_output().map_err(|error| {
        config_failure(
            stage,
            format!("cannot collect required command output: {}", error.kind()),
        )
    })?;
    if !output.status.success() {
        return Err(config_failure(
            stage,
            format!(
                "required command exited nonzero ({})",
                output.status.code().unwrap_or(1)
            ),
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| config_failure(stage, "required command returned non-UTF-8 output"))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|_| config_failure(stage, "required command returned non-UTF-8 output"))?;
    Ok(format!("{stdout}{stderr}").trim().to_string())
}

fn sha256_file(path: &Path) -> Result<String, VerifyFailure> {
    let mut file = fs::File::open(path).map_err(|error| {
        identity_failure(format!("cannot open client binary: {}", error.kind()))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            identity_failure(format!("cannot hash client binary: {}", error.kind()))
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(evidence::hex(&hasher.finalize()))
}

fn restricted_path() -> std::ffi::OsString {
    let mut directories = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    directories.sort();
    directories.dedup();
    std::env::join_paths(directories).unwrap_or_else(|_| "/usr/bin:/bin".into())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn config_failure(stage: &'static str, message: impl Into<String>) -> VerifyFailure {
    VerifyFailure::new(
        64,
        "AH-LIVE-CONFIG-001",
        stage,
        message,
        "Correct the host configuration and retry without placing credentials on the command line.",
    )
}

fn identity_failure(message: impl Into<String>) -> VerifyFailure {
    VerifyFailure::new(
        77,
        "AH-LIVE-IDENTITY-001",
        "client-provenance",
        message,
        "Install the exact authoritative client and retry.",
    )
}

#[cfg(test)]
mod tests {
    use super::version_word_matches;

    #[test]
    fn exact_version_accepts_copilot_terminal_period_only() {
        assert!(version_word_matches("1.0.83-2", "1.0.83-2"));
        assert!(version_word_matches("1.0.83-2.", "1.0.83-2"));
        assert!(!version_word_matches("1.0.83-1", "1.0.83-2"));
        assert!(!version_word_matches("9.1.0.83-2", "1.0.83-2"));
    }
}
