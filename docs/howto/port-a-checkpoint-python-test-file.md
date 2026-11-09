---
title: Port a checkpoint Python test file to Rust
type: howto
issues: [1554]
decisions: [17.126, 17.138, 17.146]
---

# Port a checkpoint Python test file to Rust

This guide ports one of the checkpoint Python test files under
`amplifier-bundle/recipes/tests/` to Rust, maps its rows in
`inventory_map.toml`, and deletes the file in the same commit (decision
17.146). Port the files one at a time, in the
[port order](../reference/workflow-checkpoint-inventory-map.md#port-order).

The examples use the first file, `test-checkpoint-conservative-boundaries.py`.
For the map's schema and guards, see the
[inventory map reference](../reference/workflow-checkpoint-inventory-map.md).

## Contents

- [Before you start](#before-you-start)
- [1. List the file's rows](#1-list-the-files-rows)
- [2. Write the Rust module](#2-write-the-rust-module)
- [3. Map the rows](#3-map-the-rows)
- [4. Delete the file and any helper it was the last user of](#4-delete-the-file-and-any-helper-it-was-the-last-user-of)
- [5. Prove the targets run](#5-prove-the-targets-run)
- [6. Commit and push](#6-commit-and-push)
- [When the port behaves differently from 918c](#when-the-port-behaves-differently-from-918c)

## Before you start

- Work in the feature worktree for PR #2020, never in the main checkout.
- Run `df -h /` and make sure the root disk has room for a full build.
- Make sure the previous commit is green and the file is the next one in the
  port order. Never start a file you cannot finish in this pass.
- Export `RUST_TEST_THREADS=8` for every cargo command.

## 1. List the file's rows

Set the file once, then list its pending rows:

```sh
export RUST_TEST_THREADS=8
F=amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py
MAP=crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml
grep -B1 "pending_file = \"$F\"" "$MAP" | grep '^ref_id'
```

For this file the result is four `[[python_test]]` rows, for the methods of
`ConservativeBoundaryTests`:

```text
test_assume_unchanged_and_skip_worktree_are_not_authority
test_duplicate_strict_paths_refuse_without_index_write
test_internal_symlink_parent_also_requires_external_preparation
test_same_bytes_replacement_record_cannot_replace_file_identity
```

Take each method's `assertions_python` from the pinned inventory, never from
the `.py` source:

```sh
INV=crates/amplihack-cli/tests/fixtures/workflow_checkpoint
cat "$INV/inventory.json.part-1" "$INV/inventory.json.part-2" \
  | jq --arg f "$F" -r '.python_assets[] | select(.path == $f)
      | .tests[] | "\(.id)\t\(.assertions | length)"'
```

## 2. Write the Rust module

Create `crates/amplihack-cli/tests/checkpoint_port_<slug>.rs`, here
`checkpoint_port_conservative_boundaries.rs`, and register it in
`crates/amplihack-cli/tests/main.rs`:

```rust
#[path = "checkpoint_port_conservative_boundaries.rs"]
mod checkpoint_port_conservative_boundaries;
```

Rules for the module:

- Port the behaviour, not only the names. Every Python assertion becomes a
  Rust check, so the Rust count is at least the Python count. Keep every
  negative case and every mutant-sensitivity check.
- Name each test after its Python method without the `test_` prefix. A
  different name is allowed; the real test run proves it executes.
- Build the repository with the
  [Rust Git fixture](../reference/workflow-checkpoint-inventory-map.md#the-rust-git-fixture)
  (`GitFixture`). Add a fixture helper only when this file needs it.
- Run every Git command through `amplihack_git::command_in`. Never use
  `inherited_command`, never format a path into a `bash -c` script, and never
  call `python3`. A grep guard refuses all three.
- Use no timers. Wait for exit statuses, readiness files or joins.
- Build byte-exact inputs at run time as byte literals in the test's
  temporary directory. Do not commit fixture files whose exact bytes matter,
  because pre-commit fixers can rewrite them.
- Add no `#[ignore]`, no `cfg` gate and no early `return;`.

### Example: a same-bytes identity replacement

`test_same_bytes_replacement_record_cannot_replace_file_identity` used a
Python hook. The port writes it as a `/bin/sh` hook, which keeps the bytes of
`authority.json` but gives it a new inode:

```rust
fx.hook(
    "pre-commit",
    r#"find "$PRIVATE" -type f -name authority.json | while IFS= read -r f; do
  cp -p "$f" "$f.next" && mv -f "$f.next" "$f"
done
"#,
);
```

`find` runs without `-L`, so the hook never follows a symlink, and the copy is
a sibling file, so it stays on the same device. Before checking the refusal,
the test asserts that:

- `PRIVATE` is under the fixture root;
- at least one file was replaced;
- the `(dev, ino)` pair changed while the bytes are equal;
- the run is refused.

A negative control runs the same flow with a hook that does nothing, and
asserts that it passes.

## 3. Map the rows

Replace each pending row with a mapped row. Keep `ref_id`, remove
`pending_file`, and add the target and the counts:

```toml
# [PLANNED] assertions_rust = 6 is the planned count; set it from the body
# of the test once it is written.
[[python_test]]
ref_id = "amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py::ConservativeBoundaryTests.test_same_bytes_replacement_record_cannot_replace_file_identity"
rust_test = "crates/amplihack-cli/tests/checkpoint_port_conservative_boundaries.rs::same_bytes_replacement_record_cannot_replace_file_identity"
assertions_python = 3
assertions_rust = 6
```

Set `assertions_python` to the `jq` result from step 1, and `assertions_rust`
to the checks the Rust body makes. Here the Python method made 3 checks;
the port makes 6: the four checks listed in step 2, and the negative
control's exit status and unchanged snapshot.
Map the file's procedural rows the same way, to a `#[test]` or to a corpus
case with a matching `inventory_ref`.

Map a site row now only if this file is the site's pending file, or if a Rust
test that is not `#[ignore]`d already covers the whole site.

## 4. Delete the file and any helper it was the last user of

```sh
git rm "$F"
```

Then list the remaining Python users of each of the five helpers:

```sh
for h in private_adapter audit_receipts canonical_staging_publication \
         canonical_staging_checkpoint_audit test-canonical-staging-harness; do
  printf '%s: ' "$h"
  git grep -l -F -e "$h" -- '*.py' | grep -v "/$h.py$" | tr '\n' ' '
  echo
done
```

A helper is deleted in the same commit as its last Python user. Count a user
only if it is still expected: a test file with pending rows, or a helper that
is itself still used. `test-canonical-staging-harness.py` uses
`canonical_staging_publication.py`, so the harness keeps that helper only
while the harness is still in use. Rows pending with a helper never keep it.
Every helper's last user is `test-checkpoint-scope.py`, so for files 1 to 13
the loop still shows a user for every helper and nothing more is deleted.

In commit 14, after `test-checkpoint-scope.py`:

- port the helpers' own rows into the modules named in the
  [port order](../reference/workflow-checkpoint-inventory-map.md#port-order)
  (`checkpoint_port_canonical_staging_harness.rs`,
  `checkpoint_port_checkpoint_audit.rs`, `checkpoint_port_audit_receipts.rs`),
  and map them;
- `git rm` all five helpers;
- update the Rust tests that read them:
  - `crates/amplihack-cli/tests/workflow_checkpoint_static_guards.rs`
  - `crates/amplihack-cli/tests/workflow_checkpoint_legacy_guards.rs`
  - `crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs`
  - `crates/amplihack-workflows/tests/canonical_staging_portability_test.rs`
  - `crates/amplihack-workflows/tests/support/canonical_staging.rs`
- remove their entries from `coverage.json` and `maintenance_paths.json`.

If a helper is still tracked after its last user is gone, the validator
refuses it with `UnaccountedPython`, even while rows are pending with it.

The guards need no edit. They work out the expected tracked set from the map,
so a deleted file with no pending rows is expected to be gone.

## 5. Prove the targets run

Run the gate:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --locked
```

Then prove every new target is listed and passes. Keep the proof outside the
repository:

```sh
P=~/.amplihack/work/pr2020-port/conservative-boundaries
mkdir -p "$P"
cargo test --locked -p amplihack-cli --test cli_integration -- --list > "$P/list.txt" 2>&1
cargo test --locked -p amplihack-cli --test cli_integration checkpoint_port_conservative_boundaries > "$P/run.txt" 2>&1
grep -c ': test$' "$P/list.txt"
grep 'checkpoint_port_conservative_boundaries::' "$P/run.txt"
```

Every mapped name must appear in `list.txt` and as `... ok` in `run.txt`. The
proof files hold test names and statuses only.

## 6. Commit and push

Commit as Ryan Sweet with every hook. Never use `--no-verify`. The
pre-commit hook can wait a long time for a build slot, so run the commit
detached and wait for it:

```sh
LOG=~/.amplihack/work/pr2020-port/conservative-boundaries
setsid -w nohup git commit \
  -m "test(workflows): port test-checkpoint-conservative-boundaries.py to Rust and delete it (#1554, P-01)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" \
  > "$LOG/commit.log" 2>&1 < /dev/null &
pid=$!
wait $pid; echo "commit exit $?"
```

Push in the background with the task temporary directory, because the
pre-push hook runs the full workspace suite. Check the exit status before
starting the next file:

```sh
TMPDIR=/d0/ryan/amplihack-test-tmp/push-2020/tmp \
  setsid -w nohup git push > "$LOG/push.log" 2>&1 < /dev/null &
pid=$!
wait $pid; echo "push exit $?"
```

If the push fails only on tests already listed in #2019 or #2099, record
their names, do not retry, and carry on with the next file. Never force-push
and never merge.

## When the port behaves differently from 918c

If a ported test shows that the Rust engine behaves differently from the
918c reference:

1. Do not commit the port. Keep the tree at the last green commit. Set the
   work aside with a WIP commit on a side branch, or with a tagged stash
   applied by its SHA; never use a bare `git stash`.
2. Do not change the engine.
3. Send the owner the input, 918c's output and the port's output.
4. Continue with the next file only if it does not depend on that case.

The disposition allowlist stays at its eight rows. A difference is never
mapped to a new disposition or left pending after its file is deleted.

## Related

- [Workflow checkpoint inventory map](../reference/workflow-checkpoint-inventory-map.md) - schema, pending rows, the guards and the port order
- [`amplihack internal workflow-checkpoint` reference](../reference/workflow-checkpoint-cli.md) - the actions the ported tests exercise
- [Validate the no-Python rule](validate-no-python.md) - what `scripts/check-no-python-assets.sh` checks
