# Migrate Session Skill (`/amplihack:migrate`)

Move the currently running amplihack CLI session (Copilot, Claude Code, Amplifier)
to a fresh azlin-managed Azure VM and resume it there in a detached tmux. One
command end-to-end.

## Overview

Mid-session host migration used to be a manual ~15-step procedure (bootstrap
toolchain → selective tarball → `bastion cp` → extract → delta sync →
reconstruct project tree → resume). The `/amplihack:migrate <hostname>` skill
collapses that into a single command.

Typical use cases:

- Local host running out of disk / RAM under heavy orchestrator load
- Moving long-running work to a beefier Azure VM
- Handing off the session to an unattended cloud host so the laptop can sleep
- Switching from a constrained dev container to a full Linux VM

## Usage

```text
> /amplihack:migrate ia2
[migrate] Detected active session: b127f92a-eb5c-4a3a-9a21-0e74ca5a6f28 (copilot)
[migrate] Session-state dir: /home/me/.copilot/session-state/b127f92a-… (6 MB)
[migrate] This migration will copy credentials (.ssh, .config/gh/hosts.yml) to ia2.
[migrate] Bootstrapping ia2 (node, npm, gh, uv, copilot, amplihack)… ok
[migrate] Building selective tarball → /tmp/amplihack-migrate-….tar.zst
[migrate] Tarball size: 421 MB
[migrate] Checking destination disk availability…
[migrate] Copying to ia2:/tmp/amplihack-migrate.tar.zst … ok
[migrate] Extracting on ia2 … ok
[migrate] Verifying destination: gh auth + copilot + session-state… ok
[migrate] Final delta sync of session-state … ok
[migrate] Reconstructing project tree on ia2 …
[migrate]   workspace.yaml: cwd=/home/rysweet/src/azork/worktrees/feat/x git_root=/home/rysweet/src/azork
[migrate]   remapped → /home/me/src/azork[/worktrees/feat/x] (under $HOME)
[migrate]   gh repo clone rysweet/azork /home/me/src/azork … ok (branch feat/x)
[migrate]   git worktree add /home/me/src/azork/worktrees/feat/x feat/x … ok
[migrate]   rewrote cwd + git_root in destination workspace.yaml … ok
[migrate]   hard-gate: cwd exists, git checkout on 'feat/x' … ok
[migrate] Starting tmux 'session-b127f92a-…' on ia2: copilot --resume b127f92a-…
[migrate] Migration complete.
✓ Session resumed on ia2 in tmux 'session-b127f92a-…'.
  Attach with: azlin connect -y ia2:session-b127f92a-…
  Resumed in project: /home/me/src/azork/worktrees/feat/x
```

### Options

| Option            | Purpose                                                    |
| ----------------- | ---------------------------------------------------------- |
| `<hostname>`      | Required. Destination azlin VM hostname.                   |
| `--session <id>`  | Override auto-detection; use a specific session id.        |
| `--dry-run`       | Print what would happen; do not transfer, reconstruct, or resume. The dry-run summary includes the planned project reconstruction (clone target path + branch). |

## What Gets Migrated

The tarball contains exactly these paths:

- `~/.config/` (includes `gh/hosts.yml` for GitHub CLI auth)
- `~/.copilot/skills/`
- `~/.amplihack/`
- `~/.simard/`
- `~/.ssh/`
- **Only the active** `~/.copilot/session-state/<id>/` (or analogous for claude)

Exclusions (pruned from the tarball):

- `~/src/*` source trees
- `**/target/`, `**/.venv/`, `**/node_modules/`, `**/__pycache__/`
- `~/.cache/`
- Inactive sessions under `session-state/`
- Files tagged as `CACHEDIR.TAG` by `--exclude-caches-under`

## What Is Not Migrated

