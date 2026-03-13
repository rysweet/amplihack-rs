# Environment Variables Reference

Variables set in the child process environment by the amplihack Rust CLI launcher.

## `AMPLIHACK_HOME`

**Type**: Path string
**Default**: `$HOME/.amplihack` (resolved at launch time)

The root directory of the amplihack installation. Recipe runner and helper scripts
use this to resolve asset paths at runtime instead of relying on hardcoded locations.

### Resolution Order

1. **Preserved** — If already set in the parent environment, the value is forwarded unchanged.
2. **Derived from HOME** — If `$HOME` is set, `AMPLIHACK_HOME` is set to `$HOME/.amplihack`.
3. **Derived from binary location** — Falls back to the parent directory of the running executable.
4. **Silent no-op** — If all strategies fail, the variable is not set.

### Security

- Paths containing `..` components are rejected (SEC-WS3-01).
- Non-absolute paths are rejected (SEC-WS3-02).
- Path existence is not checked at launch time (SEC-WS3-03) — the consumer creates it on demand.

## `AMPLIHACK_SESSION_ID`

**Type**: String (`rs-<timestamp>-<pid>`)

Unique identifier for the current launcher session. Set once per invocation and
forwarded to the child process for tracing correlation.

## `AMPLIHACK_DEPTH`

**Type**: Integer string
**Default**: `"1"`

Nesting depth counter. Incremented when an amplihack session spawns another.

## `AMPLIHACK_RUST_RUNTIME`

**Type**: Boolean flag (`"1"`)

Indicates the child was launched by the Rust CLI rather than the Python launcher.

## `AMPLIHACK_VERSION`

**Type**: Semver string

The version of the Rust CLI that launched the child process (from `CARGO_PKG_VERSION`).

## `NODE_OPTIONS`

**Type**: Node.js options string

Set to include `--max-old-space-size=32768` unless the parent environment already
specifies a `--max-old-space-size` setting.
