# Self-Heal: Auto-Restage Framework Assets on Version Change

> [Home](../index.md) > [Features](README.md) > Self-Heal Asset Re-Stage

`amplihack` re-stages framework assets in `~/.amplihack` automatically the
first time a new binary version runs, so a binary upgrade is never silently
out-of-sync with the on-disk framework.

## Problem

PR [#488](https://github.com/rysweet/amplihack-rs/pull/488) added a
post-install hook to `amplihack update` that re-stages framework assets after
the binary is replaced. That hook only fires when the **old** running binary
already contains the post-install code path. Users on pre-#488 versions who
ran `amplihack update` got the new binary but not the asset re-stage — the
new binary had no idea the prior install was stale.

The result was silent drift: a user upgrades from `0.8.55` → `0.8.111`, then
runs a command that depends on assets shipped with the newer binary, and the
command fails or behaves like the older asset version because `~/.amplihack`
was never re-staged.

## How it works

Every launch, before command dispatch, `amplihack` performs a startup-time
**version-stamp check**:

1. Read `crate::VERSION` (the currently running binary version, honoring the
   `AMPLIHACK_RELEASE_VERSION` build-time override).
2. Read the version stamp at `~/.amplihack/.installed-version`.
3. If the stamp is missing, unparseable or **older** than the binary version,
   re-stage automatically through `repair_framework_assets` in
   [implicit mode](#never-an-implicit-downgrade-1526). A stamp **newer** than
   the binary never triggers a re-stage.
   The third argument (`force_refresh: false`) means self-heal tries the local
   sources first and uses one only if it is compatible and has the binary's
   version. A source with another version is skipped with a note, not refused.
   If none qualifies, self-heal fetches the tag of the binary's release. It
   never fetches `main`. The rules are in the
   [framework source selection reference](../reference/framework-source-selection.md).
   Self-heal does not perform a separate startup compatibility scan; the
   automatic install run validates candidate and staged framework bundles. For
   the post-update install path (where the **new** binary is spawned as a
   subprocess with `--force-refresh`), see
   [Post-Update Install — Re-exec New Binary](update-reexec-new-binary.md).
4. On success, write the new version into the stamp file and emit a single
   line on stderr:

   ```
   amplihack: framework assets re-staged for vX.Y.Z
   ```
5. On failure, the error propagates and `amplihack` exits with a non-zero
   status. There is **no silent fallback**. The one exception prints a warning:
   see [Failed source fetch](#failed-source-fetch-with-a-compatible-bundle).

Manual `amplihack install` invocations also write the stamp, so both the
self-heal path and the explicit install path converge on the same source of
truth.

Once `0.8.112+` ships and a user runs it once, every subsequent launch
self-heals automatically — closing the upgrade gap permanently.

## Never an implicit downgrade (#1526)

An automatic re-stage (self-heal or the launch bootstrap) runs in **implicit
mode**: it never writes `~/.claude/settings.json`, and it replaces a binary in
`~/.local/bin` only when the running version is strictly newer than the
stamp's binaries version by semver precedence (`0.18.9 < 0.18.10`,
`1.2.0-dev < 1.2.0`). It replaces `amplihack-hooks` and
`amplihack-asset-resolver` only with the copies next to the running
executable (or `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`); a copy found
elsewhere on `PATH` has an unknown version and may only fill an empty slot.
An empty slot is always filled. Explicit `amplihack install` is unchanged,
and is the way to downgrade on purpose.

| Running vs stamp | Assets | Binaries | Stamp |
|---|---|---|---|
| older | untouched | untouched | untouched |
| equal | no-op, or the #1271 bundle repair | untouched | untouched |
| newer | re-staged | replaced if newer than the stamp's binaries version, from this build's own copies | written |
| missing or unparseable | re-staged (#502) | empty slots filled | written; binaries recorded as unknown if one was kept |

When a re-stage keeps an installed binary (including one whose copy failed)
that the stamp does not already record at the running version, one line goes
to stderr:

```
amplihack: warning: kept /home/dev/.local/bin/amplihack-hooks, /home/dev/.local/bin/amplihack (installed version: unknown) during an implicit re-stage for v1.2.0; run `amplihack install` to replace them.
```

The stamp then records the framework as current and the binaries at the
version the previous stamp gave them, or as unknown (see
[Stamp file](#stamp-file)). The next launch finds the framework current and
neither re-stages nor warns, and the guard still refuses to replace the kept
binaries until `amplihack install` is run. On a downgrade nothing is written, and one warning is printed per process; the
command keeps its usual exit status:

```
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   /home/dev/.cargo/bin/amplihack (v0.18.0)
  installed: /home/dev/.local/bin/amplihack (v0.18.39)
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
```

The `installed:` line gives a version beside the path only when every binary
in `~/.local/bin` is at it, that is when the stamp has no `binaries:` line.
After an implicit re-stage that kept some binaries, the binaries can be at
different versions: `amplihack` is refreshed from the running executable,
while `amplihack-hooks` and `amplihack-asset-resolver` are kept unless a copy
sits beside it (as after `cargo install --git … amplihack`, which installs no
hooks binary). The stamp does not record which binary is at which version, so
the line names the path and then both versions, each labelled, for example
`installed: ~/.local/bin/amplihack; framework v0.19.0; binaries kept from v0.18.1`.
When the stamp records the kept binaries as unknown (a pre-#502 install, or a
copy that failed), it ends `binaries kept from an unknown version` rather than
guessing.

Control characters in printed paths appear as `?`. The launch bootstrap
(`ensure_framework_installed`) applies the same check first; an unreadable
stamp now fails the launch, as it already failed self-heal. When the binary is
not older, its #202 hook repair still writes `settings.json` if the file is
missing or parses with no amplihack hooks. A `settings.json` that does not
parse is left unchanged and a warning is printed. Builds of untagged `main`
commits still report `MAJOR.MINOR.0` (see
[Release tags point at a stamp commit](../reference/ci-pipeline.md#release-tags-point-at-a-stamp-commit)).

### Other tools' configuration

"User settings" covers every tool's configuration file, not just Claude
Code's. An implicit re-stage, the launch bootstrap and `amplihack copilot`
refresh only files under directories amplihack owns. They treat each tool's
own configuration file this way:

| File | Implicit (self-heal, launch bootstrap, `amplihack copilot`) | `amplihack install` |
|---|---|---|
| `~/.claude/settings.json` | never written by a re-stage; the bootstrap fills in hooks only when the file has none (#202) | hooks merged, file backed up |
| `~/.copilot/config.json` | left byte-identical once it holds any amplihack entry (`installedPlugins`, `plugins` or an amplihack hook); written only when it has none, which is a first install. Decided once per run, before any writer | amplihack entries refreshed |
| `~/.copilot/installed-plugins/amplihack@local/`, `~/.copilot/.github/hooks/` | refreshed (amplihack-owned) | refreshed |
| Codex plugin (`~/.codex/hooks.json`, the Codex marketplace) | reconciled only when Codex is the selected client, exactly as by `amplihack install`; a legacy or changed ownership proof, or a Codex without native plugin support, fails the command and leaves `~/.codex` untouched (#1538). Never touched by a binary older than the install. Skipped with a warning when no source was found and the stamp differs from the binary, which is the [kept-bundle](#failed-source-fetch-with-a-compatible-bundle) state | reconciled |

A Copilot entry that a user disabled stays disabled. When `config.json`
registers amplihack from a different plugin directory, a re-stage leaves it
unchanged and prints a stderr warning naming `amplihack install`.

### Hooks a newer release adds

Because a re-stage never writes `settings.json`, upgrading the binary any
way other than `amplihack install` or `amplihack update` (npm wrapper,
tarball, `cargo install`) does not register hook events the new release
adds. The re-stage compares this build's amplihack hook registrations with
the scope's `settings.json`. If the file already has amplihack hooks but
lacks some of this build's, one line goes to stderr:

```
amplihack: warning: /home/dev/.claude/settings.json lacks hook registrations this version (1.3.0) adds: PreCompact (pre-compact). An automatic re-stage never edits settings.json; run `amplihack install` to add them.
```

Nothing is printed when no registration is missing. Nothing is printed for a
file with no amplihack hooks either, because the launch bootstrap registers
them. A file that is missing or does not parse is not reported here.

## Skip rules

The check is intentionally bypassed in cases where running an install would
recurse, undo intent, or hurt the fast-path UX:

| Trigger | Reason |
|---------|--------|
| `AMPLIHACK_SKIP_AUTO_INSTALL=<non-empty>` | Explicit opt-out for CI/testing. |
| Subcommand `install` / `uninstall` / `update` | Would recurse or undo user intent. |
| Subcommand `completions` / `doctor` / `help` | Read-only/diagnostic; should stay fast. |
| Top-level flag `--help`, `-h`, `--version`, `-V` | Short-circuits clap before dispatch. |
| No arguments | Clap will print help; nothing to dispatch. |

The argument scan runs **before** clap parses, so it adds no measurable
latency to short-circuit invocations.

## Stamp file

| Path | `~/.amplihack/.installed-version` |
|------|-----------------------------------|
| Format | Plain text, no trailing newline. One line, or two after an implicit re-stage kept a binary (#1526). |
| Contents | Line one: the framework version, matching `^\d+\.\d+\.\d+(-[\w.]+)?$` (e.g. `0.8.111`, `0.9.0-rc1`). Optional line two: `binaries: <version>` or `binaries: unknown`, the version of the binaries an implicit re-stage kept in `~/.local/bin` when it differs from line one. Binaries that re-stage replaced are at line one's version; the stamp does not record which binaries those are. Builds that predate the second line treat a two-line stamp as malformed and re-stage. |
| Write semantics | Atomic — staged at `.installed-version.tmp` and renamed into place, mirroring the existing `write_layout_marker` pattern in `commands::install::mod`. A crashed write can never leave a half-written stamp. |
| Read semantics | Missing file returns `None` (treated as "no prior install"). Malformed contents (failing the semver regex) are treated as "no prior install" so a corrupt stamp triggers a clean re-install rather than wedging the binary. All other I/O errors propagate. |
| File mode | `0o600` (owner read/write only). The stamp lives under `~/.amplihack` which is also owner-private; the explicit mode prevents drift if the user has loosened the parent's umask. |
| Symlink policy | The stamp path is checked with `symlink_metadata` before any read or write. If it is a symlink (or any non-regular file), self-heal **refuses to operate** on it — neither reads nor overwrites — and surfaces an error. This blocks a class of attacks where a hostile process points the stamp at a sensitive file to coerce truncation. |

### Concurrency

A second `amplihack` process launched on the same machine while a self-heal
install is in flight could otherwise race into `run_install` and stomp on the
first install's partially-written tree. To prevent this, self-heal acquires
an **advisory exclusive file lock** on `~/.amplihack/.install.lock` (created
on demand, mode `0o600`) for the duration of the decision-and-install window.

- The lock is held only while the check runs and, if needed, the install
  executes; it is released before command dispatch.
- A second process that arrives during the install **blocks** on the lock,
  then re-reads the stamp on the other side. Because the first process
  wrote the new stamp before releasing, the second process sees a match and
  proceeds without re-installing.
- Explicit `amplihack install`, `amplihack update` and `amplihack uninstall`
  take the same lock, so self-heal never re-stages while another install
  runs. The re-stage creates a new release and switches links like any
  install; see [Versioned Install Layout](../concepts/versioned-install-layout.md).
- The lock is advisory (`fs2::FileExt::lock_exclusive`); processes that do
  not honour it (e.g. a manual `rm -rf ~/.amplihack`) can still race, but
  no normal `amplihack` invocation will.

## Bypass: `AMPLIHACK_SKIP_AUTO_INSTALL`

Set `AMPLIHACK_SKIP_AUTO_INSTALL` to any non-empty value to disable the
check. Intended for CI pipelines and unit tests that pre-stage assets and do
not want the binary to mutate `~/.amplihack` mid-run.

```sh
# CI: stage once during job setup, then run many commands without re-stages
amplihack install
export AMPLIHACK_SKIP_AUTO_INSTALL=1
amplihack claude --print 'run tests'
amplihack copilot --print 'run tests'
```

An empty value (`AMPLIHACK_SKIP_AUTO_INSTALL=""`) is **not** treated as a
bypass — the check still runs.

### Bypass diagnostic

When the bypass is active **and** self-heal would have run (the stamp is
missing or older than the binary, or the installed bundle is incompatible),
`amplihack` emits a single diagnostic line on stderr before dispatch:

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=0.8.55 current=<version>; installed_bundle="compatible")
```

`current` is the version of the running binary. `installed_bundle` is
`"compatible"` when the installed bundle passes the #1271 check, and otherwise
holds the reason it failed.

This makes the "stale assets, intentionally" state visible in CI logs and
test output so a downstream failure can be traced back to the version skew
without requiring the user to remember the bypass was set. The line is
written exactly once per process. A matching stamp with a compatible bundle
produces no output.

Under the bypass a stamp newer than the binary also prints this line rather
than the [downgrade warning](#never-an-implicit-downgrade-1526): the bypass
never re-stages, so there is nothing for the guard to refuse.

### What the bypass skips

With the bypass set, self-heal returns before any step that touches the
network, takes a lock or starts a subprocess. In a launch such as

```sh
AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_NONINTERACTIVE=1 \
  amplihack launch --skip-update-check
```

the bypass therefore prevents all of the following:

| Skipped step | Observable sign if it ran |
|--------------|---------------------------|
| Release tag fetch (`git`, `curl`) | A `git` or `curl` process; up to 60 s of waiting |
| Advisory install lock | `~/.amplihack/.install.lock` is created |
| Fetch backoff marker | `~/.amplihack/.tag-fetch-failed` is created |
| [Kept-bundle warning](#failed-source-fetch-with-a-compatible-bundle) | `keeping the installed bundle` on stderr |

Self-heal never runs `npm`. The pre-launch npm tool update check is a separate
step, skipped in this command by `--skip-update-check` and
`AMPLIHACK_NONINTERACTIVE`, not by the bypass. See
[Manage Tool Update Notifications](../howto/manage-tool-update-checks.md).

Two checks still run under the bypass. Neither uses the network, takes a lock
or starts a subprocess:

- The [#1526 downgrade guard](#never-an-implicit-downgrade-1526), which only
  compares two version strings.
- The #1271 installed-bundle compatibility check, which reads local files. Its
  verdict is memoised on a stat fingerprint of the bundle in
  `~/.amplihack/.bundle-compat-cache.json`. When no cached verdict matches,
  which is always the case on a fresh `HOME`, the check writes that file. This
  is a plain file write, so `~/.amplihack` is not left empty. The verdict is
  what the [bypass diagnostic](#bypass-diagnostic) prints as
  `installed_bundle=`.

On a fresh `HOME` the stamp is missing and there is no installed bundle, so the
bypass diagnostic is printed once, with the reason the bundle check failed in
place of `<reason>`:

```
amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage (stamp=<missing> current=<version>; installed_bundle="<reason>")
```

#### Guard test

`tests/integration/issue_1971_skip_guard_test.rs` pins this behaviour without
any timing assertion. It runs the built binary with a per-test `HOME` and a
`PATH` that holds only stubs: a healthy `claude`, and `git`, `npm` and `curl`
stubs that append their arguments to a log file and exit 1. It then asserts
that:

1. the stub log is empty, so no `git`, `npm` or `curl` process was started;
2. `~/.amplihack/.tag-fetch-failed` does not exist;
3. `~/.amplihack/.install.lock` does not exist;
4. stderr contains none of `keeping the installed bundle`,
   `could not fetch the framework bundle` or `cannot fetch a release tag`;
5. stderr contains `AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage`.

The test does not assert that `~/.amplihack` is empty, because the #1271 check
writes `.bundle-compat-cache.json` there. The `npm` stub is present so that a
regression in the update-check flags would also show in the log.

Run it from the repository root:

```sh
cargo test -p amplihack --test integration issue_1971_skip_guard:: -- --nocapture
```

All test scratch, including the per-test `HOME`, goes under `$TMPDIR`.

The test works alongside WS3-INT-4,
`skip_update_check_flag::launch_with_skip_update_check_completes_without_npm_subprocess_overhead`
in `tests/integration/skip_update_check_flag_test.rs`. WS3-INT-4 runs the same
launch with a recording `npm` stub as the only `npm` on `PATH` and asserts that
the stub never ran; it has no time limit (issue #1973). The two tests divide
the assertions as follows:

| Assertion | Owned by |
| --- | --- |
| `--skip-update-check` runs no `npm` | WS3-INT-4 |
| The bypass runs no `git` or `curl` | `issue_1971_skip_guard` |
| No `.tag-fetch-failed` or `.install.lock` is created | `issue_1971_skip_guard` |
| No fetch or re-stage message is printed, and the bypass diagnostic is | `issue_1971_skip_guard` |

The guard test also records `npm`, so that its stub log lists every recorded
tool the launch starts. An `npm` entry in that log repeats a WS3-INT-4 failure;
it is not a separate check. Both tests run with stdin that is not a TTY, so
neither shows that the flag alone suppresses the update check; the unit test
`tool_update_check::tests::check_wanted_only_without_skip_in_an_interactive_session`
covers that. The recording stubs catch only tools found through `PATH`; a call
by absolute path would not be recorded.

See also: [Environment Variables — `AMPLIHACK_SKIP_AUTO_INSTALL`](../reference/environment-variables.md#amplihack_skip_auto_install).

## Failure mode

Per the project's Zero-BS philosophy, install failures during self-heal
**propagate**:

- The error is printed to stderr.
- The process exits with status `1`.
- The stamp file is **not** updated, so a later launch will retry.
- The advisory lock is released (RAII drop) so the retry is not blocked.

There is no `|| true`, no silent skip, and no "continue with whatever assets
happen to be on disk" fallback. A broken install is reported to the user.

These always exit `1`:

- The installed bundle is missing or incompatible and no source was available.
- A fetched release tree has the wrong or an unknown version.
- The [manifest check](../reference/framework-source-selection.md#manifest-check)
  failed.

### Failed source fetch with a compatible bundle

Without this exception, every command on an offline, upgraded host would exit 1.
Self-heal continues only when both of these are true:

- The release tag fetch failed, or this build may not fetch a tag.
- The installed bundle passes the compatibility check.

Self-heal then prints one line to stderr, leaves the stamp unchanged and runs
your command:

```
amplihack: warning: could not fetch the framework bundle for v1.2.99 (network request timed out); keeping the installed bundle. A later launch will retry.
```

A build that may not fetch makes no network request. It prints this line instead,
because a later launch won't fetch either:

```
amplihack: warning: this build (1.2.0+snapshot.7deb23da4) cannot fetch a release tag, and no local source matches it; keeping the installed bundle. Run `amplihack install --local <PATH>` to replace it.
```

The unchanged stamp is what makes a later launch try again. Each attempt that
fetches waits up to 60 seconds.

A failed fetch writes `~/.amplihack/.tag-fetch-failed`, holding the version and
the time of the failure. For the next 10 minutes, a launch for the same version
skips the fetch and prints the same warning at once, with a reason such as
`a fetch for this version failed 42 s ago, so it is not retried for another 10 min`.
The lock holder writes the marker before it releases the install lock, so
launches that were waiting behind it don't retry either. On a host with a hung
network, N parallel launches therefore cost one 60-second wait, not N of them.
Explicit `amplihack install` and `amplihack update` ignore the marker, and any
successful fetch removes it.

An explicit `amplihack install` or `amplihack update` never continues past a
failed fetch. Both exit non-zero.

The launch bootstrap's slash-command restore never fetches. If
`~/.claude/commands/amplihack/` is missing and no local source has this binary's
version, the commands stay missing and the only record is a warning in the trace
log. Run `amplihack install` to restore them. See
[Known limits](../reference/framework-source-selection.md#known-limits).

### One documented carve-out: unresolvable home directory

If `dirs::home_dir()` returns `None` (no `$HOME`, no platform fallback),
self-heal **silently skips** rather than failing the launch. Rationale:

- A binary that cannot find a home directory cannot install anywhere
  meaningful, so failing here would produce a confusing error far from the
  real misconfiguration.
- Subcommands that genuinely need `~/.amplihack` (e.g. `claude`, `copilot`)
  will fail later with their own home-directory error, which is the
  appropriate place to surface the problem.
- Subcommands that do not need a home directory (e.g. `--version`,
  `doctor`) should continue to work in restricted environments.

This is the **only** intentionally silent path in self-heal; a kept bundle
always prints a warning. It is called out explicitly so reviewers do not mistake it for a Zero-BS violation.

## Implementation

| File | Role |
|------|------|
| `crates/amplihack-cli/src/self_heal.rs` | Decision logic, advisory lock acquisition, bypass diagnostic, the #1526 downgrade guards and once-per-process warning, and public entrypoint `ensure_assets_match_binary_version(args)`. Uses closure injection (mirroring `update::post_install::run_post_update_install`) so unit tests can verify the decision tree without running a real install. |
| `crates/amplihack-cli/src/commands/install/version_stamp.rs` | Atomic stamp read/write helpers (`read_installed_version`, `read_binaries_version`, `write_installed_version`, `write_installed_versions`, `installed_version_path`) and the #1526 semver comparison (`relation`). Performs symlink refusal via `symlink_metadata`, semver-regex validation of contents, and `0o600` permission enforcement on write. |
| `crates/amplihack-cli/src/commands/install/mod.rs` | `local_install` writes the stamp on every successful install (from a local source or a release tag fetch); an implicit one that kept an installed binary also records the binaries' version, or `unknown`. `InstallMode::{Explicit, Implicit}` selects whether settings are written and binaries may be replaced; `ensure_framework_installed` applies the downgrade check before the launch bootstrap changes anything. |
| `crates/amplihack-cli/src/commands/install/binary.rs` | `deploy_binaries` keeps an existing destination in implicit mode unless the running binary is strictly newer than the stamp's binaries version (`implicit_deploy_allowed`) and the source is this build's own copy, and returns the binaries it kept. |
| `crates/amplihack-cli/src/commands/install/framework_source.rs`, `tag_fetch.rs` | Choose the source and fetch the release tag. See [Framework source selection](../reference/framework-source-selection.md#contributor-api). `SourceFetchFailed` is the only error self-heal may continue past. |
| `bins/amplihack/src/main.rs` | Calls `self_heal::ensure_assets_match_binary_version(&args)` after the existing update notice and before `Cli::parse_from`. |

Dependencies introduced: [`fs2`](https://crates.io/crates/fs2) for the
advisory file lock, [`regex`](https://crates.io/crates/regex) (already in
the workspace) for stamp validation. Both new modules are kept within the
project's 500-line module cap.

## See also

- [Install Command Reference](../reference/install-command.md) — the install
  procedure invoked by self-heal.
- [Framework Source Selection](../reference/framework-source-selection.md) —
  which source a re-stage uses and what staging checks.
- [Environment Variables Reference](../reference/environment-variables.md) —
  full env var contract, including `AMPLIHACK_SKIP_AUTO_INSTALL` and
  `AMPLIHACK_RELEASE_VERSION`.
- PR [#488](https://github.com/rysweet/amplihack-rs/pull/488) — the
  post-update install hook that this feature complements.
- PR [#500](https://github.com/rysweet/amplihack-rs/pull/500) — the initial
  shipping change (decision flow, stamp, bypass env var).
- Issue [#499](https://github.com/rysweet/amplihack-rs/issues/499) — the
  upgrade gap closed by this feature.
- Issue [#502](https://github.com/rysweet/amplihack-rs/issues/502) — the
  hardening pass tracked here (symlink refusal, `0o600`, semver validation,
  advisory lock, bypass diagnostic, `home_dir()` carve-out).
