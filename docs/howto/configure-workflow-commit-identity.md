# Configure Workflow Commit Identity

Use explicit commit identity configuration when Amplihack creates commits in
repositories where provider inference is unavailable or where VM-local Git
defaults could be unsafe.

Amplihack validates author and committer identity before every workflow-created
commit. If Amplihack cannot find a safe identity, it fails before running
`git commit`.

## Choose an identity source

Use the first source that fits your environment:

| Environment | Recommended configuration |
| --- | --- |
| Shared VM, Codespace, or Azure-hosted agent | Set `AMPLIHACK_GIT_AUTHOR_NAME` and `AMPLIHACK_GIT_AUTHOR_EMAIL`. |
| Repeated local workflow runs | Set `git_identity.author_name` and `git_identity.author_email` in `~/.amplihack/config`. |
| Azure DevOps repository | Use explicit Amplihack variables or repo-local Git config. |
| GitHub repository with authenticated `gh` | No config required when GitHub identity inference is acceptable. |
| Existing wrapper already exports Git identity | Export all four safe `GIT_AUTHOR_*` and `GIT_COMMITTER_*` variables. |
| Repository with a team-specific local identity | Set `git config --local user.name` and `git config --local user.email`. |

## Set explicit Amplihack identity

Export the author identity before running a workflow:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"

amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Fix provider-safe workflow commit attribution"
```

When both committer variables are omitted, Amplihack uses the author identity for
the committer identity too.

Set committer variables only when your workflow intentionally needs separate
human author and committer attribution:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"
export AMPLIHACK_GIT_COMMITTER_NAME="Riley Reviewer"
export AMPLIHACK_GIT_COMMITTER_EMAIL="riley@example.com"

amplihack recipe run workflow-finalize -c repo_path=.
```

Amplihack validates both identities. If you set one committer variable, you must
set the other. Service-looking committer identities are rejected because workflow
commits must not silently fall back to automation or VM account attribution.

## Save explicit identity in Amplihack config

Use `~/.amplihack/config` when you want the same explicit identity to apply to
future Amplihack workflow runs without exporting shell variables each time:

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

Environment variables override config-file values for the current process.

## Use existing Git environment variables

Advanced wrappers may already export Git's standard commit identity variables.
Amplihack accepts them only when all four values are present and safe:

```bash
export GIT_AUTHOR_NAME="Mona Example"
export GIT_AUTHOR_EMAIL="mona@example.com"
export GIT_COMMITTER_NAME="Mona Example"
export GIT_COMMITTER_EMAIL="mona@example.com"

amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Run with wrapper-provided Git identity"
```

Prefer `AMPLIHACK_GIT_*` for normal use. The standard Git environment variables
are process-wide and may affect non-Amplihack `git commit` commands in the same
shell.

## Configure repository-local Git identity

Use repository-local config when all workflow commits in the repository should
use the same identity and you do not want to export environment variables:

```bash
git config --local user.name "Mona Example"
git config --local user.email "mona@example.com"

amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c task_description="Publish workflow changes"
```

This writes to `.git/config` for the current repository only. Amplihack does not
write global Git config and does not require you to set one.

## Use GitHub inference

For GitHub repositories, authenticate `gh` before running the workflow:

```bash
gh auth status

amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c remote_host_type=github \
  -c task_description="Publish GitHub workflow changes"
```

When explicit identity, complete safe Git environment variables, and safe local
Git identity are absent, Amplihack uses the authenticated GitHub account. It uses
the public email when available and the account's GitHub noreply email
otherwise. It uses the public profile name when available and falls back to the
login when the profile name is empty. The resulting name and email must pass
validation.

## Use Azure DevOps safely

For Azure DevOps repositories, prefer explicit variables:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"

amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c remote_host_type=azdo \
  -c task_description="Publish Azure DevOps workflow changes"
```

If explicit variables are not present, a safe repository-local Git identity is
also accepted:

```bash
git config --local user.name "Mona Example"
git config --local user.email "mona@example.com"

amplihack recipe run workflow-finalize \
  -c repo_path=. \
  -c remote_host_type=azdo
```

Azure CLI account inference is used only as a last provider source and only when
the authenticated account provides a normal user email address. VM, service,
runner, automation, localhost-style, and malformed emails fail closed. The
canonical recipe host type is `azdo`; `azure-devops` is only a user-facing alias
where workflow preparation normalizes it.

## Fix an unsafe VM identity failure

If a workflow fails because it found `azureuser@...`, `runner@localhost`, or a
similar VM-local identity, set an explicit identity and rerun the workflow:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"

amplihack recipe run default-workflow \
  -c repo_path=. \
  -c task_description="Continue after configuring commit identity"
```

Do not fix the failure by setting `git config --global` on the VM. A global VM
identity can leak into unrelated repositories and does not prove the intended
attribution for the current workflow.

## Check the resulting commit identity

After the workflow creates a commit, inspect the latest commit:

```bash
git log -1 --format='Author: %an <%ae>%nCommitter: %cn <%ce>'
```

Expected output:

```text
Author: Mona Example <mona@example.com>
Committer: Mona Example <mona@example.com>
```

If the output shows a VM-local or unexpected identity, treat that as a workflow
bug. Amplihack-created commits are expected to fail before commit rather than
fall back to unsafe identity.
