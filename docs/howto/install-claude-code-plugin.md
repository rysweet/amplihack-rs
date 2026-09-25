# Install amplihack as a Claude Code plugin

The amplihack-rs repository is a Claude Code plugin and a one-plugin
marketplace. You can install it from git into Claude Code in the terminal, the
desktop app, or on the web (cloud sessions) without running `amplihack
install` first.

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

## Known limitations

- **Nested skills.** Plugin skill discovery only reads top-level skill
  directories. Skills nested one level deeper under a category, such as
  `quality/…`, are not loaded by the plugin.
- **Context references.** Some agents and commands reference
  `~/.amplihack/.claude/context/*.md`. That tree exists only after `amplihack
  install`, so in a plugin-only install those references do not resolve.
  Philosophy and pattern files live in the plugin at
  `amplifier-bundle/context/`.
- **Tracking issues in cloud sessions.** The default workflow creates GitHub
  issues and pull requests through `gh`. Cloud sessions block the GraphQL API
  that `gh issue` and `gh pr` use
  ([#1484](https://github.com/rysweet/amplihack-rs/issues/1484)).
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
