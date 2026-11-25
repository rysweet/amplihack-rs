# Find installed skills and agents

amplihack installs more skills and agents than a session lists. Every one of
them is recorded in an index that `amplihack install` and `amplihack update`
write. This page shows how to search that index and how to use an entry you
find. For the file format and the exact behaviour, see the
[directory command reference](../reference/directory-command.md).

## Before you start

Run `amplihack install` once. It stages the framework and writes the index.
If the index is missing, `amplihack directory` says so and asks you to run
`amplihack install`.

## Search for an entry

Give one or more words. Every word must appear in the entry's name or
description; letter case does not matter.

```sh
amplihack directory find kubernetes
amplihack directory find spreadsheet formulas
```

Each result is one tab-separated line: type, name, description and path.

```text
skill	xlsx	<description>	<home>/.amplihack/.claude/skills/xlsx/SKILL.md
```

When nothing matches, the command prints nothing on standard output and one
line on standard error. Try fewer or broader words.

To see only the names, grouped by type, run `find` with no words:

```sh
amplihack directory find
```

## Use a skill you found

Read the SKILL.md at the printed path in full and follow it. Resolve the
files it mentions against the folder that holds that SKILL.md. A skill that
is not in your session's list is not registered with the Skill tool, so do
not call the tool with its name.

## Use an agent you found

Start a general-purpose agent whose prompt begins:

```text
Read <path> in full and act as the agent it defines.
```

Replace `<path>` with the printed path, then give the agent its task. Do not
pass the agent's name as the agent type.

## Let a model do it

The bundled skill `skill-and-agents-directory` gives a model the same
directions. It is staged for Claude Code, Codex and Copilot like every other
bundled skill.
