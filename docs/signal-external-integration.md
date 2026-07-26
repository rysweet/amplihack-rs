# Signal External Service Integration

How the `amplihack signal` onboarding flow talks to the outside world —
`signal-cli`, the local JSON-RPC daemon, and the Azure Linux (`azlin` / `az`)
fleet — and how those integrations are made **testable, resumable, and
secure**.

This is the developer- and operator-facing companion to the two user guides:

- [Signal Onboarding](SIGNAL_ONBOARDING.md) — the `setup` / `distribute` CLI.
- [Signal Channel](signal-channel.md) — the runtime channel, config schema, and
  trust boundary.

Read this document when you need to understand the integration architecture,
extend a service adapter, write tests against the external boundaries, or audit
the security invariants (issues **#971**, **#972**).

---

## Contents

- [Overview](#overview)
- [The seam boundary](#the-seam-boundary)
  - [`SignalCliInvoker`](#signalcliinvoker)
  - [`LinkSession`](#linksession)
  - [`Clock`](#clock)
  - [`VmLister`](#vmlister)
- [Dependency-injection helpers](#dependency-injection-helpers)
- [VM discovery: azlin → az fallback](#vm-discovery-azlin--az-fallback)
- [Fleet distribution core (`--all-vms`)](#fleet-distribution-core---all-vms)
  - [Resumable on-disk state](#resumable-on-disk-state)
  - [Per-VM isolation](#per-vm-isolation)
  - [PARTIAL rollouts](#partial-rollouts)
- [`amplihack-remote` public re-exports](#amplihack-remote-public-re-exports)
- [Security invariants](#security-invariants)
- [Exit-code taxonomy](#exit-code-taxonomy)
- [Configuration and environment](#configuration-and-environment)
- [Testing against the boundaries](#testing-against-the-boundaries)
- [See also](#see-also)

---

## Overview

Signal onboarding drives four external effects:

| Effect | Backing tool | Where |
| --- | --- | --- |
| Locate the CLI | `signal-cli` on `PATH` | host + each VM |
| Device link | `signal-cli link` (device-link URI + QR) | host + each VM |
| Daemon readiness poll | wall-clock pacing | host + each VM |
| Fleet discovery | `azlin list` → `az vm list` fallback | operator workstation |

Every one of these is reached through an **injectable trait seam** rather than a
direct process/cloud call. Production code wires in real implementations; tests
construct fakes and drive them through the **same `&dyn Trait` boundary** the
production wrappers use — with **zero** cloud dependency, no real `signal-cli`,
and no device-link secret ever produced.

The seams live in
[`crates/amplihack-cli/src/commands/signal/seams.rs`](../crates/amplihack-cli/src/commands/signal/seams.rs)
and are compiled only under the `signal` cargo feature.

```text
run_setup(args)                 run_distribute(args)
      │  wires real impls              │  wires real impls
      ▼                                ▼
run_setup_with(&SignalCliBin,    run_distribute_with(&AzVmLister, args)
               &CliLinkSession,          │
               &SystemClock, args)       ▼
      │                            discover_vms ── azlin ──▶ az fallback
      ▼                                  │
 detect ▶ link ▶ daemon ▶ config    per-VM distribute core (resumable, isolated)
```

The **public** entry points `run_setup` / `run_distribute` keep their exact
signatures (they are the stable CLI surface). The injectable `_with` variants
carry the seams and are what tests target.

---

## The seam boundary

All four traits are object-safe (`&dyn Trait`) so a single core can be driven by
either the real adapter or a fake. Errors always surface — no seam silently
degrades a failure into an empty/success result.

### `SignalCliInvoker`

Locates the `signal-cli` binary.

```rust
pub trait SignalCliInvoker {
    /// Return the path to a usable `signal-cli`, or a `SignalCli` error
    /// carrying install guidance. Never silently succeeds without a binary.
    fn detect(&self) -> Result<PathBuf, SignalOpError>;
}

/// Production adapter: inspects `PATH` via the existing detection logic.
pub struct SignalCliBin;
```

A discovery failure returns `SignalOpError::SignalCli(..)` → **exit 4** with a
message telling the operator how to install `signal-cli`.

### `LinkSession`

Runs the interactive `signal-cli link` device-link handshake.

```rust
pub trait LinkSession {
    /// Link a device (named `device_name`, or a default) and return the
    /// linked account in E.164 form (e.g. "+15551234567").
    fn link(&self, signal_cli: &Path, device_name: Option<&str>)
        -> Result<String, SignalOpError>;
}

/// Production adapter over `signal-cli link`.
pub struct CliLinkSession;
```

> **Security (R9).** The device-link URI (`sgnl://linkdevice…`, legacy
> `tsdevice:…`) is a **bearer secret**. Implementations MUST print it to
> **stderr only** and MUST NOT persist or log it anywhere. Only the resulting
> **E.164 account** flows onward into config/state. Test fakes must never emit a
> real URI. See [Security invariants](#security-invariants).

### `Clock`

Abstracts the blocking pace of the daemon-readiness poll so tests run instantly
and deterministically. Deliberately minimal (ruthless simplicity):

```rust
pub trait Clock {
    /// Block for `dur`. The real clock sleeps; fakes typically no-op and
    /// record the requested duration/call-count.
    fn sleep(&self, dur: Duration);
}

/// Production adapter backed by `std::thread::sleep`.
pub struct SystemClock;
```

A `FakeClock` used in tests returns immediately from `sleep`, making the
`wait_for_daemon` loop finish without real waiting.

### `VmLister`

Enumerates VM names in an operator resource group for fleet distribution.

```rust
pub trait VmLister {
    /// List VM names in `resource_group`. A discovery failure **propagates**
    /// (no silent empty fallback): it must surface, never masquerade as
    /// "no VMs".
    fn list_vms(&self, resource_group: &str) -> anyhow::Result<Vec<String>>;
}

/// Production adapter: validates the resource group, then shells out to
/// `az vm list --resource-group <rg> --output json` (explicit argv, no shell).
pub struct AzVmLister;
```

`AzVmLister` validates the resource-group name **before** constructing the
command, invokes `az` with an explicit argv array (never `sh -c`), and reuses
the pure `vm_names_from_az_vm_list_json` extractor so the parse rule cannot
drift.

---

## Dependency-injection helpers

Two private helpers hold the orchestration logic; the public functions are thin
wrappers that inject the real adapters.

```rust
// Stable public surface — signatures are frozen.
pub fn run_setup(args: SignalSetupArgs) -> Result<(), SignalOpError> {
    run_setup_with(&SignalCliBin, &CliLinkSession, &SystemClock, args)
}

pub fn run_distribute(args: SignalDistributeArgs) -> Result<(), SignalOpError> {
    run_distribute_with(&AzVmLister, args)
}

// Injectable variants — module-private in `run.rs`; they carry the seams.
fn run_setup_with(
    cli: &dyn SignalCliInvoker,
    link: &dyn LinkSession,
    clock: &dyn Clock,
    args: SignalSetupArgs,
) -> Result<(), SignalOpError>;

fn run_distribute_with(
    lister: &dyn VmLister,
    args: SignalDistributeArgs,
) -> Result<(), SignalOpError>;
```

The `_with` helpers are **private** to `run.rs` — they are not re-exported and
are not part of any test's import surface. What tests *do* reach is the **public
`seams` module** (compiled under `--features signal`), which exposes the
object-safe traits and their pure helpers:

```rust
use amplihack_cli::commands::signal::seams::{
    Clock, SignalCliInvoker, LinkSession, VmLister,
};
```

Tests construct fakes for these traits and exercise them through the exact
`&dyn Trait` boundary the production wrappers pass in. The stable public surface
stays exactly `run_setup` / `run_distribute`; the seam traits are the injectable
extension point.

---

## VM discovery: azlin → az fallback

Discovery prefers `azlin` (the Azure Linux fleet tool) and falls back to the
generic `az vm list`. The rule is a single pure combinator so it is trivially
testable and cannot silently swallow a total failure.

```rust
/// Non-empty azlin Ok → used as-is (fallback never runs).
/// Empty Ok OR Err → run the `az` fallback.
/// Both fail → Err (a total discovery failure surfaces; never an empty fleet).
pub fn resolve_vm_list<F>(
    azlin: anyhow::Result<Vec<String>>,
    az_fallback: F,
) -> anyhow::Result<Vec<String>>
where
    F: FnOnce() -> anyhow::Result<Vec<String>>;
```

The name extractors are pure and tolerant — malformed or non-array input yields
an empty `Vec` (which then triggers the fallback rather than an error at the
parse layer):

```rust
pub fn vm_names_from_azlin_json(json: &str) -> Vec<String>;
pub fn vm_names_from_az_vm_list_json(json: &str) -> Vec<String>;
```

> **Untrusted input (R11).** VM names returned by discovery are treated as
> **untrusted**. Before any name is used to construct a command it is
> re-validated with `validate::validate_vm_name`. A discovered name that fails
> validation is rejected, not sanitized-and-run.

---

## Fleet distribution core (`--all-vms`)

`signal setup --all-vms --resource-group <rg>` and `signal distribute
--resource-group <rg>` route through the **same** shared distribute core. The
`--all-vms` flag is an alias that runs host onboarding first, then hands off to
`run_distribute` with the equivalent arguments — there is one rollout
implementation, not two.

```bash
# These are equivalent:
amplihack signal setup --all-vms --resource-group my-rg
amplihack signal distribute --resource-group my-rg
```

### Resumable on-disk state

The rollout records per-VM progress so an interrupted run resumes instead of
restarting. State lives in a JSON file written with owner-only `0600`
permissions (it is secrets-adjacent) using an atomic temp-write + `fsync` +
rename.

```rust
/// On-disk schema version. A file claiming a HIGHER version is refused
/// (upgrade amplihack) — forward-compat is never assumed.
pub const SCHEMA_VERSION: u32;

pub enum VmStatus {
    Pending,        // never attempted (or reset)
    Linking,        // device linking in progress
    Linked,         // device linked, daemon not yet confirmed
    DaemonRunning,  // daemon up, config not yet written
    ConfigWritten,  // fully onboarded — the single TERMINAL success state
    Failed,         // reason recorded; retried on resume
}

pub struct DistributeState { /* version + per-VM records */ }

impl DistributeState {
    pub fn resumable_targets(&self, all: &[String]) -> Vec<String>; // non-terminal
    pub fn succeeded_count(&self) -> usize;
    pub fn failed_count(&self) -> usize;
    pub fn save(&self, path: &Path) -> anyhow::Result<()>; // atomic, 0600
    pub fn load(path: &Path) -> anyhow::Result<Self>;       // refuses newer schema
}
```

Key rules:

- **`ConfigWritten` is the only terminal-success state.** A resumed run skips
  VMs at `ConfigWritten` and retries every other status (`Pending`, `Linking`,
  `Linked`, `DaemonRunning`, `Failed`).
- `--force` re-attempts VMs already at `ConfigWritten`.
- Loading a state file whose `version` exceeds `SCHEMA_VERSION` is **refused**
  with an explicit error — never best-effort parsed.

### Per-VM isolation

Each VM is onboarded independently. A failure on one VM records that VM as
`Failed` (with a reason) and moves on; it never corrupts a sibling's record and
never aborts the remaining rollout. State is persisted after each VM transition
so a crash mid-fleet loses at most the in-flight VM.

### PARTIAL rollouts

Whenever **one or more** target VMs end `Failed`, the run finishes as a
**PARTIAL** outcome:

```text
SignalOpError::Partial { succeeded, total, failures }  →  exit code 5
```

- `succeeded` — count of VMs that reached `ConfigWritten` this run.
- `total` — count of VMs targeted this run.
- `failures` — `Vec<(vm_name, reason)>` for each failed VM.

Exit **5** is the distinct signal for "at least one VM failed" so automation can
branch on it (retry the failures, alert, etc.). Any run with a non-empty
`failures` set returns `Partial` — **including the all-failed case**, where
`succeeded` is `0` (`run_distribute_with` returns `Partial` whenever
`failures.is_empty()` is false). A rollout where every VM succeeds exits `0`.

---

## `amplihack-remote` public re-exports

Idle-based device linking and fleet discovery reuse building blocks from
`amplihack-remote`. Step 8b promotes the required helpers to the crate's public
API (see
[`crates/amplihack-remote/src/lib.rs`](../crates/amplihack-remote/src/lib.rs)):

```rust
// Idle-watchdog re-exported from amplihack-utils.
pub use amplihack_utils::idle_watchdog;

// Azlin fleet-listing parsers (JSON + human-readable text forms).
pub use azlin_parse::{parse_azlin_list_json, parse_azlin_list_text};
```

### `parse_azlin_list_json` / `parse_azlin_list_text`

Pure parsers over `azlin list` output. Both return `Vec<VM>` (the crate-public
`amplihack_remote::VM`). Malformed or non-array input yields an empty `Vec` — a
parse failure is never an error at this layer, matching the discovery
combinator's contract.

```rust
use amplihack_remote::{parse_azlin_list_json, parse_azlin_list_text};

let vms = parse_azlin_list_json(azlin_json_output); // [] on malformed input
```

### Shell / session safety

Remote command construction is **fail-closed**: session identifiers are
validated before interpolation and invalid values are **rejected** rather than
lossy-sanitized into a different command target. The remote executor keeps the
validator internal (`amplihack_remote` does not re-export a "quote-like"
transform), so callers cannot mistake a destructive stripper for a safe quoter.

The primary defense remains **explicit argv arrays with no `sh -c`** wherever an
external process is spawned directly, plus validate-before-construct for the
small remote script fragments that must be sent to `azlin connect`.

### `idle_watchdog`

Re-exported from `amplihack-utils` for the idle-based linking flow. Reachable as
`amplihack_remote::idle_watchdog`.

---

## Security invariants

The integration layer holds these invariants; each is covered by tests.

| # | Invariant | Enforcement |
| --- | --- | --- |
| Device-link URI is secret | `sgnl://`, `tsdevice:` printed to **stderr only**, never persisted/logged | `LinkSession` returns E.164 only; state/config files audited for URI substrings |
| Only the account persists | Written config/state contains the **E.164** account, never a URI | invariant test greps every written file |
| No shell | External calls use explicit argv arrays where possible; remote script fragments validate inputs before interpolation | argv-only adapters; fail-closed session validation |
| Validate before construct | VM name, resource group, account, device name, endpoint/port validated first | `validate::*` allowlists |
| Discovery is untrusted | VM names from `azlin`/`az` re-validated before use | `validate_vm_name` on every discovered name |
| Secrets-adjacent files are private | Config + rollout state written `0600` (atomic temp + fsync + rename) | `fsutil::write_private` |
| Loopback-only daemon | Daemon endpoint must bind loopback; routable/wildcard refused | single `validate_loopback_endpoint` choke-point → exit 6 |
| No forward-compat guessing | State file with a newer schema version is refused | `DistributeState::load` |
| Failures surface | No seam degrades an error into empty/success | traits return `Result`; `resolve_vm_list` Err on total failure |

---

## Exit-code taxonomy

`SignalOpError` maps to stable process exit codes so tooling can branch on
outcomes:

| Variant | Exit | Meaning |
| --- | --- | --- |
| `Usage` | `2` | Bad flags / arguments |
| `Unsupported` | `3` | Feature disabled, or unsupported identity mode |
| `SignalCli` | `4` | `signal-cli` missing / failed |
| `Partial` | `5` | Fleet rollout partially succeeded (some VMs failed) |
| `Daemon` | `6` | Daemon start / loopback-endpoint failure |
| `Link` | `7` | Device-linking failure |
| _(success)_ | `0` | Host onboarded, or every VM reached `ConfigWritten` |

---

## Configuration and environment

The integration honors the same knobs as the CLI:

| Setting | Source | Notes |
| --- | --- | --- |
| Daemon endpoint | `--endpoint` / `--port` / `AMPLIHACK_SIGNAL_ENDPOINT` | `--port` > `--endpoint` > env; default `127.0.0.1:7583`; loopback-only |
| Device name | `--device-name` | default `amplihack-<hostname>` |
| Resource group | `--resource-group` | required for `distribute` / `--all-vms` |
| Explicit VM list | `--vms` (comma-separated) | bypasses `azlin`/`az` discovery |
| Rollout concurrency | `--concurrency` | Accepted but **not honored** for interactive device-linking: each VM emits a distinct QR a human must scan, so onboarding is always sequential. A value `>1` is announced-and-ignored (never silently dropped), reserved for future non-interactive rollout phases |
| Force | `--force` | rewrite config / retry terminal-success VMs; **never** re-links an already linked device |

Discovery uses the Azure toolchain's own auth (`azlin` / `az`); no credentials
are handled in-process.

---

## Testing against the boundaries

Because every external effect is a seam, each boundary is testable with fakes
and no real services. The seam-injection contract constructs fakes and drives
them through the same `&dyn Trait` boundary production uses. Sketch:

```rust
#![cfg(feature = "signal")]
use amplihack_cli::commands::signal::error::SignalOpError;
use amplihack_cli::commands::signal::seams::{Clock, LinkSession, SignalCliInvoker};

struct FakeSignalCli(PathBuf);
impl SignalCliInvoker for FakeSignalCli {
    fn detect(&self) -> Result<PathBuf, SignalOpError> { Ok(self.0.clone()) }
}

struct FakeLinkSession { account: String }
impl LinkSession for FakeLinkSession {
    // Returns an E.164 account and NEVER emits a device-link URI.
    fn link(&self, _cli: &Path, _name: Option<&str>) -> Result<String, SignalOpError> {
        Ok(self.account.clone())
    }
}

struct FakeClock; // sleep() is a no-op → deterministic, instant poll
impl Clock for FakeClock { fn sleep(&self, _d: Duration) {} }

// Inject through the object-safe boundary and assert the contract:
let seam: &dyn SignalCliInvoker = &FakeSignalCli("/usr/bin/signal-cli".into());
assert_eq!(seam.detect().unwrap(), PathBuf::from("/usr/bin/signal-cli"));

let link: &dyn LinkSession = &FakeLinkSession { account: "+15551234567".into() };
let account = link.link(Path::new("/usr/bin/signal-cli"), Some("amplihack-host"))?;
assert_eq!(account, "+15551234567"); // E.164 only — no "sgnl://" / "tsdevice:"
```

> The `run_setup_with` / `run_distribute_with` orchestrators are module-private,
> so integration tests cannot call them directly. They validate the seam
> **contract** (injectability, object-safety, error surfacing, no-URI); the
> private wrappers are covered by the in-crate host/fleet paths.

Contract tests that freeze this boundary:

- `crates/amplihack-cli/tests/signal_seams_injection.rs` — DI of `FakeSignalCli`
  / `FakeLinkSession` / `FakeClock` through `&dyn`; asserts a detection failure
  surfaces `SignalCli` (exit 4) and that linking returns E.164 only (no
  device-link URI).
- `crates/amplihack-remote/tests/azlin_parse_public_api.rs` — parsers callable
  as public API; malformed → empty `Vec`; valid input extracts VM names.

Run the Signal suites:

```bash
# CLI-side seams + contracts (feature-gated).
cargo test -p amplihack-cli --features signal --test 'signal_*'

# Remote-side public-API contracts.
cargo test -p amplihack-remote --test azlin_parse_public_api
```

---

## See also

- [Signal Onboarding](SIGNAL_ONBOARDING.md) — the `setup` / `distribute` CLI and
  fleet workflow.
- [Signal Channel](signal-channel.md) — runtime channel, config schema, security
  model, per-session wiring.
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated
  config the onboarding flow writes.
- Related issues: **#921** (onboarding command), **#922** (device linking),
  **#923** (fleet distribution), **#924** (idempotent daemon + config),
  **#971** (external service integration), **#972** (TDD contract tests).
