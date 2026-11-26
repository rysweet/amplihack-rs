---
title: Repair a stale framework bundle
description: Diagnose and repair an install where the binary is current but smart-orchestrator recipes are stale.
last_updated: 2026-11-11
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Repair a stale framework bundle

Use this guide when `amplihack --version` shows a current release but
`amplihack recipe run smart-orchestrator` fails with old recipe behavior,
especially Python/importlib errors or `orch_helper.py`-style failures.

Current installs validate and repair this automatically. A successful
`amplihack install` or post-update install replaces stale
`smart-orchestrator.yaml` assets with the canonical composable recipe and its
four companion recipes.

## Root cause fixed in current builds

The regression came from stale installed assets, not from the current
`amplihack-rs` source bundle. The stale path was:

```text
~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
```

Older installs could leave that top-level bundle at the old v1.5.1 monolithic
recipe while `.installed-version` still matched the binary. Startup self-heal
trusted the version stamp alone, and recipe resolution could prefer
`AMPLIHACK_HOME/amplifier-bundle/recipes` before a fresh compatible fallback.
Current builds validate the installed top-level bundle even when the version
stamp matches, refresh incompatible assets, and refuse stale smart-orchestrator
resolution when no compatible fallback exists.

## Symptoms

Common symptoms of a stale bundle:

```text
parse-decomposition failed
```

```text
orch_helper.py not found
```

```text
ModuleNotFoundError: No module named ...
```

```text
resolve-bundle-asset helper-path
```

The last string is only a stale smart-orchestrator symptom when it appears
inside the old monolithic recipe execution path. `helper-path` remains a valid
asset name and correctly resolves to
`amplifier-bundle/bin/multitask-orchestrator.sh`.

## Repair with install

Run install from the current binary:

```sh
amplihack install
```

The installer validates local sources first. If `AMPLIHACK_HOME` contains an
incompatible `amplifier-bundle/`, install skips it and tries the next source.

Expected output when an incompatible local bundle is found:

```text
⚠️  Skipping incompatible framework bundle from AMPLIHACK_HOME: /home/alice/.amplihack: recipes/smart-orchestrator.yaml is stale
📥 Fetching the framework bundle from release tag v1.2.99 because no local source matches this binary.
```

A compatible source with a different version isn't skipped. Install refuses it,
as the next section describes. The full rules are in the
[framework source selection reference](../reference/framework-source-selection.md).

## If install refuses a source

Install refuses when `AMPLIHACK_HOME` or the checkout containing your current
directory has a different version from the binary:

```text
error: refusing to stage the framework bundle from /home/alice/src/amplihack-rs (AMPLIHACK_HOME):
  source version: v1.2.0
  binary version: v1.2.99
Run `amplihack install --local /home/alice/src/amplihack-rs` to stage this tree anyway.
```

Nothing has changed at this point.

A recent checkout can be refused too. `main` declares version `1.2.0`, so a
release binary refuses every `main` checkout, however current. Being up to date
with `main` doesn't help, and pulling won't change the result.

Choose one of these:

| You want | Run |
|---|---|
| The bundle from this binary's release | `amplihack install --force-refresh` |
| The bundle from this binary's release, from a same-version worktree | `AMPLIHACK_HOME=/home/alice/src/amplihack-v1.2.99 amplihack install` |
| The tree install refused, on purpose | `amplihack install --local /home/alice/src/amplihack-rs` |

`--force-refresh` ignores the current directory and fetches the release tag
unless `AMPLIHACK_HOME` names a same-version tree. Dev and snapshot builds can't
fetch a tag, so use `--local` with those.

## If the tag fetch fails

```text
error: could not fetch the framework bundle from release tag v1.2.99: network request timed out
The installed bundle and version stamp were not changed.
Run `amplihack install --local <PATH>` to install from a checkout.
```

Retry when the network is back. If a local clone already has the tag, you can
work offline by making a worktree of it and pointing install at that:

```sh
git -C ~/src/amplihack-rs worktree add ~/src/amplihack-v1.2.99 v1.2.99
AMPLIHACK_HOME=~/src/amplihack-v1.2.99 amplihack install
```

A same-version `AMPLIHACK_HOME` is used without any network access. With
`amplihack update`, this applies only to the post-update install, because the
binary download still needs the network.

## If the manifest check fails

```text
error: the staged bundle does not match its source /home/alice/src/amplihack-rs (1 missing, 0 extra, 1 changed):
  missing: recipes/default-workflow.yaml
  changed: recipes/autodrive-merge-round.yaml
```

The previous bundle is unchanged and the stamp hasn't advanced. A difference
usually means something changed the source or the staging directory during the
copy, such as a build, a `git checkout`, or a second `amplihack install` running
at the same time. Wait for those to finish and run the install again.

Binaries and `~/.amplihack/.claude` may already be new at this point. Self-heal
retries at the next launch.

## Repair after update

Normally, no manual step is required:

```sh
amplihack update
```

After replacing the binary, update runs:

```sh
amplihack install --force-refresh
```

