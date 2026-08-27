# Launch Target Resolution

**Issues:** [#1266](https://github.com/rysweet/amplihack-rs/issues/1266) (the defects this closes) · [#585](https://github.com/rysweet/amplihack-rs/issues/585) (the npm hang this must not regress)
**Status:** Implemented. `amplihack_utils::launch_target::resolve` is the one
resolver; `bootstrap::ensure_tool_available`, `launcher_core::get_claude_cli_path`,
and the fleet reasoner all read through it.
**Scope:** `crates/amplihack-utils` — `launch_target`, `claude_native` · `crates/amplihack-cli` — `bootstrap`, `launcher`, `commands/launch` · `crates/amplihack-launcher` — `launcher_core`

> **Verification snapshot.** Measured on the dev VM (Linux x86_64) against
> `@anthropic-ai/claude-code`. The native binary is **338,860,336 B** at
> **2.1.238** and **342,563,120 B** at **2.1.239** — the size tracks the
> release, so it is quoted with a version or not at all. The copilot shim is
> **1185 B**, the unrepaired claude stub is **~500 B**, a warm `npm show` is
> **~0.3 s**, and a host launch settles at **0.45 s**.
> `npm show` corrected the design's earlier 351 ms estimate.
>
> Code comments say "~339 MB", which is 2.1.238's 338,860,336 B in decimal MB.
> An earlier draft of this page rendered the same quantity as "327 MiB" — that
> is 2.1.239's 342,563,120 B in binary MiB, i.e. a different release *and* a
> different unit presented as the same number. Prose below says "hundreds of
> megabytes" where the point is the order of magnitude; exact figures appear
> only here, where they are measured and labelled.
>
> Two figures remain **design estimates, not observations**: the per-resolution
> timings (151 ms → 116 ms → 0.14 ms) and the 60.0 s drain overrun against the
> 10 s budget. The workspace has no benchmark harness, so neither is
> reproducible from a `cargo` invocation; both are labelled where they are used
> below. Sizes and versions drift with every upstream release — read the
> absolute numbers as of the stated version, and the ratios as the durable
> claim.

## Overview

Every `amplihack claude` launch has to answer three questions:

1. Which binary will we execute?
2. Is that binary usable?
3. Do we need to install or upgrade anything first?

`amplihack_utils::launch_target::resolve` answers all three. One function, one
answer, one binary. The version that gets compared against the registry is the
version of the binary that gets executed, and nothing else in the repository
independently resolves a launch path for these purposes.

That single-resolver rule is the whole design. The rest of this document is its
consequences.

## The resolution contract

### `resolve`

```rust
use amplihack_utils::launch_target::{resolve, TargetSource};

let resolution = resolve("claude");

match resolution.target {
    Some(target) => println!("{} @ {} ({:?})", target.path.display(), target.version, target.source),
    None => eprintln!("{}", resolution.rejection_report("claude", "@anthropic-ai/claude-code")),
}
// Output on a healthy host:
// /usr/bin/claude @ 2.1.238 (Path)
```

```rust
pub struct LaunchTarget {
    pub path: PathBuf,
    pub version: String,
    pub source: TargetSource,
}

pub enum TargetSource {
    /// AMPLIHACK_CLAUDE_BINARY_PATH / CLAUDE_BINARY_PATH.
    /// `user_supplied` is false when amplihack set the variable itself —
    /// see "The override is also set programmatically" below.
    ExplicitOverride { user_supplied: bool },
    Path,             // found on $PATH
    AmplihackPrefix,  // ~/.npm-global/bin — the prefix amplihack installs into
    FallbackDir,      // ~/.cargo/bin, ~/.local/bin
}

pub enum Rejection {
    NotAbsolute,        // a relative path — see "Every candidate is absolute"
    Missing,            // no such path, or a dangling symlink
    NotAFile,           // resolves to a directory or other non-regular file
    NotExecutable,      // no executable bit for this user
    PlaceholderStub,    // `--version` failed AND the file has the placeholder's shape
    Unreadable,         // `--version` failed and the file could not be read to say why
    ProbeFailed,        // `--version` ran but exited non-zero
    ProbeTimedOut,      // `--version` exceeded the per-candidate budget
    UnparseableVersion, // `--version` succeeded but emitted no semver
    NotProbed,          // never examined — see "Stopping early is recorded"
}

pub struct Resolution {
    pub target: Option<LaunchTarget>,
    pub rejected: Vec<(PathBuf, Rejection)>,
}

/// Where `AMPLIHACK_<TOOL>_BINARY_PATH` came from. There is deliberately no
/// `Default`: the safe-looking default is `User`, the *strict* arm, so a caller
/// that forgot to thread the value would turn a preference into a hard launch
/// failure. Making every call site name the origin is the point (issue #1276).
pub enum OverrideOrigin {
    /// The variable came from the caller's environment. A failing override is a
    /// hard error — the user named a binary, and amplihack must not quietly run
    /// a different one.
    User,
    /// amplihack set the variable itself, as a preference.
    /// `configure_preferred_rustyclawd_binary` is the only producer. A failing
    /// override warns and the search continues, so a broken `rustyclawd` on
    /// `$PATH` cannot turn a working `amplihack rustyclawd` into a failed launch.
    AmplihackSupplied,
}

pub fn resolve(tool: &str, override_origin: OverrideOrigin) -> Resolution;

/// Same answer, ignoring the memo, and refreshes it. For callers that just
/// changed the filesystem — i.e. installed something. See "One probe per
/// process" below.
pub fn resolve_uncached(tool: &str, override_origin: OverrideOrigin) -> Resolution;

impl Resolution {
    /// Human-readable account of what happened, and the remedy.
    ///
    /// `tool` and `package` are parameters because this is the error path for
    /// EVERY tool. See "The report speaks about the tool it was asked about".
    pub fn rejection_report(&self, tool: &str, package: &str) -> String;
}

/// `~/.npm-global/bin` — the one directory amplihack owns, spelled once.
///
/// Takes `home` rather than reading `$HOME`, so it stays pure. Every consumer
/// of the concept calls this: `candidate_paths`, `binary_finder`'s
/// `install_fallback_dirs`, the launch command's `is_already_reachable`, and
/// `bootstrap`'s `npm_prefix_dir` (which returns this path's parent, because
/// npm's `--prefix` takes the prefix). See "Child process PATH".
pub fn amplihack_prefix_bin(home: &Path) -> PathBuf;
```

`Resolution` carries the rejection list because the error path needs it. A bare
`Option<LaunchTarget>` can say "nothing worked" but cannot say *what* it tried
and *why each one failed*, which is exactly the information the user needs when
a launch cannot proceed.

### Candidate order

Candidates are examined in this order and the **first healthy one wins**:

1. `AMPLIHACK_CLAUDE_BINARY_PATH`, then `CLAUDE_BINARY_PATH` (explicit override)
2. Each `$PATH` entry, in `$PATH` order
3. `~/.npm-global/bin` — amplihack's own npm prefix
4. The remaining install fallback directories: `~/.cargo/bin`, `~/.local/bin`

An explicit override that exists but fails the health gate is an **error**, not a
silent demotion. If you point amplihack at a specific binary and that binary is
broken, amplihack tells you so rather than quietly launching a different one.

#### Only absolute directories become candidates

`candidate_paths` discards every `$PATH` entry that is not absolute. This is not
hygiene; it closes a live path to arbitrary execution.

POSIX defines an **empty** `$PATH` element as the current directory, and
`std::env::split_paths` faithfully yields an empty `PathBuf` for one. A stray
trailing or doubled colon — `PATH=/usr/bin:` — therefore produces the bare
relative candidate `claude`, which `execvp` resolves out of the current working
directory. If a file of that name in a cloned repository prints a parseable
semver, it becomes the `LaunchTarget` and gets executed. Worse, its *parent* is
the empty path, so the child-`PATH` promotion below would put the current
directory at the **front** of the agent's `PATH` — and thus of every subagent
and every shell-out — turning one stray colon into cwd-first resolution of
`git`, `node`, and `sh`.

Filtering on `Path::is_absolute` removes the candidate and the promotion in one
step. `no_global_path_mutation.rs` asserts that no relative and no empty
directory ever reaches either the candidate list or `prepend_path`.

#### The override is also set programmatically

`AMPLIHACK_CLAUDE_BINARY_PATH` is not exclusively a user-facing variable.
`configure_preferred_rustyclawd_binary`
(`crates/amplihack-cli/src/commands/rustyclawd.rs`) sets it **in-process**
whenever `amplihack rustyclawd` finds a `rustyclawd` or `claude-code` binary,
then delegates to the ordinary claude launch path.

So the "a broken override is a hard error" rule can fire on a value amplihack set
itself. The binary it selects has passed only `is_executable_file` — existence
plus the executable bit — which is strictly weaker than the health gate. A
`rustyclawd` that exists but cannot answer `--version` would turn a previously
working `amplihack rustyclawd` into a hard failure it never had.

**The two origins are therefore treated differently:**

| Override origin | When it fails the health gate |
| --- | --- |
| Set in the user's environment | **Hard error.** The user named this exact binary; quietly launching a different one is worse than failing. |
| Set programmatically by `configure_preferred_rustyclawd_binary` | **Warn, drop the candidate, continue** down the list. This is a *preference*, not an instruction. |

`resolve` records the origin in
`TargetSource::ExplicitOverride { user_supplied }`. The internally-set case is
marked via an in-process flag, **not** a second environment variable — the child
must not inherit it, or a nested `amplihack` invocation would silently downgrade
a genuine user override.

Callers that need `user_supplied` to be true should set the variable in the
environment before the process starts. Callers expressing a preference should go
through the internal path.

### The health gate

A candidate becomes a `LaunchTarget` only if **all** of the following hold:

| Check | Rejection when it fails |
| --- | --- |
| Path is absolute | `Rejection::NotAbsolute` |
| Path exists | `Rejection::Missing` |
| Path resolves to a regular file — `fs::metadata`, **following symlinks** | `Rejection::NotAFile` |
| File is executable | `Rejection::NotExecutable` |
| `--version` exits 0 within the per-candidate budget | `Rejection::ProbeFailed` / `Rejection::PlaceholderStub` / `Rejection::Unreadable` / `Rejection::ProbeTimedOut` |
| `--version` output contains a parseable semver | `Rejection::UnparseableVersion` |

`Rejection::NotProbed` is not in this table on purpose: it is not a verdict on
the candidate at all — see "Stopping early is recorded".

The first four rows are `cheap_reject`, and they are the **only** pre-probe
checks. Absoluteness is a fact about the path; the other three are filesystem
facts that hold for any tool on any platform: it is there, it is a regular file,
you may run it. Nothing about the file's *contents* is judged before the probe —
see "The shape check is a label, never a gate".

#### Every candidate is absolute

`Rejection::NotAbsolute` is checked **first, before any filesystem call**, and
it is the one rule that is about the shape of the path rather than the state of
the disk.

A relative candidate is not a candidate, because two different subsystems would
disagree about which file it names. `cheap_reject` stats it against the
*process* current directory; `execvp` resolves a name containing no separator
against the *child's* `$PATH`. Neither of those files is the one that was named.

Relative candidates arrive from two places:

- **`$PATH`.** POSIX reads an **empty** element as the current directory, and
  trailing or doubled colons are ordinary in hand-edited shell profiles.
  `split_paths("/usr/bin:")` yields `["/usr/bin", ""]`, and joining `""` with
  `claude` gives the bare name `claude`. If a `./claude` in the current
  directory prints parseable semver, it becomes the selected `LaunchTarget`.
- **The override variables.** `CLAUDE_BINARY_PATH=claude` is spelled by a user
  and never passes through the `$PATH` seam at all.

The invariant is enforced in `cheap_reject` — the one funnel **every** producer
passes through, including whatever producer is added next — rather than at each
producer, or in the `push` closure that builds the candidate list. Filtering in
`push` would make a relative override *vanish* from the list, and the user would
get a silently different binary, which is precisely what the `source` match in
`resolve_from_candidates` exists to prevent.

Enforcing it at the funnel gives the right behaviour for free, with no new
branching:

| Source of the relative candidate | Result |
| --- | --- |
| `ExplicitOverride { user_supplied: true }` | Hard error naming the path — "not an absolute path — name the binary by full path" |
| `ExplicitOverride { user_supplied: false }` | Warn and fall through to the next candidate — it is a preference, not an instruction |
| `Path` / `AmplihackPrefix` / `FallbackDir` | Rejected before `probe_version` spawns anything |

`path_dirs` keeps its own `is_absolute` filter as a cheap pre-filter and as the
pure, mutation-free seam the ratchet test pins. Two sibling `$PATH` walks —
`binary_finder::search_path_dirs` and `docker_detector::which_docker_in` — carry
the same filter, because they are separate funnels with separate callers. The
ratchet in `tests/no_global_path_mutation.rs` scans the crate by *shape*, not by
filename: the first version of it named one file, and that is exactly how the
second funnel stayed open through the first fix.

**The file-type check must follow symlinks.** On every npm-installed host,
`~/.npm-global/bin/claude` is a *symlink* into the package directory:

```sh
ls -l ~/.npm-global/bin/claude
# lrwxrwxrwx 1 you you 60 ... claude -> ../lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe
```

Using `symlink_metadata` — or any `is_file()` derived from it — rejects every
npm-installed claude on every host, including the one amplihack installs itself.
Use `fs::metadata`, which follows the link. A dangling symlink then surfaces as
`Rejection::Missing`, which is the correct answer for it.

**Health is a filter, never an annotation.** There is no such thing as a
`LaunchTarget` with `version: "unknown"`. A binary whose version probe fails,
times out, or returns something unparseable is not a degraded candidate — it is
not a candidate. amplihack will not execute it. The type carries this: there is
no constructor for `LaunchTarget` that admits an unknown version, so "launch
something we could not identify" is not a state the program can represent.

#### The health gate is a correctness control, not a security boundary

`probe → select → spawn` is TOCTOU by construction. The file that answered
`--version` is not provably the file that gets executed a few milliseconds
later, and nothing here tries to make it so. The gate exists to stop amplihack
from launching things that do not work — a broken install, a placeholder, a
binary that hangs. It is not an integrity check and must not be relied on as
one. The module says so at its head so that a future reader does not mistake
the check for a defence, and a reviewer does not relitigate the absence of one.

### The shape check is a label, never a gate

`claude_native::has_placeholder_shape` answers one question: is this a **small
file** — under `STUB_MAX_LEN`, 4 KiB — **that does not begin with a native
executable magic number** (`\x7fELF`, a Mach-O magic, or `MZ`)? The test is the *absence* of a magic
number, not the presence of any particular text.

That distinction is load-bearing. The placeholder shipped by
`@anthropic-ai/claude-code` has **no shebang**. It is a 500-byte file whose first
bytes are:

```
echo "Error: claude native binary not installed." >&2
```

and `file` reports it as `ASCII text`. A check written to look for `#!` would
miss the exact stub this exists to name — do not write one.

**It may only ever relabel a candidate whose probe has already failed.** It is
called from `label_failed_probe`, on the `ProbeFailed` path, and from nowhere
else. It lives in `claude_native`, beside `is_materialized` and
`MIN_NATIVE_BINARY_LEN`, because it encodes what a *claude* install looks like.

This was previously a pre-probe rejection in `cheap_reject`, applied to every
tool, and it broke `amplihack copilot`. `@github/copilot` installs
`~/.npm-global/bin/copilot` as a **1185-byte `#!/usr/bin/env node` loader** —
small, no native magic, and perfectly healthy. It was rejected before it was
ever run, so `decide_install(None, _)` answered `InstallMissing`, amplihack
reinstalled `@github/copilot` on every launch, re-resolved, saw the same shim,
and hard-failed. `@openai/codex` ships the same shape.

The rule that prevents a recurrence is structural, not a special case: a
classifier that can only *rename an already-failed rejection* cannot produce a
false rejection for any tool, present or future. An `if tool == "claude"` would
have fixed copilot and left the next tool to rediscover it.

What survives is the diagnosis the fast path was written for. An incomplete
`@anthropic-ai/claude-code` install still reports "incomplete install — this is
the small placeholder the npm package ships" rather than a bare "`--version`
failed". The cost is one `execve` that returns `ENOEXEC`, on an install that was
already broken.

Two contract tests hold the boundary: `cheap_reject`'s body may not mention
`has_placeholder_shape` or `STUB_MAX_LEN`, and `launch_target.rs`'s production
code may not contain a tool name or an npm scope.

### Probe budget

| Budget | Value |
| --- | --- |
| Per-candidate `--version` timeout | 3 s |
| Total probe budget across all candidates | 10 s |
| `MAX_PROBE_CANDIDATES` | 8 |

Probing stops at the first healthy candidate, so the common case is one
subprocess. The total budget exists because a single hung or hostile binary
early in `$PATH` must not be able to stall a launch: eight candidates at the
per-candidate timeout would otherwise be 24 seconds of foreground hang.

**The bound covers the whole subprocess, output drain included.** Waiting for
the child and then joining the reader threads unconditionally is not a timeout:
if the child exits promptly but a *grandchild* inherits its stdout pipe, the
drain thread never sees EOF and the join has no ceiling at all. Against a
shim that runs `sleep 60 &` and then exits, the drain overruns to **60.0 s
against a 10 s budget** (a design estimate — no test in the workspace builds
that shim, so the figure is the `sleep`'s own duration, reasoned rather than
timed; the unboundedness it illustrates is structural, not numeric),
and against a daemon it never returns. `run_capped_output_with_timeout`
therefore bounds the joins by whatever is left of the timeout and, when that
runs out, abandons the reader threads rather than waiting on them — the child's
exit status is already known and authoritative, and a version probe needs one
semver line, not a complete transcript. The detached threads exit when the pipe
closes or when the process does.

The timeout kills the **child**, not its process group, so a candidate that
forks can still leave a grandchild running after amplihack has moved on. That is
a stray process, not a hang: the drain is abandoned and resolution returns on
budget regardless. Killing the group would mean putting the probe in its own
session, which buys tidiness at the cost of signal-delivery semantics the launch
path does not otherwise need. The stray process is accepted, and named here so
it is not rediscovered as a leak.

This is what makes the resolution memo's global lock defensible: it is held
across the probe and it is one mutex for all tools, so the wait it can impose on
another thread has to be bounded. It is, by `TOTAL_PROBE_BUDGET`.

The 3 s figure is deliberately larger than `binary_finder`'s 500 ms
`VERSION_DETECTION_TIMEOUT`. That constant gates an advisory annotation, where a
false negative costs nothing. This one gates a launch, where a false rejection
degrades the user's session.

### One probe per process

A single launch asks "which binary?" at least twice — the update notice, then
the install decision — and the probe runs against a binary of hundreds of megabytes. That costs
an estimated **~151 ms per resolution**, of which 0.15 ms is building the
candidate list and the rest is `claude --version`. Asking twice bought nothing.

`resolve` therefore memoizes, and the memo is keyed by tool **and validated
against the candidate list it was computed from**. Every input the health gate
reads is in that list, so any environment change that could change the answer —
`PATH`, `HOME`, `AMPLIHACK_CLAUDE_BINARY_PATH`,
`mark_override_amplihack_supplied` — produces a different list and misses the
memo instead of returning a stale answer.

What the memo cannot see is the filesystem changing underneath it, which is
exactly what an install does. Both post-install call sites in `bootstrap.rs`
call `resolve_uncached`, which re-probes and leaves the memo holding the fresh
answer. Nothing else should need it.

| | Before | After |
| --- | --- | --- |
| First resolution in a process | 151 ms | 116 ms |
| Every later one | 151 ms | 0.14 ms |

**These four numbers are design estimates, not measurements.** The workspace has
no benchmark harness, so nothing reproduces them from a `cargo` invocation.
What the code does guarantee independently of the timings is the shape: the
second and every later resolution in a process does no subprocess work at all,
so the win is a removed `claude --version` exec per extra resolution rather than
a percentage. Read the ratio, not the milliseconds.

The first-resolution improvement is a separate fix on the same path:
`binary_finder`'s child wait polled on a 10→100 ms backoff, so a 110 ms
`--version` was noticed at the 150 ms tick. The drain threads already know when
the child's pipes hit EOF, so the wait now sleeps on that instead — with the
same backoff retained as the fallback, because a child can close its stdio and
keep running, and a grandchild can hold the pipes open past its parent's exit.
`try_wait` remains the authority; EOF is only a hint about when to ask.

## The install decision

`decide_install` is a pure function over the resolved target and the latest
published version. It is the entire fix for the reinstall-on-every-launch defect,
and it is unit-testable without touching npm.

```rust
pub enum InstallDecision {
    UseExisting,
    InstallMissing,
    UpgradeOwned,
    /// Nothing healthy resolved, but the evidence is inconclusive rather than
    /// absent: a candidate TIMED OUT rather than answering, or resolution
    /// stopped before examining every candidate. Neither is worth ~339 MB.
    Abstain,
    /// A user-supplied override is broken, and it names a file outside
    /// `amplihack_prefix_bin` — so no install amplihack can perform would
    /// change the answer. Conclusive, unlike `Abstain`; the conclusion is that
    /// installing is futile.
    BrokenOverride,
}

pub fn decide_install(
    resolution: &Resolution,
    latest: Option<&str>,
    amplihack_bin: Option<&Path>,
) -> InstallDecision;
```

It takes the whole `Resolution`, not just its target, because the rejection list
is the difference between "nothing is installed" and "we could not tell".

It takes `amplihack_bin` — the one directory an install writes — because
"conclusive" and "an install fixes it" are not the same claim, and the override
exit below is where they come apart.

| Resolved target | Latest version from registry | Decision |
| --- | --- | --- |
| `None`, and some candidate was `ProbeTimedOut` or `NotProbed` | any | `Abstain` |
| `None`, halted on a user override **inside** `amplihack_bin` | any | `InstallMissing` |
| `None`, halted on a user override **outside** `amplihack_bin` | any | `BrokenOverride` |
| `None`, every rejection conclusive | any | `InstallMissing` |
| Healthy, source is `Path` / `FallbackDir` / `ExplicitOverride` | any | `UseExisting` |
| Healthy, source is `AmplihackPrefix` | `None` (query failed or timed out) | `UseExisting` |
| Healthy, source is `AmplihackPrefix`, version equals latest | `Some` | `UseExisting` |
| Healthy, source is `AmplihackPrefix`, version differs from latest | `Some` | `UpgradeOwned` |

Two rules are load-bearing:

**amplihack never upgrades a binary it does not own.** If the binary that will
actually be executed lives outside `~/.npm-global`, amplihack does not write
anything. Installing into its own prefix would not change what gets launched, so
the "upgrade" would be several hundred megabytes of download with no effect on
the next launch — and the launch after that would decide identically, forever.
Ownership is carried by `TargetSource` and nothing else: a candidate is
`AmplihackPrefix` only when it was found in amplihack's own prefix directory,
and `decide_install` writes only for that source. A prefix directory reached
under some other spelling (a symlink, a trailing slash) is tagged `Path` and
therefore left alone, so the failure mode is *amplihack declines to upgrade* —
never *amplihack writes outside its prefix*.

**A failed registry query never triggers an install.** `latest == None` means the
network was unavailable or slow, not that the local install is stale. A network
blip must not cause a reinstall.

**Inconclusive evidence never triggers an install either.** The same rule, on
the resolution axis. A 3 s `--version` timeout on a loaded box is the same class
of transient as a network blip, and it used to be indistinguishable from
"nothing is installed" — so it bought a reinstall of hundreds of megabytes. `Abstain` says so
instead: `ensure_tool_available` reports which candidate stopped responding, and
tells the user to re-run or to set `{TOOL}_BINARY_PATH`. One candidate timing
out is enough, because the binary that would have answered may be the one that
hung.

#### Stopping early is recorded

Resolution is bounded twice — `MAX_PROBE_CANDIDATES` probes and
`TOTAL_PROBE_BUDGET` in total — and hitting either bound stops the walk with
candidates still unexamined. Those candidates are pushed onto `rejected` as
`Rejection::NotProbed`, which is not a claim about the file: it says only that
resolution never looked.

They have to be there. `decide_install` reads the rejection list as *evidence*,
and a `None` target over a list of conclusive rejections means "there is no
working binary", which buys an install. A truncated walk means nothing of the
sort — the binary that would have answered may be the one past the cap — so
dropping the unexamined candidates made "we stopped looking" indistinguishable
from "nothing is there" and bought an install of hundreds of megabytes that resolves identically
next launch. That is issue #1266's loop, reached through the funnel built to
close it. `NotProbed` maps to `Abstain` for the same reason `ProbeTimedOut`
does.

One early exit deliberately does **not** record `NotProbed`: a user-supplied
`ExplicitOverride` that fails the health gate returns immediately, leaving the
remaining candidates unconsulted on purpose — consulting them is the silent
substitution this module exists to prevent. That is a conclusion, not a
truncation: the evidence is conclusive *for the question that was asked*, and
`decide_install` should read it that way and repair the install. Recording
`NotProbed` there would flip it to `Abstain` and turn a repairable broken
override into a hard error.

#### Conclusive is not the same as repairable

Reading that exit as "install" and stopping there re-creates #1266's loop with a
different first candidate, so the resolution carries the path out with it:

```rust
pub struct Resolution {
    pub target: Option<LaunchTarget>,
    pub rejected: Vec<(PathBuf, Rejection)>,
    /// Set when resolution stopped early on a broken **user-supplied** override.
    pub halted_on_user_override: Option<PathBuf>,
}
```

An install writes exactly one directory, `amplihack_prefix_bin`. So:

* `CLAUDE_BINARY_PATH=~/.npm-global/bin/claude` pointing at the 500-byte
  placeholder **is** repairable — that is the case the exit was written for, and
  refusing it would break the demonstrated repair path.
* `CLAUDE_BINARY_PATH=/opt/vendor/bin/claude` with a typo, a directory, or a
  binary that fails `--version` **is not**. Answering `InstallMissing` spends
  hundreds of megabytes, re-resolves to the same broken override, fails anyway,
  and decides identically on the next launch. Forever.

`BrokenOverride` is the second case. `ensure_tool_available` reports which file
the user named, why it was rejected, and that installing cannot repair a path
amplihack does not write — instead of buying the install first and failing after.

The path is carried rather than inferred because `Resolution.rejected` is
`Vec<(PathBuf, Rejection)>` and drops `TargetSource`, so a pure `decide_install`
otherwise cannot tell "nothing is installed anywhere" from "the one binary the
user named is broken".

Unexamined candidates are **summarised** in `rejection_report`, not listed one
per row. They say nothing about the file, only that resolution stopped, and a
cap hit on a long `$PATH` would otherwise bury the rows that do say something
under dozens of identical ones.

The registry query is skipped entirely when it cannot change the decision: with
no healthy target, or with a target amplihack does not own, `decide_install`
reaches its answer without reading `latest`.

The registry side is asked twice per launch for the same reason the resolution
was — once by the advisory notice, once by the install decision — and each ask
is an `npm show` subprocess (~0.3 s warm on the dev VM, up to the 3 s
`NPM_TIMEOUT` on a slow registry). `get_latest_version` memoizes per package,
including a failed query. Caching the failure is deliberate: the two callers
must agree about it — one saying "unknown" while the other says "1.2.3" is the
class of disagreement this document exists to remove — and `decide_install`
already reads unknown as "never install", so it is the safe direction as well
as the fast one.

`get_installed_version` (`tool_update_check/version.rs`) ran `npm list -g` under
npm's *ambient* prefix — which is not necessarily the prefix amplihack installs
to, and not necessarily where the launched binary lives. It is **deleted**, not
merely demoted.

The design as frozen kept it for the advisory "update available" notice. Running
the fix against the dev VM showed why that is not good enough: with the healthy
`2.1.238` binary selected for launch, the notice still read the ambient prefix
and printed `update available: @anthropic-ai/claude-code 2.1.237 → 2.1.238` —
telling the user to upgrade to the version they were already running. A notice
that names a different binary than the one being launched is the same defect as
installing one, only quieter. `maybe_print_npm_update_notice` now takes its
installed-version side from `launch_target::resolve(tool).target`, and a grep
test enforces that no install decision consults npm's ambient prefix.

## Installing claude's native binary

`@anthropic-ai/claude-code` ships a small placeholder at `bin/claude.exe` and
materializes the real platform-native binary through its `postinstall`
script (`node install.cjs`), which copies the binary out of a platform-specific
`optionalDependencies` package.

Two independent npm flags each prevent that from happening:

| Flag | Mechanism | Result |
| --- | --- | --- |
| `--ignore-scripts` | `install.cjs` never runs | placeholder stub survives |
| `--omit=optional` | the platform package is never fetched, so `install.cjs` resolves nothing | placeholder stub survives |

amplihack keeps **both** flags on every npm invocation and materializes the
binary explicitly instead, in three steps:

1. **Base install** — `run_npm_install` with `-g --prefix <prefix> --omit=optional
   <package> --ignore-scripts`. Byte-for-byte the same invocation used for every
   other package.
2. **Platform package** — one explicit, exactly-named install of the single
   `@anthropic-ai/claude-code-<platform>` package for this host, pinned to the
   base install's version, still with both protective flags.
3. **Materialize** — run the vendor's `install.cjs` from the installed package
   directory with `node`.

Step 3 needs Node. amplihack locates it with `BinaryFinder::find("node")`; if
Node is absent it **warns and skips the step**. amplihack does not download or
manage a Node runtime for this. Node is a stated prerequisite (see
[Prerequisites](PREREQUISITES.md)), and a launcher that silently provisions a
language runtime is a far larger promise than this feature makes. Without Node
the placeholder survives, the health gate rejects it, and resolution falls
through to whatever else on the host is healthy.

This mirrors what `install_npm_package` already does for `@github/copilot`, and
it is why `run_npm_install` needed no change at all: copilot's argv is unchanged
by construction, so [#585](https://github.com/rysweet/amplihack-rs/issues/585)
cannot regress. Installing exactly one platform package by exact name also cannot
reproduce #585's failure mode, which was npm reifying optional dependencies for
*every* platform.

### Success is verified by outcome, not exit code

`install.cjs` exits 0 on most of its failure paths — unsupported platform, a
release channel with no native binaries, and a failed `require.resolve` all
return 0. Its exit code is not a success signal and amplihack ignores it.

Success means `is_materialized` returns true for the resulting file: larger than
1 MiB **and** carrying a native executable magic number.

```rust
pub fn is_materialized(head: &[u8], len: u64) -> bool;
```

This is the one place in the design where validation is genuinely load-bearing
rather than defense-in-depth. Everywhere else, validation is a safety net around
an install that is expected to work.

**`is_materialized` checks completeness, not authenticity.** Magic bytes and a
length say "a native binary landed here", not "the *right* native binary landed
here". `verify_node_archive_sha256`, a few hundred lines away in the same file,
does check a manifest SHA-256 and fails closed — so the asymmetry is visible and
deserves an explanation rather than silence.

The two cases have different backstops. The Node archive arrives over a raw
`curl` to a URL with no integrity metadata attached, so if amplihack does not
check the digest, nothing does. The claude platform package arrives through
`npm install`, which verifies the tarball against the integrity hash in the
registry metadata and fails the install if it does not match. Re-hashing the
extracted file afterwards would be checking npm's work against a digest
amplihack would have to obtain from the same registry — a second read of the
same source of truth, not an independent one. The check that would add real
value is signature verification against a publisher key, which npm provenance
supplies and which is out of scope here.

Every failure path in the three steps warns and returns. None of them fail the
launch: if materialization does not happen, the health gate rejects the stub and
resolution falls through to whatever else on the host is healthy.

### Platform selection

```rust
pub fn claude_platform_packages(os: &str, arch: &str, musl: bool) -> &'static [&'static str];
```

Returns the candidate platform packages in preference order — a slice rather than
a single value, so a musl/glibc misdetection can be corrected by one bounded
retry with the alternate. An empty slice means "no known package for this
platform", which skips the step non-fatally, exactly as the copilot path already
behaves.

Every element is a `&'static str`. That is a security control, not a style
choice: no runtime-derived string can reach npm's argv. The one runtime value
that does — the version read out of the installed `package.json` and pinned onto
the platform install — is validated against an anchored `^\d+\.\d+\.\d+$` regex
and rejected before use — anchored, digit-bounded
(`^\d{1,9}\.\d{1,9}\.\d{1,9}$`), and length-capped at 64 characters.

#### The containment check is anchored to the prefix, not to the package

`install.cjs` is `canonicalize`d and asserted to live under the **canonicalized
npm prefix root** — the directory amplihack itself created — before it is
executed. A failed check warns and skips, like every other step here.

The anchor matters more than the check. The obvious spelling is to canonicalize
`<prefix>/lib/node_modules/@anthropic-ai/`, then assert the script path starts
with it. That is circular: every component of that boundary below the prefix is
package-derived, so if `@anthropic-ai` is itself a symlink to `/tmp/evil`, the
boundary canonicalizes to `/tmp/evil`, the script canonicalizes to
`/tmp/evil/claude-code/install.cjs`, `starts_with` returns **true**, and
amplihack executes attacker-controlled JS with the user's privileges. A path
cannot be allowed to define the boundary it is being checked against.

`npm_prefix_dir()` is the only link in the chain amplihack creates rather than
reads out of a package, so it is the only sound anchor. Containment is checked
against it and against nothing derived from the package name.

This is verified **behaviourally**, not by reading the source: a test builds a
temporary prefix, symlinks `@anthropic-ai` out of the tree, and asserts the
refusal. The distinction is load-bearing — the source-scanning contract test in
`claude_install_contract.rs` passes against the vulnerable spelling too, because
the vulnerable spelling also contains the word `canonicalize`. The source scan
is kept as a ratchet against the check being deleted; the behavioural test is
what proves it works.

The narrow `--ignore-scripts` exception described below is conditional on this
check. Without it, "under a prefix amplihack owns" is an assumption rather than
an assertion, and the exception is not defensible.

musl is detected with a zero-spawn filesystem probe for `/lib/ld-musl-*` and
`/usr/lib/ld-musl-*`, matching what the vendor's own `install.cjs` does when it
reads `process.report.getReport().header.glibcVersionRuntime` instead of
shelling out to `ldd`. Ambiguity defaults to glibc, and a wrong guess only
reorders the candidate list.

## Child process PATH

`augment_claude_launch_env` prepends the directory of the **resolved** target to
the child's `PATH`. `~/.npm-global/bin` is prepended only when the resolved
target actually lives there.

**One spelling of the prefix.** `~/.npm-global/bin` used to be re-derived by
string literal in four places, and `is_already_reachable` compares by value — so
moving the prefix would have broken nothing at compile time and simply made
`claude` unreachable in the child, with no symptom. `amplihack_prefix_bin(home)`
is now the single owner and every consumer calls it, including `bootstrap`'s
`npm_prefix_dir`, which returns its parent because npm's `--prefix` takes the
prefix.

**Nothing relative reaches `prepend_path`.** A relative resolved target's parent
is the *empty* path, `is_already_reachable("")` matches the empty `$PATH`
element that produced it, and prepending the empty path writes a leading colon —
the current directory at the front of the child's `PATH` for the agent, every
subagent and every shell-out. `cheap_reject` now removes relative candidates at
the resolution funnel (see "Every candidate is absolute"), so `resolved` is
already absolute by the time it arrives here; the absoluteness filter at this
site asserts that invariant rather than establishing it, because `resolved` is a
bare `&Path` from a caller this module does not control.

Prepending **moves** an entry to the front; it never **adds** one. The directory
is promoted only if it is already on `PATH`, or if it is amplihack's own npm
prefix (which amplihack owns, and which is routinely missing from a shell `PATH`
captured before the first install). Otherwise a
`CLAUDE_BINARY_PATH=/tmp/x/claude` would put `/tmp/x` ahead of `/usr/bin` for
the child *and every subagent and shell-out in that session*, so `git`, `node`
and `sh` would resolve from there too. Setting the variable already grants
control of the binary amplihack execs — that is what it is for — but it is not
consent to redirect every other binary in the session.

This matters because agents re-exec. A session launched by absolute path from
`/usr/bin/claude` will still resolve bare `claude` from its own `PATH` when it
spawns a subagent or shells out. Unconditionally putting an amplihack-writable
directory ahead of the system directories means any stub in that directory
shadows the working install for the entire session — and on a host where
`~/.npm-global/bin` is already first on the user's `PATH`, for every other shell
on the machine too.

When resolution finds no healthy target, nothing is prepended.

### Promotion still reorders — the limit of the rule, stated

`is_already_reachable` refuses to **add** a directory to the child's `PATH`. It
does not refuse to **promote** one. `CLAUDE_BINARY_PATH=/opt/tools/claude`, where
`/opt/tools` is already the last entry on your `PATH`, moves that directory ahead
of `/usr/bin` for the child — so `git`, `node`, and `sh` resolve from there too,
for the agent and everything it spawns.

The same objection that motivates the add-refusal applies, in weaker form, to
promotion. It is accepted rather than fixed, because the reorder is what the
feature needs: an agent that shells out to bare `claude` has to reach the binary
amplihack selected, and that requires its directory to win the `PATH` search.

Two exemptions are worth naming explicitly, because they are the sharpest edges
in this design:

- **`~/.npm-global/bin` is exempt from the reachability check** — and it is
  precisely the directory that holds the stub this whole document is about.
  Exempting it looks backwards until you note that amplihack *owns* that prefix,
  installs into it, and that it is routinely absent from a `PATH` captured
  before the first install or inside tmux, ssh, or a container. Without the
  exemption, the binary amplihack just installed would be unreachable to the
  session it installed it for. The stub is not the hazard here — the health gate
  has already rejected it, and the directory is only promoted when the
  **resolved** target lives there, which a stub never is.
- **The override case reorders for every binary, not just the named one.** The
  clean fix is a single-symlink shim directory containing only the selected
  binary, which is a larger change than this work and is out of scope.

If you need amplihack to use a specific binary without reordering anything else,
put that binary alone in a directory and point `CLAUDE_BINARY_PATH` at it.

## When the launch cannot proceed

If the binary amplihack was about to execute fails the health gate, amplihack
does not execute it. It falls back to the next healthy candidate in the
resolution order — which includes the fallback directories, not just `$PATH`. If there is no healthy candidate at all, the launch fails with an
error built from `Resolution::rejection_report(tool, package)`:

```
error: no usable claude binary was found

  /home/you/.npm-global/bin/claude   incomplete install — 500-byte placeholder,
                                     the native binary was never materialized
  /home/you/.local/bin/claude        --version did not complete within 3s

  Remedy: install the CLI so its native binary is materialized:
    npm install -g @anthropic-ai/claude-code
  then run `amplihack claude` again.
```

The error names the real cause and states a remedy. Asserted properties, enforced
by test:

- names the actual cause (incomplete install / native binary not materialized)
- states a remedy
- does **not** surface a bare `Exec format error (os error 8)`
- does **not** mention CPU architecture or platform mismatch

The old failure mode was to launch the stub, get `Exec format error (os error 8)`
from the kernel, and hand the user a message that sent them hunting for a
CPU-architecture problem that did not exist. `enrich_spawn_error` translates the
raw OS error through the rejection report so the message describes the thing that
actually went wrong.

Error text carries paths, rejection reasons, and the remedy — never the
environment, never the full argv.

### The report speaks about the tool it was asked about

`rejection_report` and `enrich_spawn_error` take the tool name and the npm
package as parameters. They are the error path for **every** tool, and they used
to hardcode claude's: a copilot user whose launch failed was told "No usable
claude binary was found" and handed `npm install -g @anthropic-ai/claude-code`.

There are also two headlines, because there are two failures:

- **Nothing resolved.** "No usable `<tool>` binary was found", followed by every
  candidate and why each was rejected. The list *is* the story.
- **Something resolved and would not run.** "amplihack selected `<path>`
  (version X) for `<tool>`, and it could not be run." This is the spawn-failure
  path, which by construction runs only after a target was resolved — so the
  binary that failed is precisely the one *missing* from the rejection list, and
  the "nothing was found" headline over a list that does not contain it (and is
  usually empty) was simply false.

### SEC-3: what reaches the terminal

Probe stdout is whatever an arbitrary candidate binary chose to print, and a
candidate *path* can itself be planted. Both go through one shared `strip_ansi`
(`binary_finder`; there must not be a second copy) before rendering.

**Every** renderer of these strings strips, not just `rejection_report`.
`enrich_spawn_error` formats the selected binary's path into its headline —
same provenance, same planted-filename hazard, and it runs on the failure path,
at the exact moment the user is being told what command to run next. A newline
in a path forges a plausible extra `cause:` line; `ESC ]52` writes the user's
clipboard. The crate's `SEC-WS2-02` rule is unqualified, and the two tests that
hold `rejection_report` to it — `rejection_report_strips_ansi_from_candidate_paths`
and `a_newline_in_a_candidate_path_cannot_forge_a_report_row` — are mirrored for
`enrich_spawn_error`.

Stripping removes:

- **CSI** — `ESC [` … final byte in `0x40..=0x7e`
- **String sequences** — `ESC ]` (OSC), `ESC P` (DCS), `ESC X`, `ESC ^`,
  `ESC _`, each up to `BEL` or `ST`. OSC 52 writes the user's clipboard and
  OSC 0 rewrites the window title; a CSI-only implementation let both through.
- **Two-byte escapes** — `ESC` plus one final byte, which covers `ESC c` (RIS,
  a full terminal reset)

Every remaining C0 control (and `DEL`) becomes a **single space**, except tab.
A space and not a deletion, deliberately: deleting them splices `1.2.3\n4.5.6`
into `1.2.34.5.6`, which the semver regex reads as `1.2.34` — a version that was
never printed. The practical case is `LF` and `CR`: the report renders
`"\n  {path}\n      {reason}\n"`, so a `$PATH` entry containing a newline could
otherwise forge convincing extra rows and make a rejected candidate read as a
healthy one.

## Configuration reference

| Variable | Effect |
| --- | --- |
| `AMPLIHACK_CLAUDE_BINARY_PATH` | Explicit binary to use. Must pass the health gate; a broken override set *in your environment* is an error, not a fallback. amplihack also sets this variable internally for `amplihack rustyclawd`; that case warns and falls through instead. See [The override is also set programmatically](#the-override-is-also-set-programmatically). |
| `CLAUDE_BINARY_PATH` | Same, checked second (parity with the Python implementation). |
| `RUSTYCLAWD_PATH` | Read only by `amplihack rustyclawd`. Names a preferred `rustyclawd` binary; if it is an executable file, `configure_preferred_rustyclawd_binary` sets `AMPLIHACK_CLAUDE_BINARY_PATH` to it and the ordinary claude launch path takes over. It is checked with `is_executable_file` alone, so a binary named here that then fails the health gate warns and falls through — it does not hard-fail the launch. |

There is no environment variable that disables the health gate. A binary that
cannot report its version is not launched.

## Verifying behavior on your own host

Run `amplihack claude` twice and compare:

```sh
amplihack claude --version
# 📦 Installing claude via npm package @anthropic-ai/claude-code...
# 📦 Installing platform binary @anthropic-ai/claude-code-linux-x64...
# 2.1.238 (Claude Code)

amplihack claude --version
# 2.1.238 (Claude Code)
```

The second run performs no npm work at all.

### Is the native binary actually materialized?

The materialization target is `bin/claude.exe` inside the package directory.
Inspect **that file**, not the `bin/` entry that points at it:

```sh
CC=~/.npm-global/lib/node_modules/@anthropic-ai/claude-code

ls -l "$CC/bin/claude.exe"
# -rwxr-xr-x 1 you you 338860336 ... claude.exe

file "$CC/bin/claude.exe"
# ELF 64-bit LSB pie executable, x86-64, ...
```

A **~500-byte** `claude.exe` that `file` reports as `ASCII text` is the stub. Its
first line is `echo "Error: claude native binary not installed." >&2`. If you see
it, resolution rejects it and says so.

**`~/.npm-global/bin/claude` is a symlink**, so inspecting it without
dereferencing describes the link, not the binary — 60 bytes and `lrwxrwxrwx`,
which tells you nothing about materialization:

```sh
ls -l ~/.npm-global/bin/claude
# lrwxrwxrwx 1 you you 60 ... claude -> ../lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe

file ~/.npm-global/bin/claude
# symbolic link to ../lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe
```

To go through the symlink, dereference explicitly:

```sh
ls -lL  ~/.npm-global/bin/claude    # or: stat -Lc '%s' ~/.npm-global/bin/claude
file -L ~/.npm-global/bin/claude
```

Note that `cli.js` is **not** the materialization target and its size says
nothing about whether the native binary exists.

To confirm amplihack does not degrade a working install it does not own, record
size, version, and inode before and after a launch:

```sh
stat -Lc '%i %s' /usr/bin/claude && /usr/bin/claude --version
amplihack claude --version >/dev/null
stat -Lc '%i %s' /usr/bin/claude && /usr/bin/claude --version
# identical before and after
```

`-L` dereferences, so this reports the real binary even when the path on `$PATH`
is a symlink — which it usually is.

## Design notes

### Why one resolver

Before this design there were three independent resolutions in a single launch:
the version check read whatever `npm list -g` reported under npm's ambient
prefix, the install wrote into `~/.npm-global` via `--prefix`, and the exec
picked the first binary *found* on `$PATH`. On any host where those three
disagree — which is every host where npm's global prefix is not the directory
`claude` is served from — the version check compares a version that has nothing
to do with the binary being run, concludes an upgrade is needed, installs
somewhere that is never selected, and reaches the same conclusion on the next
launch. Forever.

Adding a health gate to that arrangement does not fix it; it just makes the
useless reinstall survivable. The fix is that check, install, and launch resolve
through one function.

### What this replaces

`ensure_claude_cli` (`crates/amplihack-utils/src/claude_cli.rs`) is the
pre-existing second resolver-and-installer. The single-resolver rule means it is
deleted, and the `ClaudeCliError` variants that only it constructed go with it.

`check_claude_version` in the same module is deleted on the same rule. It had no
callers, but a dead duplicate is not inert: it was a second answer to "what
version is installed" (its own `<binary> --version` probe) and a second answer to
"what version is published" (its own `npm view` query), competing with this
module and with `tool_update_check`. Those two questions disagreeing *is* issue
#1266. The surviving implementations memoize, bound the subprocess, and sanitize
registry output before believing it; that copy did none of the three, so it was
a working example of the bug for whoever grepped for "version check" next.

With both gone, `ClaudeCliError` and `VersionStatus` are unconstructible and are
deleted too, leaving `get_claude_cli_path` — one line of delegation to
[`resolve`] — as the module's entire surface. That last line went as well, and
with it the `claude_cli` module: it had no callers. The one consumer that might
have used it, `launcher_core`, deliberately wrote its own private resolver
instead, because it needs [`resolve`]'s rejection report on failure and the
shim flattened that to `None`. A zero-caller public alias for [`resolve`] is a
second name for the single resolver, which is the thing this page exists to
prevent — callers use [`resolve`] directly.

`ClaudeCliError`, `VersionStatus`, and the `claude_cli` module itself were
`pub`, so that is a **public API change**. The two types, the removed variants,
and the module are enumerated in the PR body rather than here, so this page does
not carry a list that drifts.

### What #585 was actually about

[#585](https://github.com/rysweet/amplihack-rs/issues/585) was
`amplihack copilot` hanging while npm reified platform-specific optional
dependencies for *every* platform. The fix was `--os`/`--cpu`, since evolved into
`--omit=optional` plus an explicit single-platform follow-up install. Postinstall
scripts appear nowhere in #585's diagnosis or its remedy.

`--ignore-scripts` is asserted by a contract test that lives in
`tests/issue_585_copilot_npm_hang.rs` because that is where the npm-flag
assertions ended up, not because it was part of #585's fix. It is a generic
supply-chain protection and it is retained unchanged, for every package, on every
invocation.

### The threat model, stated honestly

The residual delta introduced by materializing claude's native binary is exactly
one named script, at an absolute path, under a prefix amplihack owns, for one
exactly-matched package name — run immediately before amplihack executes that
same package's native binary.

Declining to run a package's own postinstall while planning to exec its native
binary seconds later is not a coherent security posture. The postinstall is
strictly less privileged than what immediately follows it.

This is deliberately narrower than a script allowlist would be. An allowlist
re-enables *arbitrary* lifecycle scripts for a class of packages; this re-enables
*one script* for one exact package name. Exact string equality is enforced by
test, with negative cases for near-miss spellings such as
`@anthropic-ai/claude-code-evil` and `claude-code`.

### Environment variables are not a security boundary

Anyone who can set `AMPLIHACK_*` in this process's environment can already
execute code as this user. The override variables are a usability affordance, and
the health gate is a correctness control that stops amplihack from running things
that do not work. Neither is an integrity control and neither should be described
as one.

## Related documentation

- [System Prompt Append](SYSTEM_PROMPT_APPEND.md) — the other half of the launch path: how amplihack's routing contract reaches the agent
- [`amplihack copilot` — Subprocess-Safe Defaults](COPILOT_SUBPROCESS_SAFE.md) — the sibling flag-injection design in the same launch path
- [Copilot CLI](COPILOT_CLI.md) — the copilot install path this design mirrors
- [Prerequisites](PREREQUISITES.md) — npm and Node requirements
- [Security Recommendations](SECURITY_RECOMMENDATIONS.md) — repository-wide security posture
