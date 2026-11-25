# Choose which skills and agents are listed

amplihack lists 32 skills and 12 agents by default in Claude Code, Codex and
Copilot. Every other bundled skill and agent remains installed in the complete
framework and searchable through `amplihack directory find`. Recipes still use
the complete framework.

These steps apply to tools set up through `amplihack install`. They update
the assets each installed tool reads, without adding another tool. Start a new
session after changing the selection.

## Keep an extra entry listed

Search the installed directory, then enable the entry you want:

```sh
amplihack directory find code-visualizer
amplihack directory enable skill:code-visualizer
```

The command saves the extension in the user setting `lists.enabled` and
publishes it for every tool already installed for amplihack. Later installs
and updates keep it.

Use `skill:<leaf-name>` for skills and
`agent:<bundle-relative-path-without-extension>` for agents. For example:

```sh
amplihack directory enable agent:core/guide
```

A bare name works when it identifies one entry. An ambiguous name is rejected
with the qualified alternatives. For example, `guide` needs either
`agent:guide` or `agent:core/guide`; `tla-plus-expert` needs its skill or agent
qualification. The [directory reference](../reference/directory-command.md)
describes accepted prefixes and error results.

Run enable only when you want a persistent change. In a recipe step, read an
unlisted definition through the directory instead of changing the tool's
published assets. See [Find installed skills and agents](find-installed-skills-and-agents.md).

## Remove an extension

```sh
amplihack directory disable skill:code-visualizer
```

This removes the explicit extension and republishes the selection. It leaves
the complete framework definition installed. Default entries cannot be
disabled. Repeating enable or disable leaves the same desired selection.

## Restore the full lists

Run this one command:

```sh
amplihack config set lists.set all
```

It saves the setting and immediately restores each installed tool's full
inventory. Codex keeps its established full-mode exclusions for analyst
skills. Extensions remain recorded for a later return to the smaller set.

To return to the defaults plus your extensions:

```sh
amplihack config set lists.set default
```

You can inspect either setting with `amplihack config get`. To replace the
whole extension list, use the quoted TOML array syntax in the
[configuration reference](../reference/user-config.md#listsset-and-listsenabled).

## Read the upgrade notice

An update that removes installer-owned entries from an existing tool's lists
saves a notice in `list-changes.txt` beside the ownership records in the
[install staging folder](../reference/directory-command.md#where-the-index-lives).
It reports removed skills and agents separately for each affected tool, and
identifies edited installer-owned skills left in place. Independently
installed skills are left untouched and are not included in the notice.

The notice appears once on standard error at a terminal. Recipe steps and
commands without a terminal save it for display by a later terminal command.
A fresh default installation needs no upgrade notice.

If publication fails, the command reports the failed tool and any completed
tools. The desired configuration stays saved. Correct the reported problem
and repeat the same command to finish applying it.

## When loading the repository as a plugin

The [repository plugin route](install-claude-code-plugin.md) always exposes
the shared default set. It does not read `lists.set` or `lists.enabled`, and
directory enable cannot extend that plugin's lists. Use the managed
installation route for persistent extensions or full lists. Where the
complete framework and its index are installed, you can still find and read
an unlisted definition through the directory.
