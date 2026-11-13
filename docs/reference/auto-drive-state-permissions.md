---
title: Auto-drive State Permissions
description: How auto-drive writes its state and evidence 0700/0600 under any umask, and when the evidence check repairs or refuses an entry, decided by the state folder's location under the auto-drive state root.
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive State Permissions

Auto-drive keeps its per-PR state and evidence in a private state folder.
Every entry there must belong to the user running the drive and must not be
open to group or other users. This page lists the rules the writers follow,
what the evidence check does with each entry it opens, and the log line it
writes when it repairs one (issue #2546).

See [Auto-drive Evidence and Outcomes](./auto-drive-evidence.md#validity-and-storage)
for what the evidence itself must contain.

## Contents

- [The rule in one line](#the-rule-in-one-line)
- [Writers](#writers)
- [What the evidence check does](#what-the-evidence-check-does)
- [Where repairs happen](#where-repairs-happen)
- [The repair log line](#the-repair-log-line)
- [Errors](#errors)
- [What stays strict](#what-stays-strict)
- [Tests](#tests)

## The rule in one line

Folders are `0700`, files are `0600`, whatever the process umask. An entry
this user owns with extra group or other bits is set back to that mode and
logged. Anything else that is wrong is refused.

## Writers

A umask can only remove bits, so a writer that asks for `0700` or `0600`
when it creates an entry gets that mode even under umask `002`.

| Writer | How the mode is set |
| --- | --- |
| `EvidenceStore::create`, `write_new`, `snapshot` (`autodrive/state.rs`) | `DirBuilder::mode(0o700)`, `openat(…, 0o600)` |
| `helper::write_new_private` (`autodrive/helper.rs`) | `OpenOptions::mode(0o600)` |
| `secure_fs::create`, `secure_fs::output` (`autodrive_completion/secure_fs.rs`) | `openat(…, 0o600)` under a `0700` folder |
| `private_state.rs`, `evaluator.rs` | explicit `0o700` / `0o600` modes, tempfiles in `0700` folders |
| `autodrive_state.sh` (`autodrive_private_dir`, `autodrive_mark_phase_done`, `autodrive_clear_phase`, `autodrive_record_crusty_loop_done`, `autodrive_record_resolved`, `autodrive_use_task_file`) | `umask 077` in the subshell that writes |
| `autodrive_completion_context.sh` (`completion_context`) | `( umask 077 && set -C && printf … > "$CTX_DIR/$1" )`; `CTX_DIR` comes from `mktemp -d` |

`set -C` in `completion_context` makes the write fail rather than follow a
symlink planted at that name.

Files written by agent steps with a plain write (for example
`std::fs::write`, or `>` in a shell without `umask 077`) can still come out
`0664` under umask `002`. The evidence check repairs those, as described
below.

## What the evidence check does

The check opens each entry with `O_NOFOLLOW`, reads its facts with `fstat`
on that open descriptor, and decides. The decision is the pure function
`private_mode::access`; `private_mode::make_private` reads the facts and
acts on the answer. Both live in `autodrive/private_mode.rs`, which the
evidence store and `secure_fs` share.

The first matching row wins:

| # | Entry | Result |
| --- | --- | --- |
| 1 | Symlink | Refuse: `evidence is a symlink` |
| 2 | Wrong type (a file where a folder is expected, or the reverse, or a special file) | Refuse: `evidence is not a regular file or state directory` |
| 3 | Owner is not this user (`uid != geteuid()`) | Refuse: `untrusted evidence ownership or permissions` |
| 4 | Other-write bit set (`mode & 0o002`) | Refuse: `untrusted evidence ownership or permissions` |
| 5 | Group bits set and group is not this user's effective group (`gid != getegid()`) | Refuse: `untrusted evidence ownership or permissions` |
| 6 | File with more than one link, when the caller requires a single link (`Kind::SingleLinkFile`) or the file has extra bits | Refuse: `evidence has unexpected hard links` |
| 7 | Extra group or other bits, where repair is not allowed (`Repair::Refuse`) | Refuse: `evidence outside the auto-drive state tree has group or other access`, with the advice to `chmod 700` (folder) or `chmod 600` (file), or to keep drive state in a folder directly under the auto-drive state root |
| 8 | Extra group or other bits | Repair to `mode & 0o700`, log, continue |
| 9 | Anything else | Private: continue |

Notes on the rows:

- Row 4: umask `002` never produces an other-write bit, so a
  world-writable entry did not come from a normal writer and is never
  trusted.
- Row 5: a group taken from a setgid folder, or any group other than this
  user's own, is refused rather than repaired.
- Row 6: a hard-linked file is never repaired, because `fchmod` would change
  its other names too. The evidence store requires a single link
  (`Kind::SingleLinkFile`); `secure_fs` does not (`Kind::File`).
- Row 7: this reason is separate from row 3 so an operator can tell a
  strict-path refusal from a foreign owner.
- Row 8: `mode & 0o700` also clears setuid, setgid and sticky bits. A folder
  at `0775` becomes `0700`; a file at `0664` becomes `0600`.

The repair is `fchmod` on the descriptor that was checked, so the entry
checked is the entry changed. Nothing is changed for a refused entry.

```rust
use crate::commands::orch::autodrive::private_mode::{access, Access, Entry, Kind, Repair};

let entry = Entry {
    symlink: false, is_dir: false, is_file: true,
    uid: 1000, gid: 1000, mode: 0o664, nlink: 1,
};
// euid 1000, egid 1000, single link required, repair allowed
assert_eq!(
    access(entry, Kind::SingleLinkFile, 1000, 1000, Repair::Allowed),
    Access::Repair(0o600)
);

// The same file owned by another user is refused, even at 0600.
let foreign = Entry { uid: 1001, mode: 0o600, ..entry };
assert!(matches!(
    access(foreign, Kind::SingleLinkFile, 1000, 1000, Repair::Allowed),
    Access::Refuse(_)
));
```

## Where repairs happen

Each caller decides once, from the state folder's location, whether repairs
are allowed, and passes that down. A folder's name never allows a repair;
only its location does.

### The auto-drive state root

Real drives keep their state in the folder that `autodrive_state_dir` in
`amplifier-bundle/tools/autodrive_state.sh` chooses: one slug folder per
repository and PR, directly under the auto-drive state root. The Rust code
resolves that root the same way as the shell's `autodrive_state_root`:

```
${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}/auto-drive
```

- An empty variable counts as unset, as with the shell's `:-`.
- `AMPLIHACK_HOME` plays no part.
- `private_mode::state_root_from(state_dir, home)` is the pure form of the
  formula; `private_mode::autodrive_state_root()` reads `AMPLIHACK_STATE_DIR`
  and `HOME` and calls it.

For example, with `HOME=/home/dev`, no `AMPLIHACK_STATE_DIR`, and a repository
whose `origin` is `https://github.com/rysweet/amplihack-rs`,
`autodrive_state_dir . 2579` returns:

```
/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579
```

That slug folder is repairable. The root itself is not.

```rust
use std::ffi::OsString;
use std::path::PathBuf;
use crate::commands::orch::autodrive::private_mode::state_root_from;

assert_eq!(
    state_root_from(None, Some(OsString::from("/home/dev"))),
    PathBuf::from("/home/dev/.amplihack/state/auto-drive")
);
assert_eq!(
    state_root_from(Some(OsString::from("/srv/state")), Some(OsString::from("/home/dev"))),
    PathBuf::from("/srv/state/auto-drive")
);
// An empty value counts as unset.
assert_eq!(
    state_root_from(Some(OsString::new()), Some(OsString::from("/home/dev"))),
    PathBuf::from("/home/dev/.amplihack/state/auto-drive")
);
// Neither set: the shell's /tmp fallback.
assert_eq!(
    state_root_from(None, None),
    PathBuf::from("/tmp/.amplihack/state/auto-drive")
);
```

### The location rule

The rule is `private_mode::repairable_root(canonical_root, created_now)`. It
holds when `created_now` is true or `private_mode::in_state_root` holds:

- **Direct children only.** `in_state_root(dir)` is true only when the
  canonical parent of `dir` equals the canonical state root. The comparison
  is by path parts, never by string prefix, so a sibling such as
  `auto-drive-x/<slug>` does not match. The root itself, and folders two or
  more levels below it, do not qualify as roots. Entries inside a repairable
  slug folder are reached through the normal walk.
- **A relative root is strict.** If `AMPLIHACK_STATE_DIR` is relative, the
  shell resolves it against its own working directory, which the Rust process
  cannot know. The rule does not apply.
- **The root must not be a symlink.** The rule reads `symlink_metadata` of
  the root path as resolved from the environment, before any
  canonicalisation. If that last part is a symlink, the rule fails, even
  when the link points to a private folder. Folders above the root are not
  checked this way.
- **The root must be private and owned by this user.** The canonical root
  must be a directory, owned by `geteuid()`, with no group or other write
  bits (`mode & 0o022 == 0`). A missing root, or one that cannot be
  canonicalised, fails the rule. `autodrive_private_dir` creates the root at
  `0700`, so real drives meet this.
- **The `/tmp` fallback is strict.** When `AMPLIHACK_STATE_DIR` and `HOME` are
  both unset or empty, the root would sit under the shared `/tmp`, and the
  rule does not apply.
- **`created_now`.** `EvidenceStore::create` also treats a folder as
  repairable when its own `mkdir` call has just created it at `0700`.
  `Scope::of` has no such case.
- **Failures fail strict.** Any error while resolving or checking the root
  gives `false`. On platforms other than unix the rule is always `false`.

The rule can only turn a refusal into a repair. The checks in `access()`
(rows 1 to 6) still run first and still have the final say, and a repair
only removes mode bits.

What this means in practice:

- A drive's slug folder under the state root, written under umask `002`, is
  repaired to `0700`, and its files to `0600`, with one log line each.
- An explicit `autodrive_state_dir` anywhere else is checked strictly, as
  before #2546, and must already be `0700`. This includes a folder named
  `autodrive-state` under a shared or temporary folder. A `0775` folder there
  is refused, unchanged, with the advice to run `chmod 700 <path>` or to keep
  drive state in a folder directly under the state root.
- The runner's `<run dir>/autodrive-state` folders are no longer repairable
  by name. They are repairable only when `EvidenceStore::create` has just
  created them; an existing one must already be `0700`.
- Repairs reach only entries at or below a repairable folder, compared part
  by part. Folders above it, and any path outside it, are checked strictly.

| Caller | Repair allowed for |
| --- | --- |
| `EvidenceStore::create` | the evidence store root, when `repairable_root` holds; the device and inode check runs first, so a root swapped for a symlink is refused before the rule is consulted |
| `EvidenceStore::read_private` | each record it reads, only in a store whose root is repairable |
| `secure_fs::check_dir(state_dir)` | the state folder, only when `Scope::of` returns `Scope::StateTree`: the path is absolute, has no `..` part, exists, equals its own canonical form, and is a direct child of the state root. A folder that does not exist is `Scope::Strict`; no folder is created to decide the scope |
| `secure_fs::read`, `output`, `create` with `Scope::StateTree(state_dir)` | the folders checked on the path (the last parent, the folder above the first `.completion-*` part and every folder below it) and the target file, when they are at or below `state_dir` |
| `secure_fs` with `Scope::Strict` (`private_state::load`, `snapshot`) | nothing |

`prepare` validates its arguments (round label, record, no `..` in the state
folder) before it calls `check_dir`, so a rejected call changes no modes.
Every `--context-from-file` and `--evaluator-context-file` path must be
absolute, contain no `..` part and lie inside the state folder; otherwise
`prepare` fails with `context file must be inside the state directory`
before reading it. The runner's `completion_context` writer already puts its
files in a `.completion-context.*` folder inside the state folder.

`Scope::of` uses the canonical path only to decide eligibility.
`Scope::StateTree` holds the raw path, and every filesystem operation still
goes through the `openat(O_NOFOLLOW)` walk.

So `$HOME`, a repository root, the state root itself, a state folder outside
the state root, or a file outside the state folder is never changed.

## The repair log line

Each repair writes exactly one line to stderr, in one of three forms. All
three contain `removed group and other access`.

When the entry had only other bits, or its group has no members besides
this user:

```
INFO: auto-drive state "/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579/round-2/qa.json" had mode 0664 (gid 1000); removed group and other access (now 0600)
```

When the entry had group bits and the group lists other members:

```
WARNING: auto-drive state "/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579/round-2/qa.json" had mode 0664 (gid 1000); group "devs" has other members; removed group and other access (now 0600)
```

When the entry had group bits and the group's members could not be read:

```
WARNING: auto-drive state "/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579/round-2/qa.json" had mode 0664 (gid 1000); group members could not be read; removed group and other access (now 0600)
```

- The path and the group name are printed with Rust's `{:?}` quoting, so a
  newline or control sequence in either cannot fake a second log line.
- The gid is the group the repair trusted (row 5), so an operator can check
  it.
- There is one line per repaired entry. An entry that is already private
  writes nothing.
- A `WARNING` means a group member could have written to the entry before
  the repair. The repair itself goes ahead in every case; a failed lookup
  never stops it or turns it into an error.

### How "other members" is decided

The group lookup runs only on the repair path, and only when the entry had
group bits (`mode & 0o070 != 0`).

- `private_mode::shared_group(members, me)` is the pure decision. It returns
  `true` when any name in `members` differs from `me`. Names are compared as
  raw bytes, so a name that is not valid UTF-8 is still compared exactly.
- On unix, `private_mode::group_sharing(euid, egid)` reads this user's name
  with `getpwuid_r` and the group's `gr_mem` list with `getgrgid_r`. The
  buffer starts at the size `sysconf` reports (16 KiB if it reports none) and
  doubles on `ERANGE` up to 1 MiB. Reaching the cap, a non-zero return or a
  missing entry gives the "could not be read" form.
- `gr_mem` does not list users whose primary group is this group. Those
  users are not detected; see [What stays strict](#what-stays-strict).

```rust
use crate::commands::orch::autodrive::private_mode::shared_group;

let dev: &[u8] = b"dev";
let ops: &[u8] = b"ops";

assert!(!shared_group(&[], dev));
assert!(!shared_group(&[dev], dev));
assert!(shared_group(&[dev, ops], dev));
assert!(shared_group(&[ops], dev));
```

## Errors

A refused entry stops the check with an error naming the reason and the
path. The path is quoted with `{:?}`, as in the log line, for example:

```
untrusted evidence ownership or permissions: "/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579/round-2/qa.json"
```

A row 7 refusal also names the state root, unresolved, and gives `chmod 700`
for a folder or `chmod 600` for a file, so an operator can
see where repairable drive state belongs:

```
evidence outside the auto-drive state tree has group or other access: "/srv/shared/autodrive-state"; make it private (chmod 700) or keep drive state in a folder directly under "/home/dev/.amplihack/state/auto-drive"
```

`secure_fs` adds the context `private state ownership, permissions or file
type are unsafe`. If the `fchmod` for a repair fails, the error has the
context `repair private state mode "<path>"`, with the path quoted the same
way.

| Error | Likely cause | What to do |
| --- | --- | --- |
| `untrusted evidence ownership or permissions` | The entry is owned by another user, is world-writable, or has a foreign group | Find out who wrote it. Do not `chmod` it to make the error go away. |
| `evidence outside the auto-drive state tree has group or other access` | A file or folder outside a repairable state folder is open to group or other users; most often an explicit state folder that is not directly under the state root, or a state root that is not `0700` | Auto-drive never changes paths outside a repairable folder. Run `chmod 700` on the folder (or `chmod 600` on the file) if it is yours, or keep drive state in a folder directly under the state root named in the message. If the state root itself has group or other write bits, run `chmod 700` on the root. |
| `context file must be inside the state directory` | A `--context-from-file` or `--evaluator-context-file` path is outside `--state-dir` or contains `..` | Write the context file inside the state folder. |
| `state root changed during open` | The state root was replaced between the check and the open | Treat as tampering. |
| `evidence has unexpected hard links` | Another name points at the same file | Remove the other link, or remove the record and let the round rewrite it |
| `evidence is a symlink` / `evidence is not a regular file or state directory` | Something replaced a state entry | Treat as tampering; remove the entry and rerun the round |

## What stays strict

- **Shell finalization check.** `autodrive_private` and
  `autodrive_untrusted_entries` in `autodrive_state.sh` do not repair. They
  move any entry with group or other bits aside into an `untrusted-*`
  folder, log a warning, and the run redoes that work. Adding repair there
  would mean new shell logic; it is left for a follow-up issue.
- **Existing state folders.** No migration pass runs over old folders. Each
  entry is repaired when the check next opens it.
- **Shared primary group.** If several users share this user's primary
  group, a group member could have written a group-writable file before the
  repair. The repair removes access from then on; it cannot tell whether the
  content was changed earlier. The `WARNING` form of the log line flags this
  when the group's member list names another user. Users who have the group
  only as their primary group are not in that list and are not detected.
- **State folders outside the state root.** A state folder that is not a
  direct child of the auto-drive state root is never repaired, whatever its
  name, unless `EvidenceStore::create` has just created it. It must be `0700`
  already.
- **Non-private state roots.** A state root with group or other write bits,
  for example one an older shell created at `0775`, makes every slug folder
  under it strict. Run `chmod 700` on the root.
- **Ancestors of the state root.** The rule checks the canonical state root,
  not the folders above it. A custom `AMPLIHACK_STATE_DIR` is trusted to sit
  under folders that other users cannot rename or replace.
- **State roots reached through a symlink.** `Scope::of` requires the state
  folder's path to equal its canonical form, so a state root under a
  symlinked folder makes `check_dir` strict. On macOS `/tmp` is a symlink to
  `/private/tmp`, so a root there is strict for `check_dir`; auto-drive
  refuses macOS in any case until #1646.
- **Symlinks in the middle of a path.** `O_NOFOLLOW` only stops a symlink in
  the last part of a path. Repairs never reach outside a repairable root.

## Tests

The tests are in
`crates/amplihack-cli/src/commands/orch/autodrive/tests.rs`, so the filter
below finds them. They use temporary folders only.

- **Child processes for repairs.** Every test that expects a repair re-runs
  the test binary as a child (`--exact`, one test thread) through
  `under_umask_2546`, which sets the umask and the environment only in the
  child. The parent never changes its umask and never calls `set_var`.
- **Isolated environment.** `isolated_env_2546` creates `home`,
  `amplihack-home` and `state` folders at `0700` inside a `TempDir` and
  returns `HOME`, `AMPLIHACK_HOME` and `AMPLIHACK_STATE_DIR` for the child.
  Before any repair, the child checks that `AMPLIHACK_STATE_DIR` starts with
  the temporary path, so a test can never repair the real
  `~/.amplihack/state`.
- **Clean shell and git.** The `git` and `bash` helpers start from
  `env_clear()` and add back only `PATH`, `HOME`, `AMPLIHACK_HOME`,
  `AMPLIHACK_STATE_DIR` and `GIT_CONFIG_NOSYSTEM=1`, so `GIT_DIR` and other
  `GIT_*` variables set by git hooks do not leak in.
- **In-process tests check strict behaviour only.**

### End-to-end, real layout

| Test | Checks |
| --- | --- |
| `issue_2546_real_layout_round_under_umask_002` and `issue_2546_real_layout_round_under_umask_022` | Shared body `real_layout_round_2546(mask)`. Runs `git init` in a temporary repository with `origin` set to a GitHub-style URL, calls `autodrive_state_dir <repo> <pr>` once through `autodrive_state.sh` (the path is `<state>/auto-drive/<slug>` and the folder is `0700`), writes a round through the Rust and shell writers, then writes `qa-run.log` and `agent-findings.json` by shell redirection. `EvidenceStore::create` and `read_private` succeed, and completion `prepare` succeeds. Every folder ends `0700`, every file `0600`, and stderr has one repair line for each file that had group or other bits (the two redirected files: `0664` under umask `002`, `0644` under `022`), each starting `INFO:` or `WARNING:`, and no repair line for the slug folder itself |

### Location rule

| Test | Checks |
| --- | --- |
| `issue_2546_check_dir_repairs_slug_folder_under_state_root` | In the child, `check_dir` repairs a `0755` slug folder under the state root to `0700`. In process, a `0755` folder outside the root, including one named `autodrive-state` under a temporary shared folder, is refused with the row 7 reason and stays `0755` |
| `issue_2546_evidence_store_repairs_only_slug_folder_under_state_root` | In the child, `EvidenceStore::create` repairs a `0775` slug folder under the state root to `0700`. A `0775` folder outside the root, including one named `autodrive-state`, is refused with the row 7 reason and left unchanged |
| `issue_2546_state_folder_0775_is_repaired_and_logged` | In the child, a slug folder chmod'ed to `0775` is repaired to `0700` by `EvidenceStore::create`, with a log line |
| `issue_2546_hand_chmod_0664_is_repaired_and_logged` | In the child, a file in a slug folder chmod'ed to `0664` is repaired to `0600`, the read succeeds, and stderr names the path |
| `issue_2546_outside_state_tree_stays_strict` | `secure_fs::read` on a `0644` file in a `0755` folder outside the state root fails with the row 7 reason and changes neither mode |
| `issue_2546_completion_part_outside_state_root_stays_strict` | A `0755` folder `x` above `x/.completion-y/f` outside the state root gives `Scope::Strict`, is refused and stays `0755` |
| `issue_2546_rejected_prepare_changes_no_modes` | In the child, `prepare` with an invalid round label fails and leaves a `0775` slug folder at `0775` |
| `issue_2546_prepare_refuses_context_outside_state_folder` | In the child, `prepare` refuses `--context-from-file` and `--evaluator-context-file` paths outside the slug folder, or with `..`, and leaves them unchanged |
| `issue_2546_in_state_root_fails_strict` | In the child, each of these leaves a `0775` store refused and unchanged: a relative `AMPLIHACK_STATE_DIR`; a root at `0770`; a root at `0757`; a missing root; a root that is a symlink to a private `0700` folder; a sibling `auto-drive-x/<slug>`; the root itself as the store; `<root>/<slug>/deeper` as the store; `AMPLIHACK_STATE_DIR` and `HOME` both unset |

### Root resolution and group check

| Test | Checks |
| --- | --- |
| `issue_2546_state_root_from_cases` | `state_root_from` with `AMPLIHACK_STATE_DIR` set; unset with `HOME` set; empty treated as unset; both unset giving `/tmp/.amplihack/state/auto-drive`; a relative value kept relative |
| `issue_2546_state_root_matches_shell` | For each case above, `env -i bash` sourcing `autodrive_state.sh` and calling `autodrive_state_root` prints the same path as `state_root_from` |
| `issue_2546_shared_group_cases` | `shared_group` with no members, only this user, this user and another, another user only, and a name that differs from this user's only in bytes that are not valid UTF-8. Names are passed as `&[u8]` slices (`b"dev".as_slice()`), since `&[b"dev"]` does not coerce to `&[&[u8]]` |

### Safety rules (unchanged)

| Test | Checks |
| --- | --- |
| `issue_2546_foreign_owner_symlink_and_hard_link_are_still_refused` | `access` unit cases: foreign uid at `0600` and `0664`, symlink, wrong type, `0666` file, `0777` folder, foreign group, `nlink 2` with and without extra bits, and repair not allowed (row 7 reason): all refused; group write with `gid == egid` repaired to `0600` |

Run them with:

```bash
cargo test -p amplihack-cli autodrive
```
