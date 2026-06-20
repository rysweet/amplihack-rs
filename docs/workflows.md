# Workflows

Amplihack workflows run declarative recipes that may create commits, publish
branches, review pull requests, and finalize work.

## Workflow commit identity

Every Amplihack-owned workflow commit resolves an explicit Git author and
committer identity before calling `git commit`. Commit-producing recipes source
the shared
`amplifier-bundle/tools/git-identity.sh` helper, which exports
`GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, and
`GIT_COMMITTER_EMAIL` for the commit step. Recipes must not rely on global Git
config, VM user accounts, or shell defaults.

The identity guard covers every recipe path that executes `git commit`, not only
the original publish/finalize paths:

| Recipe | Behavior |
| --- | --- |
| `workflow-publish` | Sets explicit identity before publish commits and follow-up publish commits. |
| `workflow-finalize` | Sets explicit identity before finalization and cleanup commits. |
| `workflow-pr-review` | Sets explicit identity before PR review remediation commits. |
| `workflow-tdd` | Sets explicit identity before implementation checkpoint commits. |
| `workflow-refactor-review` | Sets explicit identity before review-feedback checkpoint commits. |
| `consensus-publish` | Sets explicit identity before consensus publication commits. |
| `consensus-pr-feedback` | Sets explicit identity before PR feedback commits. |

Static coverage fails if a recipe contains an executable `git commit`
without preparing identity first.

## Configure identity for workflows

Use explicit Amplihack variables when running workflows on shared machines,
Azure-hosted environments, or any repository where VM-local Git config might be
present:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"

amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Update workflow-owned commit behavior"
```

When both committer variables are omitted, Amplihack uses the author identity for
the committer identity. If either `AMPLIHACK_GIT_COMMITTER_NAME` or
`AMPLIHACK_GIT_COMMITTER_EMAIL` is set, both must be set and safe.

You can also save the same explicit identity in `~/.amplihack/config`:

```json
{
  "git_identity": {
    "author_name": "Mona Example",
    "author_email": "mona@example.com"
  }
}
```

Existing complete and safe Git environment variables are accepted for advanced
callers:

```bash
export GIT_AUTHOR_NAME="Mona Example"
export GIT_AUTHOR_EMAIL="mona@example.com"
export GIT_COMMITTER_NAME="Mona Example"
export GIT_COMMITTER_EMAIL="mona@example.com"
```

Repository-local Git config is also accepted:

```bash
git config --local user.name "Mona Example"
git config --local user.email "mona@example.com"
```

Amplihack never writes global Git config.

## Provider behavior

GitHub repositories can infer identity from authenticated `gh` when explicit
Amplihack variables, safe Git environment variables, and safe repo-local Git
config are unavailable. Amplihack uses the public GitHub email when available
and the authenticated account's GitHub noreply email otherwise. It uses the
public profile name when available, falling back to the login when the profile
name is empty. The resulting name and email must pass validation.

Azure DevOps repositories should use explicit Amplihack variables or safe
repo-local Git config. The canonical workflow host type is `azdo`; the
user-facing `azure-devops` value is only an alias when a workflow normalizes it.
Azure CLI account inference is accepted only when the authenticated account can
provide a normal user email address. VM, service, runner, localhost, and
automation-looking emails are rejected.

## Unsafe identity failures

Amplihack stops before committing when it finds an unsafe identity such as:

```text
azureuser@build-vm
runner@localhost
mona@
```

Fix the failure by exporting a safe identity:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"
```

Then rerun the workflow.

## More information

- [How to Configure Workflow Commit Identity](howto/configure-workflow-commit-identity.md)
- [Tutorial: Verify Workflow Commit Identity](tutorials/workflow-commit-identity.md)
- [Workflow Commit Identity Reference](reference/workflow-commit-identity.md)
