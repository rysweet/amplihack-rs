---
name: skill-and-agents-directory
description: Find amplihack skills and agents that are installed but not in your lists. Use when a task needs specialist guidance that no listed skill or agent covers (for example a language, cloud platform, document format, formal method, audit or review method), or when a skill or agent you were told to use is reported as unknown. Returns descriptions and file locations.
---

# Skill and agents directory

amplihack installs more skills and agents than appear in your lists. Every
one of them is recorded in an index that `amplihack install` writes. This
skill tells you how to find an entry in that index and how to use what you
find.

## When not to search

Do not search when a skill or agent in your lists fits the task, or when the
task needs no specialist guidance. Use the listed one, or simply do the work.

## Search

Run `amplihack directory find` with one or more words that describe what you
need:

```sh
amplihack directory find kubernetes
amplihack directory find spreadsheet formulas
```

Each result is one tab-separated line: type (`skill` or `agent`), name,
description and the absolute path of its file. Every word must appear in the
name or the description; letter case does not matter. A word that contains a
colon is matched by the part after its last colon, so a name with a prefix,
such as `amplihack:core:reviewer`, is found by `reviewer`. Use fewer or
broader words when nothing matches. `amplihack directory find` with no words
prints the names only.

Never read the whole index. Search for what the task needs.

If the command reports that the index is missing or has no entries, tell the
person that the index is missing and ask them to run `amplihack install`.
Never run `amplihack install` yourself.

## Use a skill you found

Read the SKILL.md at the path given, in full, and follow it. Resolve every
file it mentions, such as scripts, templates and references, against the
folder that holds that SKILL.md.

Do not call the Skill tool with the skill's name. A skill that is not in your
lists is not registered with it.

## Use an agent you found

Start a general-purpose agent whose prompt begins:

```text
Read <path> in full and act as the agent it defines.
```

Replace `<path>` with the path from the result, then give the agent its task
after that sentence.

If the tool has no way to start an agent, read the definition in full and
follow it in the current session.

Do not pass the agent's name as the agent type. An agent that is not in your
lists is not registered under that name.

## Turn an entry on for later sessions

Only when the user asks, run `amplihack directory enable <name>` to keep an
entry in the tool's lists from the next session. Never run it inside a recipe
step: it changes the assets that tools read. Use the qualified form when a
name is ambiguous, such as `skill:tla-plus-expert` or
`agent:specialized/tla-plus-expert`. The command prints the alternatives for
an ambiguous name.

`amplihack directory disable <name>` removes an explicit extension. It cannot
remove an entry from the default set. To restore the full lists in one command,
the user can run `amplihack config set lists.set all`.
