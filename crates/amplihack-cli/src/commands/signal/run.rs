//! Runtime orchestration for `amplihack signal setup` / `distribute` (#921).
//!
//! This is the **gated I/O shell** around the pure cores in sibling modules. It
//! drives signal-cli (detect, link, daemon), writes the config, and — for the
//! fleet path — fans onboarding out over `azlin` to each VM. All decision logic
//! lives in the pure modules ([`super::setup::plan_setup`],
//! [`super::distribute`]); this file only performs the effects.
//!
//! Policy: the interactive device-link step uses **idle/liveness detection**
//! (we wait for the signal-cli process to finish linking) — there is **no
//! fixed wall-clock timeout** on it. Failures are surfaced explicitly via
//! [`SignalOpError`]; nothing is silently degraded.

use std::io::{BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use amplihack_signal::config::{self, SignalConfig};

use super::distribute::{self, DistributeState, VmStatus};
use super::error::SignalOpError;
use super::seams::{AzVmLister, VmLister};
use super::setup::{self, Probes};
use super::{config_writer, render, validate};
use crate::{SignalDistributeArgs, SignalSetupArgs};

type OpResult<T> = Result<T, SignalOpError>;

// ---------------------------------------------------------------------------
// setup (single host)
// ---------------------------------------------------------------------------

/// Onboard the current host. Idempotent: probes what already exists and repairs
/// only the missing pieces (never re-links an already-linked device).
pub fn run_setup(args: SignalSetupArgs) -> OpResult<()> {
    let endpoint = resolve_endpoint(&args.endpoint)?;
    validate::validate_loopback_endpoint(&endpoint)
        .map_err(|e| SignalOpError::Daemon(e.to_string()))?;

    let signal_cli = detect_signal_cli()?;
    let config_path = default_config_path();

    // --- Probe the three independent facts. -------------------------------
    let existing_account = read_account_from_config(&config_path);
    let probes = Probes {
        linked: existing_account
            .as_deref()
            .map(|acct| is_linked(&signal_cli, acct))
            .unwrap_or(false),
        daemon_running: is_daemon_running(&endpoint),
        config_written: existing_account.is_some(),
    };
    let plan = setup::plan_setup(probes, args.force);
    eprintln!(
        "signal setup: linked={} daemon_running={} config_written={} → plan {plan:?}",
        probes.linked, probes.daemon_running, probes.config_written
    );

    // --- Link (only if not already linked). -------------------------------
    let account = if plan.do_link {
        link_device(&signal_cli, args.device_name.as_deref())?
    } else {
        existing_account.ok_or_else(|| {
            SignalOpError::Link("host reports linked but no account is recorded".into())
        })?
    };

    // --- Start the local daemon. ------------------------------------------
    if plan.do_start_daemon {
        start_daemon(&signal_cli, &account, &endpoint)?;
    } else {
        eprintln!("signal setup: local daemon already running on {endpoint}");
    }

    // --- Write the config. ------------------------------------------------
    if plan.do_write_config {
        write_config(&config_path, &endpoint, &account)?;
        eprintln!("signal setup: wrote {}", config_path.display());
    } else {
        eprintln!(
            "signal setup: config already present at {}",
            config_path.display()
        );
    }

    eprintln!("signal setup: done. Account {account} is ready on {endpoint}.");

    // Optional: chain into a fleet rollout.
    if args.all_vms {
        let rg = args
            .resource_group
            .clone()
            .ok_or_else(|| SignalOpError::Usage("--all-vms requires --resource-group".into()))?;
        return run_distribute(SignalDistributeArgs {
            resource_group: rg,
            vms: None,
            endpoint: args.endpoint,
            concurrency: None,
            force: false,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// distribute (fleet)
// ---------------------------------------------------------------------------

/// Roll onboarding out across a fleet. Resumable and per-VM isolated: state is
/// persisted after every VM, one VM failing never aborts the others, and a
/// re-run retries only non-terminal hosts.
pub fn run_distribute(args: SignalDistributeArgs) -> OpResult<()> {
    validate::validate_resource_group(&args.resource_group)
        .map_err(|e| SignalOpError::Usage(e.to_string()))?;
    let endpoint = resolve_endpoint(&args.endpoint)?;
    validate::validate_loopback_endpoint(&endpoint)
        .map_err(|e| SignalOpError::Daemon(e.to_string()))?;

    let azlin = detect_azlin()?;
    let state_path = distribute_state_path();
    let mut state = if state_path.exists() {
        DistributeState::load(&state_path)
            .map_err(|e| SignalOpError::Usage(format!("cannot load rollout state: {e}")))?
    } else {
        DistributeState::new()
    };

    // Discover the fleet (explicit list or `az vm list`), then choose targets.
    let all_vms = discover_vms(&args)?;
    let targets: Vec<String> = if args.force {
        all_vms.clone()
    } else {
        distribute::plan_rollout(&PreListed(all_vms.clone()), &state, &args.resource_group)
            .map_err(|e| SignalOpError::Usage(e.to_string()))?
    };

    if targets.is_empty() {
        eprintln!(
            "signal distribute: nothing to do (all {} VM(s) already onboarded)",
            all_vms.len()
        );
        return Ok(());
    }
    eprintln!(
        "signal distribute: {} target VM(s) of {} in {} (concurrency {})",
        targets.len(),
        all_vms.len(),
        args.resource_group,
        args.concurrency.unwrap_or(1).max(1)
    );

    let state_mtx = Mutex::new(&mut state);
    let concurrency = args.concurrency.unwrap_or(1).max(1);
    let outcomes = run_bounded(&targets, concurrency, |vm| {
        // Mark in-flight and persist so a crash mid-rollout is resumable.
        {
            let mut st = state_mtx.lock().unwrap();
            st.upsert(vm, VmStatus::Linking, None);
            let _ = st.save(&state_path);
        }
        let result = onboard_remote_vm(&azlin, vm, &args.resource_group, &endpoint);
        let mut st = state_mtx.lock().unwrap();
        match &result {
            Ok(()) => st.upsert(vm, VmStatus::ConfigWritten, None),
            Err(reason) => st.upsert(vm, VmStatus::Failed, Some(reason.clone())),
        }
        let _ = st.save(&state_path);
        result
    });

    // Persist final state.
    state
        .save(&state_path)
        .map_err(|e| SignalOpError::Usage(format!("cannot save rollout state: {e}")))?;

    let failures: Vec<(String, String)> = outcomes
        .into_iter()
        .filter_map(|(vm, r)| r.err().map(|reason| (vm, reason)))
        .collect();
    let succeeded = targets.len() - failures.len();

    eprintln!(
        "signal distribute: {succeeded}/{} onboarded, {} failed. State: {}",
        targets.len(),
        failures.len(),
        state_path.display()
    );
    for (vm, reason) in &failures {
        eprintln!("  FAILED {vm}: {reason}");
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(SignalOpError::Partial {
            succeeded,
            total: targets.len(),
            failures,
        })
    }
}

/// A [`VmLister`] over an already-materialized list (so planning reuses the
/// resumable-target logic without a second discovery call).
struct PreListed(Vec<String>);
impl VmLister for PreListed {
    fn list_vms(&self, _rg: &str) -> anyhow::Result<Vec<String>> {
        Ok(self.0.clone())
    }
}

/// Resolve the desired VM set from explicit `--vms` or `az vm list`. Every name
/// is validated (injection defense) before use.
fn discover_vms(args: &SignalDistributeArgs) -> OpResult<Vec<String>> {
    let names = match &args.vms {
        Some(csv) => csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        None => AzVmLister
            .list_vms(&args.resource_group)
            .map_err(|e| SignalOpError::Usage(format!("VM discovery failed: {e}")))?,
    };
    for name in &names {
        validate::validate_vm_name(name)
            .map_err(|e| SignalOpError::Usage(format!("invalid VM name from discovery: {e}")))?;
    }
    Ok(names)
}

/// Run `f` over `targets` with bounded concurrency, returning `(vm, result)`
/// pairs. Concurrency of 1 runs sequentially (the sane default for interactive
/// per-VM linking).
fn run_bounded<F>(targets: &[String], concurrency: usize, f: F) -> Vec<(String, Result<(), String>)>
where
    F: Fn(&str) -> Result<(), String> + Sync,
{
    let mut out = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(concurrency.max(1)) {
        let results: Vec<(String, Result<(), String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|vm| scope.spawn(|| (vm.clone(), f(vm))))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        out.extend(results);
    }
    out
}

/// Onboard one remote VM by invoking `amplihack signal setup` on it over azlin.
/// Each VM becomes its OWN linked device (Signal-native identity model).
fn onboard_remote_vm(
    azlin: &Path,
    vm: &str,
    resource_group: &str,
    endpoint: &str,
) -> Result<(), String> {
    validate::validate_vm_name(vm).map_err(|e| e.to_string())?;
    // The remote host runs the same onboarding; its interactive QR is streamed
    // back to this terminal so the operator can scan it for that VM.
    let remote_cmd = format!(
        "amplihack signal setup --endpoint {} --device-name amplihack-{}",
        shell_quote(endpoint),
        shell_quote(vm)
    );
    eprintln!("--- onboarding {vm} (scan its QR when prompted) ---");
    let status = Command::new(azlin)
        .args([
            "connect",
            vm,
            "--resource-group",
            resource_group,
            "--no-tmux",
            "-y",
            "--",
            &remote_cmd,
        ])
        .status()
        .map_err(|e| format!("failed to invoke azlin: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("remote onboarding exited with {status}"))
    }
}

// ---------------------------------------------------------------------------
// signal-cli effects
// ---------------------------------------------------------------------------

/// Locate signal-cli, or fail with actionable install guidance (exit 4).
fn detect_signal_cli() -> OpResult<PathBuf> {
    if let Some(p) = which("signal-cli") {
        return Ok(p);
    }
    Err(SignalOpError::SignalCli(
        "signal-cli not found on PATH. Install it, then re-run `amplihack signal setup`.\n\
         Linux (recommended): download the latest release from\n\
         https://github.com/AsamK/signal-cli/releases and place `signal-cli` on your PATH,\n\
         or use your package manager (e.g. `brew install signal-cli` on macOS).\n\
         signal-cli requires a Java 21+ runtime."
            .into(),
    ))
}

/// Run `signal-cli link`, render the emitted URI as a QR (with raw-URI
/// fallback), and block on **idle detection** — waiting for the link to
/// complete — with no wall-clock cap. Returns the linked account (E.164).
fn link_device(signal_cli: &Path, device_name: Option<&str>) -> OpResult<String> {
    let name = device_name
        .map(str::to_string)
        .unwrap_or_else(default_device_name);
    validate::validate_device_name(&name).map_err(|e| SignalOpError::Usage(e.to_string()))?;

    let mut child = Command::new(signal_cli)
        .args(["link", "-n", &name])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| SignalOpError::Link(format!("failed to spawn `signal-cli link`: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SignalOpError::Link("signal-cli produced no stdout".into()))?;

    let mut account: Option<String> = None;
    let mut rendered = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|e| SignalOpError::Link(format!("reading signal-cli: {e}")))?;
        let trimmed = line.trim();
        if !rendered && (trimmed.starts_with("sgnl://") || trimmed.starts_with("tsdevice:")) {
            println!("{}", render::render_link(trimmed));
            println!("Waiting for you to approve the link on your phone…");
            rendered = true;
        } else if let Some(rest) = trimmed.strip_prefix("Associated with:") {
            account = Some(rest.trim().to_string());
        }
    }

    // Idle detection: the process exits once linking finishes (approved) or is
    // aborted. No fixed timeout is imposed on the human step.
    let status = child
        .wait()
        .map_err(|e| SignalOpError::Link(format!("waiting for signal-cli: {e}")))?;
    if !status.success() {
        return Err(SignalOpError::Link(format!(
            "`signal-cli link` exited with {status} (linked-device limit reached, or aborted)"
        )));
    }

    account.ok_or_else(|| {
        SignalOpError::Link(
            "linking finished but signal-cli did not report the associated account".into(),
        )
    })
}

/// Start the signal-cli JSON-RPC daemon bound to `endpoint`, preferring a
/// `systemd --user` transient unit and falling back to a detached process.
fn start_daemon(signal_cli: &Path, account: &str, endpoint: &str) -> OpResult<()> {
    let cli = signal_cli.to_string_lossy().to_string();
    if which("systemd-run").is_some() && systemd_user_available() {
        let status = Command::new("systemd-run")
            .args([
                "--user",
                "--unit",
                "amplihack-signal-daemon",
                "--collect",
                "--",
                &cli,
                "-a",
                account,
                "daemon",
                "--tcp",
                endpoint,
            ])
            .status()
            .map_err(|e| SignalOpError::Daemon(format!("systemd-run failed to spawn: {e}")))?;
        if status.success() {
            eprintln!("signal setup: started daemon via systemd --user (amplihack-signal-daemon)");
            return wait_for_daemon(endpoint);
        }
        eprintln!("signal setup: systemd-run failed; falling back to a detached process");
    }

    // Detached fallback: setsid + nohup semantics via a double-fork-free spawn.
    Command::new(signal_cli)
        .args(["-a", account, "daemon", "--tcp", endpoint])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SignalOpError::Daemon(format!("failed to spawn signal-cli daemon: {e}")))?;
    eprintln!("signal setup: started detached signal-cli daemon on {endpoint}");
    wait_for_daemon(endpoint)
}

/// Poll the daemon endpoint until it accepts a TCP connection. This is a local
/// readiness check (loopback connect is immediate); it is not an interactive
/// step, so a short bounded retry loop is appropriate.
fn wait_for_daemon(endpoint: &str) -> OpResult<()> {
    for _ in 0..50 {
        if is_daemon_running(endpoint) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(SignalOpError::Daemon(format!(
        "daemon did not become reachable on {endpoint}"
    )))
}

/// Write the onboarding config atomically, `0600`.
fn write_config(path: &Path, endpoint: &str, account: &str) -> OpResult<()> {
    validate::validate_account(account).map_err(|e| SignalOpError::Usage(e.to_string()))?;
    let toml = config_writer::to_toml(endpoint, account);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SignalOpError::Usage(format!("create config dir: {e}")))?;
    }
    write_private(path, toml.as_bytes())
        .map_err(|e| SignalOpError::Usage(format!("write config: {e}")))
}

// ---------------------------------------------------------------------------
// probes
// ---------------------------------------------------------------------------

/// Whether signal-cli reports a linked device for `account`.
fn is_linked(signal_cli: &Path, account: &str) -> bool {
    Command::new(signal_cli)
        .args(["-a", account, "listDevices"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether the JSON-RPC daemon accepts a TCP connection at `endpoint`.
fn is_daemon_running(endpoint: &str) -> bool {
    match endpoint.to_socket_addrs() {
        Ok(mut addrs) => addrs.any(|a| TcpStream::connect(a).is_ok()),
        Err(_) => false,
    }
}

/// Read the `account` field from an existing config (env-independent).
fn read_account_from_config(path: &Path) -> Option<String> {
    let empty = std::collections::HashMap::new();
    let toml = config::resolve_config_source(&empty, path).ok().flatten()?;
    SignalConfig::from_sources(&empty, Some(&toml))
        .ok()
        .map(|c| c.account)
}

// ---------------------------------------------------------------------------
// paths / helpers
// ---------------------------------------------------------------------------

fn amplihack_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_config_path() -> PathBuf {
    config::default_config_path_in(&amplihack_home())
}

fn distribute_state_path() -> PathBuf {
    amplihack_home()
        .join(".amplihack")
        .join("signal-distribute-state.json")
}

/// Resolve the endpoint, letting `AMPLIHACK_SIGNAL_ENDPOINT` override the flag.
fn resolve_endpoint(flag: &str) -> OpResult<String> {
    Ok(std::env::var("AMPLIHACK_SIGNAL_ENDPOINT").unwrap_or_else(|_| flag.to_string()))
}

fn default_device_name() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "host".to_string());
    let sanitized: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("amplihack-{sanitized}")
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(bin);
        candidate.is_file().then_some(candidate)
    })
}

fn detect_azlin() -> OpResult<PathBuf> {
    if let Some(p) = std::env::var_os("AZLIN_PATH") {
        return Ok(PathBuf::from(p));
    }
    which("azlin").ok_or_else(|| {
        SignalOpError::Usage(
            "azlin not found. Set AZLIN_PATH or install azlin (https://github.com/rysweet/azlin)."
                .into(),
        )
    })
}

fn systemd_user_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Single-quote a value for safe embedding in the remote shell command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Write bytes with `0600` on Unix (create-time mode, umask-independent).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}
