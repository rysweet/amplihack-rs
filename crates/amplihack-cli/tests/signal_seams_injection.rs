//! Seam-injection contract (#921/#971 R3): the host-onboarding I/O effects are
//! abstracted behind the public, object-safe `SignalCliInvoker`, `LinkSession`,
//! and `Clock` traits so `run_setup` can be driven with fakes — no real
//! `signal-cli`, no real device-link handshake, no real sleeping.
//!
//! These tests fail to *compile* if the seams regress (not `pub`, or not
//! object-safe), and assert the fakes are injectable through `&dyn` — the same
//! boundary the production wrappers use. Fakes deliberately emit **no** real
//! device-link URI (R9) and perform no external I/O.
#![cfg(feature = "signal")]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use amplihack_cli::commands::signal::error::SignalOpError;
use amplihack_cli::commands::signal::seams::{Clock, LinkSession, SignalCliInvoker, VmLister};
use amplihack_cli::commands::signal::{run_distribute_with, run_setup_with};
use amplihack_cli::{SignalDistributeArgs, SignalSetupArgs};

static ENV_LOCK: Mutex<()> = Mutex::new(());

// --- Fakes: injected only in tests; never touch the network/process/device. --

struct FakeSignalCli(PathBuf);
impl SignalCliInvoker for FakeSignalCli {
    fn detect(&self) -> Result<PathBuf, SignalOpError> {
        Ok(self.0.clone())
    }
}

struct FakeVmLister {
    names: Vec<String>,
    calls: Cell<u32>,
}

impl VmLister for FakeVmLister {
    fn list_vms(&self, _resource_group: &str) -> anyhow::Result<Vec<String>> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.names.clone())
    }
}

struct MissingSignalCli;
impl SignalCliInvoker for MissingSignalCli {
    fn detect(&self) -> Result<PathBuf, SignalOpError> {
        Err(SignalOpError::SignalCli(
            "signal-cli not found on PATH".into(),
        ))
    }
}

/// A fake linker that returns a canned E.164 and, critically, emits NO
/// device-link URI (the real secret stays stderr-only in production; a fake
/// must never manufacture one).
struct FakeLinkSession {
    account: String,
    last_device: Cell<Option<String>>,
}
impl LinkSession for FakeLinkSession {
    fn link(&self, _signal_cli: &Path, device_name: Option<&str>) -> Result<String, SignalOpError> {
        self.last_device
            .set(device_name.map(str::to_string).or(Some("<default>".into())));
        Ok(self.account.clone())
    }
}

/// A non-blocking clock that records how often (and how long) it was asked to
/// sleep, keeping time deterministic and instant.
struct FakeClock {
    calls: Cell<u32>,
    total: Cell<Duration>,
}
impl FakeClock {
    fn new() -> Self {
        FakeClock {
            calls: Cell::new(0),
            total: Cell::new(Duration::ZERO),
        }
    }
}
impl Clock for FakeClock {
    fn sleep(&self, dur: Duration) {
        self.calls.set(self.calls.get() + 1);
        self.total.set(self.total.get() + dur);
    }
}

#[test]
fn signal_cli_invoker_is_injectable_as_dyn() {
    let fake = FakeSignalCli(PathBuf::from("/opt/signal-cli/bin/signal-cli"));
    let seam: &dyn SignalCliInvoker = &fake;
    assert_eq!(
        seam.detect().unwrap(),
        PathBuf::from("/opt/signal-cli/bin/signal-cli")
    );
}

#[test]
fn signal_cli_detection_failure_surfaces_signalcli_error() {
    let seam: &dyn SignalCliInvoker = &MissingSignalCli;
    let err = seam.detect().unwrap_err();
    // Never silently degrades to a bogus path; surfaces the taxonomy error.
    assert_eq!(err.exit_code(), 4);
}

#[test]
fn link_session_returns_account_and_emits_no_uri() {
    let fake = FakeLinkSession {
        account: "+15551230000".into(),
        last_device: Cell::new(None),
    };
    let seam: &dyn LinkSession = &fake;
    let account = seam
        .link(Path::new("/opt/signal-cli"), Some("amplihack-host"))
        .unwrap();
    assert_eq!(account, "+15551230000");
    assert_eq!(fake.last_device.take(), Some("amplihack-host".to_string()));
}

#[test]
fn clock_seam_is_injectable_and_deterministic() {
    let clock = FakeClock::new();
    let seam: &dyn Clock = &clock;
    seam.sleep(Duration::from_millis(200));
    seam.sleep(Duration::from_millis(200));
    assert_eq!(clock.calls.get(), 2);
    assert_eq!(clock.total.get(), Duration::from_millis(400));
}

#[test]
fn run_setup_with_uses_injected_cli_and_fails_before_linking_when_cli_missing() {
    let cli = MissingSignalCli;
    let linker = FakeLinkSession {
        account: "+15551230000".into(),
        last_device: Cell::new(None),
    };
    let clock = FakeClock::new();
    let args = SignalSetupArgs {
        endpoint: Some("127.0.0.1:7583".into()),
        port: None,
        device_name: Some("amplihack-test".into()),
        force: false,
        all_vms: false,
        resource_group: None,
    };

    let err = run_setup_with(&cli, &linker, &clock, args).unwrap_err();
    assert_eq!(err.exit_code(), 4);
    assert_eq!(linker.last_device.take(), None);
    assert_eq!(clock.calls.get(), 0);
}

#[test]
fn run_distribute_with_uses_injected_vm_lister_for_empty_fleet_without_external_az() {
    let _guard = ENV_LOCK.lock().unwrap();
    let home = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("signal-seams-injection-home");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let old_home = std::env::var_os("HOME");
    let old_azlin = std::env::var_os("AZLIN_PATH");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("AZLIN_PATH", home.join("fake-azlin-not-used"));
    }

    let lister = FakeVmLister {
        names: Vec::new(),
        calls: Cell::new(0),
    };
    let args = SignalDistributeArgs {
        resource_group: "rg-test".into(),
        vms: None,
        endpoint: Some("127.0.0.1:7583".into()),
        port: None,
        concurrency: None,
        force: false,
    };

    let result = run_distribute_with(&lister, args);

    unsafe {
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_azlin {
            Some(v) => std::env::set_var("AZLIN_PATH", v),
            None => std::env::remove_var("AZLIN_PATH"),
        }
    }
    let _ = std::fs::remove_dir_all(&home);

    result.unwrap();
    assert_eq!(lister.calls.get(), 1);
}
