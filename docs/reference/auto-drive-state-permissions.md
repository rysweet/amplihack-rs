---
title: Auto-drive State Permissions
description: How auto-drive writes its state and evidence 0700/0600 under any umask, and when the evidence check repairs or refuses an entry.
last_updated: 2026-11-12
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
| 7 | Extra group or other bits, where repair is not allowed (`Repair::Refuse`) | Refuse: `evidence outside the auto-drive state tree has group or other access`, with the advice to `chmod 700` (folder) or `chmod 600` (file), or to use the runner's default `autodrive-state` folder |
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

Each caller decides once, from the state root, whether repairs are allowed,
and passes that down. Path names alone never allow a repair. The rule is
`private_mode::repairable_root`:

- A state root may be repaired only if its name is `autodrive-state` (the
  runner's default, `<run dir>/autodrive-state` or
  `<launch home>/autodrive-state`), or if this call has just created it.
- An explicit `autodrive_state_dir` with any other name is checked strictly,
  as before #2546, and must already be `0700`. A `0775` explicit folder is
  refused, unchanged, with the advice to run `chmod 700 <path>` or to use
  the default state folder.
- Repairs reach only entries at or below a repairable root, compared part by
  part. Folders above the root, and any path outside it, are checked
  strictly.

| Caller | Repair allowed for |
| --- | --- |
| `EvidenceStore::create` | the evidence store root, when `repairable_root` holds; before any repair it checks that the folder it opened is the folder it checked (same device and inode), so a root swapped for a symlink is refused |
| `EvidenceStore::read_private` | each record it reads, only in a store whose root is repairable |
| `secure_fs::check_dir(state_dir)` | the state folder, only when it is named `autodrive-state` |
| `secure_fs::read`, `output`, `create` with `Scope::StateTree(state_dir)` | the folders checked on the path (the last parent, the folder above the first `.completion-*` part and every folder below it) and the target file, when they are at or below `state_dir` |
| `secure_fs` with `Scope::Strict` (`private_state::load`, `snapshot`) | nothing |

`prepare` validates its arguments (round label, record, no `..` in the state
folder) before it calls `check_dir`, so a rejected call changes no modes.
Every `--context-from-file` and `--evaluator-context-file` path must be
absolute, contain no `..` part and lie inside the state folder; otherwise
`prepare` fails with `context file must be inside the state directory`
before reading it. The runner's `completion_context` writer already puts its
files in a `.completion-context.*` folder inside the state folder.

So `$HOME`, a repository root, an explicit state folder with another name,
or a file outside the state folder is never changed.

## The repair log line

Each repair writes one line to stderr:

```
INFO: auto-drive state "/path/to/state/pr-123/round-2/qa.json" had mode 0664 (gid 1000); removed group and other access (now 0600)
```

- The path is printed with Rust's `{:?}` quoting, so a newline in a path
  cannot fake a second log line.
- The gid is the group the repair trusted (row 5), so an operator can check
  it.
- There is one line per repaired entry. An entry that is already private
  writes nothing.

## Errors

A refused entry stops the check with an error naming the reason and the
path. The path is quoted with `{:?}`, as in the log line, for example:

```
untrusted evidence ownership or permissions: "/path/to/state/pr-123/round-2/qa.json"
```

`secure_fs` adds the context `private state ownership, permissions or file
type are unsafe`. If the `fchmod` for a repair fails, the error has the
context `repair private state mode "<path>"`, with the path quoted the same
way.

| Error | Likely cause | What to do |
| --- | --- | --- |
| `untrusted evidence ownership or permissions` | The entry is owned by another user, is world-writable, or has a foreign group | Find out who wrote it. Do not `chmod` it to make the error go away. |
| `evidence outside the auto-drive state tree has group or other access` | A file or folder outside a repairable state root is open to group or other users; most often an explicit state folder with a name other than `autodrive-state` | Auto-drive never changes paths outside a repairable root. Run `chmod 700` on the folder (or `chmod 600` on the file) if it is yours, or use the runner's default `autodrive-state` folder. |
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
  content was changed earlier.
- **Explicit state folders.** A state folder with a name other than
  `autodrive-state` is never repaired. It must be `0700` already.
- **Symlinks in the middle of a path.** `O_NOFOLLOW` only stops a symlink in
  the last part of a path. Repairs never reach outside a repairable root.

## Tests

All ten tests are in
`crates/amplihack-cli/src/commands/orch/autodrive/tests.rs`, so the filter
below finds them, and they use temporary folders only. Tests that need umask `002` re-run the test
binary as a child (`--exact`, one test thread) and set the umask only in the
child; the parent test process never changes its umask.

| Test | Checks |
| --- | --- |
| `issue_2546_round_written_under_umask_002_passes_next_evidence_check` | Writes one round through the Rust writers, the `completion_context` shell writer and one plain `std::fs::write` file under umask `002`, then runs the next evidence check: every call succeeds, every folder is `0700`, every file is `0600`, and stderr has exactly one `INFO` line, naming the plain-write file |
| `issue_2546_hand_chmod_0664_is_repaired_and_logged` | A file chmod'ed to `0664` is repaired to `0600`, the read succeeds, and stderr names the path |
| `issue_2546_state_folder_0775_is_repaired_and_logged` | An `autodrive-state` folder chmod'ed to `0775` is repaired to `0700` by `EvidenceStore::create`, with a log line |
| `issue_2546_outside_state_tree_stays_strict` | `secure_fs::read` with a repairable root, on a `0644` file in a `0755` folder outside that root, fails with the row 7 reason and changes neither mode |
| `issue_2546_completion_part_outside_state_root_stays_strict` | A `0755` folder `x` above `x/.completion-y/f` is refused with `Scope::Strict` and with a root that is not `x`, and stays `0755` |
| `issue_2546_check_dir_repairs_only_runner_state_folder` | `check_dir` refuses a `0755` folder with another name and leaves it `0755`; it repairs a `0755` `autodrive-state` folder to `0700` |
| `issue_2546_rejected_prepare_changes_no_modes` | `prepare` with an invalid round label fails and leaves a `0775` `autodrive-state` folder at `0775` |
| `issue_2546_prepare_refuses_context_outside_state_folder` | `prepare` refuses `--context-from-file` and `--evaluator-context-file` paths outside the state folder, or with `..`, and leaves them unchanged |
| `issue_2546_evidence_store_repairs_only_runner_state_root` | `EvidenceStore::create` refuses a `0775` folder named `explicit-state` and leaves it unchanged; the same folder renamed `autodrive-state` is repaired to `0700` |
| `issue_2546_foreign_owner_symlink_and_hard_link_are_still_refused` | `access` unit cases: foreign uid at `0600` and `0664`, symlink, wrong type, `0666` file, `0777` folder, foreign group, `nlink 2` with and without extra bits, and repair not allowed (row 7 reason): all refused; group write with `gid == egid` repaired to `0600` |

Run them with:

```bash
cargo test -p amplihack-cli autodrive
```
