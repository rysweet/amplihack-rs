---
title: Keep Tests from Prompting for Git Credentials
type: howto
updated: 2026-10-30
related: issue #1764, issue #1278, issue #1606
---

# Keep Tests from Prompting for Git Credentials

Some tests give a repository a placeholder remote, such as
`https://dev.azure.com/org/project/_git/repo`. When git reaches that remote
and nothing supplies credentials, it asks for a user name on the terminal.
Before #1764, `timeout --kill-after=60 5400 cargo test --workspace --locked`
run from a terminal did not fail when that happened: 116 test and git
processes stopped (state `T`) and the run waited until it was killed.

This guide is for contributors who write tests, and for anyone who runs a long
command from a terminal or asks an agent to.

## Contents

- [How the trap works](#how-the-trap-works)
- [What the repository sets](#what-the-repository-sets)
- [Write a test that runs git](#write-a-test-that-runs-git)
- [The guard test](#the-guard-test)
- [Run a long command by hand or from an agent](#run-a-long-command-by-hand-or-from-an-agent)
- [Behaviour changes for users](#behaviour-changes-for-users)
- [`cargo run` and the `[env]` table](#cargo-run-and-the-env-table)
- [Reproduce the trap](#reproduce-the-trap)
- [Limits](#limits)

## How the trap works

`timeout` without `--foreground` moves itself and its command into a new
process group. Started from a terminal, that group is in the background. When
a process in a background group reads the terminal, the kernel sends it
`SIGTTIN`, which stops it. Nothing fails and nothing exits.

Git reads the terminal when a remote asks for credentials and nothing else
supplies them. It opens `/dev/tty` directly, so a null stdin does not prevent
the prompt; a run with stdin from `/dev/null` stopped in the same way. Tests
still get a null stdin, because it helps programs that read stdin.

What happens next depends on the `timeout` installed:

| `timeout` | On `SIGTTIN` | Result |
|-----------|--------------|--------|
| GNU coreutils (the Ubuntu CI runners) | Ignores it and keeps its alarm | Kills the command at the deadline and exits 124. The run waits for the whole deadline. |
| uutils coreutils (0.12 observed) | Is stopped together with git, so its alarm never fires | Waits indefinitely, until something else kills it. |

Check which one you have:

```bash
timeout --version | head -1
# Output on a uutils host: timeout (uutils coreutils) 0.12.0
```

The trap needs a controlling terminal. CI jobs have none, and nor does a
command run under `setsid`. There git cannot open `/dev/tty` and fails at once
with `No such device or address`, which is why CI never stopped.

### The order in which git looks for credentials

Git tries these in turn and uses the first that gives an answer:

1. Credential helpers configured with `credential.helper`. A helper that
   answers ends the search, so helpers keep working under every setting in
   this guide.
2. An askpass program: `GIT_ASKPASS`, otherwise `core.askPass`, otherwise
   `SSH_ASKPASS`. Git runs it and reads the answer from its stdout. `true`
   prints nothing and exits 0, so git sends an empty user name and password;
   the server refuses them and git exits with `Authentication failed`. If the
   program cannot be started, or exits with a non-zero status, git prints
   `unable to read askpass response` and goes on to step 3.
3. The terminal. If `GIT_TERMINAL_PROMPT` is `0`, git stops with
   `terminal prompts disabled`. Otherwise it opens `/dev/tty` and asks.

`GIT_TERMINAL_PROMPT=0` on its own therefore stops the prompt. An askpass
variable on its own does not: if the program is missing or fails, git falls
back to the terminal. This repository sets the askpass variables only together
with `GIT_TERMINAL_PROMPT=0`.

### What git prints

The message tells you which setting stopped git. These were observed with
git 2.56 against a server that refuses every request:

| Environment | Git prints |
|-------------|------------|
| All three variables, as under cargo | `fatal: Authentication failed for '<url>/'` |
| `GIT_TERMINAL_PROMPT=0` and no askpass program: `amplihack` run outside cargo, guard parts (b) and (c1) | `fatal: could not read Username for '<host>': terminal prompts disabled` |
| `GIT_TERMINAL_PROMPT=0` and an askpass program that fails | `error: unable to read askpass response from '<program>'`, then `terminal prompts disabled` as above |
| No variables, no controlling terminal (CI, `setsid`) | `fatal: could not read Username for '<host>': No such device or address` |
| No variables, a terminal, plain `timeout` | Nothing. Git stops on `SIGTTIN`. |

## What the repository sets

| Layer | Where | Sets | Covers |
|-------|-------|------|--------|
| Cargo `[env]` | `.cargo/config.toml` | The three variables | Every process cargo starts (test binaries under `cargo test` and `cargo nextest run`, `cargo run`, build scripts), and their children unless a child's environment is cleared |
| Test helper | `tests/support/isolated_amplihack.rs` | The three variables and a null stdin | Every `isolated(..)` child, and any test command that calls `no_git_prompt` after `env_clear()` |
| Git builders | `crates/amplihack-git/src/lib.rs` | `GIT_TERMINAL_PROMPT=0` | Every git that amplihack starts through `command()`, `command_in()`, `tokio_command()` or `tokio_command_in()`, in production and in tests |
| Shell tests | The top of each shell test that runs the system git against a remote that is not local | The three variables and a null stdin | The scripts when run by hand with `bash`, outside cargo |
| Recipe prompts | `workflow-precommit-test.yaml` steps 12 and 13, `autodrive-merge-round.yaml` step 4 | Guidance | Long commands that agents run |

The three variables:

| Variable | Value | Effect |
|----------|-------|--------|
| `GIT_TERMINAL_PROMPT` | `0` | Git never asks on the terminal. If no askpass program answers, it fails with `terminal prompts disabled`. |
| `GIT_ASKPASS` | `true` | Git runs `true` for each credential and gets an empty answer. The server refuses it and git fails with `Authentication failed`. |
| `SSH_ASKPASS` | `true` | The same, for git's last askpass fallback |

Where all three are set, as in every process cargo starts, git runs `true`
before it reaches the terminal check. A test that meets a remote needing
credentials therefore fails with `Authentication failed`, not
`terminal prompts disabled`.

### The cargo `[env]` table

`.cargo/config.toml` holds this table and nothing else:

```toml
[env]
GIT_TERMINAL_PROMPT = { value = "0", force = true }
GIT_ASKPASS = { value = "true", force = true }
SSH_ASKPASS = { value = "true", force = true }
```

`force = true` replaces a value already in the environment, such as an IDE's
askpass helper, so a test never inherits a developer's own prompt program. A
cargo config in the repository applies to everyone who builds it, so review
any change to this file line by line.

Cargo also passes `[env]` to `rustc`. Do not read these variables with `env!`
or `option_env!`, which would build the values into a binary.

### `isolated(..)` and `no_git_prompt`

`env_clear()` removes every variable from a `Command`: the `[env]` values the
test process received, and any variable already set on that `Command`.
`tests/support/isolated_amplihack.rs` therefore provides a constant and a
helper that set the variables again:

```rust
pub const NO_GIT_PROMPT_ENV: [(&str, &str); 3] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", "true"),
    ("SSH_ASKPASS", "true"),
];

/// Sets `NO_GIT_PROMPT_ENV` and a null stdin on `command`, and returns it.
pub fn no_git_prompt(command: &mut Command) -> &mut Command
```

`isolated(..)` calls `no_git_prompt` after it clears the environment, so every
child started through it gets the three variables and a null stdin.

### The git builders

`amplihack_git` has five builders. All five remove every variable in
`REPOSITORY_ENV_VARS` ([issue #1278](https://github.com/rysweet/amplihack-rs/issues/1278)).

| Builder | Kind | Sets `GIT_TERMINAL_PROMPT=0` | Use for |
|---------|------|------------------------------|---------|
| `command()` | std | Yes | Every git that no person answers, in production code and in tests |
| `command_in(dir)` | std | Yes | The same, run in `dir` |
| `tokio_command()` | async (`tokio` feature) | Yes | Async callers |
| `tokio_command_in(dir)` | async (`tokio` feature) | Yes | Async callers, run in `dir` |
| `interactive_command()` | std | No | A git that a person may need to answer at a terminal |

Use `interactive_command()` only when all three of these hold:

- a person started the command at a terminal;
- the URL comes from that person's own arguments;
- the child stays in the caller's foreground process group, with stdio
  inherited.

`amplihack plugin install` is the only caller. `interactive_command()` does
not set the variable, and does not remove a `GIT_TERMINAL_PROMPT` or askpass
variable that it inherits.

`scrub()` is unchanged. It removes the repository variables from a child that
is not git itself, and does not set `GIT_TERMINAL_PROMPT`.

The builders set the variable with `.env()`, so a later `env_clear()` on the
same `Command` removes it. No production code clears a git builder's
environment; a test that does must call `no_git_prompt` afterwards.
`GIT_TERMINAL_PROMPT` is deliberately not in `REPOSITORY_ENV_VARS`, because
`tests/issue_1278_git_env_isolation.sh` unsets every name in that list.

`scripts/check-git-command-sanitised.sh` still fails the build on a raw
`Command::new("git")` outside `crates/amplihack-git/`, and its message lists
the five builders and `scrub()`. The builders' unit tests check that the four
default builders set `GIT_TERMINAL_PROMPT=0`, that `interactive_command()` does
not, and that all five remove the repository variables. Run them with:

```bash
cargo test -p amplihack-git --all-features
```

## Write a test that runs git

| The test | Do this |
|----------|---------|
| Starts the `amplihack` binary | Use `isolated(..)` unchanged. |
| Starts it at a pseudo-terminal with rexpect's `spawn_command` | Set `.stdin(Stdio::inherit())` after `isolated(..)`. rexpect makes the pseudo-terminal stdin before `exec`, and the null stdin would replace it, so the binary would see no terminal. |
| Runs git and keeps the test process's environment | Use `amplihack_git::command_in(dir)`. The child inherits the `[env]` values and the builder adds `GIT_TERMINAL_PROMPT=0`. |
| Calls `env_clear()` on git, or on a script that runs git | Call `no_git_prompt` after `env_clear()`. |
| Is a shell script | Add the exports and `exec </dev/null` at the top. |

### After `env_clear()`

Call `no_git_prompt` after `env_clear()`. A later `.env()` or `.stdin()` call
wins, so call it before anything the test must be able to override, and set
`.stdin(Stdio::piped())` after it in a test that writes to its child's stdin.
`tests/integration/workflow_publish_resilience.rs` runs
`workflow_publish_pr.sh` this way, with the helper before the loop that
applies each case's `extra` variables. In this excerpt, a comment stands for
the variables the test sets that do not bear on prompts:

```rust
let mut cmd = Command::new("bash");
cmd.arg(workspace_helper_path("workflow_publish_pr.sh"))
    .env_clear()
    .env("HOME", tmp.path()); // plus PATH and the script's own variables
crate::isolated_amplihack::no_git_prompt(&mut cmd);
for (k, v) in extra {
    cmd.env(k, v);
}
```

The rule for a test that clears its environment: if the system git and a
remote that is not local can meet, apply the helper, whether or not today's
code path contacts the remote. A remote is local when it is a path, a
`file://` URL, or redirected to one with `url.<base>.insteadOf`. A test that
puts a fake `git` first on `PATH` does not need the helper.

Three fixtures apply it:

| File | Where |
|------|-------|
| `tests/integration/issue_1642_github_origin_parsing/fixture.rs` | `Fixture::command`, which keeps `GIT_ALLOW_PROTOCOL=file` |
| `tests/integration/workflow_publish_resilience.rs` | The `workflow_publish_pr.sh` runner above; the script runs `git remote set-head origin -a` against `https://github.com/owner/repo.git` |
| `tests/integration/workflow_finalize_resilience.rs` | `isolate_scope_child`, which `private_git` and `private_scope_command` use |

### A test that checks a child's whole environment

A test that rejects every inherited `GIT_*` variable must allow the three by
exact name and check their values. Do not delete such an assertion.
`issue_1707_suite_environment_is_isolated` in
`crates/amplihack-cli/tests/issue_1707_claim_check_shell_suites.rs` does this:

```rust
use crate::isolated_amplihack::NO_GIT_PROMPT_ENV;

for name in &names {
    assert!(
        (!name.starts_with("GIT_") || NO_GIT_PROMPT_ENV.iter().any(|(n, _)| n == name))
            && !name.starts_with("GH_"),
        "{name} reached the suite environment:\n{env}"
    );
}
for (name, value) in NO_GIT_PROMPT_ENV {
    assert_eq!(value_of(name), Some(value), "{env}");
}
```

Every other `GIT_*` variable is still rejected.

### Shell tests

Put the exports straight after the script's `set` line:

```bash
set -euo pipefail
# Issue #1764: no git this script runs may wait on a credential prompt.
export GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=true SSH_ASKPASS=true
exec </dev/null
```

To list the scripts that already have these lines:

```bash
grep -rl 'Issue #1764: no git' tests amplifier-bundle/recipes/tests
```

- Use `exec </dev/null` only in a script that is executed. In a sourced file it
  replaces the caller's stdin. Before you add it, check that nothing sources
  the script:

  ```bash
  grep -rnE '(source|\.) [^ ]*issue_1680_fresh_run_state_key\.sh' .
  ```

- `env -i` drops the exports. Where the `PATH` of an `env -i` call reaches the
  system git, add the three variables straight after `env -i`, as
  `run_prepare` in `tests/git_identity_test.sh` does:

  ```bash
  env -i \
      GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=true SSH_ASKPASS=true \
      PATH="$fake_bin:$PATH" \
      HOME="$TMPROOT/home" \
      REPO_PATH="$repo" \
      AMPLIHACK_HOME="$REPO_ROOT" \
      HELPER="$HELPER" \
      "$@" \
      bash -c '...'
  ```

  Leave alone an `env -i` call whose `PATH` holds only stubs.
- A script that already sets some of the variables keeps its existing lines.
- Never set the askpass variables without `GIT_TERMINAL_PROMPT=0`.

## The guard test

`tests/integration/issue_1764_git_never_prompts_test.rs` checks the layers
above. It runs git only against a server on `127.0.0.1` that answers every
request with `401` and `WWW-Authenticate: Basic`, from a temporary `HOME` with
`GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, so no stored
credential can reach the server.

```bash
cargo test -p amplihack --test integration issue_1764 -- --nocapture
```

| Part | Runs | Passes when |
|------|------|-------------|
| (a) Environment | Reads the test process's environment and `isolated("x").get_envs()` | Both have the three variables with the values above |
| (b) Git builder | `git ls-remote` through `amplihack_git::command()`, with a null stdin and both askpass variables removed | The builder sets `GIT_TERMINAL_PROMPT=0` on the command; git exits non-zero, not by a signal, before the test's deadline, with `terminal prompts disabled` in stderr |
| (c) Pseudo-terminal | `git ls-remote` under `script` and plain `timeout`, in a compound command | The control (no variables) shows the trap. (c1), with `GIT_TERMINAL_PROMPT=0` and no askpass variable, exits before the inner `timeout`'s deadline with a status other than 124 and `terminal prompts disabled` in its output. (c2), with all three variables, exits before that deadline with a status other than 124. |
| (d) Recipe text | Loads the two recipe files | Each of the three step prompts contains `--foreground`, `setsid`, `/dev/null`, `SIGTTIN` and `GIT_TERMINAL_PROMPT=0`, none contains a setting that weakens TLS, host-key checking or credential storage, and step 12 still contains its `workspace_command` schema |
| Cargo config | Reads `.cargo/config.toml` | The file holds only the `[env]` table with the three entries |
| Shell tests | Reads the twelve shell tests | The first two commands after each script's `set` line are the export and `exec </dev/null` |
| How-to | Reads this guide and the four pages that link it | The links are present in `mkdocs.yml`, `docs/index.md`, `docs/howto/develop-amplihack.md` and `docs/reference/default-workflow-agentic-finalization.md`, and the guide offers no setting that weakens TLS, host-key checking or credential storage |

Part (b) first checks that `amplihack_git::command()` sets
`GIT_TERMINAL_PROMPT=0` on the command it returns. The test process already
has the variable from `[env]`, so without that check part (b) would still pass
if the builder lost its default. It then shows that git started through the
builder, with no askpass program, fails at once with
`terminal prompts disabled` rather than waiting.

Part (c) runs on Linux when `script` and `timeout` are on `PATH`; otherwise it
prints why it was skipped. Its control passes if the test sees git stopped
(state `T` in `/proc/<pid>/stat`), if `timeout` exits 124, or if the test's own,
longer deadline expires. Once it sees git stopped, the test kills its `script`
process at once; closing the pseudo-terminal hangs up the stopped group, and
the test then checks that no process using its URL is left. It sends no
signal to the group itself. If the control finishes normally, the test fails
with `trap was not armed`: git was not put in a background group, so (c1) and
(c2) would prove nothing. Because
the test stops waiting as soon as git is stopped, the control and (c1) and
(c2) each take about a second, with GNU or uutils `timeout`.

Part (d) and the how-to check reject the same settings, listed in the test's
`WEAKENING` constant. Each would stop a prompt by turning off TLS
verification, by turning off host-key checking or by writing a credential to
disk, so neither this guide nor a recipe prompt may suggest one. The guide
does not name them, because the check searches it for those strings.

When part (b) or (c) fails, its message prints the child's exit status, the
time it took, its stdout and its stderr. It prints no environment variables,
either those the test set for the child or those of the test process, which
may hold `GH_TOKEN` or a similar secret. The loopback server reads at most
8 KiB of each request and never prints it, so no request header reaches the
test output either.

If part (a) fails, the `[env]` table did not reach the test binary. Find out
why. Do not add the variables to `.github/workflows/ci.yml`, which would hide
the failure this part exists to catch.

Run the test once from a terminal and once with `</dev/null`. Every git the
test starts uses a URL of the form
`http://127.0.0.1:<port>/issue-1764-<pid>.git`, so a git, `timeout` or
`script` process left behind shows up here; afterwards this prints nothing:

```bash
pgrep -fa '127\.0\.0\.1:[0-9]+/issue-1764-[0-9]+\.git'
```

Do not search for `issue-1764` alone. It also matches unrelated processes,
such as agent sessions whose arguments mention the issue.

## Run a long command by hand or from an agent

From a terminal, run a long command in a new session that has no controlling
terminal:

```bash
GIT_TERMINAL_PROMPT=0 setsid -w timeout --kill-after=60 5400 cargo test --workspace --locked </dev/null
echo "exit=$?"
```

Ctrl-C does not stop this form. `setsid -w` does not pass signals on, so
Ctrl-C ends `setsid` and leaves the run going in its own session. To stop the
run, send `SIGTERM` to its `timeout` process, which ends the command and the
processes in its group:

```bash
pgrep -fa '^timeout --kill-after=60 5400 cargo test'
kill -TERM <pid of the run's timeout>
```

If `pgrep` lists more than one run, check which is yours with
`ps -o pid,lstart,args -p <pid>` before you send the signal.

If you need to stay in the terminal's session, keep `timeout` in the
foreground instead. Ctrl-C stops this form as usual:

```bash
GIT_TERMINAL_PROMPT=0 timeout --foreground --kill-after=60 5400 cargo test --workspace --locked </dev/null
echo "exit=$?"
```

| Part | Why |
|------|-----|
| `GIT_TERMINAL_PROMPT=0` | Git fails at once instead of prompting. As a prefix, it applies to this command only and does not stay set in your shell. `[env]` covers only processes cargo starts, and other repositories have no such `.cargo/config.toml`. |
| `setsid` | The command has no controlling terminal, so nothing it starts can read the terminal or be stopped by `SIGTTIN`. |
| `-w` | In an interactive shell every command leads its own process group. Plain `setsid` then forks and returns at once, while the command runs on, detached. `-w` waits and returns the command's exit status. |
| `--kill-after=60` | Sends `SIGKILL` if the command is still running 60 s after the deadline's `SIGTERM`. |
| `--foreground` | `timeout` stays in the terminal's foreground group, so nothing is stopped. Git can then prompt in the foreground and wait for an answer; `GIT_TERMINAL_PROMPT=0` prevents that. With `--foreground`, children of the command are not killed at the deadline. |
| `</dev/null` | Gives a null stdin to programs that read stdin. It does not stop git, which reads `/dev/tty`. |

Do not run plain `timeout <secs> <command>` from a terminal.

Agents are told the same in the prompts of `step-12-run-precommit` and
`step-13-local-testing` in
`amplifier-bundle/recipes/workflow-precommit-test.yaml`, and of
`step-04-address-blockers` in
`amplifier-bundle/recipes/autodrive-merge-round.yaml`. They export
`GIT_TERMINAL_PROMPT=0` and use the `setsid -w` form, or
`timeout --foreground` where `setsid` is missing. In step 12 the wrapper is
not part of the reported command. The step's JSON still reports
`"workspace_command":"cargo test --workspace --locked"`, which the step's gate
compares exactly.

When a command needs credentials, give git a credential helper. Do not work
around a prompt by turning off TLS verification or SSH host-key checking, or
by putting a token in a URL or a plain-text credential store.

## Behaviour changes for users

| Command | Before | Now |
|---------|--------|-----|
| `amplihack launch` or `amplihack claude` with `--checkout-repo owner/repo` | A private repository with no credential helper could prompt on the terminal and be answered | Fails at once. The error contains `terminal prompts disabled`, unless an askpass program is inherited. |
| `amplihack plugin install <git URL>` | Prompts when credentials are needed | Unchanged; it uses `interactive_command()`. The clone, including any time spent at the prompt, is limited to 120 s. |
| `amplihack multitask`: its clone and its `git ls-remote --symref` in `commands/multitask/orchestrator.rs` | Could wait on a prompt | Fail at once, as above |
| The clones in `amplihack-launcher` (`repo_checkout.rs` and `launcher_core.rs`) | Could wait on a prompt | Fail at once, as above |

The other builder calls that amplihack commands reach work on a local
repository or a bundle file and never ask for credentials; they include the
conflict detector, recovery, and the bundle fetch in `amplihack-remote`.
Recipe steps and hooks that run `git` from shell do not use the builders; see
[Limits](#limits).

The `--checkout-repo` clone stays non-interactive because recipes and agents
also run it, and its owner and repository name can come from them rather than
from the person at the terminal. To clone a private repository, configure a
credential helper; helpers run before any prompt. For GitHub:

```bash
gh auth setup-git
```

The launcher's own checkout clone in
`crates/amplihack-launcher/src/launcher_core.rs` has no user-invoked caller
and is also non-interactive.

## `cargo run` and the `[env]` table

Cargo sets the three variables, with `force = true`, for every process it
starts from this checkout, including `cargo run`:

```bash
cargo run -p amplihack -- plugin install https://github.com/<owner>/<private-plugin>.git
```

Here `plugin install` cannot prompt, although it does elsewhere.
`interactive_command()` does not set `GIT_TERMINAL_PROMPT`, but it keeps the
inherited variables, so git runs `GIT_ASKPASS=true`. The server refuses the
empty answer and the clone fails with `Authentication failed`. An IDE's
askpass helper is replaced by `true` in the same way. To answer a prompt,
build first and run the binary directly. Cargo does not start it, so `[env]`
does not apply:

```bash
cargo build -p amplihack
target/debug/amplihack plugin install https://github.com/<owner>/<private-plugin>.git
```

An installed `amplihack` is not affected. A credential helper works in every
case.

## Reproduce the trap

The guard test's part (c), `part_c_plain_timeout_trap_in_a_pseudo_terminal`,
reproduces the trap on every run, with a control, and checks that nothing is
left behind. [How the trap works](#how-the-trap-works) explains what happens.
To see it by hand you need `python3`, `script` (from util-linux) and
`timeout`. The server listens only on `127.0.0.1`; do not point these
commands at any other host. The runs use a temporary `HOME`, with no global
or system git configuration, so no credential helper answers.

```bash
# A server that refuses every request. If the port is taken, choose another.
python3 -I -c '
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(401)
        self.send_header("WWW-Authenticate", "Basic realm=\"issue-1764\"")
        self.send_header("Content-Length", "0")
        self.end_headers()
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", 18764), H).serve_forever()
' & server=$! && sleep 1

home=$(mktemp -d) && cd "$home"
run() {   # Arguments: extra variables for git, such as GIT_TERMINAL_PROMPT=0.
    env -u GIT_TERMINAL_PROMPT -u GIT_ASKPASS -u SSH_ASKPASS "$@" \
        HOME="$home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
        GIT_CEILING_DIRECTORIES="$(dirname "$home")" SHELL=/bin/sh \
        script -qec 'timeout 6 git ls-remote http://127.0.0.1:18764/repo.git </dev/null; rc=$?; echo "rc=$rc"; exit $rc' /dev/null </dev/null
}

# The control: git prompts for a username and is stopped (state T).
run & sleep 2
ps -eo pid,stat,cmd | awk '$2 ~ /T/'
# Kill only `script`; closing the pseudo-terminal hangs up the stopped group.
pkill -f '^script -qec timeout 6 git'

# With the setting, git fails at once with `terminal prompts disabled`, rc=128.
run GIT_TERMINAL_PROMPT=0

cd && rm -rf "$home"; kill "$server"
ps -eo pid,stat,cmd | awk '$2 ~ /T/'             # prints only the header
```

With GNU `timeout` the control ends after 6 s with `rc=124`; with uutils
`timeout` it never returns, which is why the commands kill `script`. The
compound command inside `script` is needed: with a single command the shell
replaces itself with `timeout`, which then leads the session, cannot leave
the foreground group, and the trap does not appear.

## Limits

- The builders do not remove an inherited askpass program. If `GIT_ASKPASS`,
  `core.askPass` or `SSH_ASKPASS` names one, such as an IDE's helper or a
  desktop's `ssh-askpass`, git runs it before it checks
  `GIT_TERMINAL_PROMPT`. The program may open a window and wait for an
  answer. An empty answer ends in `Authentication failed`, and a program that
  fails ends in `terminal prompts disabled`.
- `GIT_TERMINAL_PROMPT` does not reach OpenSSH. An `ssh://` or `git@` remote
  can still ask for a passphrase or a host key on `/dev/tty`, and stop under
  plain `timeout`. `setsid` covers that case; the variables do not.
- A credential helper that opens the terminal itself is outside git's prompt
  settings.
- A Rust test that calls `env_clear()` and not `no_git_prompt` loses both the
  `[env]` values and the builder default. Nothing detects this except a
  stopped run.
- `[env]` applies only to processes cargo starts. A test binary run directly
  from `target/` does not get it; `isolated(..)` and the builders still apply.
- Recipe and tool scripts that run `git fetch`, `git ls-remote` or
  `git remote set-head` themselves get the prompt setting only from their
  environment: `[env]` under cargo, otherwise whatever their caller exported.
- Two existing tests still contact `github.com`: the publish test's
  `git remote set-head origin -a` and
  `tests/outside-in/scripts/scenario8-checks.sh`. They now fail at once
  instead of prompting.
- A test that reads its own stdin is stopped in the same way when the suite
  runs at a terminal, or under `script` as in
  [Reproduce the trap](#reproduce-the-trap), behind plain `timeout`.
  Give such a test its answer as an argument instead of reading stdin, as
  the power-steering tests in `amplihack-utils` do.
- Nothing here works around uutils `timeout`. On a host where it is installed,
  a plain `timeout` that is stopped still waits indefinitely, so use the
  commands in
  [Run a long command by hand or from an agent](#run-a-long-command-by-hand-or-from-an-agent).

## Related

- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - What else `isolated(..)` removes and sets
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The rotator stops that `isolated(..)` sets
- [Run in Non-Interactive Mode](./run-in-noninteractive-mode.md) - Running amplihack without a terminal
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
