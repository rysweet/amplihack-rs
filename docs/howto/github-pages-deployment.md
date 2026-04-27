# GitHub Pages Deployment for amplihack-rs

This guide explains how the amplihack-rs documentation site is deployed to
GitHub Pages, and what to do when the `deploy` job in the docs workflow
fails.

The published site lives at
<https://rysweet.github.io/amplihack-rs/>.

## Overview

Documentation under `docs/` is built with MkDocs (Material theme) and
published to GitHub Pages via the workflow defined in
`.github/workflows/docs.yml`. Two jobs run on every push and pull request:

- **`build`** — runs `mkdocs build --strict` to validate the docs site.
  Strict mode fails on broken links, missing nav targets, and unrecognised
  references. This job runs for both `push` and `pull_request` events so
  every PR gets validated before merge.
- **`deploy`** — runs **only on push to `main`** (gated by
  `if: github.event_name == 'push'`). It uses
  `actions/configure-pages@v5` and `actions/deploy-pages@v4` with OIDC to
  upload the built site to the `github-pages` environment.

The live site at <https://rysweet.github.io/amplihack-rs/> is updated each
time a push to `main` completes the `deploy` job successfully.

## Prerequisites

GitHub Pages must be enabled for the repository with the **GitHub Actions**
build source (not the legacy "Deploy from a branch" mode). Without this
setting, the `deploy` job fails with:

```
HTTP 404: Not Found (Ensure GitHub Pages has been enabled)
```

You can enable Pages with the GitHub CLI (requires repo admin):

```bash
gh api -X POST repos/<owner>/<repo>/pages -f build_type=workflow
```

For this repository:

```bash
gh api -X POST repos/rysweet/amplihack-rs/pages -f build_type=workflow
```

If the call returns `403 Forbidden`, an organisation policy is blocking
Pages for the repository. Ask an organisation owner to allow Pages on
`rysweet/amplihack-rs` (or to flip the relevant org-level setting), then
re-run the `gh api` call.

To verify Pages is configured:

```bash
gh api repos/rysweet/amplihack-rs/pages
```

A healthy response includes `"build_type": "workflow"` and an `html_url`
pointing at the published site.

## Workflow Behavior

### `build` job

- Triggers: every `push` and every `pull_request` touching `docs/**`,
  `mkdocs.yml`, or the workflow file itself.
- Runs `mkdocs build --strict`. Any broken internal link, missing nav
  target, or duplicate anchor causes the job (and therefore the PR check)
  to fail.
- Strict-mode failures are intentional: they prevent merging a docs change
  that would render brokenly on the live site. Fix the underlying link
  rather than relaxing the strict flag.

### `deploy` job

- Triggers: `push` to `main` only. Pull requests do **not** deploy — the
  PR check labelled `deploy` simply never runs and is reported as
  "expected" or "skipped" by GitHub.
- Depends on `build`; if `build` fails, `deploy` is skipped.
- Permissions required (declared in the workflow):
  - `pages: write`
  - `id-token: write`
  - `contents: read`
- Uploads the `site/` directory produced by `mkdocs build` and publishes
  it via `actions/deploy-pages@v4`.

## Troubleshooting

### `deploy` job fails with "Ensure GitHub Pages has been enabled"

Pages is not enabled, or it is enabled with the wrong build source.
Run the `gh api -X POST .../pages -f build_type=workflow` command shown
in [Prerequisites](#prerequisites). The next push to `main` will then
deploy successfully.

### `deploy` does not appear on a pull request

This is expected. The `deploy` job is gated by
`if: github.event_name == 'push'`, so it only runs after a PR is merged
to `main`. If you need to verify the deploy path before merging, push
the branch directly to a fork that has Pages enabled, or merge to `main`
and watch the post-merge workflow run.

### `deploy` is queued but never starts

This usually means the `github-pages` environment has a required
reviewer or a deployment branch policy that excludes the current ref.
Check **Settings → Environments → github-pages** and confirm:

- The environment allows the `main` branch (or "All branches").
- No required reviewer is blocking the run.
- No concurrency group is queuing the deploy behind another in-flight
  workflow.

### `build` job fails with `mkdocs` strict errors

Run the same command locally to reproduce:

```bash
pip install mkdocs mkdocs-material
mkdocs build --strict
```

Common causes:

- A new page was added but not registered in `mkdocs.yml` under `nav:`.
- A relative link points at a renamed or deleted file.
- A page references an anchor (`#section`) that no longer exists.

Fix the offending link or nav entry and re-run the build.

### Site is published but stale

GitHub Pages serves cached content for a short period after each
deploy. Force a refresh by appending a cache-busting query string
(e.g. `?v=2`) or wait a minute. If the content is still stale after
several minutes, re-run the latest workflow on `main` from the Actions
tab.

## Security Notes

- The deploy job uses OIDC (`id-token: write`) to authenticate to GitHub
  Pages; no long-lived deploy tokens are stored in repository secrets.
- Workflow permissions are scoped to the minimum needed: `pages: write`,
  `id-token: write`, `contents: read`.
- The docs workflow uses `pull_request` (not `pull_request_target`),
  so secrets are never exposed to forked-PR builds.
- Future hardening: pin `actions/configure-pages` and
  `actions/deploy-pages` to commit SHAs (rather than `@v5` / `@v4`) to
  defend against tag-rewrite supply-chain attacks. Use Dependabot to
  keep the pinned SHAs current.

## Related

- Live site: <https://rysweet.github.io/amplihack-rs/>
- Workflow definition: `.github/workflows/docs.yml`
- MkDocs configuration: `mkdocs.yml`
- How-to: [Develop amplihack](develop-amplihack.md)
