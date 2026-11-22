---
title: Update release lookup reference
description: How amplihack update and the launch update check find and download releases, when they use gh, and how GitHub refusals are reported.
last_updated: 2026-11-22
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Update release lookup reference

This page covers the release lookup and download in `amplihack update` and the
update check that runs at launch. For the steps to sign in, see
[Update amplihack with gh's login](../howto/update-with-gh-login.md).

## Contents

- [Choice of path](#choice-of-path)
- [Commands run through gh](#commands-run-through-gh)
- [Checks that apply on both paths](#checks-that-apply-on-both-paths)
- [Refusals from GitHub](#refusals-from-github)
- [Retries](#retries)
- [Limitations](#limitations)
- [Launch check](#launch-check)

## Choice of path

| Order | Step | Result |
| ----- | ---- | ------ |
| 1 | `AMPLIHACK_TEST_FAKE_LATEST_VERSION` | Handled before `gh` is run |
| 2 | `gh auth status` | Exit 0 selects the `gh` path. Anything else, including `gh` missing, selects the anonymous path |
| 3 | Lookup and download | On the `gh` path through `gh`. Otherwise an anonymous request |

The probe runs with stdin, stdout and stderr discarded, so only the exit status
is read. The child inherits the whole environment, so `gh` applies
`GH_CONFIG_DIR`, `GH_HOST` and `GH_TOKEN` as it does for any other command.

`AMPLIHACK_NO_UPDATE_CHECK` is read by the launch check only (see
[Launch check](#launch-check)). It does not affect `amplihack update`.

After a successful probe, a failure on the `gh` path ends the operation with
that error. There is no fallback to the anonymous request.

`amplihack update` probes `gh` twice in one run, once for the lookup and once
for the download. The call order is `auth status`, `api`, `auth status`,
`release download`.

## Commands run through gh

| Purpose | Command |
| ------- | ------- |
| Probe | `gh auth status` |
| Latest release | `gh api -i repos/rysweet/amplihack-rs/releases/latest` |
| Download | `gh release download <tag> --repo rysweet/amplihack-rs --pattern <asset> --pattern <asset>.sha256 --dir <temporary directory>` |

amplihack does not run `gh auth token`, does not pass `--show-token`, does not
read `hosts.yml`, and never puts a token on a command line or in a log.

`-i` returns the status line and headers, so the rate-limit reset header is
available on this path as it is on the anonymous one. The JSON body is parsed
by the same code on both paths and gives the same release record: version,
tag, asset URL and checksum URL.

### Download directory

The files go to a private directory (mode 0700) created under `$TMPDIR` and
removed when the update step ends. Before any file is read:

- Each expected file must exist and be a regular file. A symbolic link is
  refused.
- Each file must not exceed 512 MiB, the limit on the anonymous path. On the
  `gh` path this is checked after the download, so the file reaches the private
  directory first. This check has no automated test yet; a follow-up issue
  tracks it.

An asset name that is empty or contains `/`, `..`, `*`, `?`, `[`, `\` or a control
character is refused before `gh` is run. So is a tag that starts with `-` or
contains whitespace or a control character.

### Other outcomes on the gh path

| Condition | Error text |
| --------- | ---------- |
| HTTP 404 from the latest-release request | The latest release was not found |
| Any other status that is not 403 or 429 | `gh api returned HTTP <status>: <message>` |
| `gh` cannot be run for the lookup | `failed to query latest stable release through gh` |
| `gh release download` exits non-zero | `gh release download <tag> failed (exit status <n>): <first line of gh's error>` |
| An expected file is absent after download | `gh release download <tag> did not produce <file>` |

If the release record has no checksum URL, the existing warning is logged, as on
the anonymous path.

## Checks that apply on both paths

These are unchanged:

- `asset_url` and `checksum_url` from the release record must be on an allowed
  GitHub host. The check runs before the download on the `gh` path too.
- The archive's SHA-256 must match the checksum file from the same release.
  The check runs before the archive is extracted. A mismatch stops the update
  with `SHA-256 checksum mismatch`.
- The archive layout checks run on the extracted files.
- The 512 MiB limit applies.

## Refusals from GitHub

HTTP 403 and 429 are reported as:

```text
GitHub refused the request (HTTP <status>): <GitHub's message>. The limit resets at <HH:MM> UTC (in <N> minutes). <hint>
```

| Case | Text |
| ---- | ---- |
| Reset more than a minute away | `in 1 minute`, or `in N minutes` (rounded down) |
| Reset passed, or under a minute away | `in under a minute` |
| No reset header, or an unusable value | `GitHub did not give a reset time.` |
| GitHub's message does not mention a rate limit | The text adds that the refusal may be a rate limit |

The wording of the reset clause is `resets at <HH:MM> UTC (in <N> minutes)`, as
the tests assert. The hint names `AMPLIHACK_NO_UPDATE_CHECK`, which skips the launch check.

The reset time comes from the `x-ratelimit-reset` header (matched without
regard to case). On the `gh` path, if `gh` prints no status line, its error
text is used as the message instead. A 403 that is not a rate limit is reported
in the same format; when the header is absent the reset clause reads
`GitHub did not give a reset time.`

GitHub's message and `gh`'s error text are shortened before display: the first
line only, control characters (including escape) removed, at most 300
characters.

## Retries

| Failure | Behaviour |
| ------- | --------- |
| HTTP 5xx | Retried automatically |
| Transport error | Retried automatically |
| HTTP 403 or 429 | Reported at once. No retry, sleep or wait |

Only statuses of 500 and above are retried. A 429 is no longer retried, which
is a change from earlier releases, and the rule also applies to bounded
downloads (#1955).

## Launch check

The update check at launch runs from `maybe_print_update_notice_from_args` in
`crates/amplihack-cli/src/update/check.rs`. It uses the same release lookup as
`amplihack update`: the probe, then the latest release through `gh`, then the
tag. Without a usable `gh` it makes the anonymous request. Any error is logged
at debug level and the launch continues. The check never blocks a launch.

`AMPLIHACK_NO_UPDATE_CHECK` is handled in `classify_skip_reason` and skips this
check only.

The older `_fetch_latest_version` in `auto_update.rs` has no callers and is not
part of this behaviour.

## Limitations

- The `gh auth status` probe contacts GitHub and has no timeout.
- If `GH_HOST` names a GitHub Enterprise host, `gh` queries that host. The
  asset URL allowlist still applies.
- The 512 MiB limit on the `gh` path is checked after download and has no
  automated test.
