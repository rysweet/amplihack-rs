# Worktree Source Resolution

**Decides which commit a task worktree starts from, and checks the worktree
after it is created.**

`workflow_worktree_source.sh` is the only place that resolves the source of a
task worktree. It is used by `step-04-setup-worktree` in
`workflow-worktree.yaml` and by `step3-setup-worktree` in
`consensus-issue-worktree.yaml` (issue #1957). The same tool also tells the
later PR steps which base branch was verified, and whether the caller asked for
an explicit source at all.

A caller can name the source with two recipe inputs:

| Input | Meaning |
|---|---|
| `base_branch` | The remote branch to start from, for example `dev`. |
| `base_sha` | The exact commit to start from, as a full 40- or 64-character hex SHA. |

With neither input set, the tool runs the same base resolution the recipes used
before (#858) and says so with a WARNING.

- [Usage](#usage)
- [Precedence when both inputs are set](#precedence-when-both-inputs-are-set)
- [resolve](#resolve)
- [verify-worktree](#verify-worktree)
- [verified-base](#verified-base)
- [requested-inputs](#requested-inputs)
- [How the PR tools combine the two](#how-the-pr-tools-combine-the-two)
- [Exit codes](#exit-codes)
- [Output keys](#output-keys)
- [Order of checks](#order-of-checks)
- [When the fetch fails](#when-the-fetch-fails)
- [The legacy path](#the-legacy-path)
- [ERROR messages](#error-messages)
- [How the recipes call it](#how-the-recipes-call-it)
- [Strict fetch in workflow_fetch_origin.sh](#strict-fetch-in-workflow_fetch_originsh)
- [Security](#security)
- [Versioning](#versioning)
- [Testing](#testing)

---

## Usage

The tool is executed, never sourced. It uses `set -u`, never `eval`, and passes
`git -C <path>` on every git call, so it does not depend on the current
directory.

```bash
workflow_worktree_source.sh resolve --recipe default|consensus --repo <abs-path> \
    [--base-branch <branch>] [--base-sha <sha>]

workflow_worktree_source.sh verify-worktree <worktree-path> <sha> created|reused \
    <main-repo-path> <origin-fingerprint> [--branch <branch>] [--cleanup-created]

workflow_worktree_source.sh verified-base

workflow_worktree_source.sh requested-inputs
```

stdout carries only machine output. Every `INFO:`, `WARNING:`, `RULE:` and
`ERROR:` line goes to stderr.

`verify-worktree` has three uses: `created --cleanup-created` for a new branch
and worktree, `reused --cleanup-created` for a worktree attached in this run to
a branch that already existed (on failure the worktree is removed and the
branch is kept), and `reused` without the flag for a branch and worktree that
both existed.

### Example: start a workflow from `dev`

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix flaky reconnect in the session store" \
  -c repo_path="$(pwd)" \
  -c base_branch=dev
```

### Example: start from an exact commit, without a branch

```bash
amplihack recipe run default-workflow \
  -c task_description="Reproduce the reconnect regression at the release commit" \
  -c repo_path="$(pwd)" \
  -c base_sha=3f9c2a7e1b0d4c5a6e7f8091a2b3c4d5e6f70812
```

The worktree starts at exactly that commit. The run cannot publish a PR: a SHA
pins the tree but does not say which branch the PR should target, so the PR
tools refuse (see [How the PR tools combine the two](#how-the-pr-tools-combine-the-two)).
Use this form for investigation, bisecting or local verification. Add
`base_branch` when the run must open a PR.

### Example: start from an exact commit on `dev`

```bash
amplihack recipe run default-workflow \
  -c task_description="Bisect the reconnect regression" \
  -c repo_path="$(pwd)" \
  -c base_branch=dev \
  -c base_sha=3f9c2a7e1b0d4c5a6e7f8091a2b3c4d5e6f70812
```

The same two inputs work for `consensus-workflow`.

---

## Precedence when both inputs are set

| Inputs | Worktree starts at | `base_ref` | `base_branch` | `base_sha` |
|---|---|---|---|---|
| `base_branch` only | the tip of `origin/<b>` after the fetch | `origin/<b>` | `<b>` | the tip's full SHA |
| `base_sha` only | the commit `<sha>` | `<sha>` | empty | `<sha>` |
| both | the commit `<sha>`, **not** the branch tip | `origin/<b>` | `<b>` | `<sha>` |
| neither | the legacy base (see [The legacy path](#the-legacy-path)) | the legacy ref | the legacy branch | empty |

When both are set, `base_sha` decides the commit and `base_branch` must contain
it: `git merge-base --is-ancestor <sha> refs/remotes/origin/<b>` must succeed
after the fetch. This lets a caller pin a commit on `dev` that is older than its
tip, and still have the PR opened against `dev`.

With `base_sha` only, the worktree is pinned but no PR base is known. A SHA
proves which tree the run starts from; it does not prove which branch the
commit came from, so the tool never guesses one. `verified-base` exits 3 and
the PR tools refuse, because `requested-inputs` reports that an input was
given.

---

## resolve

```bash
workflow_worktree_source.sh resolve --recipe default --repo /home/ryan/src/amplihack-rs \
    --base-branch dev
```

stdout on success, exactly six lines in this order:

```text
base_ref=origin/dev
base_branch=dev
base_sha=5d1e0c9b8a7f6e5d4c3b2a1908f7e6d5c4b3a291
source_verified=verified
source_rule=fetched
origin_fingerprint=8e2f0a4c6b1d3e5f7a9b0c2d4e6f8a1b3c5d7e9f
```

stderr gets exactly one `RULE:` line, for example `RULE: fetched`.

### Arguments

| Flag | Required | Notes |
|---|---|---|
| `--recipe default\|consensus` | yes | Picks the legacy chain used when no input is set. |
| `--repo <abs-path>` | yes | The main repository. Must be an absolute path and, after resolving symlinks, equal `git rev-parse --show-toplevel`. |
| `--base-branch <b>` | no | Empty or whitespace only means unset. |
| `--base-sha <s>` | no | Empty or whitespace only means unset. Lower-cased before use. |

A repeated flag, an unknown flag, a missing value, a value that starts with
`-`, or a relative `--repo` is a usage error and exits 2.

### Input rules

`base_branch` is accepted only when all of these hold:

- it is at most 255 bytes;
- it matches `^[A-Za-z0-9._/+-]+$`;
- it does not start with `-` or `refs/`;
- `git check-ref-format --branch <b>` succeeds **and** prints exactly `<b>`.
  The last check refuses `@{-1}` and similar forms that git would expand to a
  different branch.

`base_sha` is accepted only when it matches `^([0-9a-f]{40}|[0-9a-f]{64})$`
after lower-casing. Abbreviated SHAs are refused, because an exact commit should
never depend on how an abbreviation resolves.

A refused value is echoed in the ERROR as its first 64 bytes, with every byte
outside the allow-list replaced by `?`. A crafted input therefore cannot write a
fake `RULE:` or `ERROR:` line, or send control bytes to the terminal.

Paths (`--repo`, the worktree, the main repository, `show-toplevel` output) are
printed in full, with every control byte replaced by `?`: 0x00 to 0x1F
(including newline and carriage return), DEL, and 0x80 to 0x9F, which some
terminals act on. The replacement is bytewise, so a non-ASCII letter whose
UTF-8 form uses a byte in 0x80 to 0x9F is shown partly as `?`. A path that contains a newline therefore cannot add a
second `ERROR:` line either.

### What resolve never does on the explicit path

- read a symbolic `origin/HEAD`;
- run `git remote set-head` or `git update-ref`;
- fetch a bare SHA from the server (that depends on server settings);
- use a cached ref or a locally present object when the fetch fails;
- print anything on stdout before every check has passed.

---

## verify-worktree

Runs after the recipe creates or reattaches the worktree, and only when
`source_verified` is not `legacy`.

```bash
workflow_worktree_source.sh verify-worktree \
    /home/ryan/src/amplihack-rs/worktrees/feat/issue-1957-pin-base \
    5d1e0c9b8a7f6e5d4c3b2a1908f7e6d5c4b3a291 created \
    /home/ryan/src/amplihack-rs \
    8e2f0a4c6b1d3e5f7a9b0c2d4e6f8a1b3c5d7e9f \
    --branch feat/issue-1957-pin-base
```

Checks run in this order and stop at the first failure:

1. The physical `git rev-parse --show-toplevel` of the worktree equals the
   worktree path.
2. Its `git rev-parse --git-common-dir` equals the main repository's.
3. The origin fingerprint is unchanged. The ERROR does not name the URL.
4. `created`: `HEAD` equals the SHA exactly.
   `reused`: the SHA is an ancestor of `HEAD` (the worktree kept earlier work,
   #1565, and must still contain the verified base).

The worktree path is compared after resolving symlinks, so a worktree directory
reached through a symlink still passes check 1.

It prints nothing on stdout and no `RULE:` line on stderr. On success stderr
is empty; on failure it prints exactly one `ERROR:` line and exits 1.

Without `--cleanup-created` it does no cleanup. With it, and with `--branch`
set, a failure first removes the worktree (`git worktree remove --force`) and,
in `created` mode only, deletes the branch; each removal prints one `INFO:`
line (or a `WARNING:` if it fails) before the final `ERROR:` line.

Cleanup runs only when checks 1 and 2 passed, that is, when the path is
confirmed to be a worktree of this repository at its own top. If either check
failed, nothing is removed, even with `--cleanup-created`, and the tool prints
`WARNING: not removing <wt>: it was not confirmed as this run's worktree (issue
#1957).` before the `ERROR:` line. The recipe
decides when to pass the flag (see [How the recipes call it](#how-the-recipes-call-it)).

---

## verified-base

Used by the later PR steps to find the base branch that step 4 verified.

```bash
workflow_worktree_source.sh verified-base
# Output: dev
```

It takes no arguments; any argument is a usage error (exit 2).

1. It reads `worktree_setup.source_verified` and `worktree_setup.base_branch`
   from the recipe context with `recipe_context_value.sh`.
2. Only when **both** context reads exit 3 (key absent) does it read
   `WORKTREE_SETUP_SOURCE_VERIFIED` and `WORKTREE_SETUP_BASE_BRANCH`, and it
   uses them only as a pair. It never takes one value from the context and the
   other from the environment.
3. It trusts only the literal value `verified`, compared case-sensitively.
4. It runs the full `base_branch` input check again on the value.
5. It checks that `refs/remotes/origin/<b>` resolves in the current repository.

| Exit | stdout | stderr | When |
|---|---|---|---|
| 0 | `<b>` | empty | `source_verified` is exactly `verified`, `<b>` passes the input check, and `refs/remotes/origin/<b>` resolves. |
| 1 | empty | one `ERROR:` line | `source_verified` is `verified` but `<b>` fails the input check or the ref does not resolve; or `recipe_context_value.sh` exits 1 or 2, or is missing from the bundle (the context cannot be read). |
| 2 | empty | usage line | Any argument was given. |
| 3 | empty | empty | No verified base: `source_verified` is `legacy`, `not-applicable`, any other value (including `VERIFIED` or a value from an older draft), absent, or there is no context; or `source_verified` is `verified` with an empty `base_branch` (a SHA-only run). |

Exit 1 means "a verified base was recorded but cannot be used", and the caller
must stop. Exit 3 means "nothing was verified", and the caller must ask
[requested-inputs](#requested-inputs) whether that is allowed.

Example ERROR lines:

```text
ERROR: verified base_branch 'dev' has no refs/remotes/origin/dev in /home/ryan/src/amplihack-rs/worktrees/feat/issue-1957-pin-base (issue #1957).
ERROR: cannot read worktree_setup from the recipe context (recipe_context_value.sh exit 2) (issue #1957).
```

---

## requested-inputs

Tells the PR tools whether the caller asked for an explicit source.

```bash
workflow_worktree_source.sh requested-inputs; echo "exit $?"
# Output: exit 0
```

It takes no arguments; any argument is a usage error (exit 2). It first reads
`worktree_setup.source_verified`. When that is exactly `not-applicable`, step 4
took the `existing_branch` or `pr_number` path and ignored `base_branch` and
`base_sha` (it says so in an `INFO:` line), so the inputs count as not
requested: it prints one `INFO:` line on stderr and exits 3, and the PR tools
use the legacy chain. Otherwise it reads the top-level `base_branch` and
`base_sha` with `recipe_context_value.sh <key> -`. It prints nothing on stdout,
so the values never reach a log.

An absent `worktree_setup.source_verified` does not count as `not-applicable`:
the input check still runs, so a lost `worktree_setup` with inputs set is
refused rather than falling back.

| Exit | When |
|---|---|
| 0 | `source_verified` is not `not-applicable`, and `base_branch` or `base_sha` is non-empty. |
| 1 | `recipe_context_value.sh` exits 1 or 2 for `worktree_setup.source_verified` or either key, or is missing from the bundle. One `ERROR:` line. A context that cannot be read is never taken as "not requested". |
| 2 | Any argument was given. |
| 3 | `source_verified` is exactly `not-applicable`, or both keys are absent or empty. |

---

## How the PR tools combine the two

| Tool | Function | Uses on exit 0 |
|---|---|---|
| `workflow_publish_pr.sh` | `resolve_pr_base_ref` | `origin/<b>` |
| `workflow_pr_ready.sh` | `resolve_expected_base_branch` | `<b>` |
| `workflow_final_status.sh` | `resolve_expected_base_branch` | `<b>` |

All three scripts run under `set -euo pipefail`, so a bare
`vb=$("$tool" verified-base); vb_rc=$?` would stop the script on exit 1 or 3
before `vb_rc` is set, and a run with no inputs would never reach the legacy
chain. Each call is instead written as `rc=0; cmd || rc=$?`, which captures any
exit status without tripping `set -e`. Neither call is wrapped in
`2>/dev/null` or `|| true`, so a refusal is never hidden:

```bash
vb_rc=0
vb=$("$tool" verified-base) || vb_rc=$?
case "$vb_rc" in
  0) base="$vb" ;;                         # verified: use it
  3) ri_rc=0
     "$tool" requested-inputs || ri_rc=$?
     case "$ri_rc" in
       3) base="$(legacy_chain)" ;;         # nothing asked for: old behaviour
       0) echo "ERROR: base_branch/base_sha given but no verified base in worktree_setup; refusing to fall back (issue #1957)." >&2
          return 2 ;;
       *) return 2 ;;
     esac ;;
  *) return 2 ;;
esac
```

A refusal returns 2. The legacy chain keeps its own `return 1` when no
supported remote ref resolves, so a caller can tell "refused" (2) from
"legacy found nothing" (1).

| `verified-base` | `requested-inputs` | Result |
|---|---|---|
| 0 | not run | Use the verified branch. |
| 1 or 2 | not run | Refuse. |
| 3 | 3 | Legacy chain (`origin/HEAD`, then the fallbacks), unchanged. |
| 3 | 0 | Refuse: inputs were given but nothing was verified. |
| 3 (`not-applicable`) | 3 | Legacy chain: step 4 ignored the inputs on the existing-branch path. |
| 3 | 1 or 2 | Refuse: the context cannot be read. |

The call sites capture the function's status the same way, and lose their
old `|| true` and `2>/dev/null`:

```bash
expected_base_rc=0
expected_base="$(resolve_expected_base_branch)" || expected_base_rc=$?
```

This form is used in `workflow_pr_ready.sh` and at both call sites in
`workflow_final_status.sh`, where the status variable is declared `local`
because both sites are inside functions. `workflow_publish_pr.sh` routes a refusal through
`finish_publish`, like its other input failures, so the run still records a
publish result before `gh` runs:

```bash
BASE_REF="$(resolve_pr_base_ref)" || finish_publish "FAILED_INVALID_INPUT" "invalid-input" "failure" "expected base branch was refused or could not be resolved (issue #1957)" 1
```

The resolver's own `ERROR:` line is already on stderr.

In the branch PR lookup of `workflow_final_status.sh` the variable is
`base_rc`.

When the resolver does not return 0:

- `workflow_pr_ready.sh` exits non-zero and makes no PR change.
- `workflow_final_status.sh`, in its PR validation, on a refusal (2) writes an
  `ERROR:` line, skips the #1268 artifact adjudication and the outstanding-PR
  report (no `gh` call), and ends with `Workflow final status failed` and a
  non-zero exit. Any other failure keeps its old meaning: the scope check
  misses and the artifacts are adjudicated.
- `workflow_final_status.sh`, in its branch PR lookup (#1596), on a refusal
  (2) skips the lookup with an `INFO:` line and fails the run before the
  success line, with `terminal_success=false` and
  `terminal_state=FAILED_BASE_NOT_VERIFIED`. Exit 1 (the legacy chain found
  no ref) still runs the lookup without a base, as before.

An empty base is never replaced by a default branch.

This closes two silent fallbacks:

- a worktree built on `dev` opening its PR against the branch `origin/HEAD`
  points at;
- a run that asked for `base_branch` or `base_sha`, but lost its
  `worktree_setup` (or ran SHA-only), quietly using the legacy chain.

Runs that never set either input see exit 3 twice and behave as before.

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. For `resolve`, the six output lines were printed. |
| 1 | Refused. Exactly one `ERROR:` line, and it is always the last line on stderr. For `resolve`, nothing was created. `verify-worktree` runs after the worktree exists; what the recipe then removes is in the [cleanup table](#how-the-recipes-call-it). |
| 2 | Usage error: unknown subcommand, unknown or repeated flag, missing value, or any argument to `verified-base` or `requested-inputs`. |
| 3 | `verified-base`: no verified base. `requested-inputs`: no input was given, or step 4 recorded `not-applicable`. Never used by `resolve` or `verify-worktree`. |

---

## Output keys

`resolve` prints `key=value` lines in this fixed order:

| Key | Values |
|---|---|
| `base_ref` | `origin/<b>`, a full SHA, or the legacy ref. |
| `base_branch` | `<b>`, or empty with `base_sha` only. On the legacy path, the legacy ref without `origin/`. |
| `base_sha` | The full SHA the worktree must start at. Empty on the legacy path. |
| `source_verified` | `verified` or `legacy`. |
| `source_rule` | `fetched` or `legacy-unverified`. |
| `origin_fingerprint` | `git hash-object --stdin` of the origin URL. Empty on the legacy path. Passed to `verify-worktree` only; never written to the JSON. |

The recipes add one more value of their own: on the `existing_branch` and
`pr_number` paths the JSON has `source_verified: "not-applicable"` and
`source_rule: "existing-branch"`. The full set of `source_verified` values in
the JSON is therefore `verified`, `legacy` and `not-applicable`.

On a refusal stdout is empty and the `RULE:` line on stderr names the refusal:

| `RULE:` value | Cause |
|---|---|
| `refuse-invalid-input` | `base_branch` or `base_sha` failed the input rules. |
| `refuse-origin-identity` | No `origin` remote, or `--repo` is not the repository top. |
| `refuse-fetch-failed` | An input was set and the strict fetch failed for any reason other than a missing branch. |
| `refuse-missing-branch` | The branch fetch failed and `git ls-remote --exit-code` reports no `refs/heads/<b>` on origin, or `refs/remotes/origin/<b>` does not exist after the fetch. |
| `refuse-missing-sha` | `base_sha` is not in the object database. |
| `refuse-not-commit` | `base_sha` names a tag, tree or blob: `rev-parse --verify <sha>^{commit}` does not equal `<sha>`. |
| `refuse-not-contained` | `base_sha` is not an ancestor of `origin/<b>`. |
| `refuse-no-base-ref` | Legacy path only: no ref in the legacy chain resolves, or (consensus) there is no `origin` remote. The message is the #858 one. |

---

## Order of checks

On the explicit path every check runs before the deconflict helper and before
any branch or worktree exists. A refusal leaves `git worktree list` and
`git branch --list` unchanged. (The stale-worktree sweep at the top of the step
runs earlier and is not undone.)

1. **Validate the inputs** (no side effects).
2. **Identity check before the fetch.** `origin` exists, and the physical
   `--repo` equals `git rev-parse --show-toplevel`. The origin URL is piped
   straight into `git hash-object --stdin` and never printed.
3. **Strict fetch.** One `workflow_fetch_origin.sh --strict` call. With a
   branch it fetches only `+refs/heads/<b>:refs/remotes/origin/<b>`, so a
   narrowed `remote.origin.fetch` cannot leave `origin/<b>` stale; with a SHA
   alone it is a plain fetch. Strict mode is one attempt, so a run makes one
   `git fetch` call and never sleeps. A refspec fetch also fails when the
   branch is not on origin, so after a failed branch fetch the tool runs
   `git ls-remote --exit-code origin refs/heads/<b>` only to name the reason:
   exit 2 (origin answered and has no such branch) gives
   `refuse-missing-branch`; any other result, including a fetch that failed
   while the branch exists, gives `refuse-fetch-failed` (see
   [When the fetch fails](#when-the-fetch-fails)). Both refuse, and a cached
   `origin/<b>` is never used.
4. **Result checks**, only after a fetch that exited 0. The branch tip exists.
   For a SHA, `git rev-parse --verify --quiet <sha>^{commit}` must print
   exactly `<sha>`. When it does not, `git cat-file -e <sha>` decides the
   rule: the object is missing (`refuse-missing-sha`) or it is a tag, tree or
   blob (`refuse-not-commit`). Only when `base_branch` is also set,
   `git merge-base --is-ancestor <sha> refs/remotes/origin/<b>` must succeed
   (else `refuse-not-contained`).

After the recipe creates the worktree, `verify-worktree` runs the checks in
[verify-worktree](#verify-worktree).

---

## When the fetch fails

A failure means the strict `git fetch` exited non-zero. Strict mode makes one
attempt with no retry and no sleep. The only other signal read is the
`git ls-remote --exit-code` check described in step 3 of the resolve order,
which picks `refuse-missing-branch` over `refuse-fetch-failed` when origin
answers that the branch does not exist.

| Inputs | Fetch failed | Result | Exit | `source_verified` | `source_rule` |
|---|---|---|---|---|---|
| branch only | yes | Refuse | 1 | — | `refuse-fetch-failed` |
| branch + sha | yes | Refuse | 1 | — | `refuse-fetch-failed` |
| sha only | yes | Refuse | 1 | — | `refuse-fetch-failed` |
| any | no | Normal checks | 0 | `verified` | `fetched` |

Cached refs and locally present objects never stand in for a fresh fetch. A
stale clone can hold the right commit while pointing at the wrong origin, and
without a fetch that cannot be ruled out. Even a `base_sha` that exists locally
is refused when origin cannot be reached.

No input allows an offline run from a pinned SHA. Restore access to origin, or
run without `base_branch` and `base_sha`; the legacy path warns that it is
unverified.

---

## The legacy path

With neither input set, `resolve` runs the chain each recipe had before #1957,
with the same retry behaviour and the same #858 messages:

| `--recipe` | Order tried |
|---|---|
| `default` | Git-verified `origin/HEAD` (after `git remote set-head origin -a` if needed), `origin/master`, `origin/develop` |
| `consensus` | Git-verified `origin/HEAD` (same), `origin/main`, `origin/master`, `origin/develop` |

It adds one stderr line:

```text
WARNING: no base_branch/base_sha given; legacy base resolution (<ref>), source not verified
```

`<ref>` is the ref the chain actually resolved, for example `origin/master`.
stderr also gets exactly one `RULE: legacy-unverified` line. stdout has
`source_verified=legacy`, `source_rule=legacy-unverified`, an empty `base_sha`
and an empty `origin_fingerprint`. The recipes skip `verify-worktree` on this
path.

The legacy `base_branch` value is reported in the JSON but never used later:
`verified-base` exits 3 when `source_verified` is `legacy`, `requested-inputs`
also exits 3, and the later PR steps keep their existing base chain. The PR
base does not change.

---

## ERROR messages

Every ERROR names the repository path, the requested and actual ref, and the
requested and actual SHA where they apply. None names the origin URL, because a
URL can contain a token.

| Check | Message |
|---|---|
| Branch input | `ERROR: base_branch '<shown>' is not a valid branch name (issue #1957).` |
| SHA input | `ERROR: base_sha '<shown>' is not a full 40- or 64-character hex SHA; abbreviated SHAs are refused (issue #1957).` |
| No origin | `ERROR: no 'origin' remote configured in <repo>; refusing to resolve a worktree source (issue #1957).` |
| Wrong repo | `ERROR: --repo <repo> is not the repository top (show-toplevel: <top>) (issue #1957).` |
| Fetch failed | `ERROR: git fetch origin failed in <repo> (requested base_branch '<shown b>', base_sha '<shown s>'); cached refs cannot prove freshness, so no worktree is created (issue #1957).` |
| Branch missing | `ERROR: base_branch '<b>' not found on origin in <repo> (refs/remotes/origin/<b> missing after fetch) (issue #1957).` |
| SHA missing | `ERROR: base_sha <s> not found in <repo> (actual: none) (issue #1957).` |
| Not a commit | `ERROR: base_sha <s> is not a commit in <repo> (resolves to <actual>) (issue #1957).` |
| Not contained | `ERROR: base_sha <s> is not contained in origin/<b> (tip <actual>) in <repo> (issue #1957).` |
| Wrong toplevel | `ERROR: worktree <wt> has toplevel <actual>, expected <wt> (issue #1957).` |
| Wrong common dir | `ERROR: worktree <wt> does not share the git common dir of <repo> (issue #1957).` |
| URL changed | `ERROR: origin URL changed during setup in <repo> (issue #1957).` |
| Created HEAD | `ERROR: new worktree <wt> HEAD is <actual>, expected base_sha <s> (issue #1957).` |
| Reused HEAD | `ERROR: reused worktree <wt> HEAD <actual> does not contain verified base <s>; delete branch <b> or rerun with existing_branch (issue #1957).` |
| Verified ref missing (`verified-base`) | `ERROR: verified base_branch '<shown b>' has no refs/remotes/origin/<shown b> in <repo> (issue #1957).` |
| Verified name invalid (`verified-base`) | `ERROR: verified base_branch '<shown b>' is not a valid branch name (issue #1957).` |
| Context unreadable (`verified-base`, `requested-inputs`) | `ERROR: cannot read <key> from the recipe context (recipe_context_value.sh exit <n>) (issue #1957).` |
| Inputs without proof (PR tools) | `ERROR: base_branch/base_sha given but no verified base in worktree_setup; refusing to fall back (issue #1957).` |

`<shown b>` and `<shown s>` are the values passed through the same `show`
filter as refused inputs: at most 64 bytes, and every byte outside the
allow-list becomes `?`. A branch value such as `x` + newline + `ERROR: fake`
therefore produces one ERROR line, not two.

The recipes add one message of their own, for an old bundle:

```text
ERROR: base_branch/base_sha given but workflow_worktree_source.sh is missing from <tools-dir>; refusing an unverified worktree source (issue #1957).
```

---

## How the recipes call it

Both step bodies capture the inputs on their first two lines, before the
recipe's own local `BASE_BRANCH` variable can overwrite them:

```bash
REQ_BASE_BRANCH="${BASE_BRANCH:-}"
REQ_BASE_SHA="${BASE_SHA:-}"
```

They read `resolve` output with a `case` loop that ignores unknown keys, and
never with `eval` or `declare`. Consensus step3 assigns each key in its own arm:

```bash
while IFS='=' read -r k v; do
  case "$k" in
    base_ref) BASE_REF="$v" ;;
    base_branch) BASE_BRANCH="$v" ;;
    base_sha) BASE_SHA="$v" ;;
    source_verified) SOURCE_VERIFIED="$v" ;;
    source_rule) SOURCE_RULE="$v" ;;
    origin_fingerprint) ORIGIN_FINGERPRINT="$v" ;;
  esac
done <<< "$SRC_OUT"
```

Step-04 has no spare lines under the brick limit, so it matches the same six
keys in one arm and assigns the upper-cased name (`base_ref` → `BASE_REF`) with
`printf -v`. Only those six fixed names can be assigned.

The worktree is created from `${BASE_SHA:-$BASE_REF}`, so with both inputs it
starts at the pinned commit, not the branch tip.

When the tool is missing from the tools directory:

| Inputs set | Behaviour |
|---|---|
| yes | The old-bundle ERROR above; exit 1. |
| no | A minimal inline legacy fallback runs, so older bundles (the #1808 M1–M4 tests) still work. |

When `verify-worktree` fails, only what this run made is removed. The recipe
passes `--cleanup-created` in states 2 and 3 and leaves the removal to the tool:

| Step-04 state | Checked as | On failure |
|---|---|---|
| 3 — new branch and worktree | `created` | Removes the worktree and deletes the branch. |
| 2 — existing branch, worktree attached in this run | `reused` | Removes only the worktree; the branch stays. |
| 1 — existing branch and worktree | `reused` | Touches nothing. |

Cleanup runs only when both the worktree path and branch name are non-empty.
`git branch -D` never runs on a branch that existed before the run.

A rerun whose existing branch is clean but behind the verified SHA is refused,
not fast-forwarded. Delete the branch, or rerun with `existing_branch`.

---

## Strict fetch in workflow_fetch_origin.sh

`workflow_fetch_origin.sh` takes `[--strict] [refspec…]`:

```bash
workflow_fetch_origin.sh --strict +refs/heads/dev:refs/remotes/origin/dev; echo "exit $?"
# Output (origin reachable): exit 0
```

**Strict mode** (`--strict`):

- runs `git fetch origin --no-tags [-- <refspec>…]` exactly once, with no retry
  and no sleep;
- exits with git's own status;
- on failure prints one line, with only the status in it:

  ```text
  WARNING: git fetch origin failed (exit 128).
  ```

**Legacy mode** (no `--strict`):

- keeps the existing loop: three attempts, waiting 1 s and then 2 s;
- reads the exit status from `git fetch` itself, so the number in each WARNING
  is git's real status;
- always exits 0, with the same command and messages as before.

**Both modes:**

- each refspec must match `^\+refs/heads/[^:]+:refs/remotes/origin/[^:]+$`
  and is passed after `--`;
- a refspec that does not match, or any other argument, exits 2;
- `GIT_TRACE`, `GIT_TRACE_PACKET`, `GIT_TRACE_CURL` and `GIT_CURL_VERBOSE` are
  unset before fetching.

---

## Security

- **Wrong start commit.** Explicit-refspec strict fetch, exact SHA equality,
  tag-object refusal, the containment check and the post-creation check.
  `origin/HEAD` is never read on the explicit path.
- **Argument injection.** The branch allow-list, the bans on a leading `-` and
  `refs/`, the `check-ref-format` output comparison, full-length SHAs only, `--`
  before refspecs, and every expansion quoted.
- **URL leaks.** The origin URL is only ever piped into `git hash-object`. No
  ERROR, JSON field or file contains it, there is no `set -x`, and the
  `GIT_TRACE*` variables are unset. git's own fetch stderr is passed through
  unchanged, as before.
- **Fail closed.** An explicit input with no proof stops the run: a failed
  fetch refuses, `verified-base` trusts only the literal `verified`, and
  `requested-inputs` exits 1, never 3, when the context cannot be read.
- **Parent-run environment.** `verified-base` reads context first, uses the
  environment only as a pair, accepts one exact value and rechecks the branch
  name and the ref. An environment value of `verified` gets no shortcut.
- **Reserved names.** The branch check also refuses `@` and `HEAD`, which git
  would resolve to something other than a branch on origin.
- **Damage on refusal.** Every refusal happens before a branch or worktree is
  created; cleanup is limited to what the run created.
- **Output injection.** No `eval`; a `case` read loop; JSON values are escaped.
  Output is printed once, after all checks pass.
- **Downgrade.** An old bundle with inputs set fails closed. `STEP_TOOLS`
  lists the tool, so a checkout missing it is not chosen.

The tool fetches only `refs/heads/<b>` and adds no network endpoint, `gh` or
`curl` call, or file outside the repository and worktree.

`jesc` in `consensus-issue-worktree.yaml` escapes only `\` and `"`. That is
enough because every value it escapes has already passed the allow-lists
above, or is a path or SHA produced by git, none of which contain control
bytes.

---

## Versioning

Output keys and their order, enum values and exit codes are a contract. Keys and
enum values are only ever added. A change that removes or renames one needs a
new subcommand name. Callers ignore keys they do not know.

---

## Testing

```bash
bash amplifier-bundle/recipes/tests/test-issue-1957-explicit-worktree-source.sh
cargo test -p amplihack-cli --test cli_integration issue_1957_explicit_worktree_source::
```

The shell test runs in a scratch `HOME` with `GIT_CONFIG_GLOBAL=/dev/null`,
`GIT_CONFIG_NOSYSTEM=1`, `GIT_TERMINAL_PROMPT=0` and `env -i`, and refuses any
origin outside its scratch directory. It runs the real step bodies of both
recipes, extracted with awk, and covers:

| Case | Setup | Expected |
|---|---|---|
| a | `git clone --local`, then origin `set-url` to a second bare repo that has moved ahead | Worktree `HEAD` equals the new remote's `dev`. |
| b | `origin/HEAD` points at an unrelated feature branch | Worktree `HEAD` equals the remote's `dev`. |
| c | `remote.origin.fetch` narrowed to one unrelated branch | `refs/remotes/origin/dev` equals the remote's `dev`. |
| d | origin points at a missing path | Every input combination refuses with `refuse-fetch-failed`, including a `base_sha` present locally. |
| e | mismatched, missing and abbreviated `base_sha` | Non-zero exit, ERROR naming the SHAs, worktree and branch lists unchanged. |
| f | `base_sha` alone; both inputs, contained and not contained | Success, success at the SHA, refusal. |
| g | no inputs | Legacy `base_ref`, the WARNING, `source_verified=legacy`. |
| h | `existing_branch` plus base inputs | `source_verified=not-applicable`. |
| B | `requested-inputs` with `source_verified=not-applicable` and `base_branch`/`base_sha` set | Exit 3 with one `INFO:` line; `legacy`, `verified`, near-miss spellings and an absent field still exit 0. |
| P | all three real PR tools with `existing_branch`, `base_branch` set and `source_verified=not-applicable` | Legacy chain, no D3 refusal; publish is not `FAILED_INVALID_INPUT` and final status is not `FAILED_BASE_NOT_VERIFIED`. |
| B | `verified-base` with `legacy`, `not-applicable`, an unknown value, `verified` with a missing ref | Exit 3, 3, 3, and 1 with one ERROR line. |
| B | inputs given and no `worktree_setup`; a SHA-only run | All three real PR tools, run under their own `set -euo pipefail`, refuse with the `ERROR:` line, and a `gh` stub that fails if called is never reached. |
| B | no inputs and no verified base (`verified-base` 3, `requested-inputs` 3) | All three real PR tools, run under `set -euo pipefail`, finish and use the legacy base from `origin/HEAD`. |
| B | an unparseable context file | `requested-inputs` exits 1. |
| F | `--strict` against an unreachable origin | A `git` wrapper counts exactly one fetch, and a `sleep` stub is never run. |

It also covers hostile inputs (`--upload-pack=…`, `-x`, `@`, `HEAD`, `@{-1}`,
`refs/heads/dev`, `dev;id`, `$(id)`, a newline followed by a fake `ERROR:`
line, an ESC byte, 300 characters, a tag-object SHA) and an old bundle with and
without inputs.

The Rust test builds a `file://` bare origin and a clone under `isolated(..)`,
runs `resolve --base-branch dev` (expects `source_verified=verified` and
`base_ref=origin/dev`), then points origin at a missing path and expects a
non-zero exit with `refuse-fetch-failed` on stderr.

## See also

- [step-04-setup-worktree](../../docs/reference/recipe-step-04-worktree-reattach-prune.md) — the three-state guard and where these checks run
- [worktree_setup context propagation](../../docs/reference/worktree-setup-propagation.md) — the JSON fields and how later steps read them
- [Worktree Branch Deconfliction](./workflow_worktree_deconflict.md) — runs after `resolve`, with `--base "$BASE_REF"` (`origin/<b>` or the SHA)
- [Re-runs never destroy an existing worktree's work](../../docs/recipes/issue-1565-rerun-preserves-worktree.md) — why a reused worktree is checked by containment
