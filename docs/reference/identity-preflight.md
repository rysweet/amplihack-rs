# Workflow Identity Preflight — Reference

`workflow-prep` verifies, once and before any step does real work, that the
GitHub account `gh` is currently acting as can write to the repository the run
is about to write to.

Implemented by `amplifier-bundle/tools/workflow_identity_preflight.sh` and
invoked from `step-00a-identity-preflight` in
`amplifier-bundle/recipes/workflow-prep.yaml`.

## Why it exists

Issue #1290. A run completed workflow preparation, workspace preparation,
requirements clarification, codebase analysis, ambiguity resolution and host
detection, then failed at `step-03-create-issue` because the account that
happened to be active was not authorised for the target repository. Six steps
of real work were discarded over a condition that was knowable before the first
one started.

Nothing had ever asked the question. `amplifier-bundle/tools/git-identity.sh`
resolves `gh api user`, but only to derive a *commit* identity; it never checks
that the identity can do anything. Whichever account was ambient acted, and the
mismatch surfaced only when a remote refused.

## What it checks

One question, asked in its general form: **can the active identity write
here?**

A single GraphQL call returns both halves of the answer — `viewer.login` and
`repository.viewerPermission` — and `ADMIN`, `MAINTAIN` or `WRITE` passes.

The check deliberately does *not* recognise particular refusals. Neither
"Enterprise Managed User" nor "Access denied by policy settings" appears in its
logic. Those are two ways of reaching the general condition; a check that knew
only them would miss an expired token, a stray `GH_TOKEN` in the environment,
or an account that was simply never granted access. The permission answer, plus
the generic authorisation vocabulary GitHub uses when it refuses
(`unauthorized`, `forbidden`, `denied`, `could not resolve to a repository`,
`bad credentials`), covers all of those at once.

## Outcomes

| Outcome | Exit | stdout | Effect on the run |
| --- | --- | --- | --- |
| The identity can write | 0 | `identity_preflight: ok login=… repo=… permission=…` | Continues; the acting account is named in the log |
| The identity demonstrably cannot | 1 | `identity_preflight: denied login=… repo=…` | Stops at `step-00a`, before any work |
| The check could not be made | 0 | `identity_preflight: unknown reason=…` | `WARNING`, continues |
| There is nothing to check | 0 | `identity_preflight: skipped reason=…` | `INFO`, continues |

"Could not be made" covers no network, a 5xx, a rate limit, a timeout, an
unrecognised API error, and `gh` not being installed. "Nothing to check" covers
a non-git directory, a repository with no `origin`, and a non-GitHub remote such
as Azure DevOps.

This asymmetry is deliberate. Issue #1268 is the local precedent for what a
brittle gate costs: a finalisation gate failed a run whose work had already
merged, and two live PRs were abandoned. A preflight that blocks work because it
could not check is worse than no preflight, so only a readable "no" stops a run.

## The failure message

A denial names all three facts an operator needs:

- **which account is active** — from the API answer, or from `gh auth status`
  when the call itself was refused;
- **which repository was targeted** — parsed from the `origin` remote, with the
  remote URL shown (credentials redacted);
- **how to switch** — `gh auth switch --hostname <host> --user <ACCOUNT>`, plus
  the accounts `gh` currently knows on that host.

When `GH_TOKEN` or `GITHUB_TOKEN` is set, the message says so, because an
exported token supplies the identity directly and `gh auth switch` will not
change it. No token value is ever printed; all output passes through a
redaction filter.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `AMPLIHACK_SKIP_IDENTITY_PREFLIGHT` | unset | Any non-empty value skips the check entirely. Use it when the workflow legitimately pushes somewhere other than `origin`, such as a fork. |
| `AMPLIHACK_IDENTITY_PREFLIGHT_TIMEOUT` | `30` | Seconds allowed for the single API call. A timeout is an unknown, not a denial. |

## Cost

Exactly one API call on the happy path, asserted by
`tests/issue_1290_identity_preflight.sh`. A second call — `gh auth status` — is
made only on the failure path, where the run is stopping anyway and naming the
accounts the operator can switch to is worth the round trip.

## Running it directly

```bash
amplifier-bundle/tools/workflow_identity_preflight.sh /path/to/repo
```

## Related

- [Workflow Commit Identity Reference](workflow-commit-identity.md) — the
  separate question of which identity *authors* the commits
- [How to Configure Workflow Commit Identity](../howto/configure-workflow-commit-identity.md)
