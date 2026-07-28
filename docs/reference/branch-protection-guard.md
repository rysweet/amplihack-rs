# Branch Protection Guard Reference

> [Home](../index.md) > Reference > Branch Protection Guard

The branch protection guard is an in-repo check that notices when the strict
up-to-date merge policy on `main` gets turned off, and fails a check so the team
finds out and turns it back on. It is the automated follow-up to the prose-only
merge policy described in the [Merge Flow Reference](merge-flow.md).

## What "strict" means and why it matters

`main` is protected so a pull request can only merge after its branch has been
brought up to date with the latest `main`. GitHub calls this "Require branches to
be up to date before merging" and stores it as
`required_status_checks.strict = true`. This is what makes merges serial and
keeps every merge preceded by an up-to-date CI run. The
[Merge Flow Reference](merge-flow.md) explains why this must stay on.

Before this guard existed, that requirement lived only in documentation. If
someone turned strict off, nothing noticed. This guard closes that gap: it reads
the live setting and fails if it is anything other than `true`.

## Detection, not prevention

This guard **detects** the setting being turned off; it does **not prevent** it.
A repository admin can still turn strict off from inside the repository. What
this guard guarantees is that the change becomes loud — a check goes red within
one CI cycle.

The durable way to **prevent** the setting from being changed is an
organization- or enterprise-level ruleset, which only an org admin can set up.
That is out of scope for this guard; the guard is the in-repo detection layer
that catches drift until (or unless) such a ruleset exists.

| Layer | Who controls it | Effect |
| --- | --- | --- |
| This guard (workflow + script) | Repo maintainers | Detects drift, fails a check within one CI cycle |
| Org / enterprise ruleset | Org admin | Prevents the setting from being changed at all |

## Components

| File | Role |
| --- | --- |
| `scripts/check-branch-protection.sh` | Reads `required_status_checks.strict` and exits non-zero when it is anything other than `true`; when the admin-read token is absent it warns and passes (skips the check). |
| `.github/workflows/branch-protection-guard.yml` | Runs the script on a daily schedule, on every push to `main`, and on manual dispatch. |
| `tests/issue_1095_branch_protection_guard_test.sh` | Offline unit test of the script's logic (runs in CI, needs no secret). |

## Required setup: the `BRANCH_PROTECTION_READ_TOKEN` secret

Reading a branch's protection settings needs admin-level read access. The token
GitHub Actions gives a workflow by default (`GITHUB_TOKEN`) does **not** have
that access and gets a `403`, even if the workflow requests
`administration: read`. So the guard runs with a dedicated fine-grained Personal
Access Token instead, supplied through the `GH_TOKEN` environment variable (the
`gh` CLI reads `GH_TOKEN` automatically).

Create the token once:

1. GitHub → Settings → Developer settings → Personal access tokens →
   Fine-grained tokens → **Generate new token**.
2. **Resource owner:** this repository's owner. **Repository access:** only
   **this** repository (not all repositories).
3. **Permissions:** Repository permissions → **Administration → Read-only**.
   Grant nothing else.
4. Generate the token and copy it.
5. In this repository: Settings → Secrets and variables → Actions →
   **New repository secret**. Name it exactly `BRANCH_PROTECTION_READ_TOKEN` and
   paste the token as the value.

If the secret is absent, the guard cannot read protection at all, so it emits a
loud `::warning::` annotation and passes neutrally rather than failing red — an
uninstalled detector is not the same as a detected violation, and a red run on
every push would be a false positive that trains reviewers to ignore the check.
It is a loud warning, never a silent pass. Provision the secret to turn the
warning into real drift detection (a hard failure when strict is off).

## The script: `scripts/check-branch-protection.sh`

### Behavior

The script reads the strict setting using structured extraction only
(`gh api ... --jq`); it never parses JSON with `grep`, `sed`, or `awk`.

```bash
strict="$(gh api "repos/${REPO}/branches/${BRANCH}/protection" \
  --jq '.required_status_checks.strict')"
```

It exits `0` when `strict` is exactly `true`. If the admin-read token is absent
the guard cannot read protection, so it emits a GitHub Actions `::warning::`
annotation and still exits `0` (an uninstalled detector is not a violation).
When the token IS present, any real problem — strict off or a failed API call —
exits `1` and prints an `::error::` annotation.

### Environment variables

| Variable | Required | Purpose |
| --- | --- | --- |
| `GH_TOKEN` | Recommended | Fine-grained PAT with `administration: read`. If empty/unset, the guard emits a "not configured" `::warning::` and exits `0` (skips the strict check — it cannot read protection without the token). |
| `GITHUB_REPOSITORY` | No | `owner/repo` slug. Set automatically inside Actions; falls back to `gh repo view --json nameWithOwner -q .nameWithOwner` when unset. |
| `PROTECTED_BRANCH` | No | Branch to check. Defaults to `main`. |

