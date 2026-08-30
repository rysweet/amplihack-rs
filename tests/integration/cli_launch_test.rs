use std::path::PathBuf;
/// Integration tests: CLI launch flow smoke tests.
///
/// These tests exercise the top-level amplihack binary through its argument
/// parsing and command dispatch layer without requiring live external tools
/// (claude, copilot, etc.) to be installed.  They are smoke-level tests
/// that confirm the binary is built, parses flags correctly, and produces
/// expected exit codes / output for basic invocations.
use std::process::Command;

#[cfg(unix)]
mod external_litellm_contract {
    use super::amplihack_bin;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    const LITELLM_ENV: &[&str] = &[
        "AMPLIHACK_LITELLM_ENDPOINT",
        "AMPLIHACK_LITELLM_API_KEY",
        "AMPLIHACK_LITELLM_API_KEY_FILE",
        "AMPLIHACK_LITELLM_COPILOT_MODEL",
    ];

    struct Harness {
        root: TempDir,
        bin_dir: PathBuf,
        child_record: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("create isolated LiteLLM test root");
            let bin_dir = root.path().join("bin");
            fs::create_dir(&bin_dir).expect("create fake binary directory");
            let child_record = root.path().join("child-record");
            for name in ["claude", "copilot", "codex", "amplifier", "RustyClawd"] {
                write_fake_agent(&bin_dir.join(name), &child_record);
            }
            Self {
                root,
                bin_dir,
                child_record,
            }
        }

        fn command(&self, subcommand: &str) -> Command {
            let mut command = Command::new(amplihack_bin());
            command
                .arg(subcommand)
                .arg("--subprocess-safe")
                .arg("--no-reflection")
                .env_clear()
                .env("HOME", self.root.path())
                .env("PATH", &self.bin_dir)
                .env("AMPLIHACK_HOME", self.root.path().join(".amplihack"))
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .env("AMPLIHACK_NONINTERACTIVE", "1")
                .env("AMPLIHACK_NO_SYSTEM_PROMPT_APPEND", "1");
            for name in LITELLM_ENV {
                command.env_remove(name);
            }
            command
        }

