# GitHub Repo Identity Parser Reference

> [Home](../index.md) > Reference > GitHub Repo Identity Parser

Workflow helpers decide which GitHub repository `gh` may act on by parsing an
origin URL, or a PR URL, into `owner/repo`. That parsing lives in one bash
file, `amplifier-bundle/tools/workflow_github_repo.sh`, and every bundle caller
sources it. The Rust crates cannot source bash, so their four parsers follow
the same rules and run the same test cases. Issues: #1642, building on #1643.

## Contents

- [Function contract](#function-contract)
- [Accepted forms](#accepted-forms)
- [Rejection rules](#rejection-rules)
- [Callers and helper lookup](#callers-and-helper-lookup)
- [Override variable](#override-variable)
- [PR URL exception in the terminal-state recipe](#pr-url-exception-in-the-terminal-state-recipe)
- [Rust parsers](#rust-parsers)
- [Tests](#tests)

## Function contract

```bash
parse_github_repo_identity <url>
```

| Result  | stdout              | stderr  | Return |
| ------- | ------------------- | ------- | ------ |
| Valid   | `owner/repo` + `\n` | nothing | `0`    |
| Invalid | nothing             | nothing | `1`    |

Several callers test the captured output (`[ -n "$repo" ]`) rather than the
return code, so a rejected URL must produce no output at all. The parser never
echoes its input, so a token embedded in the URL cannot leak through it.

Sourcing the file only defines the function. It sets no globals, prints
nothing, and runs no `set -e`, `set -u`, `set -o` or `shopt`. Every variable is
`local`, including `LC_ALL=C`, and the file is bash 3.2 compatible.

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

| Input                           | Notes                                      |
| ------------------------------- | ------------------------------------------ |
| `https://github.com/o/r`        |                                            |
| `https://github.com/o/r.git`    | one `.git` suffix is stripped              |
| `http://github.com/o/r`         |                                            |
| `git@github.com:o/r`            | scp-style SSH, with or without `.git`      |
| `ssh://git@github.com/o/r`      |                                            |
| `https://tok@github.com/o/r`    | token as userinfo                          |
| `https://x:tok@github.com/o/r`  | user and token as userinfo                 |
| `https://github.com/o/r/`       | one trailing `/` is stripped               |
| `https://github.com/o/r?x=1#f`  | the query and fragment are cut             |
| `https://github.com/o/r/pull/5` | the PR form; see [path shape](#path-shape) |

## Rejection rules

The rules run in this order. A URL that fails any of them is rejected.

1. **Prefix.** The input must start with one of these exact, case-sensitive
   prefixes: `https://github.com/`, `http://github.com/`, `git@github.com:`,
   `ssh://git@github.com/`, or `http(s)://<userinfo>@github.com/`. The input
   is never trimmed. Other hosts, `www.github.com`, uppercase hosts and ports
   are rejected.
2. **Userinfo.** The userinfo is the text between `://` and the **first**
   `@github.com/` (`${rest%%@github.com/*}` in bash). Taking the last one
   would let `https://a@github.com/x@github.com/o/r` through. The userinfo
   must be non-empty and match this allow-list, which is the RFC 3986
   userinfo set without `@`:

   ```text
   ^[A-Za-z0-9._~%!$&'()*+,;=:-]+$
   ```

   This rejects every origin reported in #1642, each of which makes git
   contact `evil.example`:

   ```text
   https://evil.example/x@github.com/a/b
   https://evil.example#@github.com/o/r
   https://evil.example?@github.com/o/r
   https://evil.example\@github.com/o/r
   ```

   Whitespace, control bytes, a second `@` and non-ASCII bytes are rejected as
   well. Real tokens (`ghp_…`, `github_pat_…`, `x-access-token:…`) fit the
   allow-list.

3. **Normalise.** Cut at the first `?`, then at the first `#`. Strip one
   trailing `/`, then one `.git` suffix from the end of the path. The suffix
   is only stripped there, so in the PR form it is never removed from the
   repo segment.
4. <a id="path-shape"></a>**Path shape.** The remaining path must be exactly
   `owner/repo`, or `owner/repo/pull/<n>` with `<n>` matching
   `^[1-9][0-9]{0,9}$`. Nothing else is accepted, so
   `https://github.com/o/r/extra`, `https://github.com/o/r/tree/main` and
   `https://github.com/o/r/../../x/y` are rejected. This keeps the reported
   identity equal to the repository that git and curl fetch once they remove
   dot segments. The PR form is accepted because four call sites pass PR
   URLs: `workflow_pr_ready.sh` (line 84), `workflow_finalization_collect.sh`
   (line 97) and `workflow-terminal-state.yaml` (lines 178 and 511). A
   canonical PR number cannot contain a dot segment.
5. **Segments.** The owner and repo must each match `[A-Za-z0-9._-]+` and
   must not be `.` or `..`. The owner must not start with `-`. These checks
   run after every normalisation step, with `LC_ALL=C` pinned so bracket
   ranges never match non-ASCII letters. In the PR form the repo segment must
   not end in `.git`, so `https://github.com/o/r.git/pull/5` is rejected
   rather than reported as `o/r.git`.

GitHub Enterprise, other hosts and percent-decoding are not supported.

## Callers and helper lookup

These files source the helper and keep no parsing of their own:

| Caller                                     | When the origin is rejected                           |
| ------------------------------------------ | ----------------------------------------------------- |
| `tools/workflow_pr_ready.sh`               | reports no GitHub repo; no repo-scoped `gh` call      |
| `tools/workflow_final_status.sh`           | reports no GitHub repo; no repo-scoped `gh` call      |
| `tools/workflow_pr_scope.sh`               | `{"ok":false,…}` JSON; no `gh` call                   |
| `tools/workflow_publish_pr.sh`             | `FAILED_PR_IDENTITY` or a non-zero exit; no `gh` call |
| `tools/workflow_terminal_evidence.sh`      | `REPO` stays empty and every `gh` call is skipped     |
| `tools/workflow_finalization_pr_signal.sh` | `WARNING: no open-PR signal: …`                       |
| `tools/workflow_finalization_collect.sh`   | `"github_remote":false`                               |
| `tools/workflow_agentic_finalization.sh`   | no PR signal; `gh` is never invoked                   |
| `recipes/workflow-terminal-state.yaml`     | no `gh` call; a hostile `PR_URL_INPUT` is not used    |

After a rejected parse no caller falls back to a `gh` call without `--repo`,
because `gh` would then resolve the hostile remote itself. Every identity is
passed as one quoted argument after `-R` or `--repo`, never through `eval` or
`sh -c`. No message printed by the helper or a caller contains the raw origin.

**Scripts.** The eight scripts look for the helper next to themselves, as they
already do for `workflow_gh_retry.sh`. Standalone scripts use `SCRIPT_DIR`, and
the two sourced libraries use their own `BASH_SOURCE[0]`. They never search
`$PWD` or the target repository:

```bash
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow_pr_ready.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
```

A missing or unreadable helper never leads to an inline fallback parser:

| Caller                                                  | Missing helper                                                                                     |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| standalone script or `workflow_agentic_finalization.sh` | `ERROR:` line naming the path and `WORKFLOW_GITHUB_REPO_HELPER`, then `exit 2`                     |
| library sourced on its own                              | one `WARNING:` line; the function stays undefined, so there is no signal and `github_remote=false` |
| terminal-state recipe                                   | `FAILED_INVALID_INPUT`, as described below                                                         |

**Recipe.** The `probe-terminal-state` step uses the first allowed candidate:
`$WORKFLOW_GITHUB_REPO_HELPER`, then `$AMPLIHACK_HOME`, `$REPO_PATH`, `$(pwd)`,
`~/.copilot` and `~/.amplihack`, each followed by
`amplifier-bundle/tools/workflow_github_repo.sh`. It skips any candidate whose
physical path, or the file its symlink resolves to (at most 40 hops), lies
inside `TARGET_REPO_DIR`, so the checkout under test cannot supply its own
parser. When nothing is found the step fails with one line that says to run
`amplihack install` and lists each searched path marked `(missing)`,
`(unreadable)` or `(skipped: inside target checkout)`.

A run on the amplihack-rs repository itself therefore needs an installed
bundle that contains the helper. Bundles installed before #1642 do not, so
refresh them:

```bash
amplihack install
ls ~/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh
```

**Shipping.** Install and staging copy `amplifier-bundle/tools/` as a whole,
so the helper sits next to `workflow_gh_retry.sh` in the installed bundle
without a manifest entry. The `install` integration test checks this.

## Override variable

| Variable                      | Default                        | Effect                                     |
| ----------------------------- | ------------------------------ | ------------------------------------------ |
| `WORKFLOW_GITHUB_REPO_HELPER` | unset; resolved as shown above | absolute path to `workflow_github_repo.sh` |

The override is trusted: it is not filtered against `TARGET_REPO_DIR` and is
used without further checks. If it is set but missing or unreadable, the
caller fails closed as in the missing-helper table and never falls through to
another copy. It has the same trust level as `PATH`, so it must be set only by
the operator or a test harness, **never from repository-controlled
configuration** such as a checked-in `.env`, recipe default or CI file in the
repository being judged.

## PR URL exception in the terminal-state recipe

`parse_github_pr_url` in `workflow-terminal-state.yaml` prints `owner/repo/N`.
It contains the only `@github.com/` pattern allowed outside the helper, and
the guard test permits exactly that one occurrence. It is safe because:

1. it calls `parse_github_repo_identity` on the same URL first and returns `1`
   if that fails;
2. the `owner/repo` it extracts must equal that identity;
3. the next segments must be `pull/<n>`, with `<n>` matching
   `^[1-9][0-9]{0,9}$`.

## Rust parsers

Each of the four Rust parsers that derives a GitHub identity applies the
userinfo allow-list, the [path shape](#path-shape) and the segment rules,
using ASCII byte predicates only (`is_ascii_alphanumeric`):

| Function                                                          | File                                                      |
| ----------------------------------------------------------------- | --------------------------------------------------------- |
| `parse_github_repo_identity`                                      | `amplihack-cli/src/commands/multitask/command_builder.rs` |
| `provider_from_remote_url`, `repository_identity_from_remote_url` | `amplihack-workflows/src/remote_repository.rs`            |
| `parse_github_repo_uri`                                           | `amplihack-cli/src/commands/launch/checkout.rs`           |
| `parse_github_uri`                                                | `amplihack-launcher/src/repo_checkout.rs`                 |

The launch and launcher parsers accept only `https://github.com/` and
`git@github.com:` prefixes (and bare `o/r` for the launcher) with exactly two
segments. In `parse_github_uri` this means the bare-form regex and both URL
regexes must also reject `.` and `..` segments and an owner starting with
`-`, which they currently accept.

The classifier in `remote_repository.rs` returns `Manual`, or a GitHub
provider with owner `unknown`, for every URL the bash parser rejects. In
particular, `owner_name_from_path` returns owner `unknown` when any segment
follows the second, unless the remainder is exactly `pull/<n>`.
The one exception is that it matches `github.com` subdomains, any case and
any port, where it may return the same `o/r` the URL names. It only labels the
provider and never hands a repository to `gh`.

These files contain `@github.com` but do not derive an identity. They were
audited and left unchanged: `provider_detection.rs`, `runner_ancestry.rs`,
`tests_blarify.rs`, `plugin_manager_tests.rs`, `provider_contract_tests.rs`
and `reflection/prompt.rs`.

## Tests

| Test                                                                  | Covers                                                                                      |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `tests/integration/issue_1642_github_origin_parsing/parser.rs`        | the shared case table in `cases.rs` against the bash helper and the multitask twin          |
| `…/hostile.rs`                                                        | each caller with each hostile origin: no repo-scoped `gh` call, no token in output          |
| `…/real_forms.rs`                                                     | each caller with each real origin form resolves to `--repo o/r`                             |
| `…/callers.rs`, `…/fixture.rs`                                        | how each caller is driven from a fixture repository; the origin lists                       |
| `…/missing_helper.rs`                                                 | missing and unreadable helper, including a broken override                                  |
| `…/recipe.rs`, `…/recipe_lookup*.rs`                                  | recipe rejection, the PR URL exception and the lookup message and skips                     |
| `…/guard.rs`                                                          | one parser definition, every caller sources it, no other `@github.com` parsing, no `set -x` |
| `…/install.rs`                                                        | `amplihack install --local` ships the helper next to the callers                            |
| `crates/amplihack-workflows/tests/issue_1642_remote_repository.rs`    | classifier and redaction                                                                    |
| unit tests in `command_builder`, `checkout.rs` and `repo_checkout.rs` | the same hostile, path-shape and real-form vectors                                          |

The case table expects `None` (rejected) for each path-shape vector:
`https://github.com/o/r/tree/main`, `https://github.com/o/r/extra`,
`https://github.com/o/r/../../x/y` and `https://github.com/o/r.git/pull/5`.
The `/pull/<n>` rows still expect `o/r`.

```bash
CARGO_TARGET_DIR="$TMPDIR/cargo-1642" cargo test -p amplihack --test integration issue_1642
CARGO_TARGET_DIR="$TMPDIR/cargo-1642" cargo test -p amplihack-cli -p amplihack-launcher -p amplihack-workflows
shellcheck -S warning amplifier-bundle/tools/workflow_github_repo.sh
```

## See Also

- [GitHub API Rate-Limit Tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md):
  the sibling shared helper, `workflow_gh_retry.sh`
- [Workflow Terminal-State Reference](workflow-terminal-state.md): the
  `probe-terminal-state` step
- [Scoped Workflow Closure Reference](scoped-workflow-closure.md): how
  `workflow_pr_scope.sh` checks PR ownership