### Exit status

| Exit | Condition |
| --- | --- |
| `0` | `required_status_checks.strict` is exactly `true`, **or** the admin-read token is absent (emits a `::warning::` and skips the check). |
| `1` | The token is configured but strict is off, or the `gh api` call failed. |

### Example output

Healthy:

```text
OK: branch protection on main: required_status_checks.strict is 'true' (strict up-to-date merges enabled).
```

Strict turned off (exit 1):

```text
::error::branch protection on main: required_status_checks.strict is 'false', expected 'true'. Someone disabled strict up-to-date merges — re-enable immediately.
```

Token missing (exit 0, warning only):

```text
::warning::branch-protection guard not configured: missing BRANCH_PROTECTION_READ_TOKEN (fine-grained PAT, this-repo-only, administration:read) — skipping strict-mode check. Provision the secret to enable drift detection.
```

### Running it locally

```bash
GH_TOKEN=<fine-grained PAT> scripts/check-branch-protection.sh
```

Check a different branch:

```bash
GH_TOKEN=<fine-grained PAT> PROTECTED_BRANCH=release scripts/check-branch-protection.sh
```

## The workflow: `.github/workflows/branch-protection-guard.yml`

### Triggers

| Trigger | Purpose |
| --- | --- |
| `schedule` (`cron: "17 6 * * *"`) | Daily catch-all so drift is detected within one CI cycle. |
| `push` to `main` | Fast feedback right after changes land on `main`. |
| `workflow_dispatch` | On-demand manual run from the Actions tab. |

### Permissions and shape

- Top-level `permissions: contents: read` (least privilege). The admin read
  comes from the PAT, not from `GITHUB_TOKEN`.
- A `concurrency` group scoped to workflow and ref, `cancel-in-progress: true`.
- A single job `strict-guard` on `ubuntu-latest`, `timeout-minutes: 5`, with a
  SHA-pinned `actions/checkout`, that runs the script with
  `GH_TOKEN: ${{ secrets.BRANCH_PROTECTION_READ_TOKEN }}`.

### Do not make this a required PR status check

The guard depends on the `BRANCH_PROTECTION_READ_TOKEN` secret, which is not
available to pull requests from forks. When the secret is absent the guard
degrades to a loud `::warning::` and a neutral pass (it cannot detect drift
without admin-read access), so it no longer fails closed. Even so, keep it a
non-required check: a warning-only run carries no real signal, so requiring it
would add a green check that guarantees nothing. Keep it as a standalone
scheduled / push / dispatch guard only.

## The test: `tests/issue_1095_branch_protection_guard_test.sh`

The test is self-contained and never touches the real GitHub API. It puts a fake
`gh` executable early on `PATH` and drives it with a `FAKE_STRICT` control
variable, asserting:

| Scenario | Expected result |
| --- | --- |
| Guard script present | `check-branch-protection.sh` exists and is executable. |
| `GH_TOKEN=x FAKE_STRICT=true` | Exit `0`. |
| `FAKE_STRICT=false` | Exit non-zero; stderr contains "expected 'true'". |
| `GH_TOKEN` unset/empty | Exit `0`; stderr contains a `::warning::` mentioning "not configured". |
| `FAKE_STRICT=apierror` | Exit non-zero. |
| `GITHUB_REPOSITORY` unset | Resolves the slug via `gh repo view`, then exits `0` when strict is `true`. |

It runs in CI as a step in the `check` (Lint & Format) job of
`.github/workflows/ci.yml`, alongside the other `scripts/check-*.sh` checks, and
needs no secret because it exercises only the script's logic.

## Responding to a red guard

1. Open the failed run and read the `::error::` annotation.
2. If it says strict is not `true`, go to Settings → Branches (or the ruleset)
   for `main` and re-enable "Require branches to be up to date before merging".
3. Re-run the guard (`workflow_dispatch`) to confirm it is green.
4. If it says the token is missing or the API call failed, check that the
   `BRANCH_PROTECTION_READ_TOKEN` secret exists and still has
   `administration: read` on this repository.

## See also

- [Merge Flow Reference](merge-flow.md) - The serial, strict, up-to-date merge
  policy this guard protects.
- [CI Pipeline Reference](ci-pipeline.md) - Required checks and the `check` job
  the unit test runs in.
- [CI Resource Discipline Reference](ci-resource-discipline.md) - Concurrency and
  timeout conventions the guard workflow follows.
