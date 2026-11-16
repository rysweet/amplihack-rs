---
title: One bundle per recipe run
description: Why a recipe run pins one checked, content-keyed copy of an amplifier-bundle and uses it for recipes and tools alike.
doc_type: explanation
last_updated: 2026-11-15
related_issues: [2528, 1741]
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
- [Why the copy's path depends on its contents](#why-the-copys-path-depends-on-its-contents)
- [Why the same trust checks for both choices](#why-the-same-trust-checks-for-both-choices)
- [Why cleanup has no separate process](#why-cleanup-has-no-separate-process)
- [Why nested runs reuse the copy](#why-nested-runs-reuse-the-copy)
- [Why state stays outside the copy](#why-state-stays-outside-the-copy)
- [Why a path into another bundle is refused](#why-a-path-into-another-bundle-is-refused)
- [Why a path into the run's own home is not another bundle](#why-a-path-into-the-runs-own-home-is-not-another-bundle)

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
- **Copy.** Copy the selected bundle into a private, read-only folder and
  point the run at the copy. This works the same way for every layout and
  every platform, and nothing outside the run can change it.

The bundle is about 13 MB, auto-drive already copied it for each drive, and
nested runs reuse the outer copy, so the cost is at most one copy per
launch. The copy was chosen.

## Why the copy's path depends on its contents

The runner can resume a run that stopped part-way, for example at a usage
limit (#1741). Before it reuses earlier results, it compares the settings of
the new launch with those it stored for the stopped run. Those settings
include `AMPLIHACK_HOME`, the recipe and agent directories passed to the
runner and the canonical paths of the sub-recipes it loaded, all of which
name the copy.

A copy at a new path for every launch, such as one inside the run's own
`runtime/` folder, therefore made every resumed run look like a different
run, and recovery started again from the first step. Pointing a symlink at
the copy would not help, because the runner resolves paths before comparing
them.

The copy is instead stored at `<work root>/bundle-pins/<key>`, where the key
is a digest of the choice, the source path, the version and the full contents
of the bundle. A relaunch with the same bundle computes the same key, gets
the same paths and resumes. A relaunch after the bundle has changed gets a
new key, and the runner sees the change and starts again from the beginning.
That costs a full rerun after an upgrade, but a resumed run never combines
results from one bundle with steps from another.

A key based on the run's recovery identity was also considered. It was
rejected because two runs of the same recipe at the same time would then
share one path while possibly needing different bundles.

A content key means that runs of the same bundle share one copy. This is safe
because the copy is read-only and its contents are fixed by its key. Before a
launch reuses an existing copy, it computes the full digest of that copy
again and compares it with a fresh copy of the source. The 24-digit key is
used only to name the folder; the decision rests on the full 256-bit digest.
The digest leaves out the write bits, because the fresh copy is writable and
the stored copy is read-only; with them included, no copy would ever match.
The source is still checked and copied at every launch, so a stored copy
never stands in for the trust checks.

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

Because runs of the same bundle share a copy, no single run owns it. Each run
holds a shared `flock` on the copy's `<key>.use` file for as long as it runs.
When a run ends, on success, on error or during unwinding, it releases its
lock and tries to take the file exclusively without waiting. If that
succeeds, no other run is using the copy, and the run removes it. A separate
`.lock` file, held while a launch creates a copy and while a run removes one,
keeps a new launch from picking up a copy that is being removed.

Linux cannot turn an exclusive `flock` into a shared one without releasing
it first, which would leave a gap in which another run could remove the
copy. Two lock files avoid needing that change.

A run killed with `SIGKILL` cannot clean up after itself, but the kernel
releases its locks. The next launch on the same work root, while it holds
`.lock`, removes every copy whose `<key>.use` it can take exclusively. No new
process, thread or timer is involved, and when `recipe run` returns it has no
cleanup work still running.

The locks block without a timeout. A launch that is stopped while it holds
`.lock` makes later launches wait until it is resumed or ended. The waiting
launch prints the lock's path, so the cause can be found.

## Why nested runs reuse the copy

Many recipes call `amplihack recipe run` from a step. If each nested run made
its own selection, a nested run could pick a different bundle from its parent
and reintroduce the mixture. It would also copy the bundle again for every
level.

The outer run therefore sets `AMPLIHACK_RUN_BUNDLE` to its copy. A nested run
that finds a valid marker uses the same copy, takes its own shared lock on
it, and never removes it. A nested run that asks for a different bundle is
refused rather than mixed. A marker that fails its checks is ignored, so a
stale or forged value cannot point a run at an unchecked directory. The
marker must name a copy in this run's own `bundle-pins/` folder, because that
is where the nested run opens the lock file. If the lock file is missing, the
nested run is refused instead of running from a copy that nothing protects.

## Why state stays outside the copy

Some code derives its state directory from `AMPLIHACK_HOME`. With
`AMPLIHACK_HOME` pointing at the copy, that state would land in the shared,
read-only copy and either fail or be deleted when the last run using it
ends. `recipe run` sets `AMPLIHACK_STATE_HOME` to the state root that
applied before it changed `AMPLIHACK_HOME`, and state readers check it first.
A nested run passes the inherited value through rather than computing it
again, since by then `AMPLIHACK_HOME` names the copy. If the state root would
still fall inside the run's `runtime/` folder or under `bundle-pins/`, the
run is refused before any step starts.

## Why a path into another bundle is refused

A recipe can be given as a path as well as by name. Previously, a path was
always treated as a standalone file: the file was loaded from where it was,
and its sub-recipes and tools came from the selected bundle. Run inside an
amplihack-rs checkout, `amplihack recipe run
amplifier-bundle/recipes/smart-orchestrator.yaml` therefore loaded the
checkout's recipe and the installed bundle's sub-recipes and tools. That is
the mixture this design exists to prevent, reached by a different route.
Several skills and how-to guides told agents to start work this way.

A path is therefore classified by where it leads, after `..` and symlinks are
resolved:

- a path inside the run's own bundle is a bundle recipe and runs from the
  copy, like a bare name. For a nested run, this includes the home it
  inherited, which the next section describes;
- a path inside another bundle's `recipes/` directory is refused, with a
  message that gives the bare name and says how to select that bundle:
  from inside its checkout with `--bundle checkout` when it is in a git
  checkout, otherwise by name with `--bundle installed`;
- any other path is a standalone file. A user recipe outside every bundle
  has no bundle of its own, so the selected bundle supplies its tools.

Refusal was chosen over silently switching to the bundle that holds the file.
Switching would let the working directory or the form of an argument select
a bundle, which is how #2528 arose, and the stderr line would no longer
follow from the flags the user gave. The refusal costs one rerun and names
the command to use.

Docs and skills now start work by bare name, so the path form is needed only
for standalone files, and for the one case the next section describes.

## Why a path into the run's own home is not another bundle

The loops that drive a run do not start their rounds by name. Each resolves
the round's recipe in the `recipes/` directory beside the tools it is
running from, which inside an auto-drive run is
`<launch copy>/amplifier-bundle/recipes`, and passes that path to
`recipe run`: `autodrive_loop.sh` for the crusty and merge rounds and the
health evaluator, `workflow_hook_repair.sh` for the hook-repair rounds.

They run the file rather than the name for two reasons. The first is why the
loops resolve the path themselves at all: a round's recipe must come from
the bundle the loop was launched from, never from the worktree under
review, which carries its own `recipes/` that a bare name would find first,
so that the tree being judged does not supply its own reviewer or stop rule
(#1543, #1528). The second applies to the recipes `prepare` rewrites in
each launch copy, those whose `name` is `auto-drive-to-merge` or
`auto-drive-light` or begins with `autodrive-`: for those, the file in the
copy is the one carrying the run's authorisation. The health evaluator and
the hook-rejection repair recipe are not rewritten and rest on the first
reason alone. Classifying such a path as a path into another bundle would
mean that no round could start inside a drive.

The path is not into another bundle, and the run can tell so without a new
test. Before a nested run looks at its recipe argument it has already
checked `AMPLIHACK_RUN_BUNDLE` against `AMPLIHACK_HOME` and accepted one of
them: the snapshot itself, or the launch copy whose `bundle-source` names
the snapshot. That accepted home is the bundle this run is already using. A
path under its `recipes/` is therefore treated like a bare name, by the same
check that let the nested run reuse the snapshot in the first place, rather
than by a second rule that could drift from it. A top-level run has accepted
no home and is unaffected.

Such a run still takes its sub-recipes, agents and tools from the snapshot.
Treating the file as a standalone recipe instead would have added the copy's
`recipes/` directory to the directories the runner searches, which is the
second lookup this design exists to remove, and the copy is writable where
the snapshot is not. Keeping the search on the snapshot costs the round
nothing: `prepare` writes a fresh launch copy, with its own authorisation,
for each nested auto-drive run, so a round started this way is authorised by
its own launch rather than by what it can reach on disk.

Because the accepted home comes from checks the run has already made, and is
kept only in memory, nothing on disk tells a later process which paths to
accept. Everything else about the classification is unchanged: canonical
paths are compared component by component, so `recipes-old/` and `tools/`
still do not match, a symlink is judged by its target (and a launch copy
holds none, since the copy skips them), and a path into some other run's
snapshot is still a path into another bundle.

## Related

- [Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md): rules, environment and errors
- [Run a recipe from a checkout's bundle](../howto/run-a-recipe-from-a-checkout-bundle.md): task steps
- [Give a test its own installed bundle](../howto/give-a-test-its-own-installed-bundle.md): fixtures for tests and scenarios that drive a run
- [Versioned install layout](./versioned-install-layout.md): how installs switch releases
- [Recipe runner architecture](./recipe-runner-architecture.md): how the CLI and the runner divide the work
