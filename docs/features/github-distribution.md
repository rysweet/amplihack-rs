# GitHub Distribution

> [Home](../index.md) > [Features](README.md) > GitHub Distribution

Publish agent bundles to GitHub repositories using `GitHubDistributor`.
The distributor uses the [`gh` CLI](https://cli.github.com/) under the hood —
no direct HTTP calls, no embedded OAuth flows.

## Prerequisites

| Requirement | Why |
|---|---|
| `gh` CLI ≥ 2.0 | Repository creation, Contents API, releases |
| `GH_TOKEN` env var **or** `--token` flag | Authentication passed to `gh` via `GH_TOKEN` |
| Rust `base64` crate (bundled) | Encoding bundle contents for the Contents API |

## Quick Start

```bash
# 1. Generate and package a bundle
amplihack bundle generate "code review agent" --output-dir ~/reviewer
amplihack bundle package ~/reviewer --format tar.gz --output ./dist

# 2. Distribute to a public GitHub repo
export GH_TOKEN="ghp_your_token"
amplihack bundle distribute ./dist/reviewer.tar.gz --github
```

The distributor will:

1. Create the repository if it does not exist (public by default)
2. Upload the bundle archive via the GitHub Contents API
3. Create a tagged release with the bundle as an asset

## API Reference

### `GitHubDistributor`

```rust
use amplihack_utils::bundle_generator::GitHubDistributor;

let distributor = GitHubDistributor::new("ghp_your_token");
```

#### `distribute`

```rust
pub fn distribute(
    &self,
    bundle: &PackagedBundle,
    repo_name: &str,
) -> Result<DistributionResult, BundleGeneratorError>
```

Distributes the bundle to a **public** repository (default). Equivalent to
calling `distribute_with_visibility(bundle, repo_name, true)`.

#### `distribute_with_visibility`

```rust
pub fn distribute_with_visibility(
    &self,
    bundle: &PackagedBundle,
    repo_name: &str,
    public: bool,
) -> Result<DistributionResult, BundleGeneratorError>
```

Distributes with explicit visibility control. When `public` is `false`, the
repository is created with `--private`.

**Returns** a `DistributionResult` containing:

| Field | Type | Description |
|---|---|---|
| `success` | `bool` | `true` on success |
| `platform` | `DistributionPlatform` | Always `Github` |
| `url` | `Option<String>` | Release URL |
| `repository` | `Option<String>` | Repository URL |
| `release_tag` | `Option<String>` | Git tag (e.g. `v1.0.0`) |
| `distribution_time_seconds` | `f64` | Wall-clock time |

#### `create_repository`

```rust
pub fn create_repository(
    &self,
    repo_name: &str,
    description: &str,
    public: bool,
) -> Result<String, BundleGeneratorError>
```

Creates a GitHub repository. If the repository already exists, returns its URL
without error (idempotent). Descriptions longer than 100 characters are
truncated on a valid UTF-8 character boundary.

#### `push_bundle`

```rust
pub fn push_bundle(
    &self,
    repo_name: &str,
    package_path: &Path,
) -> Result<(), BundleGeneratorError>
```

Uploads the bundle archive to the repository via the GitHub Contents API
(`PUT /repos/{owner}/{repo}/contents/{path}`).

**Implementation details:**

- The file is base64-encoded using the `base64` crate (standard encoding).
- The JSON request body is written to a **temp file** and passed via
  `gh api --input <file>` to avoid hitting OS argument-length limits
  (`E2BIG`) on bundles larger than ~1.5 MB.
- If the file already exists in the repository, its SHA is fetched first and
  included in the PUT request for an **idempotent update**.

#### `create_release`

```rust
pub fn create_release(
    &self,
    repo_name: &str,
    tag: &str,
    asset_path: &Path,
) -> Result<String, BundleGeneratorError>
```

Creates a GitHub release tagged with `tag` and attaches the bundle file as a
release asset.

## Configuration

### Visibility

Repositories default to **public**. Use `--private` on the CLI or pass
`public: false` in the API:

```bash
# CLI
amplihack bundle distribute ./dist/agent.tar.gz --github --private

# Rust API
distributor.distribute_with_visibility(&bundle, "my-org/my-agent", false)?;
```

### Authentication

The distributor passes the token to the `gh` CLI via the `GH_TOKEN`
environment variable. Any token accepted by `gh` works:

- Personal access tokens (classic or fine-grained)
- GitHub App installation tokens
- `GITHUB_TOKEN` from CI (copy to `GH_TOKEN`)

```bash
# Option 1: Environment variable
export GH_TOKEN="ghp_xxxx"
amplihack bundle distribute ./dist/agent.tar.gz --github

# Option 2: CLI flag
amplihack bundle distribute ./dist/agent.tar.gz --github --token ghp_xxxx
```

## Pipeline Integration

Use `bundle pipeline` to run generate → package → distribute in one command:

```bash
amplihack bundle pipeline "log analyzer agent" \
  --output-dir ~/log-analyzer \
  --distribute

# With private repo
amplihack bundle pipeline "internal tool" \
  --output-dir ~/internal-tool \
  --distribute --private
```

## Error Handling

All errors are returned as `BundleGeneratorError::Distribution` with:

| Field | Content |
|---|---|
| `message` | Human-readable description (includes `gh` stderr) |
| `platform` | `Some("github")` |
| `http_status` | `None` (status comes from `gh` exit code) |

Common failure modes:

| Symptom | Cause | Fix |
|---|---|---|
| `Failed to invoke gh CLI` | `gh` not installed or not on `PATH` | Install `gh`: `brew install gh` / `apt install gh` |
| `Failed to create repository` | Token lacks `repo` scope | Use a token with `repo` scope |
| `Failed to push bundle` | Token lacks `contents:write` | Use a fine-grained token with Contents permission |
| `Failed to read bundle` | Package path does not exist | Check `--output` from the package step |

## Security Notes

- The distributor **never** makes direct HTTP calls — all network access goes
  through the `gh` CLI, inheriting its TLS and proxy configuration.
- Tokens are passed via environment variable (`GH_TOKEN`), never as CLI
  arguments (which would be visible in `/proc`).
- No credentials are persisted to disk.
- Bundle contents are base64-encoded in a temp file that is automatically
  cleaned up after the upload completes (Rust `tempfile::NamedTempFile`
  deletes on drop).

## Testing

Unit tests live inline in `crates/amplihack-utils/src/bundle_generator.rs`.
Run them with:

```bash
cargo test -p amplihack-utils -- bundle_generator
```

Key test cases:

| Test | What it verifies |
|---|---|
| `github_distributor_new_stores_token` | Constructor stores the token |
| `distribute_fails_without_gh` | Graceful error when `gh` is missing |
| `push_bundle_json_body_structure` | JSON body has `message`, `content`, optional `sha` |
| `truncate_to_char_boundary_*` | UTF-8 safe truncation (ASCII, multi-byte, exact boundary) |
| `base64_encoding_uses_crate` | Encoding matches `base64` crate output |
