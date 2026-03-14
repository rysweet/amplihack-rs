"""
WS3 E2E CI Patch.

Adds a 'test-e2e' job to .github/workflows/ci.yml.

The new job:
  - needs: [check, test]
  - continue-on-error: true
  - permissions: {contents: read}
  - Uses Swatinem/rust-cache for dependency caching
  - Installs recipe-runner-rs pinned to --rev <sha>
  - Runs cargo test --workspace --locked -- --ignored

Exported API:
  def load_ci_yaml(path: Path | str) -> dict
  def build_test_e2e_job(rev: str) -> dict
  def add_test_e2e_job(config: dict, rev: str) -> dict
  def validate_ci_yaml_structure(config: dict) -> None
  def write_ci_yaml(config: dict, path: Path | str) -> None

Usage:
  python scripts/ws3_e2e_ci_patch.py [--rev <sha>] [--ci-path <path>]
"""

from __future__ import annotations

import argparse
import copy
import sys
from pathlib import Path

import yaml

_RECIPE_RUNNER_REPO = "https://github.com/rysweet/recipe-runner-rs"
_DEFAULT_REV = "main"

# Path to the CI workflow relative to repo root
_REPO_ROOT = Path(__file__).resolve().parent.parent
_DEFAULT_CI_PATH = _REPO_ROOT / ".github" / "workflows" / "ci.yml"


def load_ci_yaml(path: Path | str) -> dict:
    """Load and parse a CI YAML file.

    Args:
        path: Path to the YAML file.

    Returns:
        Parsed YAML content as a dict.

    Raises:
        FileNotFoundError: If the file does not exist.
        yaml.YAMLError: If the file is not valid YAML.
    """
    p = Path(path)
    if not p.exists():
        raise FileNotFoundError(f"CI YAML file not found: {p}")

    with open(p, "r") as f:
        content = yaml.safe_load(f)

    if content is None:
        raise ValueError(f"CI YAML file is empty: {p}")

    return content


def build_test_e2e_job(rev: str) -> dict:
    """Build the test-e2e CI job definition.

    Args:
        rev: The git SHA to pin recipe-runner-rs to (--rev <sha>).

    Returns:
        A dict representing the GitHub Actions job definition.
    """
    return {
        "name": "E2E Tests",
        "runs-on": "ubuntu-latest",
        "needs": ["check", "test"],
        "continue-on-error": True,
        "permissions": {
            "contents": "read",
        },
        "steps": [
            {
                "uses": "actions/checkout@v4",
            },
            {
                "uses": "dtolnay/rust-toolchain@stable",
            },
            {
                "uses": "Swatinem/rust-cache@v2",
                "with": {
                    "key": "e2e",
                },
            },
            {
                "name": "Install recipe-runner-rs",
                "run": (
                    f"cargo install --git {_RECIPE_RUNNER_REPO} "
                    f"--rev {rev} recipe-runner-rs --locked"
                ),
            },
            {
                "name": "Run E2E tests (ignored)",
                "run": "cargo test --workspace --locked -- --ignored",
            },
        ],
    }


def add_test_e2e_job(config: dict, rev: str) -> dict:
    """Add the test-e2e job to a CI config dict.

    Non-mutating: returns a new dict. The original is not modified.
    Raises if the test-e2e job already exists (idempotency guard).

    Args:
        config: Parsed CI YAML as a dict.
        rev: The git SHA to pin recipe-runner-rs to.

    Returns:
        A new dict with the test-e2e job added.

    Raises:
        ValueError: If test-e2e already exists in the config.
    """
    if "test-e2e" in config.get("jobs", {}):
        raise ValueError(
            "test-e2e job already exists in ci.yml. "
            "Refusing to add it again to prevent double-patching."
        )

    new_config = copy.deepcopy(config)
    new_config["jobs"]["test-e2e"] = build_test_e2e_job(rev=rev)
    return new_config


def validate_ci_yaml_structure(config: dict) -> None:
    """Validate that the patched CI YAML has the expected structure.

    Args:
        config: Parsed CI YAML as a dict.

    Raises:
        ValueError: If required jobs or keys are missing.
        AssertionError: If structural invariants are violated.
    """
    if "jobs" not in config:
        raise ValueError("CI YAML must have a 'jobs' key")

    jobs = config["jobs"]

    if "test-e2e" not in jobs:
        raise ValueError(
            "CI YAML must contain 'test-e2e' job after patching. "
            f"Current jobs: {sorted(jobs.keys())}"
        )

    e2e = jobs["test-e2e"]

    if e2e.get("continue-on-error") is not True:
        raise ValueError(
            f"test-e2e must have continue-on-error: true. Got: {e2e.get('continue-on-error')!r}"
        )

    perms = e2e.get("permissions", {})
    if perms.get("contents") != "read":
        raise ValueError(
            f"test-e2e must have permissions.contents: read. Got: {perms!r}"
        )


def write_ci_yaml(config: dict, path: Path | str) -> None:
    """Write a CI config dict back to a YAML file.

    Uses yaml.safe_dump to avoid Python-specific YAML tags.

    Args:
        config: The CI config dict to write.
        path: Destination file path.
    """
    p = Path(path)
    with open(p, "w") as f:
        yaml.safe_dump(config, f, default_flow_style=False, sort_keys=False)


def main(argv: list[str] | None = None) -> int:
    """Patch ci.yml to add the test-e2e job.

    Returns:
        0 on success, 1 on failure.
    """
    parser = argparse.ArgumentParser(description="Add test-e2e job to ci.yml")
    parser.add_argument(
        "--rev",
        default=_DEFAULT_REV,
        help="Git SHA to pin recipe-runner-rs installation (default: %(default)s)",
    )
    parser.add_argument(
        "--ci-path",
        type=Path,
        default=_DEFAULT_CI_PATH,
        help="Path to ci.yml (default: %(default)s)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print patched YAML without writing to disk",
    )
    args = parser.parse_args(argv)

    config = load_ci_yaml(args.ci_path)
    patched = add_test_e2e_job(config, rev=args.rev)
    validate_ci_yaml_structure(patched)

    if args.dry_run:
        print(yaml.safe_dump(patched, default_flow_style=False, sort_keys=False))
    else:
        write_ci_yaml(patched, args.ci_path)
        print(f"Patched {args.ci_path} — added test-e2e job (rev={args.rev})")

    return 0


if __name__ == "__main__":
    sys.exit(main())
