# directory command and the skills and agents index

`amplihack directory` searches the index of the skills and agents that
amplihack installs. `amplihack install` writes the index; `amplihack update`
runs `install`, so it rewrites the index too. For a task-based guide, see
[Find installed skills and agents](../howto/find-installed-skills-and-agents.md).

## Synopsis

```sh
amplihack directory find [WORD]...
amplihack directory list
```

With no subcommand, the command prints its help.

## Subcommands

| Subcommand | Output |
| --- | --- |
| `find WORD...` | Every entry whose name or description contains all the words, ignoring letter case, one per line: type, name, description, path, separated by tabs, in index order. The path is not searched. A quoted word with spaces is matched as given. A word that contains a colon is matched by the part after its last colon, so `amplihack:core:reviewer` is matched as `reviewer`. |
| `find` | The names only: `skills:`, then each skill name indented by two spaces, then `agents:` and each agent name. A name that occurs twice in one group is printed once. |
| `list` | Every entry, in the same form as `find WORD...`. |

Neither subcommand writes anything. `directory` skips the start-up repair of staged assets that most other subcommands run first, so after the binary is upgraded it reads what the last install wrote until the next install or update.

## Exit behaviour

| Situation | Output | Exit code |
| --- | --- | --- |
| Entries match | The entries on standard output | 0 |
| No entry matches | Nothing on standard output; one line on standard error | 0 |
| The index is missing, or the file has no entry | A message on standard error that the index is missing or has no entries, which asks you to run `amplihack install` | non-zero |
| A line of the index has fewer than four fields, or an unknown type | A warning on standard error with the line number; the line is skipped | unchanged |

## Where the index lives

```text
<home>/.amplihack/.claude/install/skills-and-agents-index.tsv
```

`<home>` is the `HOME` of the user who ran `amplihack install`. The index sits
in the installer's staging folder, beside the skill ownership manifest
`claude-skills-manifest.json`. The writer and the reader find that folder
through the same function, so they always agree.

The command does not read `AMPLIHACK_HOME` or `paths.home` to find the index,
because the installer does not read them to stage the framework. A session in
which `AMPLIHACK_HOME` points at a repository checkout still finds the index
the last install wrote.

## File format

- One line for each skill and each agent of amplihack's own bundle.
- Four fields separated by a tab, in this order: type (`skill` or `agent`),
  name, absolute path, description. Each line ends with a newline. A reader
  uses the first four fields of a line and ignores any after them, so a later
  version can add a field.
- No header, no comments and no timestamp. An empty bundle gives an empty
  file.
- Lines are sorted by type, then name in byte order, then path, which is
  compared one component at a time, so agents come before skills. Two
  installs of the same bundle write the same bytes.
- In a name or a description, every run of whitespace or control characters
  becomes one space, and the result is trimmed.

## What the index records

| Type | Source | Name | Path | Description |
| --- | --- | --- | --- | --- |
| `skill` | Each folder under the staged `skills` tree that holds a `SKILL.md`, at any depth | The folder's own name, so a nested skill appears under its leaf name | The `SKILL.md` | Front matter `description`, then `meta.description`, else empty |
| `agent` | Each `*.md` file under the staged `agents` tree, at any depth | Front matter `name`, then `meta.name`, else the file stem | The agent's markdown file | Front matter `description`, then `meta.description`, else empty |

An agent file with no front matter still has a line, with an empty
description. When front matter cannot be read, the entry is written with an
empty description and a warning names the file.

Two entries may share a name when two files define it; both are kept.

The index never holds:

- skills from the user's own skills folder, `<home>/.claude/skills`;
- commands.

## When the index is not written

The install writes the index to a temporary file in the same folder and then
renames it. If the write fails, or a path holds a tab or a newline, the
install prints a warning and goes on. `amplihack directory` then reports the
index as missing.
