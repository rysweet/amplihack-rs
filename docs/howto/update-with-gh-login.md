---
title: Update amplihack with gh's login
description: Let amplihack update look up and download releases with the GitHub login that gh already holds, and read the message when GitHub refuses a request.
last_updated: 2026-11-22
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Update amplihack with gh's login

Use this guide when `amplihack update` fails with HTTP 403 or 429 from GitHub.
GitHub gives anonymous requests a small allowance, and every process behind one
address shares it. A host that runs several agents can use it up. Requests made
through a signed-in `gh` have their own, larger allowance.

amplihack does not hold a token. It runs your `gh` and lets `gh` sign the
request. The [reference](../reference/update-release-lookup.md) lists the exact
commands.

## Use the login on a host with `GH_TOKEN` unset

This applies to a host where `GH_TOKEN` is unset and the login lives in a
`gh` configuration directory.

1. Point `GH_CONFIG_DIR` at the directory where `gh auth login` was completed:

   ```bash
   export GH_CONFIG_DIR=/srv/agents/gh-config
   ```

2. Check that `gh` accepts the login:

   ```bash
   gh auth status
   ```

   The command must exit with status 0. Check it with `echo $?` if the output
   is unclear.

3. Run the update:

   ```bash
   amplihack update
   ```

`gh` finds the configuration directory by itself. `GH_HOST` and `GH_TOKEN` are
honoured in the same way, because amplihack passes its whole environment to
`gh`.

## When `gh` is not used

amplihack makes the anonymous request, as before, if either of these is true:

- `gh` is not on `PATH`.
- `gh auth status` exits with a non-zero status. A host with several `gh`
  accounts where one account has a problem takes this path.

The check is silent. To see which path ran, run `gh auth status` yourself.

Once `gh auth status` has succeeded, amplihack never falls back to the anonymous
request. A fallback would hide a failure of the authenticated request and
silently switch the identity that makes it. A failure on the `gh` path is
reported as it happened.

## Read a rate-limit message

A 403 or 429 is reported with GitHub's own message and the time the limit
resets. The sample below is illustrative; the times and address will differ:

```text
GitHub refused the request (HTTP 403): API rate limit exceeded for 203.0.113.7. The limit resets at 14:05 UTC (in 23 minutes). Set AMPLIHACK_NO_UPDATE_CHECK=1 to skip the update check at launch.
```

amplihack does not wait or retry. A 429 is no longer retried, including during
bounded downloads. Run `amplihack update` again after the reset
time, or sign in with `gh` as described above.

If GitHub gives no reset time, the message says `GitHub did not give a reset
time.`

## Limitations

- The `gh auth status` probe contacts GitHub. On a host with a poor connection
  the update can pause at that step for up to 20 seconds, after which the
  anonymous request is used. `gh api` has the same 20-second limit and
  `gh release download` a 15-minute limit; either timing out ends the update
  with an error.
- If `GH_HOST` names a GitHub Enterprise host, `gh` queries that host. The
  allowlist for asset URLs still applies, so a release whose asset URL is on a
  host outside the allowlist is refused.

## Related

- [Update release lookup reference](../reference/update-release-lookup.md)
- [Manage tool update checks](manage-tool-update-checks.md)
