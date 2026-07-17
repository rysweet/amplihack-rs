# Workflow Commit Identity Reference

This reference describes Amplihack workflow commit identity behavior. The shared
identity contract is implemented by `amplifier-bundle/tools/git-identity.sh`.
Every Amplihack recipe path that creates a commit uses that helper before
invoking `git commit`.

## Contents

- [Configuration variables](#configuration-variables)
- [Identity resolution order](#identity-resolution-order)
- [Validation rules](#validation-rules)
- [Provider inference](#provider-inference)
- [Shell helper API](#shell-helper-api)
- [Helper script location resolution](#helper-script-location-resolution)
- [Recipe contract](#recipe-contract)
- [Failures](#failures)

## Configuration variables

Use explicit Amplihack identity variables when the repository cannot provide a
safe local identity or when provider inference is not desired.

| Environment variable | Config key | Required | Description |
| --- | --- | --- | --- |
| `AMPLIHACK_GIT_AUTHOR_NAME` | `git_identity.author_name` | Yes, for explicit author config | Author name for workflow-created commits. |
| `AMPLIHACK_GIT_AUTHOR_EMAIL` | `git_identity.author_email` | Yes, for explicit author config | Author email for workflow-created commits. |
| `AMPLIHACK_GIT_COMMITTER_NAME` | `git_identity.committer_name` | Optional pair | Committer name. Defaults to the author name only when both committer fields are omitted. |
| `AMPLIHACK_GIT_COMMITTER_EMAIL` | `git_identity.committer_email` | Optional pair | Committer email. Defaults to the author email only when both committer fields are omitted. |

Example:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"
export AMPLIHACK_GIT_COMMITTER_NAME="Mona Example"
export AMPLIHACK_GIT_COMMITTER_EMAIL="mona@example.com"

amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Update provider-safe workflow commits"
```

Environment variables apply only to the workflow process and its child commit
steps. Amplihack config lives in `~/.amplihack/config` as JSON:

```json
{
  "git_identity": {
    "author_name": "Mona Example",
    "author_email": "mona@example.com",
    "committer_name": "Mona Example",
    "committer_email": "mona@example.com"
  }
}
```

Environment variables override config-file values. Amplihack does not write
global Git configuration.

Partial explicit configuration is invalid. Set both author fields. Set both
committer fields or omit both committer fields to use the author identity for the
committer.

## Identity resolution order

Before each Amplihack-created `git commit`, the workflow resolves a complete
author identity and a complete committer identity in this order:

1. Explicit Amplihack environment variables and config:
   `AMPLIHACK_GIT_AUTHOR_*`, `AMPLIHACK_GIT_COMMITTER_*`, and
   `git_identity.*`.
2. Existing complete and safe Git environment variables for advanced callers:
   `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, and
   `GIT_COMMITTER_EMAIL`.
3. Safe repository-local Git configuration from `user.name` and `user.email`.
4. Authenticated provider context for the detected remote host.
5. Fail-fast with remediation instructions.

Every source is validated before use. Unsafe values are rejected even when they
come from explicit environment variables.

## Validation rules

Amplihack requires complete names and email addresses for both author and
committer identities. Missing committer fields default from the author only when
both committer fields are absent; providing exactly one committer field fails
validation.

An identity is rejected when any required field is missing or unsafe:

| Unsafe value | Examples |
| --- | --- |
| Missing name or email | Empty `user.name`, unset `GIT_AUTHOR_EMAIL`. |
| Malformed email | `mona`, `mona@`, `mona example@example.com`. |
| Localhost or machine-local email | `mona@localhost`, `mona@vm`, `mona@localdomain`. |
| VM account identity | `azureuser@...`, generated VM hostnames. |
| Service-looking email | Generic service, runner, build, or automation account emails. |

The check is intentionally fail-closed. If Amplihack cannot prove that the
identity is user-intended, it stops before the commit instead of creating a
misattributed commit.

## Provider inference

Provider inference is used only after explicit variables, existing safe Git
environment, and repository-local Git config are unavailable.

### GitHub

For GitHub remotes, Amplihack uses the authenticated `gh` account when needed.
It prefers the account's public email when one is available. When the public
email is hidden, Amplihack uses the authenticated account's GitHub noreply
address.

For the name, Amplihack prefers a non-empty public profile name. If the profile
name is absent, Amplihack falls back to the authenticated login. The resulting
name and email must pass the same safety checks as explicit identities.

Example inferred identity:

```text
GIT_AUTHOR_NAME=Mona Example
GIT_AUTHOR_EMAIL=12345678+mona@users.noreply.github.com
GIT_COMMITTER_NAME=Mona Example
GIT_COMMITTER_EMAIL=12345678+mona@users.noreply.github.com
```

GitHub inference fails if `gh` is unavailable, unauthenticated, or returns an
identity that does not pass validation.

### Azure DevOps (`azdo`)

The canonical recipe host type for Azure DevOps is `azdo`. User-facing
`azure-devops` may be accepted as an input alias by workflow preparation, but
identity code and recipe context use `azdo`.

For Azure DevOps remotes, Amplihack first expects explicit Amplihack identity
variables or safe repository-local Git config. If those are unavailable and the
Azure CLI is authenticated, Amplihack may use the Azure CLI account identity
only when it exposes a safe user email address.

Example safe Azure CLI account:

```text
mona@example.com
```

Example rejected Azure CLI accounts:

```text
azureuser@build-vm
svc-deploy@example.com
runner@localhost
```

For Azure CLI inference, Amplihack uses the safe account email for both the Git
name and email because the CLI does not consistently expose a separate human
display name.

Azure DevOps inference is conservative because Azure-hosted development
environments often expose VM-local identities that are not the user's intended
commit attribution.

## Shell helper API

Recipes source the shared helper:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/git-identity.sh"
```

### `amplihack_prepare_git_commit_identity`

Resolves, validates, and exports all Git identity variables required for a
commit:

```bash
amplihack_prepare_git_commit_identity
git commit -m "Update workflow behavior"
```

On success, the function exports:

```bash
GIT_AUTHOR_NAME
GIT_AUTHOR_EMAIL
GIT_COMMITTER_NAME
GIT_COMMITTER_EMAIL
```

On failure, it prints a clear remediation message to stderr and returns nonzero.

### `amplihack_resolve_git_identity`

Resolves the candidate author and committer identity using the documented
precedence order. This function is used by
`amplihack_prepare_git_commit_identity` and by tests that need to inspect
resolution without creating a commit.

### `amplihack_validate_git_identity`

Validates a name and email pair. The function rejects incomplete, malformed,
localhost-style, VM-looking, and service-looking identities.

### `amplihack_detect_remote_host_type`

Classifies the repository remote as `github`, `azdo`, or `unknown`.
Provider inference uses this value to decide whether `gh` or Azure CLI identity
lookup is allowed.

## Helper script location resolution

Before a recipe can source `git-identity.sh`, it must first *find* the file on
disk. The installed bundle layout differs across execution contexts, so every
commit-producing recipe step resolves the helper path with the same ordered
fallback chain rather than assuming a single location. This makes commit and
checkpoint steps robust when `AMPLIHACK_HOME` points at a downstream repository
checkout that does not itself contain an `amplifier-bundle/` tree (for example,
a project consuming an installed amplihack), while the helper is present under
`${HOME}/.amplihack/` or `${HOME}/.copilot/`.

### Resolution order

Each recipe step assigns `GIT_IDENTITY_HELPER` to the first candidate path that
exists, checked in this exact order. The chain adopts the same home-directory
fallback tail (`$(pwd)`, then `~/.copilot`, then `~/.amplihack`) and
last-resort ordering as the sibling `RUNTIME_ARTIFACT_HELPER` chain used for
`workflow_runtime_artifacts.sh`. The leading candidates differ intentionally:
git-identity resolution uses `git rev-parse --show-toplevel` for the repository
top level, whereas the runtime-artifact helper uses `${REPO_PATH:-$(pwd)}`:

| # | Candidate path | Resolves when |
| --- | --- | --- |
| 1 | `${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh` | `AMPLIHACK_HOME` (or `REPO_PATH`, or the repo top level) contains the bundle. |
| 2 | `$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh` | The current repository root contains the bundle. |
| 3 | `$(pwd)/amplifier-bundle/tools/git-identity.sh` | The working directory contains the bundle (worktrees where `pwd` differs from the top level). |
| 4 | `${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh` | The bundle is installed under the Copilot home. |
| 5 | `${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh` | The bundle is installed under the amplihack home. |

Higher-trust, workflow-scoped locations (`AMPLIHACK_HOME`, `REPO_PATH`, repo top
level) are always tried first. The user-home install locations are tried last so
they never shadow an explicitly configured bundle path.

Candidate #1 is preserved verbatim per recipe and is not universal:
`workflow-pr-review.yaml` intentionally uses `${AMPLIHACK_HOME:-$(git rev-parse
--show-toplevel)}` (no `REPO_PATH`) as its base. Only the three home-based
fallbacks (#3–#5) are appended identically across every recipe; see
[Coverage](#coverage).

The single-line form used in the commit steps is:

```bash
GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh"
[ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"
[ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"
[ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"
[ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"
[ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }
. "$GIT_IDENTITY_HELPER"
```

All candidate assignments and every `[ -f "$GIT_IDENTITY_HELPER" ]` guard are
double-quoted to prevent word-splitting and glob expansion. The chain performs
no `eval`, adds no command substitution beyond the existing `$(pwd)` and
`$(git rev-parse --show-toplevel)`, and never parses the contents of the helper
during resolution.

### Fail-visible behavior

Resolution is fail-visible, never silent-degrade. When *every* candidate is
absent, the step prints
`ERROR: git identity helper not found: <path>` to stderr and exits `2`. The
terminal `exit 2` runs only after the full chain is exhausted, so a genuinely
missing helper still stops the workflow loudly instead of producing an
unattributed or skipped commit.

`workflow-publish.yaml` additionally documents a multi-line commit example in an
inline retry block. That block uses the same five candidate paths to locate the
helper before sourcing it, and by design it does not add an `exit 2` guard — it
retains its original fail-open-to-source shape and only benefits from the added
resolution paths.

### Coverage

The five-candidate chain is applied consistently in every recipe step that
sources `git-identity.sh`:

| Recipe | Step context |
| --- | --- |
| `workflow-finalize.yaml` | Finalization and cleanup commit step. |
| `workflow-refactor-review.yaml` | Review-feedback checkpoint commit step. |
| `workflow-pr-review.yaml` | PR review remediation commit step (base candidate omits `REPO_PATH` by design). |
| `workflow-tdd.yaml` | Implementation checkpoint commit step. |
| `workflow-publish.yaml` | Publish commit step and the multi-line outside-in retry example. |
| `consensus-publish.yaml` | Consensus result publication commit step. |
| `consensus-pr-feedback.yaml` | PR feedback publication commit step. |

Leading candidates are preserved verbatim per recipe: `workflow-pr-review.yaml`
intentionally starts from `${AMPLIHACK_HOME:-$(git rev-parse --show-toplevel)}`
without `REPO_PATH`, and the three home-based fallbacks are appended without
rewriting it.

### Regression contract

A regression test asserts that the helper is located via the
`${HOME}/.amplihack/amplifier-bundle/tools/git-identity.sh` fallback when both
`AMPLIHACK_HOME` and the repository top level lack an `amplifier-bundle/` tree.
The test also keeps a negative control proving that a fully missing helper still
exits `2` (fail-visible), and static assertions that every recipe listed above
carries the complete five-candidate chain. The test uses a temporary `HOME` and
a disposable repository with `trap ... EXIT` cleanup and never writes to the
real `~/.amplihack` or `~/.copilot`.

## Recipe contract

Every Amplihack recipe path that creates a commit calls
`amplihack_prepare_git_commit_identity` immediately before `git commit`.

The guard applies to every recipe with an executable `git commit` call. Current
known commit-producing recipe paths are:

| Recipe | Commit path |
| --- | --- |
| `workflow-publish.yaml` | Publish commits and follow-up publish commits. |
| `workflow-finalize.yaml` | Finalization and cleanup commits. |
| `workflow-pr-review.yaml` | PR review remediation commits. |
| `workflow-tdd.yaml` | Implementation checkpoint commits. |
| `workflow-refactor-review.yaml` | Review-feedback checkpoint commits. |
| `consensus-publish.yaml` | Consensus result publication commits. |
| `consensus-pr-feedback.yaml` | PR feedback publication commits. |

Recipes may stage files before identity preparation, but they must not run
`git commit` until the helper has exported a complete author and committer
identity for the current shell step.

Static coverage should search all recipe YAML files for executable `git commit`
calls and fail when a commit path does not prepare identity first. This prevents
future recipes from bypassing the helper.

## Failures

When no safe identity is available, the workflow stops before the commit with a
message that points to explicit configuration:

```text
Amplihack could not determine a safe Git commit identity.
Set AMPLIHACK_GIT_AUTHOR_NAME and AMPLIHACK_GIT_AUTHOR_EMAIL, or configure
repository-local git user.name and user.email.
```

Recommended remediation:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"

amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c task_description="Publish completed workflow changes"
```

For persistent repository-local configuration without changing global Git
config:

```bash
git config --local user.name "Mona Example"
git config --local user.email "mona@example.com"
```

Do not fix this by setting global Git config on a shared VM. Global VM config can
affect unrelated repositories and does not prove the identity was intended for
the current workflow commit.

A commit step can also stop earlier, before identity resolution runs, when the
helper script itself cannot be located on disk:

```text
ERROR: git identity helper not found: <path>
```

This is the separate, fail-visible `exit 2` described under
[Helper script location resolution](#helper-script-location-resolution). It
means every candidate path was absent, not that the identity was unsafe.
Remediation for that case is covered in the how-to guide's
"Fix a 'git identity helper not found' failure" section.
