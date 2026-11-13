---
title: One bundle per recipe run
description: Why a recipe run copies one amplifier-bundle into its workspace and uses it for recipes and tools alike.
doc_type: explanation
last_updated: 2026-11-13
related_issues: [2528]
---

# One bundle per recipe run

A recipe run reads two kinds of file from `amplifier-bundle/`: recipes (YAML,
including sub-recipes and agent definitions) and tools (shell scripts and
helpers under `tools/`). This page explains why `amplihack recipe run` takes
both from a single bundle, why that bundle is the installed one unless the
user asks otherwise, and why the run works from a private copy.

For the exact rules, see
[Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md).

## Contents

- [The problem: two bundles in one run](#the-problem-two-bundles-in-one-run)
- [The problem: an upgrade during a run](#the-problem-an-upgrade-during-a-run)
- [Why the installed bundle is the default](#why-the-installed-bundle-is-the-default)
- [Why a copy rather than a reference](#why-a-copy-rather-than-a-reference)
- [Why the same trust checks for both choices](#why-the-same-trust-checks-for-both-choices)
- [Why cleanup has no separate process](#why-cleanup-has-no-separate-process)
- [Why nested runs reuse the copy](#why-nested-runs-reuse-the-copy)
- [Why state stays outside the copy](#why-state-stays-outside-the-copy)

## The problem: two bundles in one run

Recipe lookup used to start at the repository root found by walking up from
the working directory, while tool lookups in recipe YAML started at
`AMPLIHACK_HOME`. A run started inside an amplihack-rs checkout could
therefore load a recipe from the checkout and call a tool from the installed
release.

When the checkout was newer, the recipe passed arguments the older tool did
not know. Issue #2528 records such a failure: a recipe from the checkout
passed `--invocation` to an installed tool that rejected it with
`unknown argument`. Neither bundle was wrong on its own. The mixture was.

Using one bundle for both removes the mixture. Whatever bundle the run uses,
its recipes and its tools were written and tested together.

## The problem: an upgrade during a run

Long recipe runs (a full `default-workflow` can take hours) often span an
`amplihack update`. The versioned install layout switches the
`amplifier-bundle` link atomically, so no file is ever half-written, but a
step that reads `~/.amplihack/amplifier-bundle/...` after the switch sees the
new release. Steps 1 to 10 of a run could use version A and steps 11 to 20
version B, with the same kind of mismatch as above.

A run should finish with the bundle it started with. The copy described below
gives that property for both install forms: a flat `amplifier-bundle/`
directory replaced by `mv`, and a release symlink re-pointed to a new release.

## Why the installed bundle is the default

The installed bundle is the one the user installed and the one released
binaries were tested against. A checkout may be at any commit, may carry
uncommitted edits and may be newer or older than the installed binary.
Picking it silently because the working directory happens to be inside it is
how the `--invocation` failure arose.

The same applies when `AMPLIHACK_HOME` itself names a checkout, as it does in
some development shells. The installed choice skips any candidate home that
holds a `.git` entry, prints a note naming it, and moves to the next
candidate. A checkout's bundle is therefore used only when the user asks for
it.

Developers working on the bundle itself still need to run their changes, so
`--bundle checkout` (or `-c bundle=checkout`) selects the checkout's bundle
for recipes and tools together. The stderr line at the start of each run
names the bundle in use, so the choice is visible.

## Why a copy rather than a reference

Two ways of pinning were considered:

- **Reference.** Record the canonical release path and point the run at it.
  This works only for the release layout; a flat directory replaced by `mv`
  leaves nothing to point at. Protecting a release from removal while runs
  use it needs a scan of other processes' environments, which relies on
  `/proc` and does not exist on macOS.
- **Copy.** Copy the selected bundle into the run's own `runtime/` folder and
  point the run at the copy. This works the same way for every layout and
  every platform, and nothing outside the run can change it.

The bundle is about 13 MB, auto-drive already copied it for each drive, and
nested runs reuse the outer copy, so the cost is one copy per run tree. The
copy was chosen.

## Why the same trust checks for both choices

The tools in a bundle run with the user's privileges and, in auto-drive, can
approve a merge. Auto-drive already refused bundles with files that another
user, or a group, could change (#1551, #2340). Once the installed bundle
supplies tools to every recipe run, and a checkout's bundle can supply tools
on request, the same rule applies to both. A shared rule also keeps one
implementation in `trusted.rs` instead of two that could drift apart.

Checking and copying each entry in one pass, with the opened handle compared
against the checked metadata, means the copy contains exactly the files that
were checked. After the copy, nothing in the run reads the source again, so a
change to the source during the run has no effect.

A consequence is that an install made under umask `002`, which leaves files
group-writable, is refused until its modes are fixed. The error names the
file, its mode and the `chmod` command that fixes it.

## Why cleanup has no separate process

An earlier attempt at this issue added a `recipe bundle-reap` cleanup
process. One build lane for it left 847 of those processes running on its
host, started by its tests and never waited for. A process that outlives the run that
started it needs its own supervision, can pile up when runs are killed, and
is hard to test without waiting for it.

The copy lives under `<run dir>/runtime/`, which the run's `RuntimeGuard`
already removes on success, on error and during unwinding. A run killed with
`SIGKILL` cannot clean up after itself, so its folder is removed by the
end-of-run sweep of the next run that finishes. No new process, thread or
timer is involved, and when `recipe run` returns it has no cleanup work still
running.

## Why nested runs reuse the copy

Many recipes call `amplihack recipe run` from a step. If each nested run made
its own selection, a nested run could pick a different bundle from its parent
and reintroduce the mixture. It would also copy the bundle again for every
level.

The outer run therefore sets `AMPLIHACK_RUN_BUNDLE` to its copy. A nested run
that finds a valid marker uses the same copy and does not own it. A nested run
that asks for a different bundle is refused rather than mixed. A marker that
fails its checks is ignored, so a stale or forged value cannot point a run at
an unchecked directory.

## Why state stays outside the copy

Some code derives its state directory from `AMPLIHACK_HOME`. With
`AMPLIHACK_HOME` pointing at the copy, that state would land in `runtime/`
and be deleted when the run ends, or would change the bundle in the middle of
the run. `recipe run` sets `AMPLIHACK_STATE_HOME` to the state root that
applied before it changed `AMPLIHACK_HOME`, and state readers check it first.
A nested run passes the inherited value through rather than computing it
again, since by then `AMPLIHACK_HOME` names the copy. If the state root would
still fall inside a run's `runtime/` folder, the run is refused before any
step starts.

## Related

- [Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md): rules, environment and errors
- [Run a recipe from a checkout's bundle](../howto/run-a-recipe-from-a-checkout-bundle.md): task steps
- [Versioned install layout](./versioned-install-layout.md): how installs switch releases
- [Recipe runner architecture](./recipe-runner-architecture.md): how the CLI and the runner divide the work
