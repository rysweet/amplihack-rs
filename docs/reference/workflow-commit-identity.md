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
