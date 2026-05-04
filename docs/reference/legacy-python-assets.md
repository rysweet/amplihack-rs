# Legacy Python Asset Disposition

The legacy `rysweet/amplihack` documentation tree included Python source,
examples, and tests under `docs/`. `amplihack-rs` intentionally does not ship
those files because runtime, hook, recipe, and install paths are Rust/native.

## Replacement status

| Legacy area | Status in `amplihack-rs` |
| --- | --- |
| Hook scripts and hook support tools | Replaced by the native `amplihack-hooks` binary and `crates/amplihack-hooks/`. |
| Install, update, launcher, recipe, and workflow runtime helpers | Replaced by `amplihack`, `amplihack-launcher`, `amplihack-workflows`, and related Rust crates. |
| Invisible character scanning | Replaced by the Rust `scan-invisible-chars` binary. |
| Memory, reflection, fleet, and remote runtime modules | Replaced by Rust crates under `crates/amplihack-*`. |
| Historical docs examples, skill helper scripts, and Python test harnesses | Not copied one-for-one. The Markdown documentation is preserved where useful, but Python source assets are omitted unless and until they have a native implementation. |

## Why not copy the files as examples?

Keeping `.py`, `pyproject.toml`, `requirements*.txt`, or similar package files in
this repository would make the Rust migration ambiguous and would defeat the
no-Python asset guard. The guard intentionally fails if tracked Python
implementation/package assets are reintroduced.

## How to validate

```bash
scripts/check-no-python-assets.sh
scripts/check-recipes-no-python.sh
scripts/probe-no-python.sh
```
