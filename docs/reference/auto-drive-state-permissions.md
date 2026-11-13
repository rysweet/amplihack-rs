---
title: Auto-drive State Permissions
description: How auto-drive writes its state and evidence 0700/0600 under any umask, and when the evidence check repairs or refuses an entry, decided by the state folder's location under the auto-drive state root or, for files, by the folder being private.
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
logged when it lies in a state folder directly under the auto-drive state
root, or when it is a file read from a private state folder. Anything else
that is wrong is refused.

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
`std::fs::write`, or `>` in a shell without `umask 077`) take the shell's
umask: `0664` under umask `002`, `0644` under umask `022`. A drive's QA agent
that redirects a log into `<run dir>/autodrive-state`, for example
`qa-criterion1-attempt2-ffi.log`, leaves such a file. The evidence check
repairs those files, as described below.

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
| 7 | Extra group or other bits, where repair is not allowed: `Repair::Refuse` for any entry, or `Repair::PrivateFolder` for a folder | Refuse: `evidence outside the auto-drive state tree has group or other access`, with the advice to `chmod 700` (folder) or `chmod 600` (file), or to keep drive state in a folder directly under the auto-drive state root |
| 8 | Extra group or other bits: any entry under `Repair::Allowed`, or a file under `Repair::PrivateFolder` | Repair to `mode & 0o700`, log, continue |
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
  strict-path refusal from a foreign owner. A folder is never repaired under
  `Repair::PrivateFolder`; folders follow the location rule only.
- Row 8: `mode & 0o700` also clears setuid, setgid and sticky bits. A folder
  at `0775` becomes `0700`; a file at `0644` or `0664` becomes `0600`.
- Row 8 under `Repair::PrivateFolder`: the caller passes this value only for
  a file it opened through a folder descriptor that has just passed the
  folder check strictly, so that folder is owned by this user, is a real
  folder and has no group or other bits. No other user can then create,
  rename, remove or replace any entry in it, so a file there that rows 1 to
  6 accept (owned by this user, no other-write bit, this user's group, one
  link) can only have been written by this user. Its extra bits are left by
  the writer's umask, not by tampering, and removing them is safe.

`Repair` has three values:

