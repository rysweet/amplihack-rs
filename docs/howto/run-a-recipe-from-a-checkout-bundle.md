---
title: Run a recipe from a checkout's bundle
description: Choose between the installed bundle and a checkout's bundle for a recipe run, check which one a run used, and fix trust errors.
doc_type: howto
last_updated: 2026-11-13
related_issues: [2528]
---

# Run a recipe from a checkout's bundle

By default `amplihack recipe run` uses the installed bundle for recipes and
tools, even inside an amplihack-rs checkout. This guide shows how to use a
checkout's `amplifier-bundle/` instead, how to see which bundle a run used,
and how to fix the errors you may meet.

The rules are in
[Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md).

## Contents

- [Run with the installed bundle](#run-with-the-installed-bundle)
- [Run with a checkout's bundle](#run-with-a-checkouts-bundle)
- [Check which bundle a run used](#check-which-bundle-a-run-used)
- [Run a recipe file that is not in the bundle](#run-a-recipe-file-that-is-not-in-the-bundle)
- [Upgrade while runs are in progress](#upgrade-while-runs-are-in-progress)
- [Fix an untrusted bundle error](#fix-an-untrusted-bundle-error)
- [Fix a recipe not found error](#fix-a-recipe-not-found-error)

## Run with the installed bundle

Nothing extra is needed:

```bash
cd ~/src/amplihack-rs
amplihack recipe run default-workflow \
  -c task_description="Add a --json flag to recipe list" \
  -c repo_path="$PWD"
```

stderr starts with:

```
recipe bundle: /home/alice/.amplihack/releases/1.2.151/amplifier-bundle (1.2.151); tools: same bundle
```

The checkout's `amplifier-bundle/` is not read, even though the run started
inside the checkout.

If your shell sets `AMPLIHACK_HOME` to the checkout, the run skips it, prints
a note and uses the next candidate (`paths.home`, then `~/.amplihack`):

```
recipe bundle: skipping AMPLIHACK_HOME=/home/alice/src/amplihack-rs: it is a git checkout; pass --bundle checkout to use its bundle
recipe bundle: /home/alice/.amplihack/releases/1.2.151/amplifier-bundle (1.2.151); tools: same bundle
```

## Run with a checkout's bundle

Pass `--bundle checkout` from anywhere inside the checkout, or name the
checkout with `--working-dir`:

```bash
cd ~/src/amplihack-rs
amplihack recipe run default-workflow --bundle checkout \
  -c task_description="Add a --json flag to recipe list" \
  -c repo_path="$PWD"
```

```
recipe bundle: /home/alice/src/amplihack-rs/amplifier-bundle (1.2.152+checkout); tools: same bundle
```

The context key does the same and is convenient in scripts that build a list
of `-c` values:

```bash
amplihack recipe run default-workflow -c bundle=checkout \
  -c task_description="Add a --json flag to recipe list"
```

Give only one of the two. If both are given and differ, the run is refused.

Every sub-recipe, agent and tool then comes from the checkout. Before the run
starts, the checkout's bundle is checked and copied, so you can keep editing
the checkout while the run continues: the run does not see your edits.

To check the bundle without running anything, add `--dry-run`. It runs the
same checks and reports the same errors, but makes no copy and points the
runner at the checkout's bundle directly.

## Check which bundle a run used

The stderr line names the source and version. The run's log pointer records
have the same information in `recipe_bundle` and `tool_bundle`. With stderr
saved to `recipe-progress.log`:

```bash
grep '^amplihack\.recipe\.log_pointer ' recipe-progress.log \
  | sed 's/^amplihack\.recipe\.log_pointer //' \
  | jq -c 'select(.event == "early") | .recipe_bundle'
# {"source":"/home/alice/src/amplihack-rs/amplifier-bundle","version":"1.2.152+checkout",
#  "snapshot":"/home/alice/.amplihack/work/runs/3f0c9a52-.../runtime/bundle","choice":"checkout","inherited":false}
```

See [Correlate recipe runs with logs](./correlate-recipe-runs.md) for the
other ways to find a run's records.

Inside a step, `AMPLIHACK_RUN_BUNDLE_CHOICE` holds `installed` or `checkout`,
and `AMPLIHACK_HOME` points at the run's copy:

```bash
echo "$AMPLIHACK_RUN_BUNDLE_CHOICE $AMPLIHACK_HOME"
# checkout /home/alice/.amplihack/work/runs/3f0c9a52-.../runtime/bundle
```

## Run a recipe file that is not in the bundle

Give a path instead of a name. Anything containing `/` or ending in `.yaml`
counts as a path:

```bash
amplihack recipe run ./experiments/triage.yaml -c issue_number=2528
```

The file is loaded from that path. Its sub-recipes, agents and tools come from
the installed bundle, or from the checkout with `--bundle checkout`. The log
record keeps the file path in `recipe_file`.

## Upgrade while runs are in progress

Run `amplihack update` whenever you like. Each running recipe run keeps the
copy it made at start and finishes with that version, including nested runs
it starts later. New runs use the new version.

Nothing needs to be cleaned up afterwards. Each run removes its copy when it
ends. A run killed with `SIGKILL` leaves its copy behind until the next run
finishes and sweeps it.

## Fix an untrusted bundle error

```
untrusted bundle entry /home/alice/src/amplihack-rs/amplifier-bundle/tools/workflow_context.sh: mode 0664 (group or other writable); fix with chmod go-w /home/alice/src/amplihack-rs/amplifier-bundle/tools/workflow_context.sh
```

A file or directory in the bundle can be changed by your group or by other
users. This usually comes from a umask of `002` when the files were created.
Remove the write bits from the whole bundle and set a safer umask for the
shell you work in:

```bash
chmod -R go-w ~/src/amplihack-rs/amplifier-bundle
umask 022
```

For the installed bundle, fix the release directory the error names, for
example:

```bash
chmod -R go-w ~/.amplihack/releases/1.2.151
```

Errors that name a symlink (`absolute symlink`, `symlink leaves the bundle`,
`dangling symlink`, `symlink cycle`) mean a link inside the bundle points
outside it or nowhere. Replace it with a relative link to a file inside the
bundle, or remove it.

## Fix a recipe not found error

```
recipe 'my-recipe' not found in the installed bundle at /home/alice/.amplihack/releases/1.2.151/amplifier-bundle; to use this checkout's recipes, pass --bundle checkout
```

Bare recipe names are looked up only in the selected bundle's `recipes/`
directory. Either:

- the recipe exists only in your checkout: add `--bundle checkout`; or
- the recipe is a local file: pass its path, for example
  `./recipes/my-recipe.yaml`.

## Related

- [Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md): full rules and error list
- [One bundle per recipe run](../concepts/one-bundle-per-recipe-run.md): why runs work this way
- [Recipe CLI commands](./recipe-cli-commands.md): other recipe tasks
