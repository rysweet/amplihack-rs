---
title: Keep the Pre-commit Hooks Portable to macOS
type: howto
updated: 2026-11-10
related: PR #1530, issue #1423, issue #1569, issue #2286
---

# Keep the Pre-commit Hooks Portable to macOS

The hooks in `.pre-commit-config.yaml` run on contributors' Macs as well as on
Linux. CI's macOS jobs only build release binaries, so CI does not run the
hooks on macOS. If a change breaks them there, the first sign is a failed
`git commit` on a Mac. This guide lists the rules that keep the hooks working.

## Contents

- [Shell hooks: use POSIX sed and bash 3.2](#shell-hooks-use-posix-sed-and-bash-32)
- [Clippy: allow Linux-only items on other platforms](#clippy-allow-linux-only-items-on-other-platforms)
- [Test files: gate a file that imports a Linux-only item](#test-files-gate-a-file-that-imports-a-linux-only-item)
- [libc FFI: use the libc type aliases](#libc-ffi-use-the-libc-type-aliases)
- [Check a change from a Linux machine](#check-a-change-from-a-linux-machine)

## Shell hooks: use POSIX sed and bash 3.2

macOS ships BSD sed and `/bin/bash` 3.2. Scripts under `scripts/` that run as
hooks must work with both.

**Do not use `\+` in a sed basic regular expression.** GNU sed reads `\+` as
"one or more". BSD sed reads it as a literal `+`, so the expression matches
nothing and the variable ends up empty. Write the item twice with a `*`
instead:

| GNU-only    | POSIX equivalent  |
| ----------- | ----------------- |
| `[0-9]\+`   | `[0-9][0-9]*`     |
| `[^"]\+`    | `[^"][^"]*`       |

`scripts/check-brick-budget.sh` reads the limit from the Rust test this way:

```bash
# POSIX BRE only (no `\+`): BSD sed on macOS reads `\+` as a literal plus.
limit="$(sed -n 's/^const BRICK_LIMIT: usize = \([0-9][0-9]*\);.*/\1/p' "$TEST_FILE" | head -1)"
```

**Do not use `mapfile` or `readarray`.** They are bash 4 builtins. Read lines
with a `while read` loop, as `check-brick-budget.sh` does for the recipe names:

```bash
# while-read, not mapfile: macOS /bin/bash is 3.2 (issue #1423).
recipes=(); while IFS= read -r _ml; do recipes+=("$_ml"); done < <(
  awk '/^const PHASE_RECIPES: &\[&str\] = &\[/{f=1; next} f && /^\];/{exit} f' "$TEST_FILE" \
    | sed -n 's/.*"\([^"][^"]*\)".*/\1/p'
)
```

**Fail closed when parsing comes back empty.** A hook that parses nothing
must exit non-zero, not pass. `check-brick-budget.sh` exits 1 when the test
file is missing, when `BRICK_LIMIT` is empty, or when it finds no
`PHASE_RECIPES`. This guard turns a portability bug into a visible error
rather than a silent pass.

## Clippy: allow Linux-only items on other platforms

The `cargo-clippy` hook runs `cargo clippy --locked --all-targets -- -D warnings`.
If a helper or `use` is only called from `#[cfg(target_os = "linux")]` code, it
is dead code on macOS, and `-D warnings` turns that into an error.

Put the allow on that one item, limited to non-Linux targets, and add a comment
saying why:

```rust
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // /proc/<pid>/exe is Linux only
fn deleted_target_for_tool(raw: &str, tool: &str) -> Option<std::path::PathBuf> {
```

```rust
#[cfg(test)]
mod tests {
    #[cfg_attr(not(target_os = "linux"), allow(unused_imports))] // the tests are Linux only
    use super::*;
```

Rules:

- One item per attribute. Never `#![allow(...)]` on a module or crate.
- Only `dead_code` or `unused_imports`. Never `allow(warnings)` or
  `allow(clippy::all)`.
- Keep the `not(target_os = "linux")` condition. Linux builds still get full
  `-D warnings` coverage, so the item is still checked where it is used.

If the item is only ever used on Linux and nothing else needs it, gating the
item itself with `#[cfg(target_os = "linux")]` is also fine.

### Test helpers used only by a Linux-gated module

`--all-targets` also lints the integration tests under `crates/*/tests/`. A
test file is often `#![cfg(unix)]` at the top, with its Linux-only cases in a
`#[cfg(target_os = "linux")] mod`. Shared fixture code sits outside that
module, so on macOS any helper that only the Linux module calls is dead.

`crates/amplihack-cli/tests/issue_1569_auto_switch.rs` is an example. Its
`mod swap` is Linux only, and six items are used from nowhere else:
`ABORT_ENV`, `Holder`, `Fixture::hold`, `Fixture::hosts`, `Fixture::oauth_uuid`
and `Fixture::parked`. Each item has its own allow, placed after the doc
comment and directly above the item:

```rust
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
struct Holder(Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
```

```rust
    /// A process standing in for a Claude session running in `name`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // used by the Linux-only swap tests
    fn hold(&self, name: &str) -> Holder {
```

Leave the `impl Drop` alone. It is not reported as dead code, and it must stay
active so that every test child process is killed and reaped.

To find which helpers need the attribute, check each `fn`, `struct` and
`const` defined outside the Linux-gated regions and count its uses outside
them. An item with no such use needs the allow. An item that carries the allow
counts as used, so helpers it calls need no attribute of their own. An item
with a live use outside them does not, and adding the allow there would hide a
later regression. Do the same for each `use` line; if every use of an import is
inside the gated module, mark it with `allow(unused_imports)`.

When a marked helper later gains a caller outside the gated module, remove its
allow. The compiler does not warn about an allow that no longer suppresses
anything, so a stale one stays in place until someone deletes it.

## Test files: gate a file that imports a Linux-only item

The allows in the previous section deal with warnings. A `use` of an item that
is itself gated with `#[cfg(target_os = "linux")]` is a different case: on
macOS the item does not exist, so the import fails to compile with `E0432`
(unresolved import). No `allow` attribute can suppress that, and it stops
`cargo test --workspace --no-run` and the pre-push hook on every Mac.

The integration tests in `crates/amplihack-cli/tests/` are modules of one test
binary, `cli_integration`, declared in `tests/main.rs`. A module that imports
a Linux-only helper from a sibling module must be gated the same way as the
helper.

`issue_1954_install_leaves_live_runs_alone.rs` defines `proc_link`, which reads
`/proc/<pid>/<entry>`, for Linux only:

```rust
#[cfg(target_os = "linux")]
pub(crate) fn proc_link(pid: &str, entry: &str) -> String {
```

`issue_1954_first_install_restager.rs` imports it, and its only test already
needs Linux. The whole file is therefore gated with an inner attribute:

```rust
//! The stub also lists `~/.amplihack` just before it starts the new
//! binary. That listing must equal the one the test takes before it starts
//! the stub: the old binary's part of the hand-off writes nothing there.

// Linux only: the test reads /proc through proc_link.
#![cfg(target_os = "linux")]

use crate::issue_1954_install_leaves_live_runs_alone::{
    Host, RUN_SCRIPT, plant_legacy_layout, proc_link, published_release, text,
};
```

Rules:

- Use the same condition as the item you import. `proc_link` is gated to
  `target_os = "linux"`, so `#![cfg(unix)]` would still fail on macOS.
- Place `#![cfg(...)]` after the `//!` doc block and before the first `use`.
  A plain `//` comment may sit just above it.
  An inner attribute after an item does not compile (`E0753`).
  `issue_1570_run_output_stdin.rs` uses the same layout.
- Gate the file, not the `mod` line in `tests/main.rs`. No `mod` line there
  has a `cfg`, and a file-level gate keeps the condition next to the code
  that needs it.
- Gate the whole file only when every test in it needs Linux. If some tests
  run on macOS, gate the import and its callers instead, for example by moving
  the Linux-only tests into a `#[cfg(target_os = "linux")] mod`.

On Linux the condition is true, so the file compiles and its tests run as
before. On macOS the module is empty and nothing in it is compiled.

To check that no other test file imports `proc_link` outside a Linux scope,
list every use and confirm that each line is inside a `target_os = "linux"`
item or file:

```bash
git grep -n proc_link crates/amplihack-cli/tests
```

With the restager file gated, the command prints seven lines. The first is the comment above the
gate:

```text
crates/amplihack-cli/tests/issue_1954_first_install_restager.rs:28:// Linux only: the test reads /proc through proc_link.
crates/amplihack-cli/tests/issue_1954_first_install_restager.rs:32:    Host, RUN_SCRIPT, plant_legacy_layout, proc_link, published_release, text,
crates/amplihack-cli/tests/issue_1954_first_install_restager.rs:166:    let cwd_during = proc_link(&a_pid, "cwd");
crates/amplihack-cli/tests/issue_1954_install_leaves_live_runs_alone.rs:414:pub(crate) fn proc_link(pid: &str, entry: &str) -> String {
crates/amplihack-cli/tests/issue_1954_install_leaves_live_runs_alone.rs:520:    let cwd_now = proc_link(&a_pid, "cwd");
crates/amplihack-cli/tests/issue_1954_install_leaves_live_runs_alone.rs:521:    let fd_now = proc_link(&a_pid, "fd/3");
crates/amplihack-cli/tests/issue_1954_install_leaves_live_runs_alone.rs:693:    let cwd_now = proc_link(&a_pid, "cwd");
```

| Line             | Linux gate                                                   |
| ---------------- | ------------------------------------------------------------ |
| restager 28, 32, 166 | `#![cfg(target_os = "linux")]` on the whole file             |
| live-runs 414    | `#[cfg(target_os = "linux")]` on `proc_link` itself          |
| live-runs 520, 521 | `#[cfg(target_os = "linux")]` on `install_and_self_heal_leave_a_live_run_alone` |
| live-runs 693    | `#[cfg(target_os = "linux")]` on `migration_from_a_real_bundle_dir_keeps_a_live_run_alive` |

A new line in this list that is not inside one of these gates breaks the
macOS build again.

To confirm that the gate removed no tests on Linux, compare the test list
before and after the change. The names and the count must be the same:

```bash
cargo test -p amplihack-cli --test cli_integration -- --list | grep issue_1954_
```

## libc FFI: use the libc type aliases

Some `libc` constants and function arguments have different Rust types on
Linux and macOS. That causes two different failures:

| Value                        | Linux      | macOS     | Failure on macOS                          | Write                            |
| ---------------------------- | ---------- | --------- | ----------------------------------------- | -------------------------------- |
| `libc::F_WRLCK`              | `c_int`    | `c_short` | clippy `unnecessary_cast` under `-D warnings` | `libc::F_WRLCK as libc::c_short` |
| `openpty` `termp` and `winp` | `*const _` | `*mut _`  | compile error: mismatched types           | `std::ptr::null_mut()`           |

`F_WRLCK as i16` compiles on both platforms. On macOS, though, `F_WRLCK` is
already `c_short` (`i16`), so the cast does nothing and clippy's
`unnecessary_cast` lint rejects it under `-D warnings`. Clippy does not flag a
cast to the alias `libc::c_short`, and on Linux that cast is still needed to
fill the `l_type` field of `flock`.

The `openpty` arguments are a real compile error: passing `std::ptr::null()`
where macOS expects `*mut _` does not type-check. `*mut T` coerces to
`*const T`, so `null_mut()` fits both signatures. A null `termp` or `winp`
tells `openpty` to use its defaults.

## Check a change from a Linux machine

You can check the sed rule without a Mac. With `--posix`, GNU sed treats `\+`
the way BSD sed does, so an expression that uses it matches nothing and
prints nothing. Run each expression in the hook with and without `--posix`
and compare the output.

The `BRICK_LIMIT` expression:

```bash
f=tests/integration/default_workflow_decomposition_test.rs
diff <(sed -n 's/^const BRICK_LIMIT: usize = \([0-9][0-9]*\);.*/\1/p' "$f") \
     <(sed --posix -n 's/^const BRICK_LIMIT: usize = \([0-9][0-9]*\);.*/\1/p' "$f") \
  && echo same
# Output: same
```

The `PHASE_RECIPES` expression, fed the same lines the hook feeds it:

```bash
f=tests/integration/default_workflow_decomposition_test.rs
names() { awk '/^const PHASE_RECIPES: &\[&str\] = &\[/{f=1; next} f && /^\];/{exit} f' "$f"; }
diff <(names | sed -n 's/.*"\([^"][^"]*\)".*/\1/p') \
     <(names | sed --posix -n 's/.*"\([^"][^"]*\)".*/\1/p') \
  && echo same
# Output: same
```

`same` only means something if `--posix` would have caught a `\+`. To see
that it does, run the old pattern: `sed --posix -n 's/.*"\([^"]\+\)".*/\1/p' "$f"`
prints nothing.

Then run the hook itself:

```bash
scripts/check-brick-budget.sh && echo ok
# Output: ok
```

Finally, prove the hook still fails when it should. Run it against a copy of
the inputs with `BRICK_LIMIT` set to 1, so every recipe is over budget. The
script falls back to the current directory when it is not inside a git
repository, so the copy goes in a temporary directory:

```bash
repo="$(git rev-parse --show-toplevel)"
f=tests/integration/default_workflow_decomposition_test.rs
t="$(mktemp -d)"
mkdir -p "$t/tests/integration" "$t/amplifier-bundle"
cp -r "$repo/amplifier-bundle/recipes" "$t/amplifier-bundle/"
sed 's/^const BRICK_LIMIT: usize = [0-9][0-9]*;/const BRICK_LIMIT: usize = 1;/' "$repo/$f" > "$t/$f"
(cd "$t" && "$repo/scripts/check-brick-budget.sh" >/dev/null 2>&1; echo "exit $?")
rm -rf "$t"
# Output: exit 1
```

If this prints `exit 0`, the hook parsed nothing and is passing without
checking anything. Deleting the `const BRICK_LIMIT` line from the copy instead
should also give `exit 1`, with `ERROR: could not read BRICK_LIMIT`.

The integration test `brick_budget_hook_portability` runs these checks for
you under `sed --posix`:

```bash
cargo nextest run -p amplihack --test integration brick_budget_hook_portability
```

On Linux it needs GNU sed. If the host sed has no `--posix` (BusyBox, for
example), the test fails rather than quietly running the hook under a sed that
accepts `\+`.

For the Rust rules, you can try clippy against an Apple target:

```bash
rustup target add aarch64-apple-darwin
cargo clippy --locked --all-targets --target aarch64-apple-darwin -- -D warnings
```

The same check finds unresolved imports in test files. Either Apple target
will do; this example uses the Intel one:

```bash
cargo check -p amplihack-cli --tests --target x86_64-apple-darwin --locked
```

`rustup target list --installed` shows which targets the host already has.
Name the target you used when you quote the result in a PR.

Both commands often stop early because a dependency's C build script needs
the Apple SDK, which Linux hosts don't have. If one does, the macOS result is
unknown, not a pass. Say so in the PR and ask someone with a Mac to run
`pre-commit run --all-files`.

## Related

- [Develop amplihack](develop-amplihack.md) - Local development setup
- [Artifact Guard](../artifact-guard.md) - Another pre-commit hook in `.pre-commit-config.yaml`