| Value | Meaning | Passed by |
| --- | --- | --- |
| `Repair::Allowed` | The entry is at or below a repairable state folder (see [the location rule](#the-location-rule)) | `EvidenceStore` and `secure_fs` for entries in a repairable folder |
| `Repair::PrivateFolder` | The entry is a single name opened with `openat(O_NOFOLLOW)` on a folder descriptor that passed `make_private(.., Kind::Dir, ..)` in the same operation. Files may be repaired; folders may not | `EvidenceStore::read_private` and `secure_fs::read` when the folder is not repairable by location, including every `Scope::Strict` read (`private_state::load` and `private_state::snapshot`) |
| `Repair::Refuse` | Extra bits are refused, as before #2546 | Folder checks outside a repairable folder |

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

// An agent-written 0644 log in a private folder is repaired.
let log = Entry { mode: 0o644, ..entry };
assert_eq!(
    access(log, Kind::SingleLinkFile, 1000, 1000, Repair::PrivateFolder),
    Access::Repair(0o600)
);

// A 0755 folder is never repaired under PrivateFolder.
let folder = Entry { is_dir: true, is_file: false, mode: 0o755, ..entry };
assert!(matches!(
    access(folder, Kind::Dir, 1000, 1000, Repair::PrivateFolder),
    Access::Refuse(_)
));
```

## Where repairs happen

Each caller decides once, from the state folder's location, whether repairs
are allowed, and passes that down. A folder's name never allows a repair;
only its location does. Files have one further case: a file read from a
private folder is repaired wherever that folder is (see
[Files in a private folder](#files-in-a-private-folder)).

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
  `state::open_or_create_root` makes the missing parents recursively, then
  the folder itself with one non-recursive `mkdir`. Only that `mkdir`
  succeeding sets `created_now`. A folder that already exists, including one
  that another process makes between the lstat and the `mkdir` (`EEXIST`),
  is treated as existing. `Scope::of` has no such case.
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
  created them; an existing one must already be `0700`. Files read from such
  a `0700` folder are repaired under the private-folder rule below.
- Repairs reach only entries at or below a repairable folder, compared part
  by part. Folders above it, and any path outside it, are checked strictly.

| Caller | Repair allowed for | File in a private folder |
| --- | --- | --- |
| `EvidenceStore::create` | the evidence store root, when `repairable_root` holds; the device and inode check runs first, so a root swapped for a symlink is refused before the rule is consulted | not applicable: folders only |
| `EvidenceStore::read_private` | each record it reads, in a store whose root is repairable | each record it reads, in any store: `create` has already proved the root private, so `Repair::Refuse` becomes `Repair::PrivateFolder` |
| `secure_fs::check_dir(state_dir)` | the state folder, only when `Scope::of` returns `Scope::StateTree`: the path is absolute, has no `..` part, exists, equals its own canonical form, and is a direct child of the state root. A folder that does not exist is `Scope::Strict`; no folder is created to decide the scope | not applicable: folders only |
| `secure_fs::read` with `Scope::StateTree(state_dir)` | the folders checked on the path (the last parent, the folder above the first `.completion-*` part and every folder below it) and the target file, when they are at or below `state_dir` | the target file, when its parent folder passed the strict check |
| `secure_fs::read` with `Scope::Strict` (`private_state::load`, `private_state::snapshot`) | no folder | the target file, when its parent folder passed the strict check |
| `secure_fs::output`, `create` (any scope) | as for `read`; the target is created at `0600` or opened for writing, never repaired | not used |

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

So `$HOME`, a repository root, the state root itself, or a state folder
outside the state root is never changed, and a file is changed only when it
lies in a repairable folder or in a private folder.

### Files in a private folder

A file read through `EvidenceStore::read_private` or `secure_fs::read` is
opened with `openat(O_NOFOLLOW)` as a single name on the folder descriptor
that has just been checked. That check is `make_private(.., Kind::Dir, ..)`
on the same descriptor, so the folder is known to be owned by this user, a
real folder and not a symlink, with no group or other bits. Where the folder
is not repairable by location, the folder check passes only in that state.

- **Why it is safe.** In a folder that only this user can write, no other
  user can create, rename, remove or replace an entry. A file there that is
  owned by this user, has no other-write bit, has this user's group and has
  one link was written by this user. Extra bits on it are left by the
  writer's umask, so they are removed and the read goes on.
- **What it covers.** The agent-written `0644` and `0664` files in
  `<run dir>/autodrive-state` (ResearchDrafts #865), and files in any other
  private state folder, wherever it sits.
- **What it does not cover.** Folders. A folder with extra bits is still
  repaired only by location, so a `0755` folder outside the state root is
  refused before any file in it is opened. Rows 1 to 6 still apply to the
  file, so a foreign owner, a symlink, a wrong type, an other-write bit, a
  foreign group or a second link is refused as before.
- **Strict reads of private state.** `private_state::load` and
  `private_state::snapshot` read `state.json` and its `.evidence` file with
  `Scope::Strict`. Before this rule they repaired nothing. They now repair a
  `0644` or `0664` `state.json` or `.evidence` file that this user owns in a
  `0700` folder, and log one line, as for any other file in a private
  folder. This change is intended. A folder with extra bits is still refused.
- **The run folder above does not matter.** Only the folder the file is
  opened through is checked. `EvidenceStore::create` checks the store folder
  alone, on the descriptor it has just opened, and `secure_fs::read` checks
  the last parent (and, on a `.completion-*` path, the folders from the one
  above that part downwards). The run folder above `autodrive-state` is never
  checked, so its mode does not change the result. In #865 the run folder
  was `0700`; T1 also uses a `0700` run folder, and no test covers a `0755`
  one.
- **One decision per operation.** The folder is checked again on every read;
  no decision that a folder is private is cached.

For example, an existing `<run dir>/autodrive-state` at `0700` holding
`qa-criterion1-attempt2-ffi.log` at `0644`, read through
`EvidenceStore::create(..).read_private`, gives:

```
INFO: auto-drive state "<run dir>/autodrive-state/qa-criterion1-attempt2-ffi.log" had mode 0644 (gid 1000); removed group and other access (now 0600)
```

Here `<run dir>` stands for the runner's absolute run folder; the line
shows the full path. The read returns the file's bytes unchanged, and a
second read writes no line.

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
WARNING: auto-drive state "/home/dev/.amplihack/state/auto-drive/https___github.com_rysweet_amplihack-rs_2579/round-2/qa.json" had mode 0664 (gid 1000); group members could not be read (errno 2); removed group and other access (now 0600)
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
  missing entry gives the "could not be read" form, with the errno: `ERANGE`
  at the cap, the value returned, or `ENOENT` for a missing entry.
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
| none; an `INFO:` or `WARNING:` repair line naming a `0644` or `0664` file in a private state folder | An agent step wrote the file under umask `022` or `002` into a `0700` state folder, for example `qa-criterion1-attempt2-ffi.log` in `<run dir>/autodrive-state` | Nothing. The file is now `0600` and was read. To avoid the line, write agent files under `umask 077`. |
| `untrusted evidence ownership or permissions` | The entry is owned by another user, is world-writable (for example `0666`), or has group bits and a foreign group (for example `0664` with a group taken from a setgid folder). This holds in a private folder too | Find out who wrote it. Do not `chmod` it to make the error go away. |
| `evidence outside the auto-drive state tree has group or other access` | A folder outside a repairable state folder is open to group or other users, for example a `0755` state folder that is not directly under the state root, or a state root that is not `0700`. The refusal is raised on the folder, before any file in it is opened; files in it keep their modes | Auto-drive never changes a folder outside the state root. Run `chmod 700` on the folder if it is yours; its files are then repaired on the next read. Or keep drive state in a folder directly under the state root named in the message. If the state root itself has group or other write bits, run `chmod 700` on the root. |
| `context file must be inside the state directory` | A `--context-from-file` or `--evaluator-context-file` path is outside `--state-dir` or contains `..` | Write the context file inside the state folder. |
| `state root changed during open` | The state root was replaced between the check and the open | Treat as tampering. |
| `evidence has unexpected hard links` | Another name points at the same file, in a private folder too | Remove the other link, or remove the record and let the round rewrite it |
| `evidence is a symlink` / `evidence is not a regular file or state directory` | Something replaced a state entry; a private folder does not change this | Treat as tampering; remove the entry and rerun the round |

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
- **Rows 1 to 6.** A private folder changes nothing for a foreign owner, a
  symlink, a wrong type, an other-write bit (`0666`), group bits with a
  foreign group, or a second link. These are refused in every folder.
- **Folders.** The private-folder rule repairs files only. A folder is
  repaired only by the location rule.
- **State folders outside the state root.** A state folder that is not a
  direct child of the auto-drive state root, and is not private, is never
  repaired, whatever its name, unless `EvidenceStore::create` has just
  created it. It is refused before any file in it is opened, and its files
  keep their modes. It must be `0700` already; once it is, its files are
  repaired under the private-folder rule.
- **Descriptors opened earlier.** The private-folder rule shows that no other
  user can change the folder's entries from now on. It cannot show that
  nobody opened a file earlier. A descriptor another user opened while the
  folder was not yet `0700`, or a group member's descriptor on a `0664` file,
  stays open after the repair. The same holds under the location rule; the
  `WARNING` form of the log line flags a group with other members.
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
- **Isolated environment.** `isolated_2546(test, mask, overrides, body)`
  creates `home`, `amplihack-home` and `state` folders at `0700` inside a
  `TempDir` and sets `HOME`, `AMPLIHACK_HOME` and `AMPLIHACK_STATE_DIR` to
  them for the child only. `overrides` changes or removes any of the three
  for that child. Before `body` runs, the child checks that every absolute
  value of the three lies inside the temporary path, so a test can never
  repair the real `~/.amplihack/state`.
- **Clean shell and git.** The `git` and `bash` helpers start from
  `env_clear()` and add back only `PATH`, `HOME`, `AMPLIHACK_HOME`,
  `AMPLIHACK_STATE_DIR` and `GIT_CONFIG_NOSYSTEM=1`, so `GIT_DIR` and other
  `GIT_*` variables set by git hooks do not leak in.
- **In-process tests check strict behaviour only.** The one exception,
  `issue_2546_outside_state_tree_stays_strict`, repairs a file in a private
  folder inside a `TempDir`; it reads and writes nothing under `HOME`.

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
| `issue_2546_outside_state_tree_stays_strict` | `Scope::of` gives `Scope::Strict` for a folder outside the state root. `secure_fs::read` on a `0644` file in a `0755` folder there fails with the row 7 reason and changes neither mode. A `0644` file in a `0700` folder there is repaired to `0600` and read (`b"notes"`), and a second read returns the same bytes. Before the private-folder rule, this second half expected a refusal |
| `issue_2546_completion_part_outside_state_root_stays_strict` | A `0755` folder `x` above `x/.completion-y/f` outside the state root gives `Scope::Strict`, is refused and stays `0755` |
| `issue_2546_rejected_prepare_changes_no_modes` | In the child, `prepare` with an invalid round label fails and leaves a `0775` slug folder at `0775` |
| `issue_2546_prepare_refuses_context_outside_state_folder` | In the child, `prepare` refuses `--context-from-file` and `--evaluator-context-file` paths outside the slug folder, or with `..`, and leaves them unchanged |
| `issue_2546_in_state_root_fails_strict` | In the child, `in_state_root` is true for a slug folder under a private root, and false for the root itself, for `<root>/<slug>/deeper` and for a slug in the sibling `auto-drive-x`. With the root at `0770`, `0757`, `0720` or `0702`, or with the root path a symlink to a private folder, a `0775` slug folder is refused by `EvidenceStore::create` (and, for the modes, by `check_dir`) with the row 7 reason, and neither the slug folder nor the root changes. With the root missing, `in_state_root` is false. No repair line is written |
| `issue_2546_relative_state_dir_stays_strict` | In the child, with `AMPLIHACK_STATE_DIR=rel-state`, the root resolves to `rel-state/auto-drive`, and a `0775` slug folder under it is not in the root, is refused with the row 7 reason and stays `0775` |
| `issue_2546_unset_state_vars_stay_strict` | In the child, with `HOME` and `AMPLIHACK_STATE_DIR` both unset, the root is `/tmp/.amplihack/state/auto-drive` and `in_state_root` is false for a slug under it. Nothing is created under `/tmp` |
| `issue_2546_empty_state_dir_uses_home_root` | In the child, an empty `AMPLIHACK_STATE_DIR` counts as unset, so the root is under `HOME`, and a `0775` slug folder there is repaired to `0700` with one repair line |
| `issue_2546_existing_folder_is_not_treated_as_created` | In the child, under umask `002`: `open_or_create_root` reports a `0775` folder that already exists outside the root as not created, and `EvidenceStore::create` refuses it with the row 7 reason and leaves it at `0775`. A missing folder and its missing parent are made at `0700` and reported as created once. No repair line is written |

### Private folders

| Test | Checks |
| --- | --- |
| `issue_2546_private_run_dir_state_repairs_agent_written_files` | In the child, under umask `022`: a `0700` run folder holds an existing `0700` `autodrive-state` folder with `qa-criterion1-attempt2-ffi.log` (`0644`) and `store-0664.log` (`0664`), read through `EvidenceStore::create(..).read_private`, and `fs-0644.log` and `fs-0664.log`, read through `secure_fs::read(Scope::of(&state), ..)`. Every file is read twice. The bytes are unchanged, every file ends `0600`, and stderr has exactly four repair lines, one naming each file, each starting `INFO:` or `WARNING:`. The second reads write nothing |
| `issue_2546_open_folder_outside_state_root_still_refuses_files` | In the child: a `0755` folder outside the state root holds a `0644` and a `0664` file. `EvidenceStore::create` and `secure_fs::read` both fail with the row 7 reason, the folder and both files keep their modes, and no repair line is written |

### Root resolution and group check

| Test | Checks |
| --- | --- |
| `issue_2546_state_root_from_cases` | `state_root_from` with `AMPLIHACK_STATE_DIR` set; unset with `HOME` set; empty treated as unset; both unset giving `/tmp/.amplihack/state/auto-drive`; a relative value kept relative |
| `issue_2546_state_root_matches_shell` | For each case above, `env -i bash` sourcing `autodrive_state.sh` and calling `autodrive_state_root` prints the same path as `state_root_from` |
| `issue_2546_shared_group_cases` | `shared_group` with no members, only this user, this user and another, another user only, and a name that differs from this user's only in bytes that are not valid UTF-8. Names are passed as `&[u8]` slices (`b"dev".as_slice()`), since `&[b"dev"]` does not coerce to `&[&[u8]]` |

### Safety rules (unchanged)

| Test | Checks |
| --- | --- |
| `issue_2546_foreign_owner_symlink_and_hard_link_are_still_refused` | `access` unit cases: foreign uid at `0600` and `0664`, symlink, wrong type, `0666` file, `0777` folder, foreign group, `nlink 2` with and without extra bits, and repair not allowed (row 7 reason): all refused; group write with `gid == egid` repaired to `0600`. The same file under `Repair::PrivateFolder`: foreign uid at `0644` (row 3), symlink (row 1), wrong type (row 2), `nlink 2` (row 6), `0666` (row 4), `0664` with a foreign gid (row 5) and a `0755` folder (row 7) are refused; `0644` and `0664` owned by this user, with `gid == egid` and one link, are repaired to `0600` for both `Kind::File` and `Kind::SingleLinkFile`; `0600` is private |

Run them with:

```bash
cargo test -p amplihack-cli --lib autodrive
```