The hidden `--force-refresh` flag stages the bundle from a same-version
`AMPLIHACK_HOME` if there is one, and otherwise fetches the tag of the binary's
release. It validates the source, stages it, then validates the staged copy. It
doesn't use the current directory, so `update` gives the same bundle wherever
you run it.

If the post-update install fails, `amplihack update` exits non-zero, but the
binary update has already completed. Run `amplihack install` to retry the asset
repair.

## Automatic startup repair

Current binaries also check the installed bundle during normal startup. The
no-op condition is now:

```text
.installed-version matches the binary AND ~/.amplihack/amplifier-bundle is compatible
```

If either side fails, self-heal re-runs the install path and emits the normal
`framework assets re-staged` notice. If it can't fetch the release tag and the
installed bundle is compatible, it prints a warning and runs your command with
the installed bundle, and the next launch retries. See
[Self-heal failure modes](../features/self-heal-asset-restage.md#failure-mode).
Setting `AMPLIHACK_SKIP_AUTO_INSTALL`
still skips repair, but the diagnostic includes the incompatible bundle status
instead of silently trusting stale assets.

## Repair from a local checkout

Use `--local` when you intentionally want to install from a checkout:

```sh
cd /path/to/amplihack-rs
cargo build --release
./target/release/amplihack install --local .
```

The local checkout must contain a compatible `amplifier-bundle/`. Stale or
incomplete smart-orchestrator assets make install fail with a compatibility
error, whatever the checkout's version.

`--local` is the only way to stage a tree whose version differs from the
binary's. Install prints both versions and a warning, then stages it. The stamp
records the binary's version, so self-heal doesn't undo your choice.

## Verify the repaired smart-orchestrator

Confirm the staged parent recipe delegates to the four companion recipes:

```sh
grep -E 'recipe: "(smart-classify-route|smart-execute-routing|smart-reflect-loop|smart-validate-summarize)"' \
  ~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
```

Expected output includes all four recipes:

```text
recipe: "smart-classify-route"
recipe: "smart-execute-routing"
recipe: "smart-reflect-loop"
recipe: "smart-validate-summarize"
```

Confirm the companion recipes are present:

```sh
for recipe in smart-classify-route smart-execute-routing smart-reflect-loop smart-validate-summarize; do
  test -f "$HOME/.amplihack/amplifier-bundle/recipes/${recipe}.yaml" \
    && echo "ok: ${recipe}"
done
```

Confirm the resolved recipe contains none of the legacy markers:

```sh
AMPGREP='orch_helper\.py|importlib|parse-decomposition|resolve-bundle-asset helper-path'
amplihack recipe show smart-orchestrator | grep -E "$AMPGREP" && exit 1 || echo "ok: no stale markers"
```

Confirm `helper-path` still resolves to the multitask wrapper:

```sh
amplihack resolve-bundle-asset helper-path
```

Expected suffix:

```text
amplifier-bundle/bin/multitask-orchestrator.sh
```

Confirm nothing in the bundle is writable by group or others:

```sh
find ~/.amplihack/amplifier-bundle -perm /022
```

A clean bundle prints nothing. Staging sets these modes itself, so if this
command prints anything, something changed the files after staging. Run
`amplihack install --force-refresh` to replace them.

## Compare the bundle with its release tag

Install compares the staged bundle with its source, not with the tag. To check
a same-version source that may contain later changes, compare against the tag
yourself:

```sh
tag="v$(amplihack --version | awk '{print $2}')"
scratch="$(mktemp -d)"
git -C ~/src/amplihack-rs archive "$tag" amplifier-bundle | tar -x -C "$scratch"
diff -rq --no-dereference "$scratch/amplifier-bundle" ~/.amplihack/amplifier-bundle
rm -rf "$scratch"
```

Each line of output is a difference. Symlinks in the tag appear as `Only in`
lines because staging doesn't copy them, so a matching bundle prints only those.

## Verify by running smart-orchestrator

Run a minimal Q&A task:

```sh
amplihack recipe run smart-orchestrator \
  -c task_description="What is 2+2?" \
  -c repo_path=.
```

The recipe should enter the composable flow:

```text
smart-classify-route
smart-execute-routing
smart-reflect-loop
smart-validate-summarize
```

It should not attempt to import Python orchestration helpers or reference
`orch_helper.py`.

## If install still fails

Capture the compatibility error and the resolved paths:

```sh
amplihack --version
which -a amplihack
echo "AMPLIHACK_HOME=${AMPLIHACK_HOME:-$HOME/.amplihack}"
amplihack install 2>&1 | tee ./amplihack-install.log
```

Do not repair this by copying `orch_helper.py` into the bundle or remapping
`helper-path` to a Python file. Those actions recreate the stale behavior that
the installer is designed to reject.

## See also

- [Framework source selection reference](../reference/framework-source-selection.md)
- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
- [Stale smart-orchestrator asset bugfix note](../bugfixes/stale-smart-orchestrator-assets.md)
- [Install completeness verification](../reference/install-completeness.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [resolve-bundle-asset command reference](../reference/resolve-bundle-asset-command.md)
