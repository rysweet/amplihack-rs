# Update Network Resilience — Reference

> **[PLANNED — Implementation Pending]**
> This document describes the intended behavior after issue #257 is complete.
> Remove this notice once the checksum-retry and 5xx-message changes land.

The self-update subsystem retries transient HTTP failures when downloading release assets and their SHA-256 checksum files. This prevents flaky 5xx responses from GitHub's CDN from breaking otherwise successful updates.

## Retry Behavior

All release downloads — the binary archive and its `.sha256` checksum file — use `http_get_with_retry`, which wraps the single-shot `http_get` with exponential backoff.

| Parameter | Value | Notes |
|-----------|-------|-------|
| Max attempts | 3 | First attempt + 2 retries |
| Initial backoff | 500 ms | Doubles on each retry: 500 ms → 1 s |
| Retryable status codes | `429`, `500`–`599` | Rate limits and server errors |
| Retryable transport errors | All | DNS failures, timeouts, connection resets |
| Non-retryable status codes | `400`, `401`, `403`, `404`, `422` | Client errors fail immediately |

### What Counts as Retryable

The function `is_retryable_error` classifies `ureq::Error` values:

```
match error:
  HTTP 429        → retry (rate limit)
  HTTP >= 500     → retry (server error)
  Transport error → retry (network issue)
  HTTP 4xx other  → fail immediately
```

Note: `http_get` itself converts 403 and 429 responses into `anyhow` errors via `github_error_message` before returning. Since `http_get_with_retry` uses `downcast_ref::<ureq::Error>()` to classify errors, these wrapped errors cannot be classified by `is_retryable_error` and default to retryable. The planned implementation should ensure 403 errors fail immediately without retries.

### Retry Sequence Example

A typical 502-then-success sequence during checksum download:

```
attempt 1: GET https://github.com/.../amplihack-x86_64-linux.tar.gz.sha256
           → HTTP 502 Bad Gateway (retryable)
           sleep 500ms

attempt 2: GET https://github.com/.../amplihack-x86_64-linux.tar.gz.sha256
           → HTTP 200 OK
           body: "a1b2c3d4...  amplihack-x86_64-linux.tar.gz"
           → checksum verified, update proceeds
```

## Error Messages

`github_error_message` returns user-facing text for common GitHub HTTP errors:

| Status | Message Pattern |
|--------|----------------|
| `403` | Rate limit exceeded; suggests `AMPLIHACK_NO_UPDATE_CHECK=1` or waiting ~60 minutes |
| `429` | Rate limit exceeded; suggests waiting a few minutes |
| `500`–`599` | **[PLANNED]** Transient server error with status code; suggests retrying |
| Other | Generic `HTTP {status} from {url}` |

### 5xx Message Format [PLANNED]

Once implemented, `github_error_message` will return a dedicated message for server errors:

```
GitHub returned a transient server error (HTTP 502) for https://api.github.com/...
Retrying should resolve this.
```

The message will include the exact status code and URL for debugging, and direct the user to retry rather than investigate a configuration problem. Currently, 5xx codes fall through to the generic `HTTP {status} from {url}` format.

## SHA-256 Checksum Verification

After downloading the binary archive, `verify_sha256` fetches the `.sha256` sidecar file and compares digests. **[PLANNED]** The checksum download will use `http_get_with_retry` (currently it uses the single-shot `http_get`). The checksum file format follows the BSD/GNU `sha256sum` convention:

```
<64-char hex digest>  <filename>
```

Only the first whitespace-delimited token is used. The function:

1. Downloads the checksum file (with retry — [PLANNED], currently without retry)
2. Extracts the hex digest (first token, must be exactly 64 hex characters)
3. Computes SHA-256 of the downloaded archive bytes
4. Compares digests case-insensitively
5. Fails with a clear mismatch message if they differ

### Verification Failures

| Failure | Message |
|---------|---------|
| Checksum download fails (after retries) | `failed to download checksum from {url}` |
| File is not UTF-8 | `checksum file is not valid UTF-8` |
| File is empty or malformed | `checksum file is empty or malformed: {url}` |
| Digest is not 64 hex chars | `checksum file does not contain a valid SHA-256 hex digest` |
| Digest mismatch | `SHA-256 checksum mismatch for downloaded archive` with expected/actual values |

## Security Properties

- **URL allowlist enforced on every request**: `validate_download_url` rejects URLs not starting with `https://api.github.com/`, `https://github.com/`, or `https://objects.githubusercontent.com/`. This applies to both initial and retried requests.
- **Response size capped**: All HTTP responses are limited to 512 MiB via `.take()`, preventing OOM from malicious or corrupted responses.
- **Retry is bounded**: Maximum 3 attempts with exponential backoff prevents the client from becoming a DoS amplifier.
- **SHA-256 verification is mandatory when a checksum file exists**: The retry mechanism gives the checksum download a fair chance to succeed, strengthening integrity verification rather than weakening it.

## Environment Variables

| Variable | Effect on Update Network |
|----------|------------------------|
| `AMPLIHACK_NO_UPDATE_CHECK=1` | Skips the entire update check, including all network calls |
| `AMPLIHACK_NONINTERACTIVE=1` | Suppresses update checks during non-interactive (CI/scripted) runs |

## Related

- [amplihack install](./install-command.md) — Full install/uninstall CLI reference
- [Environment Variables](./environment-variables.md) — All environment variables read by `amplihack`
- [Idempotent Installation](../concepts/idempotent-installation.md) — How repeated installs are safe
