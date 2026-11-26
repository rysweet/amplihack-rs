# Choose which skills and agents are listed

amplihack lists 30 skills and 12 agents by default in Claude Code, Codex and
Copilot. Every other bundled skill and agent remains installed in the complete
framework and searchable through `amplihack directory find`. Recipes still use
the complete framework.

The default set excludes `auto-drive-to-merge`, which drives a pull request
through an owner's merge conditions, and `claude-profile-rotation`, which
rotates between several logins. Enable either with `amplihack directory enable`
when that way of working applies.

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
with the qualified alternatives. For example, `tla-plus-expert` needs its skill
or agent qualification. The [directory reference](../reference/directory-command.md)
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

In Codex, the generated command skills leave with `default` and return with
`amplihack config set lists.set all`. They cannot be enabled individually,
because the directory indexes skills and agents only. Claude Code's typed
commands remain available under both settings.

To return to the defaults plus your extensions:

```sh
amplihack config set lists.set default
```

You can inspect either setting with `amplihack config get`. Change individual
extensions with `directory enable` and `directory disable`; `config set
lists.enabled` is not supported. `config unset` and `config edit` change only
the configuration file, taking effect at the next install or update, or at
once with `amplihack config set lists.set <value>` even when the value is unchanged.

Applying a list change requires an installation that matches the running
binary. If the versions differ, run `amplihack install` first; the list command
leaves the configuration and tool files unchanged.

## Read the upgrade notice

The first install, update or launch publication that narrows an existing
tool's lists saves a notice in `list-changes.txt` beside the ownership records in the
[install staging folder](../reference/directory-command.md#where-the-index-lives).
It reports removed skills and agents separately for each affected tool,
counting only entries still in the bundle and outside the selection.

Only the command that writes the notice prints it, on standard error at a
terminal and outside a recipe step. No later command displays it. The file
remains available to read. If the file cannot be saved, the command still
prints the notice and warns. Notice failures never change the command's
result. Enable, disable and configuration changes never write
it. A fresh default installation needs no upgrade notice.

If publication fails, the command reports the failed tool and any completed
tools. The desired configuration stays saved. Correct the reported problem
and repeat the same command to finish applying it.

## Keep your own Copilot skills

In a Copilot home, amplihack manages the top-level Markdown of the skill
folders named after bundle skills, including the nested skills
`analyzing-deeply`, `architecting-solutions`, `creating-pull-requests`, `researching-topics`, `reviewing-code`, `setting-up-projects` and `testing-code`, under both settings, so a Copilot skill of your own needs a
folder name the bundle does not use.

## When loading the repository as a plugin

The [repository plugin route](install-claude-code-plugin.md) always exposes
the shared default set. It does not read `lists.set` or `lists.enabled`, and
directory enable cannot extend that plugin's lists. Use the managed
installation route for persistent extensions or full lists. Where the
complete framework and its index are installed, you can still find and read
an unlisted definition through the directory.
