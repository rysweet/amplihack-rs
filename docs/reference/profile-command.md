# Reference: `amplihack profile`

Manage Claude Code login profiles. See the
[how-to](../howto/claude-profiles.md) for a walkthrough.

## Synopsis

```
amplihack profile add <name>
amplihack profile list
amplihack profile show
amplihack profile switch <name>
amplihack profile remove <name> --yes
amplihack profile login <name>
amplihack profile run <name> -- <claude-args>...
amplihack launch|claude --profile <name> [...]
```

## Subcommands

| Command | Behavior |
|---------|----------|
| `add <name>` | Copy allowlisted shared assets from `~/.claude` into a private staging directory, then atomically publish `<root>/<name>/` (mode `0700` on Unix; Windows uses its inherited ACL). Errors if a profile exists whose name matches case-insensitively, on every platform. Does not run Claude. |
| `list` | One profile per line, sorted; the default is marked `*`. Skips dot-entries, symlinks and non-directories. Reports a corrupt or dangling default pointer instead of silently omitting the marker. |
| `show` | Prints `name: <name>` and `dir: <path>`, or `none`. Paths only; never directory contents. |
| `switch <name>` | Writes the default pointer. The profile must exist. |
| `remove <name>` | Requires `--yes`. Resolves and verifies the path is inside the root, reads the default pointer, deletes the directory, then clears a matching pointer. A missing pointer means no default; a corrupt, unreadable or dangling pointer fails closed before any deletion (repair with `switch`). Never deletes the root; never follows symlinks. |
| `login <name>` | Always runs the `claude` binary (ignores binary selection), interactively, with `CLAUDE_CONFIG_DIR=<profile dir>`. |
| `run <name> -- <args>` | One-shot launch through the normal launch path. Args are forwarded as a literal argv vector; no shell is involved. |

## Configuration

