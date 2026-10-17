# Signal-Enabled Release Builds

How amplihack's official release and snapshot binaries ship with the
[Signal channel](signal-channel.md) compiled **in**, so that an azlin fleet
obtains a Signal-capable `amplihack-hooks` binary automatically from
install/bootstrap/deploy — with **no source rebuild and no `--features signal`
opt-in** required on the target host.

> **TL;DR.** Every published archive (all targets are Unix; no Windows build
> is produced) contains **two** Signal-enabled binaries: `amplihack` and
> `amplihack-hooks`. The `signal` Cargo feature is enabled at the CI/release
> **build layer**; library defaults are unchanged.

- [Background: why this exists](#background-why-this-exists)
- [What ships](#what-ships)
- [How the fleet obtains a Signal-enabled binary](#how-the-fleet-obtains-a-signal-enabled-binary)
- [The build commands](#the-build-commands)
- [Target coverage](#target-coverage)
- [Verifying a release binary is Signal-enabled](#verifying-a-release-binary-is-signal-enabled)
- [What did *not* change](#what-did-not-change)
- [Rationale & decision record](#rationale--decision-record)
- [Reference](#reference)

---

## Background: why this exists

The per-session Signal channel is implemented in `amplihack-signal` and wired
into session lifecycle through the `amplihack-hooks` library (SessionStart).
Both the CLI binary (`amplihack`) and the multicall hook binary
(`amplihack-hooks`) are what a host actually runs — and the SessionStart hook
path lives in `amplihack-hooks`.

Historically the release pipeline built the workspace with **default features
only**. The CLI binary (`bins/amplihack`) force-links Signal through its
`amplihack-cli` dependency (`features = ["signal"]`), so `amplihack` was always
Signal-capable. But the hook binary package (`amplihack-hooks-bin`) declares
`signal` as an **off-by-default** feature:

```toml
# bins/amplihack-hooks/Cargo.toml
[features]
default = []
signal = ["amplihack-hooks/signal"]
```

Because the release build never passed `--features amplihack-hooks-bin/signal`,
the **shipped `amplihack-hooks` binary had Signal compiled out**. A fleet
installing an official artifact therefore received a hook binary whose
SessionStart path could not start the Signal channel — the "shipping gap."

Signal-enabled release builds close that gap by turning the feature on **at the
build step**, so the compiled artifact everyone downloads already contains the
Signal code.

---

## What ships

| Artifact set        | Archive                          | Binaries inside                          | Signal in `amplihack` | Signal in `amplihack-hooks` |
| ------------------- | -------------------------------- | ---------------------------------------- | :-------------------: | :-------------------------: |
| Stable release (Unix) | `amplihack-<target>.tar.gz` | `amplihack`, `amplihack-hooks` | ✅ | ✅ |
| Snapshot (Unix targets) | `amplihack-<target>.tar.gz` (uploaded as artifact `snapshot-<target>`) | `amplihack`, `amplihack-hooks` | ✅ | ✅ |

The archive layout, file names, and checksum sidecars
(`amplihack-<target>.tar.gz.sha256`) are unchanged. Only the *contents* of the Unix `amplihack-hooks` binaries changed: they now
link the Signal stack.

---

## How the fleet obtains a Signal-enabled binary

No fleet-side change is required. The existing install/bootstrap/deploy path
already downloads and unpacks the official archive:

1. **Install / bootstrap** pulls `amplihack-<target>.tar.gz` (or the snapshot
   archive) from the GitHub Release assets.
2. The archive is unpacked onto the host; `amplihack` and `amplihack-hooks`
   land on `PATH`.
3. On the next agent session, the `amplihack-hooks` SessionStart path can now
   start the Signal channel because the code is present in the binary.

Because the fix is in the artifact itself, **fleet distribution, install
scripts, and deploy tooling need no edits.** A host that previously received a
Signal-less hook binary now receives a Signal-enabled one simply by upgrading
to a build produced after this change.

> Onboarding the *runtime* (linking a device, starting the local signal-cli
> daemon, writing config) is still handled separately by
> `amplihack signal setup` / `amplihack signal distribute` — see
> [SIGNAL_ONBOARDING.md](SIGNAL_ONBOARDING.md). This document only covers the
> **binary** shipping with Signal compiled in.

---

## The build commands

The feature is enabled by naming both shipped packages explicitly and turning
on the hook package's `signal` feature. At a virtual-workspace root,
`cargo build --features …` requires an explicit package selection, so the
commands use `-p`:

**Stable release** — `.github/workflows/release.yml`, `Build release` step:

```bash
cargo build --release --locked --target ${{ matrix.target }} \
  -p amplihack -p amplihack-hooks-bin \
  --features amplihack-hooks-bin/signal
```

**Snapshot (native)** — `.github/workflows/publish-snapshot.yml`,
`Build (native)` step:

```bash
cargo build --release --locked --target ${{ matrix.target }} \
  -p amplihack -p amplihack-hooks-bin \
  --features amplihack-hooks-bin/signal
```

**Snapshot (cross)** — `.github/workflows/publish-snapshot.yml`,
`Build (cross)` step (future-proofing for `cross: true` matrix entries):

```bash
cross build --release --locked --target ${{ matrix.target }} \
  -p amplihack -p amplihack-hooks-bin \
  --features amplihack-hooks-bin/signal
```

Notes:

- **`-p amplihack -p amplihack-hooks-bin`** builds exactly the two packages
  whose binaries are packaged. The `amplihack` binary is already Signal-enabled
  via its own manifest; keeping it in the selection preserves the artifact.
- **`--features amplihack-hooks-bin/signal`** is a static argument on every
  build. All release targets are Unix, which the hook integration's subscriber
  process management requires.
- **`--locked`** is retained on every command so the newly linked Signal stack
  (`amplihack-signal`, `tokio` net, `libc`) resolves to the pinned versions in
  `Cargo.lock`.
- The **Package** step is unchanged: it still copies
  `target/<target>/release/amplihack` and
  `target/<target>/release/amplihack-hooks` into `dist/`.

---

## Target coverage

The CLI and hook binaries have different Signal portability characteristics:

- `amplihack` links the portable `amplihack-signal` stack through
  `amplihack-cli/signal` and is built for every release target.
- `amplihack-hooks` links the hook-side Signal integration only for Unix
  targets. That integration spawns `signal-subscriber`, uses Unix process
  groups, and terminates subscribers with Unix signals. No Windows target is
  built.

| Target                      | Built by         | Signal in `amplihack` | Signal in `amplihack-hooks` |
| --------------------------- | ---------------- | :-------------------: | :-------------------------: |
| `x86_64-unknown-linux-gnu`  | release/snapshot |          ✅           |             ✅              |
| `aarch64-unknown-linux-gnu` | release/snapshot |          ✅           |             ✅              |
| `x86_64-apple-darwin`       | release/snapshot |          ✅           |             ✅              |
| `aarch64-apple-darwin`      | release/snapshot |          ✅           |             ✅              |

PR CI validates the Unix hook builds. The structural regression test
(`tests/integration/signal_release_feature_build_test.rs`) asserts the release
workflow has no Windows leg and enables the hook Signal feature exactly once.

---

## Verifying a release binary is Signal-enabled

After downloading and unpacking a release or snapshot archive:

```bash
# The Signal subcommand is always registered. In a Signal-enabled build it
# runs; in a Signal-less build it exits with a "rebuild with --features signal"
# error. A shipped hook binary now supports the SessionStart Signal path.
amplihack signal --help

# Heuristic only — a release build may strip symbols, so absence here does not
# prove Signal is missing. When present, these strings indicate the Signal
# stack was linked into the shipped hook binary (Linux/macOS):
strings ./amplihack-hooks | grep -i 'signal-cli\|amplihack-signal' | head
```

For Unix hook binaries, the authoritative release-build check is the CI/release
workflow command that compiles `amplihack-hooks-bin` with
`--features amplihack-hooks-bin/signal`.

---

## What did *not* change

- **Library default-feature policy is untouched.** `amplihack-hooks-bin` still
  declares `default = []`; a plain `cargo build` of the workspace still produces
  a Signal-less hook binary. Only the *release/CI* build turns the feature on.
- **`amplihack-signal` behavior and its message-validation code are unchanged.**
  The fix compiles already-shipped logic into a second binary; it does not add
  or alter Signal capability.
- **`bins/amplihack/Cargo.toml` is unchanged** — the CLI binary was already
  Signal-enabled.
- **No required status-check names changed.** Only the `run:` bodies of existing
  build steps were edited. Job/step `name:`, `env:`, `permissions:`, packaging,
  upload, and release steps are identical. Required checks
  (`Lint & Format`, `Test`, `Install Smoke Test`, `Build <target>`, `Release`)
  are preserved.

---

## Rationale & decision record

**Decision: the gap is real and is fixed at the CI/release build layer — not by
changing library defaults.**

- **Why not flip the library default to `signal`?** That would force Signal into
  *every* build of the hook binary, including local/dev workspace builds and any
  downstream consumer, changing behavior far beyond the shipping artifacts. The
  goal is only that **official shipped binaries** include Signal, so the change
  belongs in the release/CI build commands.
- **Why keep the source opt-in?** Developers building the workspace without
  Signal (e.g., for a minimal hook binary or to avoid the signal-cli runtime
  dependency during unrelated work) retain that option. The `signal` feature
  remains a real, off-by-default compile-time switch.
- **Why enable on all targets?** Parity with the already-Signal-enabled CLI
  binary. Selective enablement would create a matrix where the two shipped
  binaries disagree on Signal support per platform — confusing and unnecessary,
  since the stack already compiles everywhere.

---

## Reference

- [Signal channel](signal-channel.md) — the per-session channel implementation.
- [Signal onboarding](SIGNAL_ONBOARDING.md) — `signal setup` / `signal
  distribute` runtime onboarding for hosts and fleets.
- [Signal external integration](signal-external-integration.md) — integrating
  external systems with the Signal channel.
- `.github/workflows/release.yml` — stable release build & packaging.
- `.github/workflows/publish-snapshot.yml` — per-SHA snapshot build & packaging.
- `bins/amplihack-hooks/Cargo.toml` — the `signal` feature definition on
  `amplihack-hooks-bin`.
