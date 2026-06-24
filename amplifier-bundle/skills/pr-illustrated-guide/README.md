# PR Illustrated Guide

Turns a pull request into a reviewer-friendly **illustrated walkthrough**: the
problem it solves, the approach taken, an exemplar-driven tour of the code with
**mermaid diagrams** and **deep links to the actual diffs**, the key decisions
and trade-offs, and what the tests cover. Output is a single markdown file
ready to paste into the PR description or post as a comment.

Works with **GitHub** (`gh`) and **Azure DevOps** (`az repos`), auto-detected
from the git remote. Trivial PRs are skipped automatically.

## Quick Start

Just describe what you want:

```
Generate an illustrated guide for PR #123
```

```
Write a walkthrough for this PR
```

With no PR number, the skill infers the PR from the current branch:

```
Skill(skill="pr-illustrated-guide")
```

It can also run at the **end of `default-workflow`**, right after a PR is
opened, to attach a walkthrough — no workflow YAML edits required.

## Features

### Triviality Filter

Skips PRs that don't warrant a narrative, emitting a one-line reason:

| Rule                   | Threshold                                                   |
| ---------------------- | ---------------------------------------------------------- |
| Too few files          | fewer than **3** files changed                             |
| Too little real change | fewer than **~30** meaningful lines (excl. whitespace/comments/lockfiles) |
| Config/typo only       | only config files, formatting, or typo fixes               |

### Fixed Five-Section Document

1. **Problem Statement** — from PR title, body, and linked issues / work items.
2. **Approach Overview** — high-level strategy, with a mermaid diagram when it helps.
3. **Detailed Walkthrough** — exemplar code snippets (one per repeated pattern),
   mermaid for complex flows, deep diff links, and callouts for configurable
   constants and non-obvious decisions.
4. **Key Decisions & Trade-offs** — notable choices and the reasoning.
5. **Testing** — which tests were added/changed and what they cover.

### Platform-Aware Deep Links

| Platform     | PR data via                | Diff deep link                                                        |
| ------------ | -------------------------- | --------------------------------------------------------------------- |
| GitHub       | `gh pr view` / `gh pr diff`| `…/pull/<N>/files#diff-<hash>` (falls back to `/files`)               |
| Azure DevOps | `az repos pr show` / `list`| `…/pullrequest/<N>?_a=files&path=/<path>`                            |

### GUI / TUI Screenshots

Detects UI changes (`.tsx`, `.jsx`, `.vue`, `.svelte`, CSS, Playwright tests)
and **attempts** to capture screenshots with Playwright, embedding them in the
walkthrough. If Playwright or a dev server is unavailable, it **degrades
gracefully** to a textual description of the visual change.

### Safe Output

Writes the markdown to an OS temp file (`0600`), prints the absolute path, and
**offers** — never forces — to set it as the PR description or post it as a
comment. Publishing is confirmation-gated; nothing is auto-committed or
auto-uploaded.

## How It Works

```
Resolve input → Detect platform → Fetch PR data → Triviality filter
  → Analyze diff (exemplars + constants) → Detect GUI/TUI (screenshots)
  → Build deep links + mermaid → Assemble 5-section doc
  → Write temp file · print path · offer to post
```

## Files

| File           | Role                                                            |
| -------------- | --------------------------------------------------------------- |
| `SKILL.md`     | Agent-facing entry point: triggers, pipeline, document contract. |
| `reference.md` | Deep logic: CLI bindings, heuristics, URL formats, worked example. |
| `tests/`       | Structural contract test suite (`test_skill_structure.sh`).      |

See [`SKILL.md`](./SKILL.md) for activation and [`reference.md`](./reference.md)
for exact commands, field mappings, and a full worked example.

## Security

All fetched PR content (title, body, diff, paths) is treated as **inert data**,
never as commands. CLI calls are built as **argv arrays** — PR numbers
(`^\d+$`) and branch names (`^[\w./-]+$`) are validated before use, never
interpolated into shell strings. The skill relies on your pre-authenticated
`gh` / `az` sessions and never reads, stores, or logs tokens.
