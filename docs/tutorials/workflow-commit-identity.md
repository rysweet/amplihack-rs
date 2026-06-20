# Tutorial: Verify Workflow Commit Identity

This tutorial shows Amplihack workflow commit identity behavior. It uses a
disposable local repository so you can verify author and committer resolution
without publishing branches or creating pull requests.

Do not use `workflow-publish` for this tutorial in a real repository. Publish
workflows can create and push real commits.

## Before you start

The helper is available at:

```text
$AMPLIHACK_HOME/amplifier-bundle/tools/git-identity.sh
```

Use a temporary directory:

```bash
DEMO_REPO="$(mktemp -d)"
cd "$DEMO_REPO"
git init
```

## 1. Configure an explicit identity

Set the identity that Amplihack should use for workflow-created commits:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"
```

The committer identity defaults to the author identity only when both
`AMPLIHACK_GIT_COMMITTER_NAME` and `AMPLIHACK_GIT_COMMITTER_EMAIL` are omitted.
If you set either committer variable, set both.

## 2. Prepare identity with the helper

Source the helper and prepare the commit identity:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/git-identity.sh"
amplihack_prepare_git_commit_identity
```

The helper exports:

```text
GIT_AUTHOR_NAME=Mona Example
GIT_AUTHOR_EMAIL=mona@example.com
GIT_COMMITTER_NAME=Mona Example
GIT_COMMITTER_EMAIL=mona@example.com
```

## 3. Create a local test commit

Create and commit a local file:

```bash
printf 'workflow identity demo\n' > identity-demo.txt
git add identity-demo.txt
git commit -m "test: verify workflow commit identity"
```

## 4. Inspect the commit

Check the author and committer on the latest commit:

```bash
git log -1 --format='Author: %an <%ae>%nCommitter: %cn <%ce>'
```

Expected output:

```text
Author: Mona Example <mona@example.com>
Committer: Mona Example <mona@example.com>
```

## 5. Try repository-local configuration

Unset explicit Amplihack variables and configure identity only for the temporary
repository:

```bash
unset AMPLIHACK_GIT_AUTHOR_NAME
unset AMPLIHACK_GIT_AUTHOR_EMAIL
unset AMPLIHACK_GIT_COMMITTER_NAME
unset AMPLIHACK_GIT_COMMITTER_EMAIL

git config --local user.name "Mona Example"
git config --local user.email "mona@example.com"

amplihack_prepare_git_commit_identity
```

Amplihack accepts the repo-local identity because it is complete, email-safe, and
scoped to the current repository.

## 6. Understand provider behavior

GitHub repositories can infer identity from authenticated `gh` when explicit
identity, complete safe Git environment variables, and repo-local identity are
unavailable:

```bash
gh auth status
amplihack_detect_remote_host_type
amplihack_prepare_git_commit_identity
```

For GitHub, Amplihack uses the public email when available and the GitHub noreply
email otherwise. It uses the public profile name when available and falls back to
the login when the profile name is empty. The resulting name and email must pass
validation.

For Azure DevOps, the canonical host type is `azdo`:

```bash
amplihack recipe run workflow-publish \
  -c repo_path=. \
  -c remote_host_type=azdo \
  -c task_description="Publish Azure DevOps workflow changes"
```

Use that command only in a branch or repository where publishing is intended.
Azure CLI inference is accepted only when the account provides a safe user email.
VM, service, runner, automation, localhost-style, and malformed emails fail
closed.

## 7. Recognize fail-fast protection

When Amplihack sees an unsafe identity such as `azureuser@build-vm`,
`runner@localhost`, `svc-deploy@example.com`, or a missing email, it stops before
committing.

Fix the failure by setting explicit Amplihack variables:

```bash
export AMPLIHACK_GIT_AUTHOR_NAME="Mona Example"
export AMPLIHACK_GIT_AUTHOR_EMAIL="mona@example.com"
```

The workflow should never create an Amplihack-owned commit attributed to a VM,
service, runner, or ambiguous login-only account. A fail-fast identity error is
the intended safe behavior.
