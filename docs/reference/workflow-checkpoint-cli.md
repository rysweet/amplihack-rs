---
title: amplihack internal workflow-checkpoint
type: reference
issues: [1554, 1914]
decisions: [17.116, 17.126]
---

# `amplihack internal workflow-checkpoint` reference

The publish, review-feedback and final-cleanup checkpoints of the default
workflow call this hidden subcommand for scope admission, strict staging and
outgoing-only publication checks. It is the Rust replacement for the Python
heredoc that used to live in `workflow_publish_checkpoint.sh` and for the
inline `python3` calls in the related tools and recipes (decision 17.126).
The behaviour of decision 17.116 is unchanged.

No recipe or tool step needs `python3` for checkpoints. They need the
`amplihack` binary on `PATH`, or `AMPLIHACK_BIN` set to it.

## Contents

- [Invocation](#invocation)
- [Callers](#callers)
- [Exit status](#exit-status)
- [Error mapping](#error-mapping)
- [Actions](#actions)
- [Publication rules (17.116)](#publication-rules-17116)
- [Environment](#environment)
- [JSON output](#json-output)
- [Private files](#private-files)
- [Authorised changes (17.126)](#authorised-changes-17126)
- [Tests](#tests)

## Invocation

```sh
"${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint <action> [args...]
```

`workflow_publish_checkpoint.sh` wraps this in a shell function:

```sh
workflow_checkpoint() { "${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint "$@"; }
```

The subcommand is hidden from `amplihack --help` and from
`amplihack internal --help`. It accepts arguments that start with `-`, so
path lists and refspecs pass through unchanged.

`AMPLIHACK_BIN` is trusted on the same terms as `PATH`. The tools never derive
it from worktree contents. `autodrive_loop.sh` and
`workflow_hook_repair.sh` resolve the binary the same way.

## Callers

| Asset | Actions it calls |
| --- | --- |
| `amplifier-bundle/tools/workflow_publish_checkpoint.sh` | `selectors`, `capture`, `admit`, `check`, `paths`, `before`, `sync-fetch-mode`, `after`, `destination-is-head`, `verify-pushed-ref`, `sha256`, `admission-refused-result` |
| `amplifier-bundle/tools/workflow_cleanup_checkpoint.sh` | `cleanup-skip-result` |
| `amplifier-bundle/tools/workflow_stage_changes.sh` | `stage-strict` |
| `amplifier-bundle/tools/workflow_pr_head_sync.sh` | `advertised-ref` |
| `amplifier-bundle/recipes/workflow-publish.yaml` (two steps) | `worktree-path` |
| `amplifier-bundle/recipes/workflow-pr-review.yaml` (two steps) | `worktree-path` |

## Exit status

Each status is the one the 918c256cf Python returned for the same input. The
two exceptions are listed under
[Authorised changes (17.126)](#authorised-changes-17126).

| Status | Meaning | Returned by |
| --- | --- | --- |
| 0 | Success. Any result is on stdout. | every action |
| 1 | What the Python left uncaught: a missing argument, an unreadable `advertised-ref` file or status, a failed `worktree-path` extraction, a missing action name, or a panic. stderr has one line starting `checkpoint internal error: `. stdout is empty. | every action |
| 2 | Refusal. stderr has exactly one line, `checkpoint scope refusal: <reason>` (`stage_workflow_changes: strict refusal: <reason>` for `stage-strict`). stdout is empty. | every checkpoint action, `stage-strict` |
| 3 | The exact ref is absent from a validated advertisement. stdout and stderr are empty. | `advertised-ref` only |
| 78 | A malformed advertisement. stdout and stderr are empty. | `advertised-ref` only |
| 79 | A Git command exited 79 (terminal policy status). Passed through unchanged; never reported as a refusal. stdout is empty. | every action that runs Git |
| any | The verifier's own status. The verifier's stdout is on stdout and its stderr on stderr, whatever the status. | `verify-pushed-ref` only |

No failure path maps to 0.

The shell keeps its existing mapping: in `workflow_checkpoint_enter` and the
operation wrapper, a non-zero status other than 79 becomes 78, and 79 stays
79.

At both `destination-is-head` call sites the shell keeps the real status:

```sh
rc=0; DESTINATION_IS_HEAD=$(workflow_checkpoint destination-is-head) || rc=$?; [ "$rc" -eq 0 ] || { reason="publication-destination-inspection-failed"; exit "$rc"; }
```

A terminal 79 from the destination read therefore leaves step 15 with 79 and
no push.

**stdout is written only on success**, except for `verify-pushed-ref`, which
copies the verifier's stdout on every status as the Python did. On every
other non-zero status stdout is empty, so the shell never reads a partial
authority value or NUL record.

**Refusal observations follow admission.** Once an action has loaded and
verified the private authority record, a refusal also appends a `refusal`
observation to `operations.jsonl` in the private directory. Before that
point no observation is written. An error writing the observation is ignored;
it never changes the refusal or its status.

**A panic exits 1**, the status of an uncaught Python exception. `run`
catches it with `catch_unwind`; for `internal workflow-checkpoint` the panic
hook prints one fixed line, `checkpoint internal error: unexpected panic`,
with no source location or payload. This needs the release profile's
`panic = "unwind"`, which the workspace `Cargo.toml` already has.

**Unknown and missing actions.** An action not in the table exits 2 with
`checkpoint scope refusal: unknown private checkpoint operation`. A missing
action name exits 1, as `sys.argv[1]` did.

Only `internal workflow-checkpoint` passes a coded status through `main`.
Every other internal command keeps its own behaviour; for example
`internal validate-recipe-runner` still prints its JSON error on stderr and
exits 1.

## Error mapping

| Site | Status | Reason text |
| --- | --- | --- |
| A malformed push-destination advertisement (`capture`, `admit`, `check`, `before`, `after`, `destination-is-head`, `verify-pushed-ref`) | 2 | `malformed publication advertisement`, `invalid publication destination identity`, `duplicate publication advertisement`, `ambiguous absent publication destination`, or the `'ascii' codec can't decode byte …` text for a non-ASCII object ID |
| A push-destination transport exiting other than 0, 2 or 79 | 2 | `publication destination inspection failed (exit <n>)` |
| A push-destination transport exiting 79 | 79 | `publication destination inspection failed (exit 79)` |
| Any other Git command exiting 79 | 79 | none from the engine |
| Any other Git command failing | 2 | `Git inspection failed (exit <n>)` |
| A Git or helper process that cannot start | 2 | `could not run <program>: <os error>` |
| Reading or writing the index, capture, authority record, operation token, private history, replay directory or an observation | 2 | `<what>: <os error>` |
| `after` with a non-integer status | 2 | `invalid literal for int() with base 10: '<value>'` |
| A missing positional argument | 1 | `checkpoint internal error: IndexError: list index out of range` |

## Actions

### Checkpoint actions

These replace the functions of the former `CHECKPOINT_PY` heredoc. They read
the private authority record named by `WF_CHECKPOINT_RECORD` (except
`selectors` and `capture`) and run Git in the owned task worktree.

The action table in `crates/amplihack-cli/src/workflow_checkpoint/mod.rs`
records, for each action, how it reads its arguments, whether it refuses
inherited Git selectors, what a malformed advertisement means for it and
when it writes stdout, each with the 918c256cf line it keeps. Extra
arguments are ignored unless the Python checked the count.

| Action | Arguments | stdout on success | Purpose |
| --- | --- | --- | --- |
| `selectors` | none | nothing | Refuses inherited Git selectors (see [Environment](#environment)). Runs before any checkpoint work. |
| `capture` | `<phase>`: `publish`, `review` or `cleanup` | capture JSON | Records pending source and the complete outgoing history against the push destination, for the scope decision. Writes nothing to the implementation repository. |
| `admit` | `<phase>` | admission values (eight fixed fields) | Validates the scope decision and writes the private authority record. |
| `check` | none | nothing | Re-validates the authority record, the binding and the current outgoing range. |
| `paths` | none | NUL-terminated pending paths | Lists the pending paths that the authority permits. Never wider than the authority. |
| `before` | `<op> [args...]`, where `<op>` is `commit`, `push`, `pull`, `fetch` or `sync` | operation token (JSON) | Checks the operation before it runs. For `push`, the arguments are the exact push selection and are checked against the allow-list. For cleanup `sync`, the arguments are `workflow_checkpoint_sync_body <branch>`. |
| `sync-fetch-mode` | `<token>` | `fetch` or `skip-absent` | Cleanup only. Says whether the sync must fetch. `skip-absent` only under the first-publication rule. |
| `after` | `<token> <op> <rc>` | operation result (four fields) | Checks the result of the operation the token describes. |
| `destination-is-head` | none | `true` or `false` | Reads the push destination's advertisement and reports whether the task ref is already at `HEAD`. |
| `verify-pushed-ref` | `<path to verify_pushed_ref.sh> <branch> <sha>` | the verifier's stdout | Publish only. Runs the verifier with the bound transport and exits with its status. |

### Replacements for inline Python calls

| Action | Arguments | Input | Output | Exit |
| --- | --- | --- | --- | --- |
| `sha256` | none | stdin, read as bytes | lowercase hex digest and `\n` | 0 |
| `worktree-path` | none | `RECIPE_VAR_worktree_setup` (JSON) | `str(worktree_path)` and `\n` | 0; 1 if the variable is unset, is not JSON, is not an object or has no `worktree_path` |
| `admission-refused-result` | none | `RECIPE_VAR_worktree_setup`, optional | one-line `commit_result` JSON | always 0 |
| `cleanup-skip-result` | `<reason>` | none | one-line skip JSON | 0; 1 without a reason |
| `stage-strict` | `<nul-file>` | the NUL-terminated path list in `<nul-file>` | nothing; Git stages the paths | 0; 2 on refusal; 79 if Git exits 79 |
| `advertised-ref` | `<saved-output-file> <ref> <ls-remote-rc>` | the saved `ls-remote` output | nothing | 0 present, 3 absent, 78 malformed, 1 unreadable file or non-integer status |

None of these six looks at inherited Git selectors; the Python they replace
never did.

#### `sha256`

Replaces `python3 -I -c 'import hashlib,sys; …'` when step 15 computes
`WF_CHECKPOINT_STAGE_HASH`. It does not call `sha256sum`, which macOS lacks.

```sh
$ printf 'abc' | amplihack internal workflow-checkpoint sha256
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
```

#### `worktree-path`

Replaces `python3 -I -c 'import json,os; …["worktree_path"]'` in the four
recipe steps:

```sh
checkpoint_worktree="$("${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint worktree-path)"
```

It reads `RECIPE_VAR_worktree_setup` as Python's `json.loads` did (the last
duplicate key wins) and prints `str()` of the value, so a number prints as
`7` and `null` as `None`. An empty string prints an empty line.

#### `admission-refused-result`

Step 15 prints this as its `commit_result` when scope admission refuses. The
shell then exits with the admission's own status.

```sh
$ RECIPE_VAR_worktree_setup='{"branch_name":"feat/issue-1554"}' \
    amplihack internal workflow-checkpoint admission-refused-result
{"pushed": "false", "sha": "", "branch": "feat/issue-1554", "reason": "scope-admission-refused"}
```

`branch` is `json.dumps` of `branch_name` whatever its type (`1e-05`, `7`,
`null`, a list or an object in document order), or `""` when the variable is
unset, is not valid JSON or is not an object.

#### `cleanup-skip-result`

Step 20b prints this in capture mode when it skips the cleanup push.

```sh
$ amplihack internal workflow-checkpoint cleanup-skip-result 'worktree path gone'
{"skipped": true, "admitted": false, "reason": "worktree path gone"}
```

A non-UTF-8 byte in the reason is written as `\udcXX`, as Python's
`surrogateescape` argv did.

#### `stage-strict`

`stage_workflow_changes --strict --path-list <nul-file>` calls
`stage-strict "<nul-file>"`. The action validates the whole list in memory
before the first index write, then passes the same bytes to
`git --literal-pathspecs add --pathspec-from-file=- --pathspec-file-nul`.
Paths are bytes; they are never converted to UTF-8.

On refusal stderr has `stage_workflow_changes: strict refusal: <reason>`
(preceded by Git's own stderr when Git failed) and the exit status is 2.
The reasons are:

| Reason | Cause |
| --- | --- |
| `path list must be a regular file` | `<nul-file>` is a symlink, directory or special file |
| `unterminated path list` | the last path has no trailing NUL |
| `duplicate path` | a path appears twice |
| `path must be an exact worktree-relative identity` | empty, absolute, or contains an empty, `.` or `..` component |
| `path spelling differs from filesystem identity` | a component exists under another spelling (case-insensitive filesystem) |
| `symlink parent is unsupported` | a parent component is a symlink |
| `non-directory parent` | a parent component exists and is not a directory |
| `directory or special-file selector` | the path names a directory, socket, FIFO or device |
| `missing untracked path` | the path does not exist and is not tracked |
| the operating-system error text | the list could not be opened or read |

An empty list stages nothing and exits 0.

#### `advertised-ref`

`workflow_pr_head_sync.sh --probe-origin-ref` asks whether the effective
fetch origin has the task ref, before first publication. The shell still
runs `ls-remote`, saves its output and passes the file, the ref and the
transport status, in the 918c order:

```sh
"${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint advertised-ref "$probe_output" "refs/heads/$branch" "$probe_rc" || inspection_rc=$?
```

That is `advertised-ref <saved-output-file> <ref> <ls-remote-rc>`. The
action applies the `ORIGIN_REF_PY` rules:

- records are split as Python's `bytes.splitlines()`, so a final record
  without a line feed and a CRLF record are accepted;
- each record is exactly `<oid>\t<name>`, `<name>` is `HEAD` or starts with
  `refs/`, and is matched byte for byte;
- each `<oid>` is 40 or 64 lowercase hex digits on its own, so 40- and
  64-digit IDs may be mixed;
- no name repeats, and transport status 2 with any record is malformed.

| Input | Exit |
| --- | --- |
| valid advertisement containing the exact ref | 0 |
| valid advertisement without the ref, or empty | 3 |
| a malformed record, a bad oid, a duplicate name, or status 2 with records | 78 |
| an unreadable file, a non-integer status or a missing argument | 1 |

3 skips the early sync before first publication, 0 selects the strict fetch
and any other status stops with that status.

## Publication rules (17.116)

The Rust actions keep every rule of decision 17.116. The full requirement is
in the [canonical staging test contract](../../amplifier-bundle/recipes/tests/canonical-staging.md#dated-publication-requirement-decision-17116).

- **Outgoing range.** `before push`, `check` and `destination-is-head` read the
  bound push destination's advertisement at push time. One read per process
  serves all its range checks. Local tracking refs and `FETCH_HEAD` never
  shrink the range. Every outgoing commit is checked against every parent.
- **Every pushed ref.** Only the one task ref selected by the checkpoint is
  allowed. Extra refspecs, push options, mirror or tag expansion, configured
  push refspecs, alternate routing and recursive submodule pushes are refused
  before transport.
- **Failures stop.** A failed or partial fetch or pull, a 79, or an unreadable
  or malformed advertisement stops before any push and reports its real
  non-zero status.
- **First publication.** Cleanup's `sync-fetch-mode` prints `skip-absent` only
  when a validated fetch-origin advertisement shows the exact task ref absent,
  there is no task upstream, and the fetch URL equals the bound push URL. In
  every other case it prints `fetch` or refuses.

The push URL is handled as raw bytes and is never printed. Records hold only
its SHA-256 (`url_sha256`, `push_url_sha256`, `fetch_url_sha256`).

## Environment

| Variable | Read by | Meaning |
| --- | --- | --- |
| `AMPLIHACK_BIN` | the shell wrappers | Binary to run. Defaults to `amplihack` on `PATH`. |
| `WF_CHECKPOINT_RECORD` | checkpoint actions after `admit` | Path of the private authority record. |
| `WF_CHECKPOINT_STAGE_HASH` | `admit`, `check` | Stager identity. Computed by step 15 at run time with `sha256`. |
| `RECIPE_VAR_worktree_setup` | `worktree-path`, `admission-refused-result` | Step 04 output (JSON). |
| `TMPDIR` | checkpoint actions | Must be an absolute, assigned directory outside the worktree. Production keeps the `${TMPDIR:?assigned TMPDIR required}` checks. |

**Inherited Git selectors are refused, never unset.** `selectors`, and every
checkpoint action that runs Git, refuse when any of these is set:
`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`,
`GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_CONFIG`,
`GIT_CONFIG_COUNT`, `GIT_CONFIG_PARAMETERS`, `GIT_NAMESPACE`,
`GIT_SHALLOW_FILE`, `GIT_GRAFT_FILE`, `GIT_REPLACE_REF_BASE`,
`GIT_CEILING_DIRECTORIES`, `GIT_GLOB_PATHSPECS`, `GIT_NOGLOB_PATHSPECS` and
`GIT_ICASE_PATHSPECS`. The refusal reason is
`unsupported inherited Git selector`. The list is the same as the Python helper's and a
unit test pins it. `GIT_EXEC_PATH`, `GIT_SSH*` and `GIT_ASKPASS` are not on
the list on purpose.

`stage-strict` runs Git with the environment unchanged, as the Python it
replaced did. `advertised-ref` runs no Git at all.

All Git calls use separate arguments and `--`; none goes through `sh -c`.
Input to Git is written on a separate thread while stdout and stderr are
read, so large inputs cannot deadlock a recipe step.

## JSON output

Values that are hashed, digested or compared byte for byte (authority records,
receipts, evidence digests) use the same bytes as Python's
`json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)`:

- keys sorted by code point (Python's `str` order);
- non-ASCII characters as `\uXXXX`, with surrogate pairs above U+FFFF;
- the short escapes `\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t`;
- `/` written as it is and DEL (U+007F) as `\u007f`;
- floats as Python's `repr` (`1e-05`, `1e+16`, `100.0`).

```text
{"a":{"b":1,"c":"é"},"z":"😀"}
```

The one-line shell results (`admission-refused-result`,
`cleanup-skip-result`) use Python's default separators (`", "` and `": "`)
and keep keys in insertion order, as shown in [Actions](#actions).

Unit tests pin both forms with byte vectors produced by the former Python
helper.

## Private files

The authority record, retained-history files and `objects/info/alternates`
of the private evidence repository are created with mode `0600`, `O_NOFOLLOW`
and create-new semantics, inside a private `0700` directory under the absolute
`TMPDIR`. A later `chmod` is kept as a second safeguard.

## Authorised changes (17.126)

The port keeps the 918c256cf behaviour everywhere except these two rows,
which decision 17.126 authorises.

| Site | 918c256cf | Now |
| --- | --- | --- |
| Both `destination-is-head` call sites in `workflow_publish_checkpoint.sh` | `\|\| { …; exit 1; }` turned a terminal 79 from the destination read into 1 | the real status is kept, so a terminal 79 exits 79 before any push |
| `stage-strict` | Git exit 79 during `git add` was a strict refusal (2) | 79 is passed through unchanged |

Any other legacy behaviour the port does not keep is listed in the PR for the
owner's disposition; it is not an accepted change.

## Tests

| Location | Covers |
| --- | --- |
| `crates/amplihack-cli/src/workflow_checkpoint/spec_tests.rs` | exit mapping, the action table row by row, advertisement status by row, NUL records, canonical JSON, base64 path identities |
| `crates/amplihack-cli/src/workflow_checkpoint/legacy_parity_tests.rs` | the 918c parser table, the three D1 inputs, Python number, `str()` and `surrogateescape` bytes, the crash status |
| `crates/amplihack-cli/tests/workflow_checkpoint_static_guards.rs` and `workflow_checkpoint_legacy_guards.rs` | absence and structure checks: no `python3` in scope, the `main.rs` exit scope, no `panic = "abort"`, every action has a caller and a dispatch arm, the selector list equals 918c's, the root manifest is unchanged |
| `bins/amplihack/tests/workflow_checkpoint_legacy_parity.rs` | the legacy rows at the binary boundary |
| `bins/amplihack/tests/workflow_checkpoint_exit79.rs` | a terminal 79 from the destination read exits 79 at both sites |
| `bins/amplihack/tests/workflow_checkpoint_wrappers.rs` | the shell wrappers against the compiled binary and disposable Git remotes |

Each ported test has the name of its Python method without the `test_`
prefix. Tests that need the environment, `TMPDIR` or Git run the binary as a
subprocess with a private `HOME`, `TMPDIR` and
`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`.

Run them with the rest of the workspace:

```sh
cargo test --workspace --locked
```

The canonical staging fixtures and their evidence directory are described in
the [canonical staging fixtures README](../../crates/amplihack-workflows/tests/fixtures/canonical_staging/README.md).

## Related

- [Canonical staging test contract](../../amplifier-bundle/recipes/tests/canonical-staging.md) - scope, 17.116 publication rules and delivery requirements
- [Default workflow agentic finalization](default-workflow-agentic-finalization.md) - the step 15 and step 20b results these actions feed
- [Binary resolution](binary-resolution.md) - how tools find the `amplihack` binary