        fn child_record(&self) -> String {
            fs::read_to_string(&self.child_record).unwrap_or_default()
        }
    }

    fn write_fake_agent(path: &Path, child_record: &Path) {
        let script = r#"#!/bin/sh
if [ "${1-}" = "--version" ]; then
  printf 'test-agent 1.2.3\n'
  exit 0
fi
if [ "${1-}" = "--help" ]; then
  printf '%s\n' '--setting-sources COPILOT_PROVIDER_TYPE COPILOT_PROVIDER_BASE_URL COPILOT_PROVIDER_API_KEY COPILOT_MODEL COPILOT_OFFLINE'
  exit 0
fi
{
  printf 'argv='
  printf '%s|' "$@"
  printf '\nANTHROPIC_BASE_URL=%s\n' "${ANTHROPIC_BASE_URL-}"
  printf 'ANTHROPIC_AUTH_TOKEN=%s\n' "${ANTHROPIC_AUTH_TOKEN-}"
  printf 'COPILOT_PROVIDER_TYPE=%s\n' "${COPILOT_PROVIDER_TYPE-}"
  printf 'COPILOT_PROVIDER_BASE_URL=%s\n' "${COPILOT_PROVIDER_BASE_URL-}"
  printf 'COPILOT_PROVIDER_API_KEY=%s\n' "${COPILOT_PROVIDER_API_KEY-}"
  printf 'COPILOT_MODEL=%s\n' "${COPILOT_MODEL-}"
  printf 'COPILOT_OFFLINE=%s\n' "${COPILOT_OFFLINE-}"
  printf 'AMPLIHACK_LITELLM_ENDPOINT=%s\n' "${AMPLIHACK_LITELLM_ENDPOINT-}"
  printf 'AMPLIHACK_LITELLM_API_KEY=%s\n' "${AMPLIHACK_LITELLM_API_KEY-}"
  printf 'AMPLIHACK_LITELLM_API_KEY_FILE=%s\n' "${AMPLIHACK_LITELLM_API_KEY_FILE-}"
  printf 'AMPLIHACK_LITELLM_COPILOT_MODEL=%s\n' "${AMPLIHACK_LITELLM_COPILOT_MODEL-}"
  printf 'UNRELATED_SECRET=%s\n' "${UNRELATED_SECRET-}"
} > __CHILD_RECORD__
"#
        .replace(
            "__CHILD_RECORD__",
            &format!("'{}'", child_record.to_string_lossy().replace('\'', "'\\''")),
        );
        fs::write(path, script).expect("write fake agent");
        let mut permissions = fs::metadata(path).expect("stat fake agent").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).expect("make fake agent executable");
    }

    fn combined(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn assert_stable_error(output: &Output, expected_code: &str, forbidden: &[&str]) {
        let diagnostic = combined(output);
        assert!(
            !output.status.success(),
            "expected {expected_code}, but launch succeeded: {diagnostic}"
        );
        assert!(
            diagnostic.starts_with(expected_code)
                || diagnostic.starts_with(&format!("error: {expected_code}")),
            "diagnostic must begin with {expected_code}, got: {diagnostic}"
        );
        for secret in forbidden {
            assert!(
                !diagnostic.contains(secret),
                "diagnostic leaked forbidden route data {secret:?}: {diagnostic}"
            );
        }
    }

    fn readiness_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local readiness double");
        listener
            .set_nonblocking(true)
            .expect("make readiness double bounded");
        let endpoint = format!("http://{}", listener.local_addr().expect("local address"));
        let (sender, receiver) = mpsc::channel();
        let status = status.to_owned();
        let content_type = content_type.to_owned();
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .expect("set request timeout");
                        let mut request = vec![0; 4096];
                        let count = stream.read(&mut request).unwrap_or_default();
                        let request = String::from_utf8_lossy(&request[..count]).into_owned();
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream
                            .write_all(response.as_bytes())
                            .expect("write readiness response");
                        sender.send(request).expect("record readiness request");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            sender.send(String::new()).expect("record missing request");
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("readiness double failed: {error}"),
                }
            }
        });
        (endpoint, receiver, handle)
    }

    #[test]
    fn supported_launchers_document_both_activation_flags() {
        for subcommand in ["launch", "claude", "copilot"] {
            let output = Command::new(amplihack_bin())
                .args([subcommand, "--help"])
                .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
                .output()
                .expect("run launcher help");
            let help = combined(&output);
            assert!(output.status.success(), "{subcommand} help failed: {help}");
            assert!(
                help.contains("--litellm") && help.contains("--no-litellm"),
                "{subcommand} must expose external gateway activation controls: {help}"
            );
        }
    }

    #[test]
    fn claude_route_checks_readiness_then_launches_with_an_isolated_environment() {
        let harness = Harness::new();
        let (endpoint, request, server) =
            readiness_server("200 OK", "application/json", r#"{"status":"healthy"}"#);
        let secret = "virtual-key-must-not-leak";
        let output = harness
            .command("claude")
            .arg("--litellm")
            .env("AMPLIHACK_LITELLM_ENDPOINT", &endpoint)
            .env("AMPLIHACK_LITELLM_API_KEY", secret)
            .env("ANTHROPIC_API_KEY", "direct-provider-key")
            .env("HTTPS_PROXY", "http://proxy.invalid")
            .env("UNRELATED_SECRET", "unrelated-parent-secret")
            .output()
            .expect("run routed Claude launch");
        let observed_request = request
            .recv_timeout(Duration::from_secs(4))
            .expect("readiness double must terminate");
        server.join().expect("join readiness double");

        assert!(
            output.status.success(),
            "healthy external route should launch Claude: {}",
            combined(&output)
        );
        assert!(
            observed_request.starts_with("GET /health/readiness HTTP/1.1\r\n"),
            "expected one readiness GET, got: {observed_request:?}"
        );
        assert!(
            observed_request.contains("Accept: application/json")
                || observed_request.contains("accept: application/json"),
            "readiness request must request JSON: {observed_request:?}"
        );
        assert!(
            !observed_request
                .to_ascii_lowercase()
                .contains("authorization")
                && !observed_request.contains(secret),
            "readiness request must be unauthenticated: {observed_request:?}"
        );

        let child = harness.child_record();
        assert!(child.contains(&format!("ANTHROPIC_BASE_URL={endpoint}")));
        assert!(child.contains(&format!("ANTHROPIC_AUTH_TOKEN={secret}")));
        assert!(
            !child.contains("--litellm"),
            "control flag reached child: {child}"
        );
        for name in LITELLM_ENV {
            assert!(
                child.contains(&format!("{name}=\n")),
                "gateway control {name} reached child: {child}"
            );
        }
        assert!(
            !child.contains("direct-provider-key")
                && !child.contains("proxy.invalid")
                && !child.contains("unrelated-parent-secret"),
            "direct-provider or proxy route reached child: {child}"
        );
    }

    #[test]
    fn copilot_route_derives_v1_and_enforces_offline_custom_provider() {
        let harness = Harness::new();
        let (endpoint, request, server) =
            readiness_server("200 OK", "application/json", r#"{"status":"healthy"}"#);
        let output = harness
            .command("copilot")
            .arg("--litellm")
            .env("AMPLIHACK_LITELLM_ENDPOINT", &endpoint)
            .env("AMPLIHACK_LITELLM_API_KEY", "copilot-virtual-key")
            .env("AMPLIHACK_LITELLM_COPILOT_MODEL", "gateway-coding")
            .output()
            .expect("run routed Copilot launch");
        let _ = request.recv_timeout(Duration::from_secs(4));
        server.join().expect("join readiness double");

        assert!(
            output.status.success(),
            "healthy external route should launch Copilot: {}",
            combined(&output)
        );
        let child = harness.child_record();
        assert!(child.contains("COPILOT_PROVIDER_TYPE=openai"), "{child}");
        assert!(
            child.contains(&format!("COPILOT_PROVIDER_BASE_URL={endpoint}/v1")),
            "{child}"
        );
        assert!(
            child.contains("COPILOT_PROVIDER_API_KEY=copilot-virtual-key"),
            "{child}"
        );
        assert!(child.contains("COPILOT_MODEL=gateway-coding"), "{child}");
        assert!(child.contains("COPILOT_OFFLINE=true"), "{child}");
        assert!(!child.contains("--remote"), "{child}");
    }

    #[test]
    fn no_litellm_is_a_strict_bypass_even_with_malformed_route_inputs() {
        let harness = Harness::new();
        let output = harness
            .command("claude")
            .arg("--no-litellm")
            .env("AMPLIHACK_LITELLM_ENDPOINT", "not a URL")
            .env("AMPLIHACK_LITELLM_API_KEY", "ignored-secret")
            .output()
            .expect("run bypassed Claude launch");
        assert!(
            output.status.success(),
            "--no-litellm must preserve ordinary launch behavior: {}",
            combined(&output)
        );
        let child = harness.child_record();
        assert!(
            !child.contains("--no-litellm"),
            "amplihack control flag reached child: {child}"
        );
        assert!(
            !child.contains("ANTHROPIC_AUTH_TOKEN=ignored-secret"),
            "disabled route configured child: {child}"
        );
    }

    #[test]
    fn configuration_conflicts_and_partial_routes_fail_before_spawn() {
        type InvalidRouteCase<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)], &'a str);
        let cases: &[InvalidRouteCase<'_>] = &[
            (
                &["--litellm", "--no-litellm"],
                &[],
                "conflicting activation controls",
            ),
            (
                &["--litellm"],
                &[("AMPLIHACK_LITELLM_ENDPOINT", "https://gateway.example")],
                "missing credential",
            ),
            (
                &[],
                &[("AMPLIHACK_LITELLM_API_KEY", "orphan-key")],
                "partial route",
            ),
            (
                &["--litellm"],
                &[
                    ("AMPLIHACK_LITELLM_ENDPOINT", "https://gateway.example"),
                    ("AMPLIHACK_LITELLM_API_KEY", "inline-key"),
                    ("AMPLIHACK_LITELLM_API_KEY_FILE", "/tmp/key"),
                ],
                "credential-source conflict",
            ),
        ];

        for (args, environment, label) in cases {
            let harness = Harness::new();
            let mut command = harness.command("claude");
            command.args(*args);
            for (name, value) in *environment {
                command.env(name, value);
            }
            let output = command.output().expect("run invalid route");
            assert_stable_error(
                &output,
                if *label == "credential-source conflict" {
                    "AH_LITELLM_CREDENTIAL"
                } else {
                    "AH_LITELLM_CONFIG"
                },
                &["inline-key", "orphan-key", "gateway.example"],
            );
            assert!(
                harness.child_record().is_empty(),
                "{label} spawned the child"
            );
        }
    }

    #[test]
    fn unsafe_and_boundary_endpoints_are_rejected_before_network_or_spawn() {
        for endpoint in [
            "http://gateway.example",
            "ftp://127.0.0.1/gateway",
            "https://user:password@gateway.example",
            "https://gateway.example/v1",
            "https://gateway.example/%2e%2e/admin",
            "http://localhost:4000",
        ] {
            let harness = Harness::new();
            let output = harness
                .command("claude")
                .arg("--litellm")
                .env("AMPLIHACK_LITELLM_ENDPOINT", endpoint)
                .env("AMPLIHACK_LITELLM_API_KEY", "endpoint-test-secret")
                .output()
                .expect("run unsafe endpoint case");
            assert_stable_error(
                &output,
                "AH_LITELLM_ENDPOINT",
                &[endpoint, "endpoint-test-secret"],
            );
            assert!(
                harness.child_record().is_empty(),
                "unsafe endpoint spawned the child: {endpoint}"
            );
        }
    }

    #[test]
    fn unavailable_gateway_fails_closed_without_launching_or_falling_back() {
        let harness = Harness::new();
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve unavailable port");
        let endpoint = format!(
            "http://{}",
            listener.local_addr().expect("reserved address")
        );
        drop(listener);
        let output = harness
            .command("claude")
            .arg("--litellm")
            .env("AMPLIHACK_LITELLM_ENDPOINT", &endpoint)
            .env("AMPLIHACK_LITELLM_API_KEY", "unavailable-secret")
            .output()
            .expect("run unavailable gateway case");
        assert_stable_error(
            &output,
            "AH_LITELLM_READINESS",
            &[&endpoint, "unavailable-secret"],
        );
        assert!(
            harness.child_record().is_empty(),
            "gateway failure must not launch or fall back"
        );
    }

    #[test]
    fn claude_cloud_and_session_bypass_arguments_fail_before_network_or_spawn() {
        for argument in [
            "--environment=cloud-session",
            "--teleport",
            "--from-pr=123",
            "--fork-session",
            "--remote-control",
        ] {
            let harness = Harness::new();
            let output = harness
                .command("claude")
                .arg("--litellm")
                .arg(argument)
                .env(
                    "AMPLIHACK_LITELLM_ENDPOINT",
                    "https://argument-policy.example",
                )
                .env("AMPLIHACK_LITELLM_API_KEY", "argument-secret")
                .output()
                .expect("run unsafe routed argument case");
            assert_stable_error(
                &output,
                "AH_LITELLM_ARGUMENT",
                &["argument-policy.example", "argument-secret"],
            );
            assert!(
                harness.child_record().is_empty(),
                "unsafe argument spawned the child: {argument}"
            );
        }
    }

    #[test]
    fn malformed_readiness_payload_has_a_protocol_error_without_body_leakage() {
        for body in [
            r#"{"status":"unhealthy","token":"body-secret"}"#,
            r#"{"status":"healthy","status":"unhealthy"}"#,
            r#"{"status":"healthy"} trailing"#,
        ] {
            let harness = Harness::new();
            let (endpoint, request, server) = readiness_server("200 OK", "application/json", body);
            let output = harness
                .command("claude")
                .arg("--litellm")
                .env("AMPLIHACK_LITELLM_ENDPOINT", &endpoint)
                .env("AMPLIHACK_LITELLM_API_KEY", "protocol-secret")
                .output()
                .expect("run malformed readiness case");
            let _ = request.recv_timeout(Duration::from_secs(4));
            server.join().expect("join readiness double");
            assert_stable_error(
                &output,
                "AH_LITELLM_PROTOCOL",
                &[&endpoint, "protocol-secret", body, "body-secret"],
            );
            assert!(
                harness.child_record().is_empty(),
                "invalid readiness body spawned child"
            );
        }
    }

    #[test]
    fn failing_readiness_http_contract_uses_readiness_error() {
        for (status, media_type, body) in [
            (
                "503 Service Unavailable",
                "application/json",
                r#"{"status":"healthy"}"#,
            ),
            ("200 OK", "text/plain", r#"{"status":"healthy"}"#),
        ] {
            let harness = Harness::new();
            let (endpoint, request, server) = readiness_server(status, media_type, body);
            let output = harness
                .command("claude")
                .arg("--litellm")
                .env("AMPLIHACK_LITELLM_ENDPOINT", &endpoint)
                .env("AMPLIHACK_LITELLM_API_KEY", "readiness-secret")
                .output()
                .expect("run failing readiness case");
            let _ = request.recv_timeout(Duration::from_secs(4));
            server.join().expect("join readiness double");
            assert_stable_error(
                &output,
                "AH_LITELLM_READINESS",
                &[&endpoint, "readiness-secret", body],
            );
            assert!(
                harness.child_record().is_empty(),
                "failed readiness contract spawned child"
            );
        }
    }

    #[test]
    fn unsupported_targets_and_modes_fail_before_readiness_or_spawn() {
        for (target, extra) in [
            ("codex", Vec::<&str>::new()),
            ("amplifier", Vec::<&str>::new()),
            ("claude", vec!["--docker"]),
            ("claude", vec!["--auto"]),
        ] {
            let harness = Harness::new();
            let mut command = harness.command(target);
            command
                .arg("--litellm")
                .args(extra)
                .env("AMPLIHACK_LITELLM_ENDPOINT", "https://unsupported.example")
                .env("AMPLIHACK_LITELLM_API_KEY", "unsupported-secret");
            let output = command.output().expect("run unsupported route");
            assert_stable_error(
                &output,
                "AH_LITELLM_UNSUPPORTED",
                &["unsupported.example", "unsupported-secret"],
            );
            assert!(
                harness.child_record().is_empty(),
                "unsupported target or mode spawned child: {target}"
            );
        }
    }
}

