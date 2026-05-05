# Five-Type Memory (Superseded)

> Ported from legacy upstream `docs/memory/5-TYPE-MEMORY-GUIDE.md`.
> The upstream page exists only as a redirect to current memory docs; the
> redirect is preserved here for amplihack-rs users who follow links from
> older READMEs or external references.
>
> **See also:** [Memory Backend Architecture](./memory-backend-architecture.md) ·
> [Memory Tree](./memory-tree.md) ·
> [Agent Memory Quickstart](../howto/agent-memory-quickstart.md)

This older guide described an automatic 5-type memory flow with hook behavior
and backend assumptions that are **not** the current source of truth for
amplihack-rs.

## Use These Docs Instead

- [Agent Memory Quickstart](../howto/agent-memory-quickstart.md)
- [Memory Backend Architecture](./memory-backend-architecture.md)
- [Memory Backend Reference](../reference/memory-backend.md)
- [LadybugDB Reference](../reference/ladybug-reference.md)
- [Memory Tree](./memory-tree.md)

## Why This Page Was Retired

The current docs separate two concerns that the 5-type guide conflated:

- **The in-repo CLI memory backend** — exposed by the `amplihack memory`
  subcommands (`tree`, `export`, `import`, `clean`) and backed by either
  SQLite or LadybugDB. See
  [Memory Backend Architecture](./memory-backend-architecture.md).
- **The generated `amplihack new --enable-memory` scaffold** — a standalone
  goal-agent package that ships its own local `./memory/` directory and its
  own backend selection. See
  [Agent Memory Quickstart](../howto/agent-memory-quickstart.md).

The older automatic-hook narrative and backend performance claims from the
previous version of this page should not be treated as the current user
guide.

> **About the name.** "Five-type memory" refers to the original taxonomy of
> `conversation`, `decision`, `pattern`, `context`, and `learning`. A sixth
> type, `artifact`, was added later for binary or large-payload entries; the
> page name is preserved for compatibility with older external links. All six
> values are accepted by `amplihack memory tree --type` for backwards
> compatibility — see [Memory Tree](./memory-tree.md) for the supported list.
