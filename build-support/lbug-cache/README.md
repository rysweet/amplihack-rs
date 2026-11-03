# Local lbug build cache, v1

Approved scope: [the retained deputy decision](approved-v1.md), PM328
comment6616249815. The decision hash and superseded proposal hashes are in
[approval-v1.json](approval-v1.json). This module serves local development
builds; release and snapshot builds and distributed installers use source.

The production entry point is `scripts/cargo-with-lbug.sh`. It uses the installed
Rust compiler to bootstrap `main.rs` and `cache.rs` without Cargo or third-party
dependencies into a private temporary directory. The helper removes its mapped
bootstrap executable immediately. No cached executable is trusted or executed.
Rustup automatic toolchain installation is disabled before bootstrap. There is
no helper networking, archive format, authentication or publication interface.

Inputs: original Cargo arguments, effective native environment, root manifest
and locked lbug identity, compiler identity, and an optional owned cache root.
Outputs: the original Cargo streams/status, fixed stderr diagnostics, and an
optional verified local payload. The application target directory is unchanged. CI installs a temporary dispatcher
in its own runner directory so nested development commands also use verification;
the original Cargo executable and fixture PATH overrides retain their behavior.
`selection_env`, `digest`, `verify` and `ARCHIVES` are internal test contracts;
they are not an application API.

Request resolution admits pinned registry lbug 0.15.4 on native macOS/GNU/Linux
x86-64/aarch64, supported simple Cargo configuration and one plain compiler
executable. Ambiguous input selects source. Keys bind version, target, compiler
family and major. The manifest's context also binds full compiler identity,
workspace configuration, OS/SDK identity and native settings. Machine-specific
CPU flags select source. Overrides, including empty values,
remain upstream inputs. Release/install/bench and custom profiles use source.

Each entry has exactly eighteen regular static archives, a merged header tree,
generated `lbug.hpp`, and a strict `manifest.sha256`. The manifest grammar is:

```text
lbug-local-v1 <64 lowercase hex context digest>\n
<64 lowercase hex file digest>  <decimal byte length>  <relative path>\n
```

Inventory lines are sorted by path and cover all payload files exactly once.
The manifest cannot describe itself. Recomputing and comparing its entire
canonical encoding rejects duplicate/extra lines and noncanonical syntax.
Header generation preserves each upstream include root's relative paths and
accepts duplicate files only when bytes agree. The shim's external `lbug.hpp`
includes are generated from upstream's bundled include list. A compiler syntax
check uses only the staged include root before installation. FFI shim archives,
shared libraries and CMake outputs are excluded.

A requesting source build runs at most once. Its successful output supplies both
linking and the cache. Packaging errors are separate diagnostics and keep Cargo
success. The helper never reruns a successful Cargo child to repair an artifact.
A stable OS lock belongs to the wrapper through its Cargo child's link step;
upstream build scripts do not acquire it. Waiters reverify an installed winner.
Exclusive staging and native atomic no-replacement rename protect installation.
Locks are never removed. Interrupted staging is never selected. Integrity
rejection vetoes reuse/repair for the invocation and changes upstream's tracked
compiler-flags fingerprint, invalidating warm managed output. Valid repeated
selection keeps a stable fingerprint.

The trust boundary is the owning OS user. Hashes detect corruption; they cannot
authenticate a payload rewritten together with its manifest by that same user.
Private cache roots and no-follow file opens reject foreign/symlinked inputs.
V1 does not claim a mutually untrusted same-user producer isolation boundary.
The archived `v2-contracts/` are deferred cross-host publication work, not v1
tests or implemented APIs.

Validation:

```sh
cargo test --offline --locked --manifest-path build-support/lbug-cache/Cargo.toml
cargo test --offline --locked --manifest-path build-support/lbug-cache/tests/contract-harness/Cargo.toml
python3 -m unittest discover -s tests -p 'test_issue_1917_lbug_*.py' -v
npm test
```

The real Cargo fixture substitutes a small native producer and tests production
selection, warm invalidation, corruption, rejection, publication-free races and
interruption. It is not evidence of native lbug ABI or performance. Run
`scripts/verify-lbug-native.sh` only under the admitted detached resource runner
for actual CMake/core/shim and bounded database acceptance. Inspect release
loader metadata with `scripts/check-lbug-release-rpath.sh`.