/// Path to the compiled amplihack binary.
fn amplihack_bin() -> PathBuf {
    // Cargo builds the binary as a prerequisite of this [[test]] target and
    // exposes its exact path here, honouring the active profile / target dir.
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

/// Assert that a Command produces the expected exit status.
fn assert_exit(cmd: &mut Command, expect_success: bool) {
    cmd.env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to run command: {e}"));
    if expect_success {
        assert!(status.success(), "Expected success, got: {status}");
    } else {
        assert!(!status.success(), "Expected failure, got: {status}");
    }
}

// ---------------------------------------------------------------------------
// --help and --version smoke tests
// ---------------------------------------------------------------------------

#[test]
fn help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("--help"), true);
}

#[test]
fn version_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("--version"), true);
}

#[test]
fn version_subcommand_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("version"), true);
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("totally-unknown-subcommand"), false);
}

// ---------------------------------------------------------------------------
// Plugin subcommand help
// ---------------------------------------------------------------------------

#[test]
fn plugin_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["plugin", "--help"]), true);
}

#[test]
fn memory_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["memory", "--help"]), true);
}

#[test]
fn recipe_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["recipe", "--help"]), true);
}

#[test]
fn mode_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["mode", "--help"]), true);
}

// ---------------------------------------------------------------------------
// Version output content
// ---------------------------------------------------------------------------

#[test]
fn version_output_contains_amplihack() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.to_lowercase().contains("amplihack"),
        "Expected 'amplihack' in version output, got: {combined}"
    );
}
