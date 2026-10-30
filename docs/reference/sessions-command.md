# Reference: `amplihack sessions`

Archive old Claude Code session logs out of `~/.claude/projects`, and put an
archived session back. The [rotator](./profile-command.md#rotate-daily-session-archive)
also runs the archive once a day.

Claude Code keeps one log per session under `~/.claude/projects`, and never
removes them. The directory grows by several GiB a month, and Claude Code's
`/usage` reads all of it, so every `amplihack profile usage` probe slows down
as it grows (#1698). `sessions archive` moves each old session into one
compressed archive under `~/.amplihack/session-archive`, and
`sessions restore` brings one back.

## Contents

- [Synopsis](#synopsis)
- [Archive](#archive)
  - [What a session is](#what-a-session-is)
  - [Age](#age)
  - [Live sessions](#live-sessions)
  - [Archive steps](#archive-steps)
  - [Archive names](#archive-names)
  - [Output](#output)
- [Restore](#restore)
- [Destination layout](#destination-layout)
- [Manifest](#manifest)
- [Lock](#lock)
- [Configuration](#configuration)
- [Exit status](#exit-status)
- [Messages](#messages)
- [Privacy](#privacy)
- [Platform support](#platform-support)
- [Related](#related)

## Synopsis

```
amplihack sessions archive [--days N] [--dest DIR] [--apply]
amplihack sessions restore <SESSION_ID> [--dest DIR]
```

| Command | Behaviour |
|---------|-----------|
| `archive` | **Dry run.** Prints how many sessions, and how many bytes, would be archived. Creates, locks and changes nothing, but does check that DEST does not [overlap](#destination-layout) the projects root. |
| `archive --apply` | Archives every old session that is not live, then removes its originals. |
| `restore <SESSION_ID>` | Extracts one archived session back into `~/.claude/projects/<project>/`. Never overwrites a file. Keeps the archive. |

| Flag | Default | Meaning |
|------|---------|---------|
| `--days N` | `3` | A session is old when nothing in it has been written for `N` days. `N` is a whole number of at least 1; `--days 0` is a usage error. |
| `--dest DIR` | `~/.amplihack/session-archive` | The archive directory, called DEST below. A relative path is taken from the current directory. |
| `--apply` | off | Archive and remove. Without it nothing changes. |

Both commands refuse to run when the effective user id differs from the real
one, for example under a setuid wrapper. They print
`refusing to run with an effective user id different from the real user id`
and exit 1.

## Archive

### What a session is

The projects root is `~/.claude/projects`, resolved once. `CLAUDE_CONFIG_DIR`
does not change it, and there is no option to. A missing root means zero
sessions and exit 0.

| Path | Treated as |
|------|------------|
| `<root>/<project>/` | A project, when it is a real directory. A symlinked project is skipped and not counted. |
| `<root>/<project>/<id>.jsonl` | A session with id `<id>`, when it is a regular file. |
| `<root>/<project>/<id>/` | Part of session `<id>` (subagent logs, tool results), only when `<id>.jsonl` exists. |
| `<root>/<project>/memory/` and any other directory with no matching `.jsonl` | Not a session. Never read or touched. |

An id must be valid UTF-8, non-empty, must not start with `.`, and must not
contain `/`, NUL or a control character. A session with any other id is
skipped.

If `<id>.jsonl`, `<id>/` or anything below `<id>/` is a symlink, FIFO,
socket or device, the whole session is skipped. Nothing in it is followed,
archived or removed, and stderr gets one `skipped` line (see
[Messages](#messages)). Skipped sessions are not in the summary counts. Every
walk uses `lstat`, so no symlink is ever followed out of the projects root.

For each session the command records, from metadata only:

| Value | Definition |
|-------|------------|
| bytes | The sum of the sizes of the session's regular files. |
| parts | Regular files plus directories: `<id>.jsonl` is 1, `<id>/` and each subdirectory are 1 each, and each file is 1. This is the number of members its archive must hold. |
| newest mtime | The latest modification time of any part. |

### Age

A session is old when every part's modification time is at or before
`now − N × 86400` seconds. One recently written file anywhere in `<id>/`
keeps the whole session.

### Live sessions

A session that is still running is never archived, however old its log
looks. Claude Code writes `sessions/<pid>.json` in its config directory for
each running session, with the session's `sessionId` and `pid`. The command
reads every such directory it can find:

| Source | Which directories |
|--------|-------------------|
| Home | `~/.claude/sessions` |
| Profiles | `<profile>/sessions` for every profile in the [profile root](./profile-command.md#profile-root). Profiles share `projects/` with `~/.claude`, but each has its own `sessions/`. |
| Running processes (Linux) | `<CLAUDE_CONFIG_DIR>/sessions` for each process of the same user whose `/proc/<pid>/environ` sets `CLAUDE_CONFIG_DIR`. Processes that cannot be read are skipped. |

The same directory reached twice is read once.

A pid file counts when it is a regular `*.json` file. It is opened without
following symlinks and read up to 64 KiB. Only `sessionId` and `pid` are
used; other fields are ignored. Two pids are checked: `pid` from the file
and the file name's numeric stem. The session is live when **either** is
alive. A pid outside `1` to `2147483647` is not checked, so `pid: 0` or a
negative pid protects nothing. A pid is alive when `kill(pid, 0)` succeeds or
fails with `EPERM`; only `ESRCH` means dead.

`sessionId` is matched against session ids in every project. A pid file
that cannot be parsed protects nothing and is passed over without a
message, so the daily run does not repeat a line for it in the rotator log
every day. A dead process's pid file protects nothing.

The check fails closed. A missing `sessions/` directory is empty, but any
other error reading one, or reading the profile root, stops the command
before anything is archived:

```
cannot read live sessions in /home/ryan/.claude-profiles/work/sessions (permission denied); nothing archived
```

The live set is built once at the start. With `--apply`, just before a
session's originals are removed, the set is rebuilt if it is more than 30
seconds old and that session is checked again.

A process that has reused a dead session's pid keeps that session. This
errs towards keeping logs.

### Archive steps

With `--apply`, the command first checks DEST (see
[Destination layout](#destination-layout)), creates it if needed, takes the
[lock](#lock), and removes temporary files left by a run that crashed. Then,
for each old session that is not live:

1. Writes `<DEST>/<project>/.<id>.tar.gz.tmp-<pid>` (mode `0600`). Each file
   is opened without following symlinks, and its tar header is built from
   the open file, so a file swapped for a symlink during the run is not
   followed. Members are named `<id>.jsonl` and `<id>/…`, relative to the
   project directory, and keep their modes and mtimes.
2. Flushes the archive to disk.
3. Reads the whole archive back. The member names must match the parts
   recorded when the session was walked, the member count must equal parts, the member sizes must add up to
   bytes, and the gzip trailer checksum must be correct.
4. Links the archive to its [final name](#archive-names) without replacing
   anything, then removes the temporary name and flushes the project
   directory.
5. Checks the session again: its parts and mtimes must be unchanged and it
   must not be live. If this check fails, the command removes the final
   archive it linked in step 4, so no archive is left for a session whose
   originals are kept.
6. Appends one line to the [manifest](#manifest) and flushes it.
7. Removes `<id>.jsonl` and `<id>/`. Removal never follows symlinks.

If any of steps 1 to 6 fails, the command removes the temporary file and any
final archive it made, keeps the originals, reports the session as `failed`
and goes on to the next one. If step 7 fails part-way, the archive and its
manifest line stay, and the session is reported as `failed` with the path it
could not remove.

A first run over a large backlog takes a while: on the host that prompted
this command, about 28,000 sessions and 8 GiB.

### Archive names

The archive is `<DEST>/<project>/<id>.tar.gz`. If that name already exists,
for example because the session was archived, restored and has aged again,
the archive is `<id>.<YYYYMMDDTHHMMSSZ>.tar.gz`, using the UTC time of the
run, then `<id>.<YYYYMMDDTHHMMSSZ>-1.tar.gz` up to `-9`. An existing archive
is never overwritten. If all eleven names are taken, the session fails with
`no free archive name`.

The [manifest](#manifest) records the name used, so `restore` finds the
newest archive of a session.

### Output

With `--apply`, one line on stdout:

```
archived 28,013 sessions, 8.02 GiB (older than 3 days; 61 live sessions kept; 0 failed)
```

As a dry run:

```
would archive 28,013 sessions, 8.02 GiB (older than 3 days; 61 live sessions kept); dry run, nothing changed (use --apply)
```

| Field | Meaning |
|-------|---------|
| sessions | Sessions archived, or that would be. Counts use thousands separators, and the word is always `sessions`, for example `archived 1 sessions`. |
| GiB | Their bytes divided by 2³⁰, to two decimal places, for example `0.00 GiB`. |
| live sessions kept | Sessions old enough to archive that were kept only because they are live. |
| failed | Sessions that went wrong in [Archive steps](#archive-steps). |

There is no line per session archived. Each skipped or failed session gets
one line on stderr; see [Messages](#messages).

## Restore

```
amplihack sessions restore 4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10
```

1. Checks that the id follows the [id rules](#what-a-session-is), that DEST
   is [private](#destination-layout), and takes the [lock](#lock). Restore
   never creates DEST. A missing DEST prints `cannot prepare <DEST> (not found)`
   and exits 1.
2. Finds the session in `<DEST>/MANIFEST.jsonl`. When the id appears on more
   than one line, the last line wins. An unknown id, or a missing manifest,
   prints `session <id> not found in <DEST>/MANIFEST.jsonl` and exits 1. A
   manifest larger than 64 MiB is not read at all: restore prints
   `manifest <DEST>/MANIFEST.jsonl is larger than 64 MiB; nothing restored`
   and exits 1.
3. Checks the line's fields. `project` must be one valid name. `archive`
   must be relative, contain no `..`, start with the project, and end in
   `.tar.gz`. Anything else prints `refused session <id>: bad manifest line`
   and exits 1.
4. Opens the archive once, without following symlinks. A missing archive
   prints `archive <DEST>/<archive> not found` and exits 1.
5. Reads every member before writing anything. Each member must be a regular
   file or a directory, with a path of `<id>.jsonl` or `<id>/…` and no
   absolute, `..` or other special component, including after PAX long-name
   handling. The member count must equal the manifest's `parts` and the
   sizes must add up to its `bytes`. If any member fails, the whole archive
   is refused, nothing is written, and restore prints
   `refused <DEST>/<archive>: bad member <member>` or
   `refused <DEST>/<archive>: member count or size mismatch` and exits 1.
6. Refuses with `refused <path>: symlink` if `~/.claude/projects/<project>`
   is a symlink. Otherwise extracts into `~/.claude/projects/<project>/`,
   creating that directory if needed. Missing directories are created with
   mode `0700`, and an existing one is used only if it is a real directory.
   Files are created with mode `0600` and never replace an existing file.
   If the data exceeds the manifest's `bytes`, writing stops, files already
   written are kept, and restore prints
   `refused <DEST>/<archive>: more data than the manifest records` and
   exits 1.

On success:

```
restored 5 files to /home/ryan/.claude/projects/-home-ryan-src-amplihack-rs
```

`N` in `restored N files` counts regular files written, not directories.
The word is always `files`, for example `restored 1 files`.

An existing file, or an existing non-directory where a directory is needed,
is kept. It is reported on stderr as a conflict and the rest of the session
is still restored:

```
conflict: /home/ryan/.claude/projects/-home-ryan-src-amplihack-rs/4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10.jsonl already exists; kept
restored 4 files to /home/ryan/.claude/projects/-home-ryan-src-amplihack-rs; 1 conflict kept
```

The summary ends `; 1 conflict kept` for one conflict and
`; N conflicts kept` for more, for example `; 2 conflicts kept`. Any
conflict makes the exit status 1.

Restored files get the time of the restore as their modification time, not
the time stored in the archive. A restored session is therefore new: it is
kept for `--days` days and is not archived again by the next daily run.

The archive and the manifest are left unchanged. When the restored session
is archived again later, its archive gets a [timestamped name](#archive-names)
and a new manifest line.

## Destination layout

```
~/.amplihack/session-archive/                      0700
├── .archive.lock                                   0600
├── MANIFEST.jsonl                                  0600
└── -home-ryan-src-amplihack-rs/                    0700
    ├── 4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10.tar.gz 0600
    └── 4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10.20261102T031500Z.tar.gz
```

Each project under DEST has the same name as its directory under
`~/.claude/projects`.

Before it changes anything, `archive --apply` and `restore` check DEST:

| Check | Error |
|-------|-------|
| DEST is not the projects root, inside it, or a directory that contains it. The dry run checks this too. | `destination <DEST> overlaps <root>; nothing changed` |
| DEST is a directory owned by you and not writable by group or others. A DEST you named that is a symlink is checked at its target. | `destination <DEST> is not private (must be your own directory, not writable by group or others)` |
| Each `<DEST>/<project>/` is a real directory owned by you. | The session fails with `archive directory not private`. |

DEST is created with mode `0700` when missing. Archives are not encrypted;
their privacy rests on these modes.

## Manifest

`<DEST>/MANIFEST.jsonl` has one JSON line per archived session, appended
after the archive is in place and before the originals are removed:

```json
{"time":"2026-10-29T12:00:00Z","session_id":"4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10","project":"-home-ryan-src-amplihack-rs","bytes":184213,"archive":"-home-ryan-src-amplihack-rs/4f1c2a7e-9b3d-4c55-8e21-6a0f3d9b7c10.tar.gz","parts":5}
```

| Field | Type | Meaning |
|-------|------|---------|
| `time` | string | When the session was archived, UTC. |
| `session_id` | string | The session id. |
| `project` | string | The project directory name. |
| `bytes` | integer | The sum of the session's file sizes. |
| `archive` | string | The archive's path relative to DEST. |
| `parts` | integer | Files plus directories in the archive. |

The file is opened with `O_APPEND` and without following symlinks, and each
line is written in one write and flushed. Readers skip lines they cannot
parse and ignore unknown fields. `restore` refuses a manifest larger than
64 MiB without reading it; see [Restore](#restore).

## Lock

`archive --apply` and `restore` take an exclusive lock on
`<DEST>/.archive.lock` without waiting, and hold it until they finish. The
file stays on disk. The dry run takes no lock.

When another run holds the lock:

```
another sessions archive run is in progress (lock: /home/ryan/.amplihack/session-archive/.archive.lock)
```

The command then exits 1 and changes nothing.

Temporary files from a crashed run (`<DEST>/<project>/.<id>.tar.gz.tmp-<pid>`)
are removed at the start of the next `--apply` run, under the lock. Only
regular files with exactly that name pattern, one level below DEST, are
removed.

## Configuration

| Setting | Flag | Environment variable | Default |
|---------|------|----------------------|---------|
| Days | `--days` | [`AMPLIHACK_SESSIONS_ARCHIVE_DAYS`](./environment-variables.md#amplihack_sessions_archive_days) | `3` |
| Destination | `--dest` | [`AMPLIHACK_SESSIONS_ARCHIVE_DEST`](./environment-variables.md#amplihack_sessions_archive_dest) | `~/.amplihack/session-archive` |
| Daily run | none | [`AMPLIHACK_SESSIONS_ARCHIVE_DAILY`](./environment-variables.md#amplihack_sessions_archive_daily) | on |

A flag overrides its variable, and the variable overrides the default. An
empty variable counts as unset. An invalid `AMPLIHACK_SESSIONS_ARCHIVE_DAYS`
is an error that names the variable without repeating its value:

```
AMPLIHACK_SESSIONS_ARCHIVE_DAYS must be a whole number of at least 1
```

`HOME` must be set; otherwise the command prints `HOME is not set` and
exits 1.

To archive sessions older than a week into a directory on another disk:

```
amplihack sessions archive --days 7 --dest /mnt/backup/claude-sessions
amplihack sessions archive --days 7 --dest /mnt/backup/claude-sessions --apply
```

The [daily run](./profile-command.md#rotate-daily-session-archive) uses the
same days and destination, read from the rotator's environment.

## Exit status

| Status | When |
|--------|------|
| 0 | Every session was archived, or would be, or skipped; or the restore wrote every file. |
| 1 | A session failed, a restore had a conflict, the lock was held, the live check could not read a source, DEST failed a check, the session or archive was not found, or another setup error. |
| 2 | Usage error, such as `--days 0`, an unknown flag or a missing session id. |

## Messages

Every path and name in a message has its control characters escaped. Error
detail is reduced to one of `permission denied`, `not found`,
`already exists`, `disk full` or `I/O error`, shown as `<kind>` below.

Every message below goes to stderr. The tests check these strings exactly.

| Message | Meaning |
|---------|---------|
| `skipped <project>/<id>: symlink` | The session contains a symlink. Left as it is. |
| `skipped <project>/<id>: special file` | The session contains a FIFO, socket or device. Left as it is. |
| `skipped <project>/<id>: bad name` | The id breaks the [id rules](#what-a-session-is). Left as it is. |
| `skipped <project>/<id>: unreadable (<kind>)` | Its metadata could not be read. Left as it is. |
| `failed <project>/<id>: write failed (<kind>)` | The archive could not be written. Originals kept. |
| `failed <project>/<id>: verify mismatch` | The archive read back did not match the session. Originals kept. |
| `failed <project>/<id>: changed during archive` | A part was written, added, removed or replaced while it was archived. Originals kept. |
| `failed <project>/<id>: now live` | The session started running during the run. Originals kept. |
| `failed <project>/<id>: no free archive name` | All eleven [archive names](#archive-names) exist. Originals kept. |
| `failed <project>/<id>: archive directory not private` | `<DEST>/<project>` is not a real directory owned by you. Originals kept. |
| `failed <project>/<id>: manifest write failed (<kind>)` | The manifest line could not be written. Originals kept. |
| `failed <project>/<id>: cannot read live sessions (<kind>)` | The live set could not be rebuilt before removal. Originals kept. |
| `failed <project>/<id>: remove failed (<kind>): <path>` | Archived and recorded, but `<path>` could not be removed. |
| `cannot read live sessions in <path> (<kind>); nothing archived` | The [live check](#live-sessions) failed closed. |
| `cannot prepare <path> (<kind>)` | DEST, its lock or its manifest could not be created or opened. |
| `cannot remove <path> (<kind>)` | A crash leftover could not be removed. The run goes on. |
| `cannot read the current directory for a relative destination` | `--dest` or the variable is relative and the current directory is gone. Nothing read. |
| `another sessions archive run is in progress (lock: <path>)` | The [lock](#lock) is held. Nothing changed. |
| `destination <DEST> overlaps <root>; nothing changed` | DEST is, contains or lies inside the projects root. |
| `destination <DEST> is not private (must be your own directory, not writable by group or others)` | DEST failed the [privacy check](#destination-layout). |
| `refusing to run with an effective user id different from the real user id` | The command runs setuid or similar. Nothing read. |
| `HOME is not set` | The home directory is unknown. Nothing read. |
| `AMPLIHACK_SESSIONS_ARCHIVE_DAYS must be a whole number of at least 1` | The variable is set to an invalid value. Nothing read. |
| `session <id> not found in <DEST>/MANIFEST.jsonl` | `restore`: no manifest line for `<id>`, or no manifest. |
| `manifest <DEST>/MANIFEST.jsonl is larger than 64 MiB; nothing restored` | `restore`: the manifest is too large to read. |
| `refused session <id>: bad session id` | `restore`: the id breaks the [id rules](#what-a-session-is). Nothing read. |
| `refused session <id>: bad manifest line` | `restore`: the line's `project` or `archive` field breaks the [rules](#restore). Nothing written. |
| `archive <DEST>/<archive> not found` | `restore`: the manifest names an archive that does not exist. |
| `refused <DEST>/<archive>: bad member <member>` | `restore`: a member is not a regular file or directory, or its path is unsafe. Nothing written. |
| `refused <DEST>/<archive>: member count or size mismatch` | `restore`: the archive does not match the manifest's `parts` or `bytes`. Nothing written. |
| `refused <DEST>/<archive>: not a readable archive` | `restore`: the file is not a gzip tar, or is truncated or corrupt. Nothing written. |
| `cannot write <path> (<kind>)` | `restore`: a directory or file could not be created or written. Files already written are kept. |
| `refused <DEST>/<archive>: more data than the manifest records` | `restore`: extraction stopped. Files already written are kept. |
| `refused <path>: symlink` | `restore`: the target project directory is a symlink. Nothing written. |
| `conflict: <path> already exists; kept` | `restore` did not overwrite `<path>`. |

The summary lines `archived …`, `would archive …` and `restored N files to
<target>`, with its optional conflict ending, go to stdout; see
[Output](#output) and [Restore](#restore). In the rotator log, the daily run
also writes
`sessions archive: ignored AMPLIHACK_SESSIONS_ARCHIVE_DAILY value (use on, off, 1 or 0); running`
when that variable holds an unknown value; see the
[daily session archive](./profile-command.md#rotate-daily-session-archive).

On a filesystem without hard links, every session fails with
`write failed (<kind>)` and every original is kept.

## Privacy

The commands read file metadata, pid files and, on Linux, the
`CLAUDE_CONFIG_DIR` value of your own processes. They copy session files
into archives byte for byte, but never parse, print or log their contents.
They never open credential files. No message contains file contents, the
text of a parser or I/O error, or an environment value.

## Platform support

The `sessions` commands exist on Unix only. On macOS there is no `/proc`, so
the [live check](#live-sessions) uses `~/.claude/sessions` and the profiles'
`sessions/` directories only. A session run with a `CLAUDE_CONFIG_DIR`
outside those is not seen as live there.

## Related

- [`amplihack profile` reference: daily session archive](./profile-command.md#rotate-daily-session-archive) - How the rotator runs the archive once a day
- [Shared session directories](./profile-command.md#shared-session-directories) - Why every profile's `projects/` is the same directory
- [Environment variables](./environment-variables.md#amplihack_sessions_archive_days) - The three variables in full
