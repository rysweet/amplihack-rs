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

Each cloud session starts in a fresh container, so install the plugin from the
environment's **setup script**. It runs before Claude Code starts:

```bash
claude plugin marketplace add rysweet/amplihack-rs
claude plugin install amplihack@amplihack
```

Edit the setup script from the cloud environment menu in the session's title
bar (**Edit** → **Setup script**). New sessions pick up the change.

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
- If any binary is missing, it either starts `claude-plugin/bin/install-runtime`
  in the background or tells Claude the command to run:

| Setting                                    | Behavior when the runtime is missing              |
| ------------------------------------------ | ------------------------------------------------- |
| Cloud session (`CLAUDE_CODE_REMOTE=true`)  | Installs in the background (default)              |
| Terminal or desktop                        | Tells Claude how to install it (default)          |
| `AMPLIHACK_PLUGIN_AUTO_INSTALL=1`          | Always installs in the background                 |
| `AMPLIHACK_PLUGIN_AUTO_INSTALL=0`          | Never installs automatically                      |

`install-runtime` does three things:

1. It downloads the checksum-verified prebuilt `amplihack` and
   `amplihack-hooks` release binaries with the same downloader the npm wrapper
   uses (`npm/lib/bootstrap.js`), and falls back to a cargo build from the
   checkout.
2. It copies them into `~/.local/bin`.
3. It builds `recipe-runner-rs` with `cargo install --git
   https://github.com/rysweet/amplihack-recipe-runner --locked`, which takes a
   few minutes.

It is idempotent and safe to run by hand:

```bash
sh ~/.claude/plugins/cache/amplihack/amplihack/<version>/claude-plugin/bin/install-runtime
```

Background installs log to `${CLAUDE_PLUGIN_DATA}/install-runtime.log`, or to
`~/.amplihack/plugin/install-runtime.log` when `CLAUDE_PLUGIN_DATA` is not set.
While an install is running, an `install.lock` directory exists next to the
log.

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
  already runs `amplihack-hooks`. That means `amplihack install` wired the hooks
  (user scope, or the interactive installer's repo-local scope) and they
  already fire. Other mentions, such as a permission rule, do not count.

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
- **Adding agents.** New agents must be listed in `plugin.json`, because
  Claude Code rejects directories in `agents`. The
  `claude_code_plugin_manifest` test fails and names the file when one is
  missing.

## Update and uninstall

```text
/plugin marketplace update amplihack
/plugin uninstall amplihack@amplihack
/plugin marketplace remove amplihack
```

Uninstalling the plugin leaves the runtime binaries in place. To remove them,
delete `~/.local/bin/amplihack`, `~/.local/bin/amplihack-hooks` and
`~/.cargo/bin/recipe-runner-rs`.

## Validate changes

Contributors changing the plugin files should run:

```bash
claude plugin validate .
cargo test -p amplihack-cli --test claude_code_plugin_manifest
shellcheck -s sh -S style claude-plugin/bin/*
```

`claude plugin validate .` warns that no `version` is set. That is
deliberate. When a version is set, Claude Code caches the plugin by that version
and delivers no update until it changes. amplihack releases advance by git tag
while `package.json` stays at the workspace base version, so a pinned version
would freeze installs. Without one, the cache follows the commit and
`/plugin marketplace update amplihack` picks up new content.

`--strict` reports two more expected warnings:

- it does not follow the `skills` symlink (a session does);
- it scans `agents/eval-recipes/README.md`, which the explicit `agents` list
  keeps from loading.
