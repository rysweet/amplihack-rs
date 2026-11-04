---
title: Install amplihack as a Claude Code plugin
description: Install the Claude Code plugin and run workflows when GitHub GraphQL is refused.
type: howto
updated: 2026-11-04
---

# Install amplihack as a Claude Code plugin

The amplihack-rs repository is a Claude Code plugin and a one-plugin
marketplace. You can install it from git into Claude Code in the terminal, the
desktop app, or on the web (cloud sessions) without running `amplihack
install` first.

## Contents

- [Install](#install)
- [What you get](#what-you-get)
- [The native runtime](#the-native-runtime)
- [Relationship to `amplihack install`](#relationship-to-amplihack-install)
- [Cloud workflow setup](#cloud-workflow-setup)
- [Known limitations](#known-limitations)
- [Update and uninstall](#update-and-uninstall)
- [Validate changes](#validate-changes)

## Install

### Terminal and desktop

Inside Claude Code:

```text
/plugin marketplace add rysweet/amplihack-rs
/plugin install amplihack@amplihack
```

From a shell, the same steps are:

```bash
claude plugin marketplace add rysweet/amplihack-rs
claude plugin install amplihack@amplihack
```

To pin a branch or tag, use `rysweet/amplihack-rs#<ref>`. Any git URL works in
place of the `owner/repo` shorthand. Start a new session after installing.

### Claude Code on the web (cloud sessions)

Each cloud session starts in a fresh container, so install the plugin, and
the native runtime it drives, from the environment's **setup script**. It runs
before Claude Code starts:

```bash
claude plugin marketplace add rysweet/amplihack-rs
claude plugin install amplihack@amplihack
plugin_dir=$(node -e '
  const plugins = process.env.CLAUDE_CODE_PLUGIN_CACHE_DIR ||
    (process.env.CLAUDE_CONFIG_DIR || require("os").homedir() + "/.claude") + "/plugins";
  const records = require(plugins + "/installed_plugins.json").plugins["amplihack@amplihack"] || [];
  const user = records.find((r) => r.scope === "user");
  if (!user) { console.error("amplihack@amplihack is not installed at user scope"); process.exit(1); }
  console.log(user.installPath);
') && sh "$plugin_dir/claude-plugin/bin/install-runtime"
```

The last step reads the user-scope install's path from Claude Code's
`installed_plugins.json`. The plugin cache can hold older versions too, and a
project-scope record can come first.

Edit the setup script from the cloud environment menu in the session's title
bar (**Edit** → **Setup script**). New sessions pick up the change.

How [cloud environments](https://code.claude.com/docs/en/cloud-environments)
treat this script shapes what you get:

- **The result is cached.** After the setup script finishes, the filesystem is
  snapshotted and later sessions start from that snapshot. The script runs
  again only when you change it or its network settings, or when the cache
  expires after roughly seven days. So the plugin stays at the commit it was
  installed at until the cache is rebuilt. To pick up a newer amplihack sooner,
  make any edit to the setup script.
- **Keep it under about five minutes.** The cache is built only from a setup
  script that finishes in roughly five minutes. The release download takes
  seconds, but the `recipe-runner-rs` cargo build takes a few minutes on a cold
  container. If the script cannot fit, leave out the `install-runtime` line:
  the plugin's `SessionStart` hook installs the runtime in the background
  instead, but then it does so in every new session, because that install is
  not part of the snapshot.
- **Attach amplihack-rs to the environment.** GitHub release-asset requests
  from a cloud session reach only the repositories attached to it. When
  rysweet/amplihack-rs is not attached, the release download is refused and
  `install-runtime` falls back to building amplihack from source, which is too
  slow for a setup script.

## What you get

| Component      | Source in the repo                          | Invoked as                              |
| -------------- | ------------------------------------------- | --------------------------------------- |
| Skills         | `skills/` → `amplifier-bundle/skills/`      | `amplihack:dev-orchestrator`, …         |
| Agents         | `amplifier-bundle/agents/{core,specialized,workflows}/` | `amplihack:architect`, `amplihack:builder`, … |
| Slash commands | `docs/claude/commands/amplihack/`           | `/amplihack:fix`, `/amplihack:analyze`, … |
| Hooks          | `claude-plugin/hooks.json`                  | Run automatically                       |

The manifest is `.claude-plugin/plugin.json` and the marketplace entry is
`.claude-plugin/marketplace.json`. Both point at the repository root, which is
itself the plugin. Claude Code refuses component paths or symlinks that leave
the plugin directory, and the skills, agents, and commands already live in
different parts of this repository. Using the root lets them all load without
duplicating anything.

## The native runtime

Skills, agents, and commands load as soon as the plugin is installed. Two
things also need amplihack's native binaries:

- **Recipe-driven skills.** `dev-orchestrator`, `default-workflow`,
  `investigation-workflow` and the other recipe skills run `amplihack recipe
  run`, which needs `amplihack` and `recipe-runner-rs`.
- **Hooks.** The hooks run `amplihack-hooks`.

The plugin's `SessionStart` hook, `claude-plugin/bin/bootstrap`, handles this:

- It exports `AMPLIHACK_HOME` (the plugin root, a full checkout, so recipes
  resolve from it) and a `PATH` that includes `~/.local/bin` and
  `~/.cargo/bin` into the session. It also exports
  `AMPLIHACK_AGENT_BINARY=claude` so recipe agent steps run under Claude Code.
  In cloud sessions, which run as root, it exports `IS_SANDBOX=1` so the
  `claude` child processes accept `--dangerously-skip-permissions`.
- If any binary is missing, or the runtime was installed for a different
  plugin version, it either starts `claude-plugin/bin/install-runtime` in the
  background or tells Claude the command to run. `install-runtime` records the
  plugin version it installed for, so a plugin update also updates the
  binaries instead of running new recipes on old ones.

| Setting                                    | Behavior when the runtime is missing              |
| ------------------------------------------ | ------------------------------------------------- |
| Cloud session (`CLAUDE_CODE_REMOTE=true`)  | Installs in the background (default)              |
| Terminal or desktop                        | Tells Claude how to install it (default)          |
| `AMPLIHACK_PLUGIN_AUTO_INSTALL=1`          | Always installs in the background                 |
| `AMPLIHACK_PLUGIN_AUTO_INSTALL=0`          | Never installs automatically                      |

When the runtime is present but was installed for another plugin version,
the background reconcile runs silently; only a missing runtime is reported to
Claude. The reconcile only ever replaces binaries the plugin installed itself:
`install-runtime` records their sha256 sums in `owned-binaries`. An `amplihack`
or `amplihack-hooks` you installed any other way (`amplihack install`, your own
build, a package) is left as it is, whatever its version. That holds even when
only one of the two is yours: the plugin will not install the other next to it
and shadow yours on `PATH`. It records that settled state instead of a failure,
and each session start tells Claude which binary to install your own way. When no `sha256sum` or `shasum` is available,
ownership cannot be proven, so nothing is replaced. Source builds are stamped
with the release they stand for, the way the release workflow stamps its
builds. A reconcile counts as done only when the
binaries the plugin manages are at the wanted release; merely being present is
not enough. Two guards keep a broken install from looping:

- A failed install writes `install.failed`. It is not retried automatically
  for six hours, or until the plugin version changes.
- `SessionStart` also fires on resume, `/clear`, and compaction. An install is
  never started on compaction.

`install-runtime` does three things:

1. It downloads the latest release's prebuilt `amplihack` and
   `amplihack-hooks` binaries with the same downloader the npm wrapper uses
   (`npm/lib/bootstrap.js`). Each archive is checked against the `.sha256`
   file published with the same release. That catches a corrupted download,
   not a compromised release. Binaries already on `PATH` are replaced when
   their version differs. Without `node` it builds from the checkout with
   cargo, outside the plugin directory.
2. It copies them into `~/.local/bin`.
3. It builds `recipe-runner-rs` with `cargo install --git
   https://github.com/rysweet/amplihack-recipe-runner --rev <sha> --locked`.
   The commit comes from `claude-plugin/recipe-runner.rev`, so the code a
   plugin commit builds is fixed and reviewable in that commit. cargo skips
   the build when that exact commit is already installed. amplihack's own
   launcher tracks the runner's `main` branch instead; the plugin pins because
   it installs without asking. Bump the file to move the plugin to a newer
   runner.

It is idempotent and safe to run by hand:

```bash
sh ~/.claude/plugins/cache/amplihack/amplihack/<version>/claude-plugin/bin/install-runtime
```

State and logs live in the plugin's data directory, `${CLAUDE_PLUGIN_DATA}`.
When that variable is unset, as it is for commands Claude runs through the Bash
tool and for setup scripts, both scripts fall back to the same directory:
`~/.claude/plugins/data/amplihack-amplihack`, with the plugins root moved by
`CLAUDE_CODE_PLUGIN_CACHE_DIR` or `CLAUDE_CONFIG_DIR` when either is set. Background installs log to `install-runtime.log` there. While an install
is running, an `install.lock` directory exists next to the log. A manual run
takes the same lock and steps aside when an install is already running. The
lock is reclaimed only when no `install-runtime` process holds it; a recorded
pid that now belongs to some other process does not count. When `ps` cannot
say (no procps, or a BusyBox `ps`), the lock is kept until it is three hours
old, since starting a second install is worse than waiting. Reclaims are
serialised, so concurrent runs never install at the same time.

`install-runtime` installs the latest published release. It resolves the tag
through the `github.com/…/releases/latest` redirect, not the rate-limited
GitHub API. Set `AMPLIHACK_NPM_VERSION` to pin a different release.

## Relationship to `amplihack install`

`install-runtime` never runs `amplihack install`. That command publishes every
skill into `~/.claude/skills` and registers hooks in `~/.claude/settings.json`,
which would give a plugin user each skill twice.

You can still use both on the same machine. Every plugin hook goes through
`claude-plugin/bin/amplihack-hook`, which exits quietly without calling the
binary when:

- `amplihack-hooks` is not installed yet, or
- a hook `command` in `${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json`, or in
  the project's `.claude/settings.json` or `.claude/settings.local.json`,
  already runs an `amplihack-hooks` binary that exists. That means `amplihack
  install` wired the hooks (user scope, or the interactive installer's
  repo-local scope) and they already fire. Claude Code keeps a plugin's copy of
  a handler separate from the settings copy, so without this check each hook
  would run twice. Other mentions, such as a permission rule, do not count.
  Neither does a registration whose binary has since been removed. Managed
  policy settings and `--settings` files are not inspected.

The wrapper also sets `AMPLIHACK_HOME` to the plugin root for `amplihack-hooks`.
Hook processes do not see the variables `bootstrap` writes to
`CLAUDE_ENV_FILE`; those reach Bash tool commands only.

So hooks never fire twice. Skills installed by `amplihack install` appear
unprefixed (`dev-orchestrator`), and the plugin's copies appear as
`amplihack:dev-orchestrator`. If you use the plugin, you do not need
`amplihack install`.

## Cloud workflow setup

Install and authenticate the GitHub CLI (`gh`) for the target repository using
the session's normal authentication mechanism. Make `jq` and GNU `timeout`
available on the workflow's `PATH`. The compatibility layer uses `jq` for
REST conversions. It does not read or print `GH_TOKEN`; the real `gh api`
handles authentication and transport.

The workflow requires a command named `timeout`: workflow preparation and
the issue-tracking helper invoke it directly to bound issue lookup, creation
and other provider calls. Perl is a fallback for the compatibility layer's
GraphQL probe only. It does not replace these workflow calls, even when a
cached probe answer or `AMPLIHACK_GH_REST_ONLY=1` avoids a new probe.

macOS does not ship GNU `timeout`. With Homebrew, install `coreutils` using
`brew install coreutils`, then expose its unprefixed commands in the shell or
setup script that launches the workflow:

```bash
export PATH="$(brew --prefix coreutils)/libexec/gnubin:$PATH"
timeout 5 sh -c 'printf "timeout is ready\n"'
# Output: timeout is ready
```

Having only `gtimeout` on `PATH`, or defining an interactive shell alias, does
not supply the executable named `timeout` that workflow subprocesses need.

Run `default-workflow` from the repository after installing the runtime:

```bash
amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Fix #1484: run default-workflow when GitHub GraphQL is refused"
```

The tracking step uses the selected issue, or finds or creates a tracker.
On github.com, supported tracking and pull-request publishing operations use
REST when the session refuses GraphQL. No separate workflow is required.
The [recipe CLI reference](../reference/recipe-cli-reference.md) describes
context flags and the runner's output.

### Tracking selection

Use `-c issue_number=1484` to select an existing issue explicitly. Selection
precedes automatic title lookup and creation. Task references are checked
against the same repository and must identify an open issue, not a pull
request. An explicit issue that cannot be verified produces a warning and
local tracking, preserving the explicit selection.

With no selected issue, the workflow lists up to the newest 1,000 open issues
and uses the first full-title match. It compares the complete sequence of
Unicode letter and digit words, ignoring case. For example, `É` matches `é`;
multi-character folds such as `ß` to `SS` are not guaranteed. Controls become
spaces before both lookup and creation. A title with no words matches nothing.
A warning reports when the list reaches the cap, since an older matching
tracker may be omitted.

A failed automatic lookup stops creation. Only recognised unavailable-host,
missing-`gh` or unresolvable-repository failures permit the warned local-tracking
fallback. Authentication errors, malformed output, missing `jq`, failed JSON
conversion and unavailable probe bounds remain errors. A successful lookup
with no match permits creation.

To skip remote issue tracking while retaining GitHub pull-request publishing:

```bash
amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Fix #1484: run default-workflow when GitHub GraphQL is refused" \
  -c remote_host_type=github \
  -c issue_tracking=local
```

The tracking step emits `tracking_system=local` and a local tracking reference.
Publishing still requires an authenticated `gh` and access to the repository.
Local tracking supplies no replacement publishing client when `gh` is absent.

`remote_host_type=github|azdo|other` overrides provider detection from `origin`.
Use `other` for local issue tracking on an unrecognised provider. This setting
selects workflow provider behaviour; it does not change the hostname used by
`gh` or enable GitHub publishing on another provider.

### Compatibility scope and host selection

Each Unix recipe run installs a private compatibility launcher when a real
`gh` and the bundled adapter are available. Steps, helpers and child agents
inherit it. A `gh` command outside that run keeps its normal behaviour.
Help, `label create` and commands outside the adapted groups use the real
CLI directly without a GraphQL probe.
Merge authorisation checks run before any compatibility probe or translation.
Compatibility settings, cached probe answers and `AMPLIHACK_REAL_GH` do not
grant permission to merge.

For calls that need GraphQL, the layer selects the host in this order: the
final `-R/--repo` value when host-qualified, the final `--hostname` value, a
host-qualified `GH_REPO`, a recognised positional repository/issue/PR URL,
`GH_HOST`, then `origin`. An unqualified `OWNER/REPO` supplies no hostname.
An explicit github.com at a higher tier takes precedence over lower tiers.
URLs in titles, bodies, searches and jq expressions remain data.

The viewer probe runs for at most ten seconds by default, with a two-second
termination grace period. Ctrl-C interrupts it and prevents the requested
operation from starting. If a fresh probe is required and neither GNU
`timeout` nor Perl is available, the call fails before the probe or requested
operation. A valid cached answer or explicit REST-only setting can avoid a
new probe.

| Probe result | Behaviour |
| --- | --- |
| GraphQL works | The real `gh` runs directly with its original input, output, signals and exit status. The operation is never replayed. |
| GraphQL is refused | Supported calls use REST on github.com. A blocked enterprise host is refused before REST transport. |
| The result is inconclusive | The real `gh` runs with stderr held and signals forwarded. If it then reports the GraphQL refusal, an eligible read can replay over REST. A write fails with a rerun hint and is never replayed automatically. |

Before rerunning a failed write, check whether it already changed the remote
object. A multi-part operation can succeed in part. Failed follow-up changes
are reported; creation is not repeated to repair metadata. JSON conversion
errors retain their failing exit status through cleanup and cannot become a
successful empty result or trigger duplicate tracker creation.

### Compatibility settings

Set environment variables before starting the recipe. Probe answers are
cached separately for each host in private state.

| Variable | Default and use |
| --- | --- |
| `AMPLIHACK_GH_COMPAT` | Compatibility is enabled. Set `0` to disable it for the run; merge checks remain active. |
| `AMPLIHACK_GH_REST_ONLY` | Unset. Set `1` to force the github.com REST path without probing. Host restrictions and merge checks still apply. |
| `AMPLIHACK_GH_COMPAT_STATE_TTL_MIN` | `60` minutes for a blocked answer. Accepts a non-negative integer. |
| `AMPLIHACK_GH_COMPAT_RECHECK_MIN` | `5` minutes for working or inconclusive answers. Accepts a non-negative integer. |
| `AMPLIHACK_GH_COMPAT_PROBE_TIMEOUT` | `10` seconds, plus two seconds of termination grace. Accepts a positive integer. |
| `AMPLIHACK_GH_COMPAT_STATE` | Uses private state by default. Overrides its base path; `.ok` and `.unknown` identify other answers, with `@HOST` for hosts other than github.com. Use an owned private location. |
| `AMPLIHACK_GH_COMPAT_LOG` | Defaults to `gh-compat.log` in `AMPLIHACK_WORKFLOW_ARTIFACT_DIR`, or `TMPDIR` when unset. Selects the transport-log destination. Use an owned private location. |
| `AMPLIHACK_GH_COMPAT_VERBOSE` | Unset. Set `1` to echo the same restricted transport records to stderr. |
| `AMPLIHACK_REAL_GH` | Selects the downstream executable. It must resolve to a terminal `gh`, not a compatibility or guard wrapper. |

Invalid numeric values use their defaults. Compatibility logs contain only
the method, endpoint path and HTTP or fixed exit status. They exclude query
strings, bodies, titles, credentials and raw stderr, including on failures.
Paths with sensitive data use route templates. Errors still reach the caller
on stderr; a local error without a request creates no transport record.

To use the real GitHub CLI without compatibility for a run:

```bash
AMPLIHACK_GH_COMPAT=0 amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Fix #1484: run default-workflow when GitHub GraphQL is refused"
```

GraphQL-backed operations then depend on the session's own `gh` connectivity.

### REST support and troubleshooting

| Command group | Operations covered when GraphQL is refused |
| --- | --- |
| `gh pr` | `view`, `list`, `create`, `edit`, `comment`, `ready`, `merge`, `close`, `diff`, `checks`. |
| `gh issue` | `view`, `list`, `create`, `comment`, `close`, `reopen`, `edit`. |
| `gh label` | `list`; `create` uses the real CLI's REST implementation directly. |
| `gh api graphql` | The exact viewer-login and viewer/repository-permission query shapes used by workflow preflight. Arbitrary queries and mutations fail. |
| `gh auth status` | The real CLI's result, with a REST account check for the recognised GraphQL refusal on github.com when no hostname override is supplied. |

Supported commands retain `gh` JSON field names, `--jq` output and create URLs.
Unknown JSON fields and fields that REST cannot supply fail visibly. List
commands page up to `--limit` (default 30), with bounded extra reads when
filters discard entries. Per-PR sub-lists, including comments, reviews, files,
commits and checks, read every page. `pr checks` exits `1` for a failed or
cancelled check, `8` for pending checks and `0` for passed or skipped checks;
transport or conversion failures take precedence.

When a call fails, use the diagnostic to choose the remedy:

- For `timeout: command not found`, make GNU `timeout` available under that
  name on the workflow's `PATH`; see [cloud workflow setup](#cloud-workflow-setup).
  Installing Perl alone does not satisfy this workflow prerequisite.
- For an unsupported flag or field, remove it or use an environment with
  GraphQL access. Projects, pinned-issue fields, `--template`, `--web`,
  `pr create --dry-run` and `issue list --milestone` have no supported mapping.
  `pr close --delete-branch`, `issue edit --add-assignee`, `pr merge --admin`
  and custom GraphQL headers are also rejected before side effects.
- For a refused issue/PR `--search`, use GitHub search in an environment that
  permits `/search/issues`. Results stop at 1,000. The layer never imitates
  GitHub search syntax locally. Author, label and assignee filters without
  `--search` still work over REST.
- For refused draft or auto-merge operations, use an environment that supports
  them. `pr ready` and auto-merge use Claude cloud proxy extensions; those
  routes are not part of GitHub's public REST API. Auto-merge rejects
  `--match-head-commit`, `--subject`, `--body` and `--delete-branch`, including
  explicitly empty subject or body values, because those options cannot be
  preserved by the proxy route. Conflicting merge modes and combinations with
  `--disable-auto` fail before mutation.
- For a branch-deletion failure after merging, inspect the merge and branch
  separately. `pr merge --delete-branch` deletes only a non-empty, encoded
  same-repository head ref. It validates the deletion plan before sending the
  merge and requires a response containing boolean `merged:true` before
  reporting success or deleting. A failed deletion returns nonzero and reports
  that the merge succeeded. Fork branches are left alone, and a warning reports
  that local branch cleanup is not reproduced.
- For a missing option value or excess positional targets, correct the
  arguments and rerun. Each edit accepts one issue or PR. `--body=` and
  `--body ""` intentionally clear content; a trailing `--body` fails before
  mutation. Provider-supplied PR numbers must be positive JSON integers before
  the adapter uses them in endpoints.
- For JSON conversion failures, inspect any mutation reported as already sent
  before retrying. The shared retry helper preserves the conversion exit and
  sends no further request. Unavailable reset metadata still permits the
  normal header or minimum-wait fallback; a failed REST fallback ends the call.
- For missing closing references, check the PR body. REST can identify open
  or merged PRs whose bodies close an issue through timeline cross-references.
  A link made only in the Development sidebar has no REST trace. The design
  step refuses to proceed when the required closing-reference list is empty.

The retry helper treats the recognised GraphQL refusal as permanent. It does
not wait for a quota reset to resolve that refusal.

## Known limitations

- **Nested skills.** Plugin skill discovery only reads top-level skill
  directories. Skills nested one level deeper under a category, such as
  `quality/…`, are not loaded by the plugin.
- **Context references.** Some agents and commands reference
  `~/.amplihack/.claude/context/*.md`. That tree exists only after `amplihack
  install`, so in a plugin-only install those references do not resolve.
  Philosophy and pattern files live in the plugin at
  `amplifier-bundle/context/`.
- **GitHub in cloud sessions.** REST compatibility covers supported workflow
  calls when GraphQL is refused. Search, proxy extensions and closing-reference
  visibility have limits; see [cloud workflow setup](#cloud-workflow-setup).
- **Skill-to-agent redirect.** The `pre-tool-use` hook's redirect from a skill
  name to the matching agent compares bare names. Plugin skills are namespaced
  (`amplihack:…`), so under the plugin that redirect does not trigger.
- **Adding agents.** New agents must be listed in `plugin.json`, because
  Claude Code rejects directories in `agents`. The
  `claude_code_plugin_manifest` test fails and names the file when one is
  missing.

## Update and uninstall

```bash
claude plugin marketplace update amplihack   # refresh the catalog
claude plugin update amplihack@amplihack     # install the newer version
```

`marketplace update` only refreshes the catalog; `plugin update` installs the
new version, which loads in your next session. Automatic updates are off for
third-party marketplaces unless you turn them on for this marketplace in
`/plugin`. In cloud sessions the plugin updates when the environment cache is
rebuilt (see above).

```bash
claude plugin uninstall amplihack@amplihack
claude plugin marketplace remove amplihack
```

Uninstalling the plugin leaves the runtime binaries in place. To remove them,
delete `~/.local/bin/amplihack`, `~/.local/bin/amplihack-hooks` and
`~/.cargo/bin/recipe-runner-rs`.

## Validate changes

CI runs the test and shellcheck. It does not run `claude plugin validate`,
because CI deliberately never installs the real Claude client (a guard test,
`issue_1445_host_only_litellm`, forbids it); the Rust test pins what the
validator checks. Contributors changing the plugin files should run all three:

```bash
claude plugin validate .
cargo test -p amplihack-cli --test claude_code_plugin_manifest
shellcheck -s sh -S style claude-plugin/bin/*
```

`claude plugin validate .` warns that no `version` is set. That is
deliberate. When a version is set, Claude Code caches the plugin by that version
and delivers no update until it changes. amplihack releases advance by git tag
while `package.json` stays at the workspace base version, so a pinned version
would freeze installs. Without one, the cache follows the commit, and
`claude plugin update amplihack@amplihack` installs each new commit.

`--strict` reports two more expected warnings:

- it does not follow the `skills` symlink (a session does);
- it scans `agents/eval-recipes/README.md`, which the explicit `agents` list
  keeps from loading.
