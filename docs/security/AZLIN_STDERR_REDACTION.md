# Azlin stderr Redaction (Orchestrator Cleanup)

> Status: Implemented · Severity: P2 (security hardening) · Crate: `amplihack-remote`
> Resolves: [#882](https://github.com/rysweet/amplihack-rs/issues/882) · Follow-up from PR #881 / issue #870.

## Summary

The remote VM orchestrator invokes the Azure CLI wrapper (`azlin`) to provision,
reuse, and tear down Azure VMs. When a cleanup call fails, the orchestrator
surfaces the failure to the operator by folding the child process's **stderr**
into a log line and/or a `RemoteError`.

Raw Azure CLI stderr can contain sensitive identifiers — subscription GUIDs,
resource URIs, and **SAS token query strings** (which are bearer credentials).
Logging that verbatim risks disclosing secrets to log sinks (CWE-532).

`Orchestrator::cleanup` now **redacts known-sensitive patterns from azlin stderr
before** it reaches any `tracing` sink or `RemoteError`. The failure is still
surfaced explicitly — redaction never swallows or downgrades an error.

## What gets redacted

Redaction is performed by a module-local, pure helper, `redact_sensitive`, in
`crates/amplihack-remote/src/orchestrator.rs`. It applies two passes:

| Pass | Pattern | Behavior |
| --- | --- | --- |
| 1. SAS query params | `sig`, `se`, `sp`, `sv`, `st`, `sr`, `ss`, `srt`, `spr`, `skoid`, `sktid`, `skt`, `ske`, `sks`, `skv` | The **parameter name is retained**; its value is replaced with `[REDACTED]` (e.g. `...&sig=abc123%2F...` → `...&sig=[REDACTED]`). |
| 2. Canonical GUIDs | 8-4-4-4-12 hex (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`) | The whole GUID is replaced with `[REDACTED]` (covers subscription IDs, tenant IDs, resource GUIDs). |

Passes run **SAS-first, then GUID**, so the `[REDACTED]` placeholder cannot
corrupt later matching. Benign text (VM names, error phrases, region names,
non-secret words) is preserved so the log line stays actionable.

### Properties

- **Pure & idempotent** — no I/O, no side effects; `redact_sensitive(redact_sensitive(s)) == redact_sensitive(s)`.
- **ReDoS-safe (CWE-1333)** — both patterns are linear (no nested quantifiers / catastrophic backtracking).
- **Encoding-safe** — the stderr sink is already-valid UTF-8 from `String::from_utf8_lossy`, and the spawn-error sink is a formatted `std::io::Error`; the transform is UTF-8-boundary safe and never panics on either.
- **Fail-safe** — applied at *both* fallible cleanup sinks, so there is no unredacted path to a log or error.

## Behavioral contract (unchanged)

Redaction is transparent to control flow. The existing `force` semantics are
preserved exactly:

| Condition | `force = false` | `force = true` |
| --- | --- | --- |
| azlin exits non-zero | `Err(RemoteError::cleanup_ctx(<redacted msg>, ctx))` | `warn!("<redacted msg>")` then `Ok(false)` |
| azlin fails to spawn | `Err(RemoteError::cleanup(<redacted msg>))` | `warn!("<redacted msg>")` then `Ok(false)` |
| azlin times out | `Err(RemoteError::cleanup_ctx(<static msg>, ctx))` | `warn!` then `Ok(false)` |

Errors are always propagated (when not forcing) or logged (when forcing) — never
silently degraded.

> **Redaction scope:** Only the two rows that fold **external process output**
> into the message are redacted — the non-zero-exit path (child **stderr**) and
> the spawn-error path (the `std::io::Error`). The timeout path emits a fixed
> `"VM cleanup timed out"` string with no external content, so redaction is a
> no-op there; it is listed only for completeness.

## API

`redact_sensitive` is a private implementation detail (module-scoped, not part of
the public crate API). No public signatures change. There is nothing new to
import or call from outside `orchestrator.rs`; the hardening is automatic for
every `Orchestrator::cleanup` invocation.

```rust
// Internal — not exported.
fn redact_sensitive(input: &str) -> String;
```

## Configuration

None. Redaction is **always on** and requires no flags, environment variables,
or configuration changes. It applies to all callers of `Orchestrator::cleanup`,
including forced/best-effort teardown paths.

## Examples

Input stderr (illustrative):

```text
ERROR: The Resource 'Microsoft.Compute/virtualMachines/ci-runner-7' under
resource group 'rg-amplihack' was not found in subscription
'00000000-0000-0000-0000-000000000000'. Upload URL:
https://amplihackblob.blob.core.windows.net/vhds/disk.vhd?sv=2022-11-02&ss=b&srt=sco&sp=rwdlac&se=2026-01-01T00:00:00Z&st=2025-01-01T00:00:00Z&spr=https&sig=EXAMPLE-fake-sas-signature-value
```

Redacted output that reaches the log / `RemoteError`:

```text
ERROR: The Resource 'Microsoft.Compute/virtualMachines/ci-runner-7' under
resource group 'rg-amplihack' was not found in subscription
'[REDACTED]'. Upload URL:
https://amplihackblob.blob.core.windows.net/vhds/disk.vhd?sv=[REDACTED]&ss=[REDACTED]&srt=[REDACTED]&sp=[REDACTED]&se=[REDACTED]&st=[REDACTED]&spr=[REDACTED]&sig=[REDACTED]
```

The VM name, resource group, error phrasing, and storage host remain intact for
troubleshooting; the subscription GUID is gone, and the SAS **signature**
(`sig` — the actual bearer credential) is redacted. The non-secret SAS metadata
params (`sv`, `ss`, `sp`, `se`, `st`, …) are also scrubbed as a conservative
over-redaction — see the note below.

> **On over-redaction:** Only `sig` is a secret; the other SAS params are
> metadata. The pass redacts the whole known-param set rather than just `sig` to
> stay simple and fail-safe (a new credential-bearing param can't leak through a
> too-narrow allowlist). The cost is slightly less readable URLs in logs, which
> is an acceptable trade for cleanup diagnostics.

## Testing

Inline `#[cfg(test)]` unit tests in `orchestrator.rs` cover:

1. Subscription/GUID redaction — a canonical GUID becomes `[REDACTED]`.
2. SAS query-param redaction — `sig`/`se`/`sp`/… values become `[REDACTED]` while param names remain.
3. Benign-text preservation — non-secret strings pass through unchanged.
4. Multi-secret redaction — a message with both a GUID and a SAS string is fully scrubbed.

Run:

```bash
cargo test -p amplihack-remote
```

## Known limitations (tracked as P2 follow-up)

`redact_sensitive` targets the two highest-value classes for this issue. It does
**not** yet scrub:

- Storage `AccountKey=` / connection strings
- Bearer / OAuth / JWT tokens
- Generic `password=` / `secret=` / `token=` pairs

The helper is intentionally extensible. A future refactor may centralize a single
shared redactor across `orchestrator.rs`, `packager.rs`, and `session.rs`, plus a
lint asserting that any remote-crate `warn!`/`error!` interpolating external
process output routes through it.

## See also

- [`TOKEN_SANITIZATION_GUIDE.md`](TOKEN_SANITIZATION_GUIDE.md)
- [`SECURITY_API_REFERENCE.md`](SECURITY_API_REFERENCE.md)
- [`SECURITY_TESTING_GUIDE.md`](SECURITY_TESTING_GUIDE.md)