| Item | Value |
|------|-------|
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Profile root. Must be absolute and non-empty. Default `~/.amplihack/claude-profiles/`. |
| `AMPLIHACK_CLAUDE_PROFILE` | Profile name a launch passes to its children so nested launches use the same profile. Set by amplihack on the child process; may also be set by hand. Empty means unset. See [Resolution and environment](#resolution-and-environment). |
| `<root>/<name>/` | Profile directory (mode `0700` on Unix; Windows uses its inherited ACL). |
| Root directory | Created with mode `0700` on Unix; Windows uses its inherited ACL. A symlinked root is refused. An existing custom root is trusted input. |
| `<root>/.default` | Pointer file: only the profile name (mode `0600` on Unix, max 256 bytes). Windows uses its inherited ACL. Written by creating a new temp file (`create_new`, never through a symlink) and renaming it over the pointer. |

### Seeding

`add` calls the store's `seed_shared_assets`, which copies from the source
`$HOME/.claude` into the new profile by value. The allowlists are fixed
constants:

| Constant | Contents |
|----------|----------|
| `SHARED_ASSET_FILES` | `CLAUDE.md` |
| `SHARED_ASSET_DIRS` | `agents`, `commands`, `output-styles`, `rules`, `skills` |
| `SHARED_SETTINGS_KEYS` | top-level `settings.json` keys: `alwaysThinkingEnabled`, `attribution`, `cleanupPeriodDays`, `hooks`, `includeCoAuthoredBy`, `language`, `model`, `outputStyle`, `permissions`, `respectGitignore`, `spinnerVerbs`, `statusLine` (12 keys) |

Rules:

- `settings.json` is parsed and only `SHARED_SETTINGS_KEYS` are written to the
  profile's `settings.json`. If nothing survives the filter, no file is
  written. `env`, `apiKeyHelper` and all unknown keys are dropped.
- On Unix, directories are created `0700` and files `0600`; Windows uses
  inherited ACLs. Files use `create_new` (`O_EXCL` on Unix), so nothing is
  overwritten and no destination symlink is followed.
- The 12-key list is kept in sync with the code by `tests_docs_sync.rs`, which
  checks both directions (every constant key is documented here and in the
  how-to guide, and no extra key is documented).
- A symlinked source root fails with `SymlinkRefused`. Nested symlinks and
  special files (FIFOs, sockets, devices) are skipped. On Unix the source root
  is opened exactly once with `O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` and
  that descriptor anchors the whole copy. The five allowlisted directories,
  `CLAUDE.md` and `settings.json` are opened with `openat(root_fd, name, ...)`
  and `O_NOFOLLOW`, never by absolute path, so replacing `~/.claude` (or any
  ancestor) with a symlink after the anchor is taken cannot redirect the copy:
  the descriptor still pins the original directory. Ancestors of the root are
  resolved once, at anchor time. Each directory is opened with
  `O_NOFOLLOW|O_DIRECTORY` and entries are read and stat'ed relative to it
  (`openat`/`fstatat` with `AT_SYMLINK_NOFOLLOW`), so a directory swapped for a
  symlink during the copy is never followed. Files are opened `O_NOFOLLOW`
  and `O_NONBLOCK`, verified as regular files with `fstat` on the opened
  descriptor, and read through that same descriptor with a bounded read. Entry
  names must be a single path component. A missing `CLAUDE.md` or
  `settings.json` is skipped. Only the final component of the root is
  no-follow: a symlinked ancestor of `~/.claude` is resolved normally, once.
  Windows opens source files with `FILE_FLAG_OPEN_REPARSE_POINT` and rejects
  reparse-point handles; its path-based directory traversal still permits
  same-user ancestor/directory races. Other non-Unix platforms keep the
  path-based copy without descriptor-relative hardening.
- A missing `~/.claude` source produces an empty profile. An existing source
  that is not a directory fails the add and rolls back the new profile.
- The profile root cannot be inside the shared source tree. This is checked
  before the root is created, including for missing path suffixes, to prevent
  recursive seeding.
- Caps: nesting is limited to 16 levels, a seed to 5,000 entries, each file to
  64 MiB, and `settings.json` to 1 MiB; exceeding a cap is an error. Special
  files are skipped without being opened, so a FIFO cannot block the copy.
- Excluded entirely: `.credentials.json`, `.claude.json`, `history*`,
  `projects`, `sessions`, `todos`, `statsig`, `shell-snapshots`, `ide`,
  and every other entry not on the allowlists (so plugins and MCP server
  settings are not carried over).
- Invalid `settings.json` yields `InvalidSharedSettings`. A seeding error
  removes the private staging directory so no profile is published; if cleanup
  also fails, the error reports both failures (never a pre-existing profile).
- Values are never logged; messages contain paths and counts only.
- No sync after creation: later edits to `~/.claude` do not update profiles.

## Name rules

Pattern `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$` (validated before any path join). Additionally rejected: `..`
anywhere, trailing `.`, path separators, NUL, and Windows-reserved names
(`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`, also with an extension,
case-insensitive). `add` rejects names that differ only by case from an existing profile on
every platform, so `Work` and `work` can never coexist.

## Resolution and environment

`amplihack claude`, `amplihack launch` and `amplihack profile run` pick a
profile in this order:

1. `--profile <name>` (or the `<name>` given to `profile run`). The inherited
   variable and the pointer are not read, so a dangling or corrupt pointer
   does not block it.
2. An inherited, non-empty `AMPLIHACK_CLAUDE_PROFILE`. The pointer is not
   read, even if it is dangling.
3. `<root>/.default`
4. none — `CLAUDE_CONFIG_DIR` is left exactly as inherited.

When a profile resolves, three variables are set on the child `Command` only
(never `set_var`), and only for Claude-compatible binaries. This applies to
every command that starts Claude with a profile: `amplihack claude`,
`amplihack launch`, `amplihack profile run` and `amplihack profile login`.

| Variable | Value |
|----------|-------|
| `CLAUDE_CONFIG_DIR` | `<root>/<name>`, overriding any ambient value |
| `AMPLIHACK_CLAUDE_PROFILE` | `<name>` |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | `<root>`, only when `AMPLIHACK_CLAUDE_PROFILE_ROOT` was set in the launching process; otherwise left unset so the child uses the default root too |

Copilot and Codex children get none of these, but variables already in the
environment pass through them unchanged.

### Nested launches keep the parent's profile

Every `amplihack claude` started inside a profiled session (recipe agent
steps, `multitask`/orch workstreams, a nested `amplihack claude` run by hand)
inherits `AMPLIHACK_CLAUDE_PROFILE` and so resolves the same profile at tier 2.
The pin holds for the whole tree of sessions:

- A later `amplihack profile switch` only rewrites the pointer. Running
  sessions, and every launch started from them, keep their profile.
- A parent started with `--profile work` while the pointer names `personal`
  runs every descendant as `work`.

How the name reaches each kind of descendant:

| Launch path | How the variables arrive |
|-------------|--------------------------|
| Direct child, `amplihack recipe run` | Inherited environment (the recipe runner is detached with `setsid` but keeps the environment). |
| Recipe agent steps | Inherited environment. `recipe-runner-rs` builds each step's environment from its own, and never drops `AMPLIHACK_*` variables when it trims the environment to fit its budget. The only `AMPLIHACK_*` value it removes is one whose `KEY=value` string is over the kernel's 131,072-byte per-string limit, which no child could receive anyway; a profile name and root are far smaller. In a bash step, values the recipe itself supplies (context keys and their uppercase aliases) that are too large for the environment become shell-local variables and are not exported (checked at the revision pinned in `claude-plugin/recipe-runner.rev`, `4d5187e`). Agent steps take no per-step environment. In bash steps, a step's `env:` block can set any variable, including these, and so can the runner's uppercase export of context keys (see [Recipe context cannot set the profile](#recipe-context-cannot-set-the-profile)). |
| `multitask` / orch workstreams | Inherited environment: each workstream's `run.sh` is started with the parent's full environment. As a backup, the generated `run.sh` also exports `AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT`, single-quoted, when they are set and non-empty, so a `run.sh` re-run by hand or on resume uses the same profile. `CLAUDE_CONFIG_DIR` is not exported; the nested launch rebuilds it from the name and checks it again. If either variable is set but is not valid UTF-8, the workstream is refused before any script is written (the error names the variable, not its value), because leaving it out would send the nested launch to the default root. |

Only the name and the root are passed on. Each nested launch resolves the
name against the root and runs the full check below again, so an inherited
`CLAUDE_CONFIG_DIR` is never trusted on its own.

An inherited value is checked exactly like a flag: it is parsed with the
[name rules](#name-rules) without trimming (so `" work"` is invalid), resolved
inside the root, and re-validated before spawn. Any failure — an invalid
name, a value that is not valid UTF-8, a profile that no longer exists, a
symlinked or out-of-root directory — aborts the launch with the
inherited-profile error. It never falls back to the pointer or to no profile.

You can set the variable by hand to pin a profile for one shell or command:

```bash
AMPLIHACK_CLAUDE_PROFILE=work amplihack recipe run default-workflow
```

A stale export in a shell startup file acts as a per-shell default that beats
the pointer; `unset AMPLIHACK_CLAUDE_PROFILE` removes it.

`amplihack recipe run` and `amplihack multitask` do not resolve a profile
themselves. Started from a plain shell with no inherited
`AMPLIHACK_CLAUDE_PROFILE`, each agent step resolves the pointer on its own,
so a `profile switch` part way through moves later steps to the new profile.
Start them from inside an `amplihack claude` session, or set
`AMPLIHACK_CLAUDE_PROFILE` as above, to pin one profile for the whole run.

Fleet and remote VM launches do not pass the profile on, so nested launches
inside them use the pointer.

Copilot and Codex launches don't set the variables themselves, but they don't
remove an inherited value either. A nested `amplihack claude` started from a
Copilot session that inherited `AMPLIHACK_CLAUDE_PROFILE` uses that profile
and checks it again. Without it, the launch uses the pointer.

### Recipe context cannot set the profile

`amplihack recipe run` refuses any context key whose uppercase form starts
with `AMPLIHACK_`, on both the `--set` and the `--context-file` path, before
the runner starts:

```text
recipe context key "amplihack_claude_profile" is reserved: the AMPLIHACK_ namespace cannot be set from recipe context
```

Only the key is printed, never its value. Keys that merely contain
`amplihack` (such as `use_amplihack`) are allowed.

Known limitation: the recipe runner itself still exports every top-level
context key to bash steps under its uppercase name. A recipe's own YAML
`context:` default or a step `output:` named `amplihack_claude_profile` (or
`amplihack_claude_profile_root`) is not checked by amplihack and can override
the pinned value for an `amplihack claude` run inside a bash step. The name
and directory are still validated, so this can only select one of your own
profiles. The runner fix is tracked in
[rysweet/amplihack-recipe-runner#146](https://github.com/rysweet/amplihack-recipe-runner/issues/146).

### Pre-spawn check

Immediately before spawn, the profile path is
re-resolved and re-validated: it must be a real directory (not a symlink),
its canonical path must equal the expected path under the profile root, and
it must be owned by the current user with no group/other permission mode bits
(`mode & 0o077 == 0`: `0700` passes; `0755`, `0750` and `0705` fail). ACLs are
not inspected. Only the profile directory is checked, not its parents.
If the check fails, run `chmod go-rwx <profile-directory>` on Unix.
Any failure aborts the launch. This check applies only to profile launches;
Claude and Copilot launches without a profile are unchanged.
Ownership and group/other mode-bit checks are available on Unix. On other
platforms, profile launches still require a real, contained directory, but
the operating system does not expose the same checks through this command.

The profile selects a configuration directory; it does not change Claude's
authentication environment. Claude Code documents separate directories for
claude.ai logins and API-key accounts, but two Claude Console sign-ins without
API keys are not isolated because their credentials live outside the
configuration directory. An inherited `ANTHROPIC_API_KEY` can also select API
billing instead of a subscription login. See the official
[multiple-account](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts)
and [environment variable](https://code.claude.com/docs/en/env-vars)
documentation.

`profile run` applies the same check as `launch --profile`: if Docker is
active (including `AMPLIHACK_USE_DOCKER`), it fails with the unsupported-mode
error before resolving or creating anything. It never silently drops the
profile.

`--profile` together with `--docker` or `--auto` fails with an
unsupported-mode error before anything is spawned. This includes Docker
activated through the environment (`AMPLIHACK_USE_DOCKER`). With Docker
activated that way and only a default pointer, the pointer is not applied and
a warning is printed.

An inherited, non-empty `AMPLIHACK_CLAUDE_PROFILE` is refused the same way.
With `--docker`, `--auto` or Docker activated through the environment, the
launch fails with the inherited-profile error and reason `not applied with
--docker, --auto or Docker`:

```
error: inherited AMPLIHACK_CLAUDE_PROFILE "work": not applied with --docker, --auto or Docker
hint: unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto
```

The refusal happens before the value is parsed and before the profile root
is opened, so the value drives no filesystem reads. A value that is not
valid UTF-8 gets the same refusal, not an invalid-name error. An empty value
counts as unset and does not trigger it.

These modes never set, change or re-check `CLAUDE_CONFIG_DIR`. A launch
that gets past the refusal keeps whatever `CLAUDE_CONFIG_DIR` it inherited,
with no pre-spawn check. Inside a profiled session that is still the
parent's profile directory, so unsetting `AMPLIHACK_CLAUDE_PROFILE` alone
would run an `--auto` launch on the parent's account, not the default one.
Unset both variables for that one command:

```bash
env -u AMPLIHACK_CLAUDE_PROFILE -u CLAUDE_CONFIG_DIR amplihack claude --auto -- -p "summarise the README"
```

A `--docker` launch does not forward `CLAUDE_CONFIG_DIR` into the
container, but unsetting both is still the safe form to use.

Amplihack's own orchestration (recipe agent steps, `multitask`/orch
workstreams) never passes `--docker` or `--auto` to nested launches. It does
not clear `AMPLIHACK_USE_DOCKER`, though. A profiled session can carry a
truthy `AMPLIHACK_USE_DOCKER` if you export it inside the session, or if it
was exported when the session started but the Docker daemon was not running
(env-activated Docker only applies when the daemon is up). Once the daemon is
running, every nested launch of a recipe or `multitask` run started from that
session is refused with the error above. Unset `AMPLIHACK_USE_DOCKER` before
starting the run. Inside a container `AMPLIHACK_USE_DOCKER` has no effect,
so nested launches there are not refused.

A default pointer on its own is never an error in these modes: with
`--docker` or `--auto` it is not read, and with Docker activated through the
environment it is skipped with the warning above.

A symlinked profile root is refused by every subcommand and by the pointer
reads/writes; a missing root is treated as empty.

## Literal argument forwarding

`run <name> -- <args>` passes args as an argv vector with no shell, so
`amplihack profile run work -- "; rm -rf ~"` hands that string to Claude
as one literal argument and executes nothing.

## Errors

Errors print one line to stderr as `error: <message>` plus `hint: <next step>`
and exit with status 1. Messages never include file contents.

| Condition | Hint |
|-----------|------|
| Invalid name | Lists the allowed pattern |
| Profile exists / not found | `amplihack profile list` |
| Symlinked or out-of-root profile | Remove the symlink and re-add |
| Pointer dangling (names a missing profile) | `amplihack profile switch <name>` |
| Pointer corrupt or oversized | Re-run `profile switch <name>` |
| `remove` without `--yes` | Re-run with `--yes` |
| `--profile` with `--docker`/`--auto` | Launch without the flag combination |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` with `--docker`, `--auto` or env-activated Docker | `unset AMPLIHACK_CLAUDE_PROFILE and CLAUDE_CONFIG_DIR to run with --docker/--auto` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` is an invalid name or not found | Unset `AMPLIHACK_CLAUDE_PROFILE`, or create the profile with `amplihack profile add <name>` |
| Inherited `AMPLIHACK_CLAUDE_PROFILE` names a profile that exists but cannot be used | The hint for the underlying cause, e.g. `check permissions on the profile root` for an I/O error or the `chmod go-rwx` hint for unsafe permissions |
| Recipe context key in the `AMPLIHACK_` namespace (`recipe run -c amplihack_…=…`) | Remove the key; the namespace is reserved |
| Invalid `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Use an absolute path |

The inherited-profile message names `AMPLIHACK_CLAUDE_PROFILE`, shows the
value escaped (Rust `{:?}` form) and cut to 64 characters, and gives a fixed
reason: `invalid name`, `not found`, `refused path`, `unsafe permissions`,
`i/o error on profile root (<kind>)`, `not usable` or
`not applied with --docker, --auto or Docker`. `<kind>` is the I/O error
kind, such as `permission denied`; it never contains the value. For
example, as printed to stderr:

```
error: inherited AMPLIHACK_CLAUDE_PROFILE "work": not found
hint: unset AMPLIHACK_CLAUDE_PROFILE, or create the profile with `amplihack profile add <name>`
```

Control characters in the value are escaped, so a malformed value cannot
write terminal escape sequences.

## Security notes

When creating a profile amplihack reads the allowlisted assets and parses
`settings.json`; it never modifies the source directory and never copies
credential or history files. Allowlisted setting values and asset contents are
copied verbatim, so any secrets embedded in hooks, status-line commands or
skills remain the user's responsibility. Unix source reads are anchored to a
no-follow root descriptor; non-Unix platforms have a residual same-user
check-then-open race. Profiles are not a security boundary against the same
OS user.
The pre-spawn re-validation narrows but does not close the window between the
check and `exec`; it is not atomic. This is defense in depth: anyone who can
write the profile store already has the user's privileges.
