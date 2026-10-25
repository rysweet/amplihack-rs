# GitHub Repo Identity Parser Reference

> [Home](../index.md) > Reference > GitHub Repo Identity Parser

Workflow helpers decide which GitHub repository `gh` may act on by parsing an
origin URL, or a PR URL, into `owner/repo`. That parsing lives in one bash
file, `amplifier-bundle/tools/workflow_github_repo.sh`, and every bundle caller
sources it. The Rust crates cannot source bash, so their parsers follow the
same contract and run the same test cases. Issues: #1642, building on #1643.

## Contents

- [Function contract](#function-contract)
- [Parsing rules](#parsing-rules)
- [Callers and helper lookup](#callers-and-helper-lookup)
- [Override variable](#override-variable)
- [PR URL exception in the terminal-state recipe](#pr-url-exception-in-the-terminal-state-recipe)
- [Rust twins](#rust-twins)
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
    "https://x:${TOKEN}@github.com/rysweet/amplihack-rs.git"
rysweet/amplihack-rs
```

`TOKEN` stands for any credential in the userinfo; the parser discards it.
Credential-shaped values are never written literally in this repository: the
Rust tests build them with `concat!` so that secret scanners do not read them
as credentials.

## Parsing rules

The executable specification is the case table
`tests/integration/issue_1642_github_origin_parsing/cases.rs`; every parser
listed under [Rust twins](#rust-twins) runs it or its vectors. The helper's
header comment states the rules. In outline:

- the prefix is one of `https://github.com/`, `http://github.com/`,
  `git@github.com:`, `ssh://git@github.com/` or
  `http(s)://<userinfo>@github.com/`, exact and case-sensitive, with no trim;
- the userinfo is the text before the **first** `@github.com/` and must match
  `^[A-Za-z0-9._~%!$&'()*+,;=:-]+$`, which rejects every origin reported in
  #1642 (each makes git contact `evil.example`);
- `?query` and `#fragment` are cut, then one trailing `/` and one `.git`;
- <a id="path-shape"></a>the path must be exactly `owner/repo`, or
  `owner/repo/pull/<n>` with `<n>` matching `^[1-9][0-9]{0,9}$`, so the
  reported identity always equals the repository git fetches once it removes
  dot segments;
- each segment matches `[A-Za-z0-9._-]+`, is not `.` or `..`, and the owner
  does not start with `-`.

The PR form is accepted because callers pass PR URLs. An origin of
`https://github.com/o/r/pull/5` is therefore read as `o/r`; git still contacts
`github.com` for that repository, so this is accepted and safe. GitHub
Enterprise, other hosts, ports and percent-decoding are not supported.

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

**Scripts.** Standalone scripts find the helper through `SCRIPT_DIR`, and the
two sourced libraries through their own `BASH_SOURCE[0]`, as they already do
for `workflow_gh_retry.sh`. They never search `$PWD` or the target repository.
A missing or unreadable helper never leads to an inline fallback parser:

| Caller                                                  | Missing helper                                                                                                    |
| ------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| standalone script or `workflow_agentic_finalization.sh` | `ERROR:` line naming the path and `WORKFLOW_GITHUB_REPO_HELPER`, then `exit 2`                                    |
| `workflow_finalization_pr_signal.sh` sourced on its own | one `WARNING:` line; the function stays undefined, so there is no PR signal                                       |
| `workflow_finalization_collect.sh` sourced on its own   | one `WARNING:` line at source time; `collect_evidence` then prints an `ERROR:` line and exits 2, with no evidence |
| terminal-state recipe                                   | `FAILED_INVALID_INPUT`, as described below                                                                        |

**Recipe.** The `probe-terminal-state` step uses
`$WORKFLOW_GITHUB_REPO_HELPER` when it is set, and fails with
`FAILED_INVALID_INPUT` if that file is missing or unreadable. Otherwise it
uses the same resolution ladder as the strict terminal gate (issue #962):
`$AMPLIHACK_HOME`, `$REPO_PATH`, `$(pwd)`, `~/.copilot` and `~/.amplihack`,
each followed by `amplifier-bundle/tools/workflow_github_repo.sh`. When no
rung holds a readable helper, the step fails with `FAILED_INVALID_INPUT` and
says to run `amplihack install`.

### Self-hosted runs need a reinstalled bundle

A run on the amplihack-rs repository itself needs an installed bundle that
contains the helper. Bundles installed before #1642 do not, so refresh them:

```bash
amplihack install
ls ~/.amplihack/amplifier-bundle/tools/workflow_github_repo.sh
```

Install and staging copy `amplifier-bundle/tools/` as a whole, so the helper
sits next to `workflow_gh_retry.sh` without a manifest entry.

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
2. it cuts `?`, `#` and one trailing `/` in the same order as the helper, and
   the `owner/repo` it extracts must equal that identity;
3. the next segments must be `pull/<n>`, with `<n>` matching
   `^[1-9][0-9]{0,9}$`.

## Rust twins

Each Rust site that derives a GitHub identity follows the contract above with
ASCII byte predicates only (`is_ascii_alphanumeric`):

| Function                                                                                               | File                                            | Shared vectors                                              | Exemptions                                                                     |
| ------------------------------------------------------------------------------------------------------ | ----------------------------------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `provider_from_remote_url`, `repository_identity_from_remote_url`, `github_repository_from_remote_url` | `amplihack-workflows/src/remote_repository.rs`  | `include!`s `cases.rs` (`tests/issue_1642_shared_cases.rs`) | SR6 host rows only                                                             |
| `parse_github_repo_uri`                                                                                | `amplihack-cli/src/commands/launch/checkout.rs` | hostile, path-shape and real-form vectors in its unit tests | accepts only `https://github.com/` and `git@github.com:`, exactly two segments |
| `parse_github_uri`                                                                                     | `amplihack-launcher/src/repo_checkout.rs`       | hostile, path-shape and real-form vectors in its unit tests | as above, plus bare `o/r`                                                      |

Multitask's `populate_workstream_scope` calls
`github_repository_from_remote_url` and has no parser of its own.

**SR6.** The workflows classifier matches `github.com` and its subdomains in
any case and with any port, so it may accept `https://GitHub.com/o/r`,
`https://www.github.com/o/r`, `HTTPS://github.com/o/r` and the port rows that
bash rejects, but only with the `owner/repo` the URL names. Neither caller
hands the result to `gh`: the classifier labels the provider, and multitask
compares the stored `owner/repo` to decide which processes belong to a
workstream. The exempt rows are listed by
name in `SR6_EXEMPT`; any other difference fails the test. It does not trim
the URL, so whitespace rows are rejected as in bash;
`provider_detection.rs` trims at the git-config boundary.

These files contain `@github.com` but do not derive an identity, and were
audited and left unchanged: `provider_detection.rs`, `runner_ancestry.rs`,
`tests_blarify.rs`, `plugin_manager_tests.rs`, `provider_contract_tests.rs`
and `reflection/prompt.rs`.

## Tests

| Test                                                           | Covers                                                                             |
| -------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `tests/integration/issue_1642_github_origin_parsing/parser.rs` | `cases.rs` against the bash helper                                                 |
| `…/hostile.rs`, `…/real_forms.rs`                              | each caller with each origin: no stray `gh` call, `--repo o/r`, no token in output |
| `…/missing_helper.rs`                                          | missing and unreadable helper, including a broken override                         |
| `…/recipe.rs`, `…/recipe_lookup.rs`                            | recipe rejection, the PR URL exception, a missing helper and a broken override     |
| `…/guard.rs`                                                   | one parser definition, every caller sources it, no other `@github.com` parsing     |
| `…/install.rs`                                                 | `amplihack install --local` ships the helper next to the callers                   |
| `crates/amplihack-workflows/tests/issue_1642_*.rs`             | `cases.rs` against the classifier; classifier and redaction                        |

```bash
cargo test -p amplihack --test integration issue_1642
cargo test -p amplihack-cli -p amplihack-launcher -p amplihack-workflows
shellcheck -S warning amplifier-bundle/tools/workflow_github_repo.sh
```

## See Also

- [GitHub API Rate-Limit Tolerance](../WORKFLOW_GH_RATE_LIMIT_TOLERANCE.md):
  the sibling shared helper, `workflow_gh_retry.sh`
- [Workflow Terminal-State Reference](workflow-terminal-state.md): the
  `probe-terminal-state` step
- [Scoped Workflow Closure Reference](scoped-workflow-closure.md): how
  `workflow_pr_scope.sh` checks PR ownership