- **Source code checkouts** (`~/src/*`) — **reconstructed** on the destination
  via `gh repo clone` / `git worktree add` from the paths recorded in
  `workspace.yaml`, rather than copied in the tarball. See
  [Project reconstruction](#8-project-reconstruction--workspaceyaml-rewrite).
- **Bidirectional sync / live mirroring** — single-shot migration only.
- **Non-azlin destinations** (raw SSH, other clouds) — may come in v2.
- **Uncommitted / unpushed changes** in `~/src/*` — reconstruction clones the
  branch as it exists on the remote; local-only commits or dirty working-tree
  changes that were never pushed are **not** carried over.

## How It Works

### 1. Session detection

Cascade (first hit wins):

1. `--session <id>` flag
2. Env var (`$COPILOT_SESSION_ID`, `$CLAUDE_SESSION_ID`)
3. Newest-mtime directory under the CLI's session-state root
4. Error with clear message if none found

The CLI type is inferred from `$AMPLIHACK_AGENT_BINARY`, falling back to a
parent-process-chain scan.

### 2. Destination bootstrap (idempotent)

`bootstrap-dest.sh` is streamed over `azlin connect` and installs any missing
toolchain: node (>= 18), gh, uv, @github/copilot, amplihack. Already-installed
tools are skipped via `command -v <tool>` guards.

### 3. Selective tarball

`tar --use-compress-program=zstd` with the include/exclude lists above. The
`zstd` codec gives ~3× faster compression than gzip at comparable ratios.

### 4. Pre-flight disk check

The destination must have at least **2× the tarball size** free. If not, the
skill aborts and preserves the local tarball for manual recovery.

### 5. Ship → extract

`azlin cp` ships the tarball; the destination extracts via
`tar --use-compress-program=unzstd -xpf … -C /` and removes the tarball.

### 6. Verification

On the destination:

- `gh auth status` — warns (non-fatal) if not authenticated
- `<cli> --version` — must succeed
- Session-state dir must exist

### 7. Final delta sync

An `rsync -a --delete` over the azlin SSH transport captures events written to
the active session-state during the tarball transfer. This is the second of
the **two-pass sync** design.

Accepted risk: events written during the final ~1–2s rsync window can still be
lost. There is no source-side event suspension.

### 8. Project reconstruction & workspace.yaml rewrite

Because `~/src/*` source trees are **not** included in the tarball (see
[What Is Not Migrated](#what-is-not-migrated)), the resumed session would
otherwise land in `$HOME` with no git context — the working directory and git
root recorded in the session's `workspace.yaml` point at the **source user's**
home (e.g. `/home/rysweet/src/azork/…`), a path that does not exist on the
destination. This phase re-materializes that project tree and rewrites the
persisted paths so resume lands in a valid checkout.

It runs on the destination **after the final delta rsync** (so rsync cannot
clobber the rewritten `workspace.yaml`) and **before the tmux resume** (so the
CLI reads the corrected paths). Steps:

1. **Read** the flat top-level scalars from the active session's
   `workspace.yaml`: `cwd`, `git_root`, `repository`, `host_type`, `branch`.
   Parsing uses `grep`/`sed` only — no `yq` dependency is added.

   **Skip conditions (not an error).** Reconstruction is *skipped* — and resume
   proceeds without the hard-gate — when the session has no reconstructable
   project, i.e. `workspace.yaml` is absent, or `repository`/`git_root` are
   absent, or `host_type` is present but **not** in the allowlist (e.g. a
   non-github remote that v1 cannot clone). In these cases the source session
   legitimately had no clonable git project (or one v1 does not support), so
   landing in `$HOME` is the correct, expected outcome; the skill emits a warning
   and continues. This is distinct from a **malformed** field on a session that
   *does* claim a github repository, which is a hard failure (exit 11).
2. **Validate** the untrusted fields before they reach the shell or filesystem
   (reject-don't-sanitize). Validation applies only once a session is in scope
   for reconstruction (i.e. it claims a `repository` with `host_type: github`):
   - `repository` must match `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`
   - `branch` must match `^[A-Za-z0-9._/-]+$` (no leading `-`, no `..`)
   - `host_type` must be in the allowlist (`github`)
   - An **empty or newline-containing** value for a field that is *present* is
     rejected (exit 11). A *cleanly absent* `repository`/`git_root` triggers the
     skip path above, not exit 11.
3. **Remap** the recorded cross-user path to the destination user's home. The
   destination paths are re-derived entirely from the **validated** `repository`
   and `branch` — the untrusted source path is used only to classify the session,
   never copied verbatim:
   - **Plain (non-worktree) session** → `git_root_dest = cwd_dest =
     $HOME/src/<repo>`.
   - **Worktree session** → `git_root_dest = $HOME/src/<repo>` (the main clone)
     and `cwd_dest = $HOME/src/<repo>/worktrees/<branch>`.

   A session is classified as a **worktree session** by the presence of
   `/worktrees/` in the source `cwd` — **not** by comparing `git_root` to `cwd`.
   amplihack records `git_root` **equal to** `cwd` for worktree sessions (the
   worktree is its own recorded root), so a `git_root != cwd` heuristic would
   misclassify every session. The source `git_root` value is therefore **not
   reused**; `git_root_dest` is always re-derived as the main clone path.

   Deeply/double-nested source worktrees (e.g.
   `…/src/<repo>/worktrees/A/worktrees/B`) are **normalized to a single level**:
   `cwd_dest = $HOME/src/<repo>/worktrees/<branch>`, where `<branch>` is the
   recorded (already length-capped) branch. The worktree directory tail equals
   the full branch name including any `feat/` prefix; branch names containing `/`
   yield nested directories, which `git worktree add` handles.

   The result is normalized and asserted to be strictly under `$HOME` (no `sudo`,
   no `chown`).
4. **Reconstruct** the tree idempotently:
   - `host_type: github` → `gh repo clone <repository> <git_root_dest>`
     (falls back to `git clone https://…` if `gh` clone fails), then
     `git checkout <branch>`.
   - Every external-service call (`gh repo clone`, `git clone`, `git fetch`)
     is wrapped in **bounded retry-with-backoff** (default 3 attempts,
     `2 · 2ⁿ⁻¹`s delay capped at 30s) so a transient network / DNS / TLS /
     rate-limit failure does not hard-fail reconstruction. Tune via the
     `AMPLIHACK_MIGRATE_RETRIES` and `AMPLIHACK_MIGRATE_RETRY_DELAY`
     environment variables; the retry count is exhausted before exit 13.
   - If `<git_root_dest>/.git` already exists, `git fetch` + `git checkout`
     instead of re-cloning.
   - **Worktree** sessions: after the main clone at `git_root_dest`, attempt
     `git worktree add <cwd_dest> <branch>`. The **worktree linkage** is
     best-effort, but `cwd_dest` **must still end up a valid checkout** (the
     hard-gate in step 6 enforces this). Fallback order when `git worktree add`
     fails (e.g. the branch was created locally and never pushed):
     1. Retry after `git fetch origin <branch>`.
     2. Fall back to a standalone `git clone`/`git checkout <branch>` **into**
        `cwd_dest` so the resumed CLI still lands in a real checkout.
     3. Only if the branch cannot be materialized on the remote at all does
        `cwd_dest` remain absent — the hard-gate then aborts with exit 10.

     "Best-effort" therefore refers to the *worktree wiring*, not to whether
     `cwd_dest` exists. An unpushed worktree branch is a genuine, loud failure
     (exit 10) rather than a silent hollow resume — see
     [Uncommitted / unpushed changes](#what-is-not-migrated).
5. **Rewrite** `cwd` **and** `git_root` in the destination `workspace.yaml` to
   the remapped paths. The rewrite is atomic and preserves the original file
   mode; `sed` replacement values are escaped (`&`, `\`).
6. **Hard-gate** before resume (only when reconstruction was **not** skipped per
   the step-1 skip conditions): `cwd_dest` must exist, be a directory, and be a
   git checkout on the recorded `branch`. If any assertion fails the migration
   aborts non-zero (see [Exit Codes](#exit-codes)) rather than resuming into a
   hollow `$HOME`. No `resume-auto-cd … missing or not a directory` warning is
   left behind. For skipped sessions the gate is bypassed and resume proceeds in
   `$HOME` as before.

Values from `workspace.yaml` are passed as **positional arguments** into a
single-quoted remote heredoc (`bash -s --`) so they can never be interpreted as
command text — this defends against shell and git-option injection from the
untrusted YAML.

### 9. Resume in detached tmux

The destination launches `copilot --resume <id>` (or `claude --resume <id>`)
inside a new detached tmux session named `session-<id>`. The skill prints the
`azlin connect -y <host>:<tmux-name>` command for the user to attach.

For the Amplifier CLI, resume is currently TBD — the skill migrates the
filesystem and prints manual-attach instructions instead of failing.

## Failure Modes

| Failure                                              | Handling                                                      |
| ---------------------------------------------------- | ------------------------------------------------------------- |
| `azlin` not installed on source                      | Abort with install hint (source dep check)                    |
| Destination unreachable                              | Surface `azlin connect` stderr; no partial state              |
| `gh auth` fails post-transfer                        | Warn but continue (user can re-auth on destination)           |
| Insufficient disk on destination                     | Abort before ship; preserve local tarball                     |
| Bootstrap (`apt`/`uv`/`npm`) failure                 | Abort; print remediation; tarball not built yet               |
| `tar` fails                                          | Abort; cleanup                                                |
| Delta rsync fails                                    | Warn; do not abort (initial tarball already transferred)      |
| Invalid `workspace.yaml` field (repo/branch present but malformed) | Abort **before** clone with validation error (exit 11)      |
| Session has no repository / non-github `host_type`   | Skip reconstruction (warn); resume in `$HOME` (no hard-gate)   |
| Cross-user path not remappable / escapes `$HOME`     | Abort before clone (exit 12)                                  |
| `gh repo clone` / `git clone` fails                  | Retry with backoff (`AMPLIHACK_MIGRATE_RETRIES`); abort exit 13 only after retries exhausted |
| `git worktree add` fails (branch pushed)             | Fall back to standalone clone/checkout at `cwd_dest`          |
| Worktree branch never pushed to remote               | Hard-gate aborts (exit 10) — no silent hollow resume          |
| Post-clone hard-gate fails (cwd missing / wrong branch) | Abort; refuse hollow resume into `$HOME` (exit 10)         |
| `tmux new-session` already exists on destination     | Treated as success; user attaches to existing session         |

## Exit Codes

Codes `2`–`9` are **pre-existing** (they predate the reconstruction phase);
codes `10`–`13` are added by the project-reconstruction phase and are chosen to
avoid collision with the existing range.

| Code   | Meaning                                                                 |
| ------ | ----------------------------------------------------------------------- |
| `0`    | Success — session reconstructed and resumed on the destination.         |
| `1`    | Destination verification failed a hard prerequisite (`gh` or the CLI binary missing on the destination). |
| `2`    | Invalid invocation — unknown option, missing/extra positional, or malformed destination hostname. |
| `3`    | Required dependency missing on the **source** host (e.g. `azlin`).      |
| `4`    | Active session could not be detected, or an explicit `--session` id was malformed. |
| `5`    | Destination bootstrap failed (`bootstrap-dest.sh` could not install the toolchain). |
| `6`    | Selective `tar` build failed, or no migratable amplihack paths were found. |
| `7`    | Insufficient free disk on the destination (needs ≥ 2× tarball size).    |
| `8`    | Transfer failed — `azlin cp` of the tarball or remote extraction errored. |
| `9`    | Post-transfer destination verification failed (session-state / auth / CLI checks). |
| `10`   | Reconstruction hard-gate failed — `cwd` missing, not a directory, or not a git checkout on the recorded branch. Migration refuses to resume into a hollow `$HOME`. |
| `11`   | `workspace.yaml` field validation failed for a session that **claims** a github repository — `repository`/`branch`/`host_type` failed the regex/allowlist, or a *present* field was empty / contained a newline. A cleanly **absent** `repository`/`git_root`, or a non-github `host_type`, is a **skip** (warn + resume in `$HOME`), not exit 11. |
| `12`   | Cross-user path remap failed — the computed destination path could not be normalized strictly under `$HOME`. |
| `13`   | Project reconstruction failed — `gh repo clone` / `git clone` of `repository` into the remapped `git_root` did not succeed after the retry budget (`AMPLIHACK_MIGRATE_RETRIES`, default 3) was exhausted. |

## Manual Recovery

If the skill aborts mid-way, you can finish manually:

```bash
# 1. ship whatever tarball exists in /tmp/
azlin cp /tmp/amplihack-migrate-*.tar.zst <host>:/tmp/

# 2. extract on destination
azlin connect -y <host> -- tar --use-compress-program=unzstd -xpf /tmp/amplihack-migrate-*.tar.zst -C /

# 3. reconstruct the project tree so resume lands in a real checkout
#    (read cwd/git_root/repository/branch from the session's workspace.yaml)
azlin connect -y <host> -- gh repo clone <owner/repo> "$HOME/src/<repo>"
azlin connect -y <host> -- git -C "$HOME/src/<repo>" checkout <branch>
#    then edit cwd + git_root in
#    ~/.copilot/session-state/<id>/workspace.yaml to the reconstructed path

# 4. start the session manually
azlin connect -y <host>:session-<id> -- copilot --resume <id>
```

## Security

This skill **intentionally** copies credentials (`~/.ssh`, `~/.config/gh/hosts.yml`)
to the destination. A warning is printed before transfer. Only migrate to hosts
you fully trust (e.g., your own azlin-managed VMs). Do not use this skill with
shared or untrusted destinations.

### Untrusted `workspace.yaml` fields

The `cwd`, `git_root`, `repository`, `host_type`, and `branch` fields consumed by
the [project reconstruction phase](#8-project-reconstruction--workspaceyaml-rewrite)
originate in the **source user's** home directory and are treated as untrusted
input flowing into the shell, filesystem paths, and `git`/`gh` argv. The
reconstruction phase applies defense-in-depth:

- **Reject, don't sanitize.** `repository`, `branch`, and `host_type` must pass
  strict regex / allowlist checks; empty fields or embedded newlines are rejected
  (blocks YAML → multiline-shell smuggling).
- **Git option-injection defense.** Leading-`-` values in `branch` / `repository`
  are rejected so they cannot be smuggled as `gh` / `git` flags.
- **Path-traversal defense.** The destination path is normalized and asserted to
  be strictly under `$HOME`; the `src/<repo>[/worktrees/<branch>]` tail is
  re-derived from the validated `repository` + `branch`, never copied verbatim.
- **No injection surface.** Validated values are passed as positional arguments
  into a single-quoted remote heredoc (`bash -s --`), so they are never
  interpreted as command text.
- **No privilege escalation.** Reconstruction always clones under `$HOME`; no
  `sudo` / `chown` is used, even for cross-user path migration.
- **No new credentials.** Reconstruction reuses the already-migrated `gh` auth;
  no token is ever logged.
- **Atomic, mode-preserving rewrite.** The `workspace.yaml` rewrite preserves the
  original file mode and only clones into a fresh or owner-verified `.git` path to
  block symlink / TOCTOU clobbering.

## Scope Boundaries

**In scope (v1)**:

- Copilot migration end-to-end
- Claude Code migration best-effort (resume flag verified at runtime)
- Amplifier migration of filesystem only (resume step TBD)
- azlin-only destinations
- **Project-tree reconstruction** on the destination (clone repo / recreate
  worktree at the persisted path) with cross-user path remapping and a hard
  resume gate

**Not in v1**:

- Copying uncommitted / unpushed `~/src/*` changes (reconstruction clones the
  pushed branch only)
- Bidirectional sync / live mirroring
- Raw SSH / non-azlin destinations
- Source-side cleanup after successful migration

## See Also

- **Remote work skill** — related skill that does **not** copy credentials.
- [azlin](https://github.com/rysweet/azlin) — destination provisioning and transport.
