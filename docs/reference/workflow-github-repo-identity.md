# GitHub Repo Identity Parser Reference

> [Home](../index.md) > Reference > GitHub Repo Identity Parser

Workflow helpers decide which GitHub repository `gh` may act on by parsing
`remote.origin.url`, or a PR URL, into `owner/repo`. That parsing lives in
exactly one bash file, `amplifier-bundle/tools/workflow_github_repo.sh`. Every
bundle caller sources it. The Rust multitask launcher carries a twin that obeys
the same contract and is tested against the same cases, and the Rust origin
classifier in `amplihack-workflows` rejects every URL the bash parser rejects.

- **Audience:** contributors who change `amplifier-bundle/tools/*` or
  `workflow-terminal-state.yaml`, and operators who need to know why a workflow
  reported "no GitHub repo" for an origin.
- **Issues:** #1642 (one shared, hardened parser, and the Rust
  `remote_repository.rs` fix); #1643 (the original userinfo rule).

## Contents

- [Why one parser](#why-one-parser)
- [Function API](#function-api)
- [Accepted forms](#accepted-forms)
- [Rejected forms](#rejected-forms)
- [Parsing rules](#parsing-rules)
- [Callers](#callers)
- [Locating the helper](#locating-the-helper)
- [Self-hosted runs need a reinstalled bundle](#self-hosted-runs-need-a-reinstalled-bundle)
- [Configuration](#configuration)
- [PR URL parsing in the terminal-state recipe](#pr-url-parsing-in-the-terminal-state-recipe)
- [Rust parsers](#rust-parsers)
- [Failure semantics](#failure-semantics)
- [Security invariants](#security-invariants)
- [Testing](#testing)

---

## Why one parser

The rule being enforced: **`gh` may act only on the repository git actually
talks to.** If the parser and git disagree about the host, a workflow could
run `gh pr ready`, `gh pr view` or `gh pr create` against `github.com/o/r`
while `git push` goes to a different server.

The parser used to exist in several copies, and only one of them had the
userinfo rule from #1643. The loose copies accepted origins like these:

```text
https://evil.example/x@github.com/a/b    # git contacts evil.example
https://evil.example#@github.com/o/r
https://evil.example?@github.com/o/r
https://evil.example\@github.com/o/r
```

Each of those read as a GitHub repo even though git contacts `evil.example`.
Now every caller sources the one parser, so they all reject the same origins,
and a guard test stops new copies from appearing.

## Function API

```bash
parse_github_repo_identity <url>
```

| Result  | stdout              | stderr  | Return |
| ------- | ------------------- | ------- | ------ |
| Valid   | `owner/repo` + `\n` | nothing | `0`    |
| Invalid | nothing             | nothing | `1`    |

The parser never echoes its input, so its output can't leak a token that is
embedded in the URL.

**Empty output on failure is part of the contract.** Several callers branch
on the captured output (`repo=$(parse_github_repo_identity "$url")`, then
`[ -n "$repo" ]`) rather than on the return code. If the parser printed even
a diagnostic on failure, those callers would take the GitHub path with a
garbage `--repo`. Any future variant that needs to report _why_ a URL was
rejected must use a new function name; it must not change this one.

The file is safe to source from any script:

- It only defines functions. Sourcing it prints nothing and sets no globals.
- It runs no `set -e`, `set -u`, `set -o` or `shopt`, and it works when the
  caller has `set -euo pipefail` in effect.
- Every variable is `local`, including `LC_ALL=C`.
- It is bash 3.2 compatible. Regexes are held in local variables, and the
  file passes `tests/issue_1423_bash4_constructs.sh`.

Example, run from the repository root:

```bash
$ bash -c '. "$1"; parse_github_repo_identity "$2"' _ \
    amplifier-bundle/tools/workflow_github_repo.sh \
    https://x:tok@github.com/rysweet/amplihack-rs.git
rysweet/amplihack-rs

$ bash -c '. "$1"; parse_github_repo_identity "$2"; echo "rc=$?"' _ \
    amplifier-bundle/tools/workflow_github_repo.sh \
    'https://evil.example/x@github.com/a/b'
rc=1
```

## Accepted forms

Each of these parses to `o/r`:

| Input                                       | Notes                                |
| ------------------------------------------- | ------------------------------------ |
| `https://github.com/o/r`                    |                                      |
| `https://github.com/o/r.git`                | one `.git` suffix is stripped        |
| `https://github.com/o/r/`                   | one trailing `/` is stripped         |
| `http://github.com/o/r`                     |                                      |
| `git@github.com:o/r`                        | scp-style SSH                        |
| `git@github.com:o/r.git`                    |                                      |
| `ssh://git@github.com/o/r`                  |                                      |
| `https://tok@github.com/o/r`                | token as userinfo                    |
| `https://x:tok@github.com/o/r`              | user:token as userinfo               |
| `http://x:tok@github.com/o/r`               | user:token as userinfo over `http`   |
| `https://ghp_AbC%2Fx@github.com/o/r`        | percent-encoded userinfo is accepted |
| `https://x-access-token:tok@github.com/o/r` | Actions-style token userinfo         |
| `https://github.com/o/r/pull/5`             | extra path segments are ignored      |
| `https://github.com/o/r/tree/main`          | extra path segments are ignored      |
| `https://github.com/o/r?x=1`                | `?query` is cut                      |
| `https://github.com/o/r#frag`               | `#fragment` is cut                   |

## Rejected forms

Each of these returns `1` with empty stdout:

| Input                                           | Reason                                                                                                                        |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `https://evil.example/x@github.com/a/b`         | userinfo contains `/` (host confusion)                                                                                        |
| `https://evil.example#@github.com/o/r`          | userinfo contains `#`                                                                                                         |
| `https://evil.example?@github.com/o/r`          | userinfo contains `?`                                                                                                         |
| `https://evil.example\@github.com/o/r`          | userinfo contains `\`                                                                                                         |
| `https://a@github.com/x@github.com/o/r`         | userinfo ends at the **first** `@github.com/`, so the owner segment is `x@github.com`, which fails the `[A-Za-z0-9._-]` check |
| `https://a@b@github.com/o/r`                    | `@` is not allowed in userinfo                                                                                                |
| `https://@github.com/o/r`                       | empty userinfo                                                                                                                |
| `https://ev il@github.com/o/r`                  | space in userinfo                                                                                                             |
| URL with a TAB or LF anywhere                   | control character                                                                                                             |
| `https://github.com/./r`, `…/o/..`              | `.` or `..` segment                                                                                                           |
| `https://github.com/-o/r`                       | owner starts with `-` (option injection)                                                                                      |
| `https://github.com/ówner/r`                    | non-ASCII; rejected under any locale                                                                                          |
| ` https://github.com/o/r` (leading space)       | input is never trimmed                                                                                                        |
| `https://GitHub.com/o/r`                        | prefixes are case-sensitive                                                                                                   |
| `https://www.github.com/o/r`                    | only the exact host `github.com`                                                                                              |
| `ssh://git@github.com:22/o/r`                   | ports are not accepted                                                                                                        |
| `https://ghe.example.com/o/r`                   | only github.com is supported                                                                                                  |
| `https://github.com/o`, `https://github.com/o/` | no repo segment                                                                                                               |
| `https://github.com/o/.git`                     | repo segment is empty after `.git` is stripped                                                                                |
| `file:///srv/git/o/r`, `/srv/git/o/r`           | not a GitHub URL                                                                                                              |

### Why the allowlist is stricter than #1643

#1643 hardened `_finalization_github_repo` with a **denylist**: reject
userinfo containing `/`, `?`, `#` or `\`. That closes the four reported
origins, but anything the list forgot (whitespace, control bytes, `@`,
non-ASCII that some locale maps into `[a-z]`) is still accepted. The shared
parser uses an **allowlist** instead: the userinfo must consist only of the
characters RFC 3986 allows there, minus `@`. Every real token form (`ghp_…`,
`github_pat_…`, `x-access-token:…`, percent-encoded values) fits, so no real
origin is lost. A URL the allowlist doesn't recognise fails closed: the
caller sees "no GitHub repo" and `gh` is never invoked.

The security property is the **host**, not the path. Extra segments after
`owner/repo` are ignored because they can't change which server git
contacts; rejecting them would turn every PR URL into "not GitHub".

## Parsing rules

Rules are applied in this order. The bash parser and the
[multitask twin](#multitask-twin) follow them step for step. The
[remote repository classifier](#remote-repository-classifier) follows its own
rules, which differ in how it finds the userinfo, strips `.git` and matches
the host.

1. **Prefix.** The input must start with one of these exact, case-sensitive
   prefixes:
   - `https://github.com/`
   - `http://github.com/`
   - `git@github.com:`
   - `ssh://git@github.com/`
   - `https://<userinfo>@github.com/` or `http://<userinfo>@github.com/`
2. **Userinfo.** The userinfo is the text between `://` and the **first**
   `@github.com/`. In bash that is `${after%%@github.com/*}`; it must be `%%`
   and not `%`, which would match the last occurrence. In the multitask twin
   it is `split_once("@github.com/")`. The userinfo must be non-empty and match an
   **allowlist**, the RFC 3986 userinfo characters minus `@`:

   ```text
   ^[A-Za-z0-9._~%!$&'()*+,;=:-]+$
   ```

   So `/`, `?`, `#`, `\`, `@`, whitespace, control characters and non-ASCII
   bytes are all rejected. This allowlist replaces the old denylist.

3. **Normalise.** Cut at the first `?`, then at the first `#`, strip one
   trailing `/`, then strip one `.git` suffix (`r.git.git` gives `r.git`).
4. **Segments.** The owner is segment 1 and the repo is segment 2. Any later
   segments, such as `pull/5`, are ignored.
5. **Validate.** Reject an empty segment, `.` or `..`, any byte outside
   `[A-Za-z0-9._-]`, and an owner that starts with `-`. Bash checks this with
   `LC_ALL=C` pinned, so bracket ranges can never match non-ASCII letters.
   Validation always runs **after** every normalisation step, in bash and in
   every Rust parser, so `.git` stripping can never produce an unchecked
   segment: `https://github.com/o/..git` normalises to `o/.` and is rejected.

## Callers

Every caller sources the shared file. None of them keeps a local parser or a
`*@github.com/*` pattern.

| Caller                                                          | Kind                              | When the origin is rejected                                                                                                                                 |
| --------------------------------------------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tools/workflow_pr_ready.sh`                                    | standalone                        | reports no GitHub repo and calls no `gh`                                                                                                                    |
| `tools/workflow_final_status.sh`                                | standalone                        | reports no GitHub repo and calls no `gh`                                                                                                                    |
| `tools/workflow_pr_scope.sh`                                    | standalone                        | `{"ok":false,…}` JSON and calls no `gh`                                                                                                                     |
| `tools/workflow_publish_pr.sh`                                  | standalone                        | `FAILED_PR_IDENTITY` or a non-success exit, and calls no `gh`                                                                                               |
| `tools/workflow_terminal_evidence.sh`                           | standalone                        | `REPO` stays empty; every `gh` call, with or without `--repo`, is skipped                                                                                   |
| `tools/workflow_finalization_pr_signal.sh`                      | library                           | exit 1 with `WARNING: no open-PR signal: …`                                                                                                                 |
| `tools/workflow_finalization_collect.sh`                        | library                           | `"github_remote":false` in its JSON                                                                                                                         |
| `tools/workflow_agentic_finalization.sh`                        | entry point for the two libraries | no PR signal; `gh` is never invoked. It also checks that the helper exists before sourcing either library (see [Locating the helper](#locating-the-helper)) |
| `recipes/workflow-terminal-state.yaml` (`probe-terminal-state`) | recipe                            | no `gh` call; a hostile `PR_URL_INPUT` is never taken as a target                                                                                           |

`workflow_finalization_collect.sh` never calls `gh`. For it, "GitHub remote"
means "the shared parser accepts the origin or PR URL". A PR URL such as
`https://github.com/o/r/pull/7` counts, because extra segments are ignored:

```bash
if parse_github_repo_identity "$origin_url" >/dev/null 2>&1; then
  github_remote="true"
fi
```

It uses an `if` rather than a bare `&&` list, so the enclosing function can't
return non-zero under `set -e`.

The old `_finalization_github_repo` function is gone.
`workflow_finalization_pr_signal.sh` now calls `parse_github_repo_identity`
directly.

Output formats, retry behaviour and classification are the same as before.
The only change in behaviour is that hostile origins, and owner/repo names
outside `[A-Za-z0-9._-]`, are now rejected.

## Locating the helper

Callers find the shared file the same way they already find
`workflow_gh_retry.sh`. Because install and staging copy `amplifier-bundle/`
as a whole tree, the helper ships next to `workflow_gh_retry.sh` in
`~/.amplihack/amplifier-bundle/tools/` and `~/.copilot/amplifier-bundle/tools/`.
No manifest entry is needed.

### Standalone scripts

These resolve the helper from `SCRIPT_DIR` only, never from the cwd. The
lookup runs at top level, straight after the gh-retry source. It rejects a
path that is missing **or** unreadable, so a `chmod 000` helper exits `2`
here instead of failing later at `.` with some other code:

```bash
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow_pr_ready.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
# Defines parse_github_repo_identity (issue #1642).
# shellcheck source=workflow_github_repo.sh disable=SC1090
. "$GITHUB_REPO_HELPER"
```

`workflow_pr_scope.sh` prints
`{"ok":false,"reason":"missing_github_repo_helper"}` on stdout before it
exits. `workflow_terminal_evidence.sh` keeps sourcing gh-retry only when it is
present, but a missing parser is always a hard exit.

### Library scripts

`workflow_finalization_pr_signal.sh` and `workflow_finalization_collect.sh`
are sourced by other scripts, so they must not `exit`. They resolve the
helper relative to their own `BASH_SOURCE[0]`:

```bash
${WORKFLOW_GITHUB_REPO_HELPER:-<dir of BASH_SOURCE[0]>/workflow_github_repo.sh}
```

If the file is missing or unreadable they print one `WARNING:` line to
stderr that names only the path, and leave `parse_github_repo_identity`
undefined, so they **fail closed**: `finalization_open_pr_signal` checks that the function is
defined and reports no PR signal, and collect's silenced calls return `127`,
so `github_remote` stays false.

This fallback exists only for the case where a library is sourced on its own.
In normal use both libraries are sourced by
`workflow_agentic_finalization.sh`, and that entry point checks the helper
**before** it sources anything:

```bash
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow GitHub repo helper not found: $GITHUB_REPO_HELPER" >&2
  exit 2
fi
```

A missing helper means a broken install, so the process exits loudly with
`2` instead of quietly reporting "no PR".

### Terminal-state recipe

The `probe-terminal-state` step looks for the helper **after** it sets
`TARGET_REPO_DIR` and **before** `cd "$TARGET_REPO_DIR"`. It takes the first
of these candidates that exists and is allowed:

1. `$WORKFLOW_GITHUB_REPO_HELPER`, if set. If it is set but is not a
   readable file, the step fails with `FAILED_INVALID_INPUT` straight away.
   It does **not** fall through to the other candidates, so a test that
   points the override at a missing file can't pass by accident through an
   installed copy.
2. `$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_github_repo.sh`, only if
   `AMPLIHACK_HOME` is set
3. `$REPO_PATH/amplifier-bundle/tools/workflow_github_repo.sh`, only if
   `REPO_PATH` is set
4. `$(pwd)/amplifier-bundle/tools/workflow_github_repo.sh`
5. `~/.copilot/amplifier-bundle/tools/workflow_github_repo.sh`
6. `~/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh`

`TARGET_REPO_DIR` is the untrusted checkout being judged. It is not a separate
directory: when no worktree is given it equals `REPO_DIR`, which comes from
`REPO_PATH` and defaults to `.`. So candidates 3 and 4 can point straight into
it. The step therefore **skips any candidate from 2 to 6 whose directory lies
inside `TARGET_REPO_DIR`**:

- Both sides are resolved to physical paths with `cd -P … && pwd -P` in a
  subshell, so `..` and directory symlinks can't hide the containment. This is
  bash 3.2 safe and needs no `realpath`.
- A candidate is inside the target when its directory equals the target or
  starts with the target followed by `/`.
- The helper file itself is also resolved: its own symlinks are followed
  with `readlink`, then its directory with `cd -P`. A candidate whose helper
  file lands inside `TARGET_REPO_DIR` is skipped too, so a helper symlinked
  into the checkout is never sourced.
- If `TARGET_REPO_DIR` can't be resolved, the step fails with
  `FAILED_INVALID_INPUT`, as the following `cd` would.

Candidate 1 is the test and override hook and is not filtered (see
[Configuration](#configuration)). If it is set but is not a readable file, the
step fails with `FAILED_INVALID_INPUT` and does **not** fall through to
candidates 2 to 6.

When amplihack runs on its own checkout, candidates 3 and 4 are skipped and
the lookup falls through to the installed copy in step 5 or 6.

If no allowed candidate exists, the step calls
`fail_terminal_state FAILED_INVALID_INPUT` with a one-line reason that says
what to run and lists every path it searched, in lookup order, each with why
it was not used. The reason contains no origin or PR URL:

```text
ERROR: workflow-terminal-state: FAILED_INVALID_INPUT: workflow_github_repo.sh not found; run 'amplihack install' to install or update the amplihack bundle (or set WORKFLOW_GITHUB_REPO_HELPER). Searched: /home/ryan/src/amplihack-rs/amplifier-bundle/tools/workflow_github_repo.sh (skipped: inside target checkout); /home/ryan/.copilot/amplifier-bundle/tools/workflow_github_repo.sh (missing); /home/ryan/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh (missing)
```

| Marker                              | Meaning                                                                         |
| ----------------------------------- | ------------------------------------------------------------------------------- |
| `(missing)`                         | no file there                                                                   |
| `(unreadable)`                      | the file exists but can't be read                                               |
| `(skipped: inside target checkout)` | the candidate, or the file its symlink resolves to, is inside `TARGET_REPO_DIR` |

A path that appears twice (for example `REPO_PATH` and `$(pwd)` naming the
same directory) is listed once.

## Self-hosted runs need a reinstalled bundle

A workflow run **on the amplihack-rs repository itself** finds the helper only
in an installed bundle, because the checkout's own `amplifier-bundle/tools` is
the untrusted target and is always skipped. Bundles installed before #1642 do
not contain `workflow_github_repo.sh`, so on those machines the
terminal-state step fails with the
[message above](#terminal-state-recipe) until the bundle is refreshed:

```bash
amplihack install
ls ~/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh
# Output: /home/ryan/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh
```

This is deliberate. Falling back to the checkout's copy would let the branch
under test choose its own parser and undo the hardening. The cost of a stale
install is a clear, actionable failure, never a bypass. Runs on any other
repository are unaffected, because their checkout contains no
`amplifier-bundle/` and the lookup already ends at an installed copy.

## Configuration

| Variable                      | Default                            | Effect                                                                                                                                                                                                                                                                                                                                                      |
| ----------------------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `WORKFLOW_GITHUB_REPO_HELPER` | unset (resolve as described above) | Absolute path to `workflow_github_repo.sh`. **Test and override hook**; production installs leave it unset. Set but missing or unreadable: standalone scripts and `workflow_agentic_finalization.sh` exit `2`, libraries fail closed, and the recipe reports `FAILED_INVALID_INPUT`. The recipe's `TARGET_REPO_DIR` containment check does not apply to it. |

The override has the same trust level as `PATH`: anyone who can set
environment variables for a workflow can already run code in it, so the
variable gives an attacker nothing new. It exists so tests, and operators
debugging a broken install, can point callers at a specific copy. It is never
filtered and never falls through to another candidate.

Test harnesses that run the terminal-state recipe from a temporary
`REPO_PATH` or cwd set this variable to the workspace copy, as they already do
for `WORKFLOW_PR_SCOPE_HELPER`:

```rust
cmd.env(
    "WORKFLOW_GITHUB_REPO_HELPER",
    workspace_helper_path("workflow_github_repo.sh"),
);
```

## PR URL parsing in the terminal-state recipe

`parse_github_pr_url <url>` in `workflow-terminal-state.yaml` prints
`owner/repo/N`. It parses by position, not by substring search:

1. Require an `http://` or `https://` prefix.
2. `identity=$(parse_github_repo_identity "$url") || return 1`. All host and
   userinfo checks happen here.
3. Cut `?query` and `#fragment`. Take `rest=${url#*://}`. If `rest` starts
   with `github.com/`, strip that; otherwise strip up to the first
   `@github.com/`. That is safe because the accepted userinfo can't contain
   `@` or `/`.
4. Segments 1 and 2 must equal `$identity`. This catches
   `https://a@github.com/x@github.com/o/r/pull/5`: the parser rejects it in
   step 2, and even a parser that accepted it would yield an identity that
   disagrees with the stripped path.
5. Segments 3 and 4 must be `pull/<n>`, with `n` matching `^[1-9][0-9]{0,9}$`.
   That rules out `0`, leading zeros, and numbers long enough to overflow
   `$(( ))`.

The strip in step 3, `${rest#*@github.com/}`, is the **only** `@github.com/`
pattern allowed in the bundle outside `workflow_github_repo.sh`. It runs
only after the parser has accepted the same URL, and a comment at the strip
says so. The guard test allows exactly this one occurrence, and only in
`recipes/workflow-terminal-state.yaml`. A second strip in the recipe, or one
anywhere else, fails the guard. Widening the parser's API to also return PR
numbers was considered and rejected: a narrow, named exception is easier to
audit.

| `PR_URL_INPUT`                                | Result            |
| --------------------------------------------- | ----------------- |
| `https://github.com/o/r/pull/5`               | `o/r/5`           |
| `https://tok@github.com/o/r/pull/5`           | `o/r/5`           |
| `https://o:x@github.com/o/r/pull/5`           | `o/r/5`           |
| `https://evil.example#@github.com/o/r/pull/5` | rejected          |
| `https://github.com/o/r/pull/0`               | rejected          |
| `https://github.com/o/r/pull/007`             | rejected          |
| `https://github.com/o/r/pull/12345678901`     | rejected          |
| `git@github.com:o/r/pull/5`                   | rejected (scheme) |
| `ssh://git@github.com/o/r/pull/5`             | rejected (scheme) |

## Rust parsers

Bash and Rust can't share code. Four Rust functions read GitHub URLs; each
one rejects every hostile origin above.

| Function                                                          | File                                                      | Role                                                       | #1642 change              |
| ----------------------------------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------- | ------------------------- |
| `parse_github_repo_identity`                                      | `amplihack-cli/src/commands/multitask/command_builder.rs` | twin of the bash parser; sets `WorkstreamScope.repository` | hardened to this contract |
| `provider_from_remote_url`, `repository_identity_from_remote_url` | `amplihack-workflows/src/remote_repository.rs`            | classifies the origin as GitHub, Azure DevOps or Manual    | hardened (was vulnerable) |
| `parse_github_repo_uri`                                           | `amplihack-cli/src/commands/launch/checkout.rs`           | `--checkout-repo` argument                                 | tests only                |
| `parse_github_uri`                                                | `amplihack-launcher/src/repo_checkout.rs`                 | launcher checkout argument                                 | tests only                |

**Invariant (bash ⇒ Rust):** any URL the bash parser rejects gives `None`,
`Err`, provider `Manual` or owner `unknown` in every Rust parser, or the
**same** `o/r` that the URL plainly names. It never gives a _different_ GitHub
owner.

The "same `o/r`" case exists only in the
[classifier](#remote-repository-classifier). It matches `github.com` and its
subdomains case-insensitively and ignores the port, so
`https://www.github.com/o/r`, `https://GitHub.com/o/r` and
`https://github.com:443/o/r` are rejected by bash but come back as GitHub
`o/r` from the classifier. That is allowed: the owner is the one the URL
names, and the classifier only labels the provider; it never hands a repo to
`gh`. The other three parsers accept nothing bash rejects. Any Rust parser
may reject a URL that bash accepts (see the `.git.git` row below).

### Multitask twin

`parse_github_repo_identity(&str) -> Option<&str>` follows the
[parsing rules](#parsing-rules) step for step:

- The userinfo is found with `split_once("@github.com/")` and checked against
  the same allowlist. An empty userinfo is rejected.
- Normalisation happens in the same order. `.git` is removed once with
  `strip_suffix`. The input is never passed through `.trim()`.
- Segments are checked byte by byte with `is_ascii_alphanumeric()` plus
  `.`, `_` and `-`, after normalisation. An owner starting with `-` is
  rejected.

The function stays private. Its tests in
`multitask/issue_1642_repo_identity.rs` `include!` the shared case table and
go through `populate_workstream_scope`, checking the stored
`WorkstreamScope.repository`.

**Fallback redaction.** When `populate_workstream_scope` can't parse a repo
string, it uses that string as the fallback `WorkstreamScope.repository`.
If the string contains `://`, its userinfo is stripped first, so a token is
never saved to state or passed to the launcher. The stripping reuses
`redact_url` from `crates/amplihack-cli/src/runner_ancestry.rs`, which is
widened from private to `pub(crate)`; it removes everything up to the last `@`
before the first `/` after `://`. A string with no `://`, such as a local
path, falls back unchanged and is not passed to `redact_url`, which would
truncate it to 64 characters. The fallback never equals a parsed
`owner/repo`, so scope comparison stays fail-closed.

### Remote repository classifier

`remote_repository.rs` decides the `RepositoryProvider` and the
`RepositoryIdentity { owner, name, remote_url }` for a workflow's origin.
Before #1642 it ended the authority only at `/`, `?` or `#` and took the host
after the **last** `@`, so `https://evil.example\@github.com/o/r` came back as
GitHub `o/r`. The public API is unchanged; the rules are now:

1. **Authority for parsing** ends at the first `/`, `?`, `#` **or `\`**. A
   backslash can never be part of a GitHub authority.
2. **Userinfo** is the text before the **first** `@` in that authority. If
   present it must be non-empty and every byte must be in the
   [allowlist](#parsing-rules) (`[A-Za-z0-9._~%!$&'()*+,;=:-]`). Otherwise
   the URL has no parts and the provider is `Manual`.
3. **scp form** (`user@host:path`): a host part containing `\`, or starting
   with `-`, never gives GitHub.
4. **Owner and name** (GitHub only) are checked after all normalisation,
   including `trim_end_matches(".git")`, which strips repeatedly. Each must
   match `[A-Za-z0-9._-]+` and not be `.` or `..`; the owner must not start
   with `-`. Failure gives `("unknown", fallback_name)`.

Host matching itself is unchanged: `github.com` and its subdomains, compared
case-insensitively, with any port ignored. This is the one place a Rust
parser accepts a URL that bash rejects; see the
[bash ⇒ Rust invariant](#rust-parsers) for why that is safe. This classifier
only labels the provider; it never hands a repo to `gh`.

```rust
use amplihack_workflows::remote_repository::repository_identity_from_remote_url;

let (provider, id) =
    repository_identity_from_remote_url(Some(r"https://evil.example\@github.com/o/r"), "r");
// provider == RepositoryProvider::Manual, id.owner == "unknown"

let (provider, id) =
    repository_identity_from_remote_url(Some("https://x:tok@github.com/o/r.git"), "r");
// provider == RepositoryProvider::GitHub, id.owner == "o", id.name == "r"
// id.remote_url == Some("https://[redacted]@github.com/o/r.git")
```

| Input                                               | Provider     | Owner / name                                                                                     |
| --------------------------------------------------- | ------------ | ------------------------------------------------------------------------------------------------ |
| the 4 hostile origins from #1642                    | `Manual`     | `unknown` / fallback                                                                             |
| `https://a@b@github.com/o/r`                        | `Manual`     | `unknown` / fallback                                                                             |
| `https://@github.com/o/r`                           | `Manual`     | `unknown` / fallback                                                                             |
| `https://a@github.com/x@github.com/o/r`             | `GitHub`     | `unknown` / fallback (authority is `a@github.com`; owner `x@github.com` fails the segment check) |
| `https://www.github.com/o/r`                        | `GitHub`     | `o` / `r` (bash rejects: same owner, allowed)                                                    |
| `https://GitHub.com/o/r`                            | `GitHub`     | `o` / `r` (bash rejects: same owner, allowed)                                                    |
| `https://github.com:443/o/r`                        | `GitHub`     | `o` / `r` (bash rejects: same owner, allowed)                                                    |
| `evil.example\@github.com:o/r`                      | not `GitHub` | —                                                                                                |
| `-oProxyCommand=x:o/r`                              | not `GitHub` | —                                                                                                |
| `https://github.com/o/..git`                        | `GitHub`     | `unknown` / fallback                                                                             |
| `https://github.com/o/.git.git`                     | `GitHub`     | `unknown` / fallback (bash gives `o/.git`: safe-direction difference)                            |
| the real forms in [Accepted forms](#accepted-forms) | `GitHub`     | `o` / `r`                                                                                        |

**Redaction.** `redact_remote_url` keeps its own, wider authority cut (first
`/`, `?` or `#`, _not_ `\`), so it never prints a byte that it hid before
#1642:

- If that authority contains both `\` and `@`, the whole authority is
  replaced: `https://u:p@evil.example\@github.com/o/r` becomes
  `https://[redacted]/o/r`.
- Otherwise it still replaces everything up to the **last** `@`:
  `https://a@b@github.com/o/r` becomes `https://[redacted]@github.com/o/r`.

| Input                                      | `remote_url`                            |
| ------------------------------------------ | --------------------------------------- |
| `https://x:tok@github.com/o/r.git`         | `https://[redacted]@github.com/o/r.git` |
| `https://a@b@github.com/o/r`               | `https://[redacted]@github.com/o/r`     |
| `https://evil.example\@github.com/o/r`     | `https://[redacted]/o/r`                |
| `https://u:p@evil.example\@github.com/o/r` | `https://[redacted]/o/r`                |

In the backslash rows the result has no `@` and no host. The format for the
other rows is `{scheme}://[redacted]@{host}{rest}`, as today.

Neither the parsed parts nor any `Display`/`Debug` output carry the raw URL.

### Launch and launcher checkout parsers

`launch/checkout.rs` (`parse_github_repo_uri`) and
`amplihack-launcher/src/repo_checkout.rs` (`parse_github_uri`) keep their
production code. They have no userinfo branch: they accept only exact
`https://github.com/` and `git@github.com:` prefixes (plus bare `o/r` for the
launcher) and check each segment. `#[cfg(test)]` tests pin this:

- All four hostile origins, `https://a@b@github.com/o/r` and
  `https://tok@github.com/o/r` give `None` / `Err`.
- `https://github.com/o/r`, `https://github.com/o/r.git`,
  `git@github.com:o/r`, `git@github.com:o/r.git` and, for the launcher,
  `o/r` give `o/r`.

A future userinfo branch can't reintroduce the bug unnoticed.

## Failure semantics

| Situation                                                        | Result                                                                                                             |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Origin rejected by the parser                                    | caller reports no GitHub repo; `gh` is never invoked                                                               |
| Helper missing or unreadable, standalone script                  | ERROR line on stderr, `exit 2`, before any `gh` call                                                               |
| Helper missing or unreadable, `workflow_agentic_finalization.sh` | ERROR line on stderr, `exit 2`, before either library is sourced                                                   |
| Helper missing or unreadable, library sourced on its own         | WARNING line on stderr; function undefined, so calls return `127`, giving no signal and `github_remote=false`      |
| Helper missing or unreadable, terminal-state recipe              | `FAILED_INVALID_INPUT`                                                                                             |
| `WORKFLOW_GITHUB_REPO_HELPER` set but broken                     | same as "helper missing" for that caller; never falls through to another copy                                      |
| No identity at all                                               | no `gh` call. A call without `--repo` is not a fallback, because `gh` would then resolve the hostile remote itself |

## Security invariants

- No valid identity means no `gh` call at all.
- Every `gh` call passes the identity as a separate quoted argument after
  `--repo`.
- Userinfo is checked against an allowlist, and the locale is pinned to `C`,
  so bash and the multitask twin accept exactly the same bytes. The
  classifier applies the same byte allowlist to the userinfo before the first
  `@` in its authority.
- PR URLs are parsed by position, and the PR number is strictly canonical.
- Owners starting with `-` are rejected, as defence in depth against
  option injection.
- Anything the bash parser rejects is `None`, `Err`, `Manual` or owner
  `unknown` in every [Rust parser](#rust-parsers), or the same `o/r` the URL
  names, never a different GitHub owner. The same-`o/r` case is only the
  classifier's subdomain, case and port matching.
- Segment checks run after all normalisation in bash and in Rust.
- No error, warning or `fail_terminal_state` reason contains the raw origin or
  PR URL.
- Neither the helper nor any of its 8 callers enables `set -x` or `xtrace`,
  which would print the token-bearing origin to stderr. The guard test
  enforces this.
- Standalone scripts find the helper through `SCRIPT_DIR` and libraries
  through `BASH_SOURCE`, never through the cwd. The recipe lookup runs before
  `cd "$TARGET_REPO_DIR"` and skips every derived candidate whose physical
  directory is inside `TARGET_REPO_DIR`.
- **What the lookup protects, honestly.** Skipping the checkout's copy of
  the parser is defence in depth, not a trust boundary. The recipe's sibling
  helpers (`workflow_pr_scope.sh`, `workflow_final_status.sh`) are still
  resolved through `AMPLIHACK_HOME`, `REPO_PATH` and the cwd, so a hostile
  checkout run with `REPO_PATH` pointing at itself can still supply those.
  Hardening that lookup is follow-up work; #1642 guarantees only that the
  identity parser itself is never taken from the target checkout.
- **Remaining risk:** the recipe's `REPO_PATH` and `$(pwd)` candidates are
  still consulted when they lie _outside_ the target, for example a worktree
  run whose `REPO_PATH` is the main checkout. They are no wider than the paths
  `WORKFLOW_PR_SCOPE_HELPER` resolution already used, but they are not
  install-root-only. Symlinked helper files are followed rather than
  rejected outright, because install preserves symlinks; only a target inside
  the checkout is skipped. `WORKFLOW_GITHUB_REPO_HELPER` is trusted without
  filtering.
- **Out of scope:** GitHub Enterprise and other hosts are never accepted.

## Testing

| Test                                                                          | What it covers                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `tests/integration/issue_1642_github_origin_parsing.rs`                       | test crate root: shared hermetic fixtures (stub `gh`, temp `HOME`, hostile-origin list) and the modules below                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `tests/integration/issue_1642_github_origin_parsing/parser.rs`                | one case table run against **both** the bash helper and the Rust twin: hostile, real, edge (empty or `@` userinfo, ports, `www.`, newline), extra segments and PR URLs. Change either parser without a matching row and CI fails                                                                                                                                                                                                                                                                                                                             |
| `…/cases.rs`                                                                  | the shared case table, `include!`d by `parser.rs` and by the Rust unit tests                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `…/callers.rs`                                                                | every caller × every hostile origin (no identity, empty stub-`gh` log) and × every real form (the caller takes its GitHub path against `o/r`)                                                                                                                                                                                                                                                                                                                                                                                                                |
| `…/missing_helper.rs`                                                         | a missing helper exits `2` for the standalone scripts and for `workflow_agentic_finalization.sh` (also when the override names a missing file); a `chmod 000` override exits `2` for all five standalone scripts and the entry point, `gh` never invoked; the libraries sourced on their own warn and fail closed                                                                                                                                                                                                                                            |
| `…/recipe.rs`                                                                 | hostile origins and PR URLs; `https://a@github.com/x@github.com/o/r` PR URL is rejected                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `…/recipe_lookup.rs`                                                          | a set-but-missing or unreadable override gives `FAILED_INVALID_INPUT` even when `AMPLIHACK_HOME` holds a real helper; with no allowed copy, the reason names `amplihack install` and lists each searched path with the exact marker strings `(missing)`, `(unreadable)` and `(skipped: inside target checkout)`; a path reached through two candidates (`REPO_PATH` equal to the cwd) is listed exactly once; a helper planted in the checkout, a tools directory symlinked into it, or a helper file symlinked into it is never sourced (marker-file check) |
| `…/guard.rs`                                                                  | exactly one `parse_github_repo_identity` definition; every caller sources the helper; no other `@github.com` glob or strip except the recipe exception; no `set -x`/`xtrace` in the helper or callers. Each rule has a self-test that feeds it a violation                                                                                                                                                                                                                                                                                                   |
| `…/install.rs`                                                                | `amplihack install --local` into a private home ships the helper as executable, and the installed `workflow_pr_ready.sh` rejects a hostile origin with `gh` never invoked                                                                                                                                                                                                                                                                                                                                                                                    |
| `crates/amplihack-cli/src/commands/multitask/issue_1642_repo_identity.rs`     | the same accept/reject cases for the Rust twin, plus fallback redaction: `https://tok@evil.example/o/r` is stored without `tok`                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `crates/amplihack-cli/src/commands/launch/checkout.rs` (`#[cfg(test)]`)       | the four hostile origins give `None`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `crates/amplihack-launcher/src/repo_checkout.rs` (`mod tests`)                | hostile origins, `a@b@` and `tok@` forms give `Err`; https, `.git`, scp and bare `o/r` give `Ok("o/r")`                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `crates/amplihack-workflows/tests/issue_1642_remote_repository.rs`            | the [classifier table](#remote-repository-classifier): hostile origins give `Manual`/`unknown`, real forms give GitHub `o/r`, the `..git` and `.git.git` rows, `https://a@github.com/x@github.com/o/r` gives GitHub with owner `unknown`, the `www.`/case/port rows give GitHub `o/r`, the scp rows, every exact `remote_url` in the redaction table (the backslash rows give `https://[redacted]/o/r`), and redaction never prints `u:p`, `tok`, `a@b` or `a@`                                                                                              |
| `crates/amplihack-workflows/tests/provider_contract_tests.rs`                 | existing provider contract, unchanged                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `workflow_publish_resilience`, `workflow_finalize_resilience`, `issue_1641_*` | suites that copy `tools/*.sh` into a temp directory also copy `workflow_github_repo.sh`, since every caller now needs it beside them                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `tests/integration/default_workflow_terminal_state.rs`                        | recipe runs with `WORKFLOW_GITHUB_REPO_HELPER` set; the `issue_815_804_*` and `issue_1062_*` harnesses find the helper through `AMPLIHACK_HOME` or the real bundle path                                                                                                                                                                                                                                                                                                                                                                                      |

The hostile tests are hermetic:

- The stub `gh` is first on `PATH`, and `GH_TOKEN` is empty.
- `HOME` and `GH_CONFIG_DIR` point at temp directories.
- `GIT_TERMINAL_PROMPT=0` and `GIT_ALLOW_PROTOCOL=file` are set.
- `GH_HOST` and `GH_REPO` are unset, because `GH_REPO` would bypass `--repo`.

Hostile origins are passed through argv as Rust raw strings, and each test
asserts that `git config --get remote.origin.url` returns exactly one `\`.

Run them:

```bash
CARGO_TARGET_DIR="$TMPDIR/target" cargo test -p amplihack --test integration issue_1642
CARGO_TARGET_DIR="$TMPDIR/target" cargo test -p amplihack-cli --lib multitask
CARGO_TARGET_DIR="$TMPDIR/target" cargo test -p amplihack-cli --lib launch::checkout
CARGO_TARGET_DIR="$TMPDIR/target" cargo test -p amplihack-workflows
CARGO_TARGET_DIR="$TMPDIR/target" cargo test -p amplihack-launcher repo_checkout
shellcheck -S warning amplifier-bundle/tools/workflow_github_repo.sh \
  amplifier-bundle/tools/workflow_{pr_ready,final_status,pr_scope,publish_pr,terminal_evidence}.sh \
  amplifier-bundle/tools/workflow_{finalization_pr_signal,finalization_collect,agentic_finalization}.sh
```

CI runs the same `shellcheck -S warning` step on the helper, next to the
per-helper steps for `workflow_worktree_sweep.sh` and
`workflow_worktree_deconflict.sh`.

## See Also

- [GitHub API Rate-Limit Tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md):
  the sibling shared helper, `workflow_gh_retry.sh`
- [Workflow Terminal-State Reference](workflow-terminal-state.md): the
  `probe-terminal-state` step and its branch PR lookup
- [Scoped Workflow Closure Reference](scoped-workflow-closure.md): how
  `workflow_pr_scope.sh` validates PR ownership
