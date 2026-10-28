---
title: Keep Tests out of the Real Profile Root
type: howto
updated: 2026-10-27
related: issue #1699, issue #1690, issue #1661
---

# Keep Tests out of the Real Profile Root

`amplihack install` refreshes the hooks in every profile's `settings.json`
(issue #1661). Before #1699, an install or test run with a temporary `HOME`
could reach the real profile root, for example through an inherited
`AMPLIHACK_CLAUDE_PROFILE_ROOT`, and write a hook path such as
`/tmp/.tmpcZSqcm/.local/bin/amplihack-hooks` into it. Once the temp
directory was gone, every hook in every real session failed with
`/bin/sh: 1: …/amplihack-hooks: not found`.

Two checks now prevent this: the refresh refuses such a hook path, and CI
fails a test run that changes the real profile root.

## The refusal rule

Before it reads any profile, the refresh checks the hook binary path once
against the profile root (`hook_path_refusal` in
`crates/amplihack-cli/src/commands/profile/settings_refresh.rs`). The first
matching row wins:

| # | Refused when | Reason |
|---|--------------|--------|
| 1 | `HOME` is unset, relative or `/`. | `cannot determine HOME; refusing to write hook paths` |
| 2 | The hook binary path is relative. | `hook binary {bin} is not an absolute path` |
| 3 | The hook binary is missing, a directory or a dangling symlink. | `hook binary {bin} does not exist or is not a file` |
| 4a | The root is outside `HOME` and the binary is under a temp dir. | `hook binary {bin} is under temp dir {tmp} but profile root {root} is outside HOME {home}` |
| 4b | The root is outside `HOME` and the binary is under `HOME`. | `hook binary {bin} is under HOME {home} but profile root {root} is outside HOME` |

Temp dirs are `std::env::temp_dir()`, `$TMPDIR` and `/tmp`. Every path is
compared both as written and with symlinks resolved, by whole components,
and the stricter answer wins. So a root inside `HOME` that is a symlink to a
directory outside `HOME` counts as outside `HOME`.

A refusal never fails the install. Nothing under the root is opened or
written, and each profile gets a warning such as:

```
  ⚠️  Profile work: settings.json not refreshed (hook binary /tmp/.tmpQ3x9aB/home/.local/bin/amplihack-hooks is under temp dir /tmp but profile root /home/dev/.claude-profiles is outside HOME /tmp/.tmpQ3x9aB/home)
```

**If your real root is outside `HOME`** (row 4b, for example
`/data/claude-profiles` with `~/.local/bin/amplihack-hooks`), the rule cannot
tell it apart from a test with a temp `HOME`. Either move the root under
`HOME` (`amplihack profile root ~/.amplihack/claude-profiles`, then
`amplihack install`), or install the hooks binary outside `HOME`, for example
under `/usr/local/bin`.

## What the guard checks

`scripts/check-real-profile-root.sh` records the real profile roots before a
test run and reports any change after it. It never writes into a root. It
watches, after resolving symlinks and removing duplicates:

1. `$AMPLIHACK_CLAUDE_PROFILE_ROOT`, if set;
2. the first line of `~/.amplihack/profile-root`, if it is an absolute path;
3. `~/.claude-profiles`;
4. `~/.amplihack/claude-profiles`.

Every entry is recorded without following symlinks: files by SHA-256 and
mtime, symlinks by target and mtime, directories by mtime. The snapshot file
is created mode 0600, because it lists every profile path and hashes
`.credentials.json`.

In CI, the `test` job takes the snapshot into `$RUNNER_TEMP` before
`cargo nextest run --workspace --locked` and compares with `if: always()`
afterwards. The snapshot is never uploaded. The `check` job runs the
self-test, `bash tests/issue_1699_real_profile_root_guard.sh`.

## Run the guard locally

```bash
scripts/check-real-profile-root.sh snapshot ~/.amplihack/work/real-root.snap
cargo nextest run -p amplihack-cli
scripts/check-real-profile-root.sh compare ~/.amplihack/work/real-root.snap
echo "exit: $?"
```

| Exit | Meaning |
|------|---------|
| 0 | No difference. |
| 1 | At least one difference. |
| 2 | The scan was impossible: a usage error, a missing snapshot, a relative `HOME` or root, a root that resolves to `/` or `HOME`, a path that cannot be resolved, a failed walk, or no SHA-256 tool. |

A running rotator or Claude session changes files under the root too
(`.auto-switch.d/`, usage readings, session files). The guard has no exclude
list, so a real leak cannot hide behind one. Stop the rotator with
`amplihack profile rotate stop` for a clean local result.

## Read its output

`compare` prints one line per difference, with paths shell-quoted and
control characters removed. It never prints contents or hashes, because CI
logs are public.

| Line | Meaning |
|------|---------|
| `changed:file <path>` | Contents or mtime changed. |
| `changed:link <path>` | Target or mtime changed. |
| `changed:dir <path>` | An entry was added, removed or renamed in it. |
| `added: <path>` / `removed: <path>` | The entry appeared or disappeared. |
| `created during run: <root>` | The root did not exist at snapshot time. |
| `note: <root> absent` | The root existed at neither time. Not a failure. |

To find the test responsible, look for tests that set `HOME` but not the
profile root, and for tests that set `AMPLIHACK_TEST_PROFILE_REFRESH`:

```bash
grep -rlE 'set_var\("HOME"|\.env\("HOME"' --include='*.rs' crates/amplihack-cli/src \
  | xargs -r grep -LE 'AMPLIHACK_CLAUDE_PROFILE_ROOT|ROOT_ENV|ROOT_VAR'
grep -rn AMPLIHACK_TEST_PROFILE_REFRESH crates/
```

The fix is to point the root into the test's temp directory in the same
`env_lock()` critical section that sets `HOME`, and restore both with
`crate::test_support::EnvGuard`. Tests of the refresh itself should call
`refresh_in_checked` with an injected `HOME` and temp dir list instead.

## Limits

- A temp `HOME` that is a parent of the real root (`HOME=/home`) passes the
  rule. Test homes are fresh `tempfile` directories, so this does not happen
  in practice.
- The rule checks the hook path, not the binary's contents. It stops
  dangling paths; it is not a defence against an attacker.
- The guard watches only the four roots above.
- A file that cannot be hashed (unreadable, or gone before hashing) is
  recorded with an empty hash plus its mtime. A content change without an
  mtime change is missed for such a file. Both scan paths behave this way.
- A file swapped for a symlink between the walk and the hashing has its
  target hashed. The guard only reports and never prints the hash, so this
  race is accepted.
- On Windows the refresh skips every profile, so neither check applies.

## Related

- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The #1690 guard this one is modelled on
- [Claude Profiles How-To](./claude-profiles.md) - Profile roots and how amplihack resolves them
- [`amplihack profile` reference](../reference/profile-command.md) - The install refresh rules
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
