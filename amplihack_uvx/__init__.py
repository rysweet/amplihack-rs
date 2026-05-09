"""uvx entrypoint for the native amplihack Rust CLI."""

from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
from importlib import metadata
from pathlib import Path
from urllib.parse import unquote, urlparse

DIST_NAME = "amplihack"
DEFAULT_REPO_URL = "https://github.com/rysweet/amplihack-rs.git"


def _binary_name(name: str) -> str:
    return f"{name}.exe" if os.name == "nt" else name


def _cache_root(commit_or_ref: str) -> Path:
    explicit = os.environ.get("AMPLIHACK_UVX_CACHE")
    if explicit:
        return Path(explicit).expanduser().resolve()
    platform_key = f"{sys.platform}-{platform.machine()}"
    return (
        Path.home()
        / ".cache"
        / "amplihack"
        / "uvx-wrapper"
        / commit_or_ref
        / platform_key
    )


def _local_path_from_file_url(url: str) -> Path | None:
    parsed = urlparse(url)
    if parsed.scheme != "file":
        return None
    return Path(unquote(parsed.path)).resolve()


def _source_from_direct_url() -> tuple[str, str | None, Path | None]:
    dist = metadata.distribution(DIST_NAME)
    raw = dist.read_text("direct_url.json")
    if raw is None:
        return DEFAULT_REPO_URL, None, None

    data = json.loads(raw)
    url = data.get("url")
    if not isinstance(url, str) or not url:
        return DEFAULT_REPO_URL, None, None

    local_path = _local_path_from_file_url(url)
    vcs_info = data.get("vcs_info")
    if isinstance(vcs_info, dict):
        commit_id = vcs_info.get("commit_id")
        requested_revision = vcs_info.get("requested_revision")
        ref = commit_id if isinstance(commit_id, str) and commit_id else requested_revision
        return url, ref if isinstance(ref, str) and ref else None, None

    return url, None, local_path


def _run_checked(args: list[str], env: dict[str, str]) -> None:
    result = subprocess.run(args, env=env, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"{args[0]} exited with code {result.returncode}")


def _cargo_install_args(
    package: str,
    path_name: str,
    install_root: Path,
    repo_url: str,
    ref: str | None,
    local_path: Path | None,
) -> list[str]:
    args = ["cargo", "install", "--root", str(install_root), "--locked"]
    if local_path is not None:
        args.extend(["--path", str(local_path / "bins" / path_name)])
    else:
        args.extend(["--git", repo_url])
        if ref:
            args.extend(["--rev", ref])
        args.append(package)
    return args


def _ensure_native_binaries() -> Path:
    cargo = shutil.which("cargo")
    if cargo is None:
        raise RuntimeError("cargo is required to install amplihack from uvx")

    repo_url, ref, local_path = _source_from_direct_url()
    cache_key = ref or (str(local_path) if local_path is not None else "latest")
    install_root = _cache_root(cache_key.replace(os.sep, "_"))
    bin_dir = install_root / "bin"
    main_binary = bin_dir / _binary_name("amplihack")
    hooks_binary = bin_dir / _binary_name("amplihack-hooks")
    if main_binary.exists() and hooks_binary.exists():
        return main_binary

    install_root.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(install_root / "target")
    for package, path_name in (
        ("amplihack", "amplihack"),
        ("amplihack-hooks-bin", "amplihack-hooks"),
    ):
        _run_checked(
            _cargo_install_args(package, path_name, install_root, repo_url, ref, local_path),
            env,
        )
    return main_binary


def main() -> int:
    try:
        binary = _ensure_native_binaries()
        env = os.environ.copy()
        env["PATH"] = f"{binary.parent}{os.pathsep}{env.get('PATH', '')}"
        os.execvpe(str(binary), [str(binary), *sys.argv[1:]], env)
    except (OSError, RuntimeError, metadata.PackageNotFoundError, json.JSONDecodeError) as error:
        print(f"amplihack uvx wrapper failed: {error}", file=sys.stderr)
        return 1
    return 1
