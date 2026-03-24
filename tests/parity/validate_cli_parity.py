#!/usr/bin/env python3
"""CLI parity runner for native Rust command migration.

Runs Python and Rust implementations in isolated sandboxes, compares outputs
and filesystem side effects, and optionally mirrors both runs in side-by-side
tmux panes for observable debugging.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml


@dataclass
class EngineResult:
    stdout: str
    stderr: str
    exit_code: int
    sandbox_root: Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate Python↔Rust CLI parity")
    parser.add_argument(
        "--scenario",
        default=Path(__file__).with_name("scenarios").joinpath("tier1.yaml"),
        type=Path,
        help="Scenario YAML file",
    )
    parser.add_argument(
        "--python-repo",
        default=Path.home() / "src" / "amploxy",
        type=Path,
        help="Python repo root containing src/amplihack",
    )
    parser.add_argument(
        "--python-exe",
        type=Path,
        help="Python interpreter to use for the Python CLI (defaults to repo .venv if present)",
    )
    parser.add_argument(
        "--rust-binary",
        default=Path.home() / "src" / "amplihack-rs" / "target" / "debug" / "amplihack",
        type=Path,
        help="Rust amplihack binary to compare",
    )
    parser.add_argument(
        "--rust-hooks-binary",
        default=Path.home() / "src" / "amplihack-rs" / "target" / "debug" / "amplihack-hooks",
        type=Path,
        help="Rust amplihack-hooks binary for direct hook runtime cases",
    )
    parser.add_argument(
        "--observable",
        action="store_true",
        help="Run each case in side-by-side tmux panes with tee-backed logs",
    )
    parser.add_argument(
        "--keep-sandboxes",
        action="store_true",
        help="Do not delete per-engine sandbox directories",
    )
    parser.add_argument(
        "--case",
        action="append",
        help="Run only the named scenario case (repeatable)",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="Optional JSON report output path",
    )
    parser.add_argument(
        "--shadow-mode",
        action="store_true",
        help="Log divergences but do not fail the overall run",
    )
    parser.add_argument(
        "--shadow-log",
        type=Path,
        help="Optional JSONL file for shadow-mode divergence records",
    )
    parser.add_argument(
        "--ssh-target",
        help="Optional SSH target (for example azlin) that runs the parity harness remotely",
    )
    parser.add_argument(
        "--remote-rs-repo",
        type=Path,
        help="Remote amplihack-rs checkout root when using --ssh-target",
    )
    parser.add_argument(
        "--remote-python-repo",
        type=Path,
        help="Remote amploxy checkout root when using --ssh-target",
    )
    parser.add_argument(
        "--remote-python-exe",
        type=Path,
        help="Remote Python interpreter when using --ssh-target",
    )
    parser.add_argument(
        "--remote-rust-binary",
        type=Path,
        help="Remote Rust amplihack binary when using --ssh-target",
    )
    parser.add_argument(
        "--remote-scenario",
        type=Path,
        help="Remote scenario path when using --ssh-target",
    )
    parser.add_argument(
        "--remote-report",
        type=Path,
        help="Remote report path when using --ssh-target",
    )
    parser.add_argument(
        "--remote-shadow-log",
        type=Path,
        help="Remote shadow log path when using --ssh-target",
    )
    return parser.parse_args()


def load_scenarios(path: Path) -> list[dict[str, Any]]:
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    cases = data.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ValueError(f"No cases found in {path}")
    return cases


def resolve_python_exe(explicit: Path | None, python_repo: Path) -> Path:
    if explicit is not None:
        return explicit.expanduser().absolute()
    candidate = python_repo / ".venv" / "bin" / "python"
    if candidate.exists():
        return candidate.absolute()
    return Path(sys.executable).absolute()


def resolve_engine_command(
    *,
    engine_name: str,
    case: dict[str, Any],
    python_exe: Path,
    python_repo: Path,
    rust_binary: Path,
    rust_hooks_binary: Path,
) -> list[str]:
    override = case.get(f"{engine_name}_command")
    if override is None:
        if engine_name == "python":
            return [str(python_exe), "-m", "amplihack", *case["argv"]]
        return [str(rust_binary), *case["argv"]]

    replacements = {
        "${PYTHON_EXE}": str(python_exe),
        "${PYTHON_REPO}": str(python_repo),
        "${PYTHON_HOOKS_DIR}": str(python_repo / ".claude" / "tools" / "amplihack" / "hooks"),
        "${RUST_BINARY}": str(rust_binary),
        "${RUST_HOOKS_BINARY}": str(rust_hooks_binary),
    }
    if isinstance(override, list):
        return [render_command_token(str(item), replacements) for item in override]
    if isinstance(override, str):
        return [render_command_token(override, replacements)]
    raise ValueError(f"{engine_name}_command must be a list or string")


def render_command_token(value: str, replacements: dict[str, str]) -> str:
    rendered = value
    for placeholder, replacement in replacements.items():
        rendered = rendered.replace(placeholder, replacement)
    return rendered


def main() -> int:
    args = parse_args()
    if args.ssh_target:
        return run_remote_harness(args)
    python_repo = args.python_repo.resolve()
    python_exe = resolve_python_exe(args.python_exe, python_repo)
    rust_binary = args.rust_binary.resolve()
    rust_hooks_binary = args.rust_hooks_binary.resolve()

    if not python_repo.joinpath("src", "amplihack", "cli.py").exists():
        raise FileNotFoundError(f"Python repo not found: {python_repo}")
    if not python_exe.exists():
        raise FileNotFoundError(f"Python executable not found: {python_exe}")
    if not rust_binary.exists():
        raise FileNotFoundError(f"Rust binary not found: {rust_binary}")
    if not rust_hooks_binary.exists():
        raise FileNotFoundError(f"Rust hooks binary not found: {rust_hooks_binary}")

    cases = load_scenarios(args.scenario)
    if args.case:
        requested = set(args.case)
        cases = [case for case in cases if case["name"] in requested]
        if not cases:
            raise ValueError(f"No matching cases for {sorted(requested)}")
    report: list[dict[str, Any]] = []
    failures = 0
    divergences: list[dict[str, Any]] = []

    for case in cases:
        name = case["name"]
        print(f"\n=== {name} ===")
        session_name = f"cli-parity-{int(time.time())}-{sanitize_name(name)}" if args.observable else None
        py_result = run_engine_case(
            engine_name="python",
            command=resolve_engine_command(
                engine_name="python",
                case=case,
                python_exe=python_exe,
                python_repo=python_repo,
                rust_binary=rust_binary,
                rust_hooks_binary=rust_hooks_binary,
            ),
            case=case,
            python_repo=python_repo,
            observable=args.observable,
            session_name=session_name,
        )
        rust_result = run_engine_case(
            engine_name="rust",
            command=resolve_engine_command(
                engine_name="rust",
                case=case,
                python_exe=python_exe,
                python_repo=python_repo,
                rust_binary=rust_binary,
                rust_hooks_binary=rust_hooks_binary,
            ),
            case=case,
            python_repo=python_repo,
            observable=args.observable,
            session_name=session_name,
        )
        match, details = compare_case(case, py_result, rust_result)
        report.append({"name": name, "match": match, "details": details})

        if match:
            print("PASS")
        elif args.shadow_mode:
            divergences.append({"name": name, "details": details})
            print("DIVERGED")
        else:
            failures += 1
            print("FAIL")
            print(json.dumps(details, indent=2))

        if not args.keep_sandboxes:
            shutil.rmtree(py_result.sandbox_root, ignore_errors=True)
            shutil.rmtree(rust_result.sandbox_root, ignore_errors=True)

    if args.report:
        args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
    if args.shadow_log:
        args.shadow_log.parent.mkdir(parents=True, exist_ok=True)
        with args.shadow_log.open("w", encoding="utf-8") as handle:
            for item in divergences:
                handle.write(json.dumps(item, sort_keys=True))
                handle.write("\n")

    summary = f"\nSummary: {len(cases) - failures}/{len(cases)} cases matched"
    if args.shadow_mode:
        summary += f", {len(divergences)} divergences logged"
    print(summary)
    if args.shadow_mode:
        return 0
    return 0 if failures == 0 else 1


def run_remote_harness(args: argparse.Namespace) -> int:
    local_repo_root = Path(__file__).resolve().parents[2]
    remote_repo_root = resolve_remote_repo_root(args, local_repo_root)
    remote_python_repo = args.remote_python_repo or args.python_repo
    remote_script = remote_repo_root / "tests" / "parity" / "validate_cli_parity.py"
    remote_scenario = resolve_remote_path(
        args.scenario,
        local_repo_root,
        remote_repo_root,
        explicit=args.remote_scenario,
    )
    remote_rust_binary = args.remote_rust_binary or remote_repo_root / "target" / "debug" / "amplihack"
    remote_report = args.remote_report or Path(f"/tmp/cli-parity-report-{int(time.time())}.json")
    remote_shadow_log = args.remote_shadow_log or Path(f"/tmp/cli-parity-shadow-{int(time.time())}.jsonl")

    remote_command = [
        "python3",
        str(remote_script),
        "--scenario",
        str(remote_scenario),
        "--python-repo",
        str(remote_python_repo),
        "--rust-binary",
        str(remote_rust_binary),
    ]
    if args.remote_python_exe:
        remote_command.extend(["--python-exe", str(args.remote_python_exe)])
    if args.observable:
        remote_command.append("--observable")
    if args.keep_sandboxes:
        remote_command.append("--keep-sandboxes")
    if args.shadow_mode:
        remote_command.append("--shadow-mode")
    if args.report:
        remote_command.extend(["--report", str(remote_report)])
    if args.shadow_log:
        remote_command.extend(["--shadow-log", str(remote_shadow_log)])
    for case_name in args.case or []:
        remote_command.extend(["--case", case_name])

    completed = subprocess.run(
        ["ssh", args.ssh_target, "bash", "-lc", shlex.quote(shlex.join(remote_command))],
        text=True,
    )
    if completed.returncode != 0:
        return completed.returncode

    if args.report:
        fetch_remote_file(args.ssh_target, remote_report, args.report)
    if args.shadow_log:
        fetch_remote_file(args.ssh_target, remote_shadow_log, args.shadow_log)
    return 0


def resolve_remote_repo_root(args: argparse.Namespace, local_repo_root: Path) -> Path:
    if args.remote_rs_repo:
        return args.remote_rs_repo
    return local_repo_root


def resolve_remote_path(
    local_path: Path,
    local_repo_root: Path,
    remote_repo_root: Path,
    *,
    explicit: Path | None,
) -> Path:
    if explicit is not None:
        return explicit
    try:
        relative = local_path.resolve().relative_to(local_repo_root)
    except ValueError:
        raise ValueError(
            f"Cannot infer remote path for {local_path}; pass an explicit remote path"
        ) from None
    return remote_repo_root / relative


def fetch_remote_file(target: str, remote_path: Path, local_path: Path) -> None:
    local_path.parent.mkdir(parents=True, exist_ok=True)
    content = subprocess.run(
        ["ssh", target, "cat", str(remote_path)],
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    local_path.write_text(content, encoding="utf-8")


def run_engine_case(
    *,
    engine_name: str,
    command: list[str],
    case: dict[str, Any],
    python_repo: Path,
    observable: bool,
    session_name: str | None,
) -> EngineResult:
    sandbox_root = Path(tempfile.mkdtemp(prefix=f"cli-parity-{engine_name}-"))
    env = build_env(case, sandbox_root, python_repo)
    run_setup(case.get("setup"), sandbox_root, env)

    cwd = sandbox_root / case.get("cwd", ".")
    cwd.mkdir(parents=True, exist_ok=True)
    stdin = case.get("stdin", "")
    timeout = int(case.get("timeout", 30))

    if observable:
        return run_observable(
            engine_name, command, cwd, env, stdin, timeout, sandbox_root, session_name
        )
    return run_direct(command, cwd, env, stdin, timeout, sandbox_root)


def build_env(case: dict[str, Any], sandbox_root: Path, python_repo: Path) -> dict[str, str]:
    env = os.environ.copy()
    home = sandbox_root / "home"
    stubs = sandbox_root / "python-stubs"
    memory_stub = stubs / "amplihack_memory"
    tmpdir = sandbox_root / "tmp"
    home.mkdir(parents=True, exist_ok=True)
    stubs.mkdir(parents=True, exist_ok=True)
    memory_stub.mkdir(parents=True, exist_ok=True)
    tmpdir.mkdir(parents=True, exist_ok=True)
    env["HOME"] = str(home)
    env["TMPDIR"] = str(tmpdir)
    env["TMP"] = str(tmpdir)
    env["TEMP"] = str(tmpdir)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    python_paths = [str(python_repo / "src")]
    if case.get("env", {}).get("AMPLIHACK_PARITY_NO_MEMORY_STUB") != "1":
        (memory_stub / "__init__.py").write_text(
            """
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum


class ExperienceType(Enum):
    SUCCESS = "success"
    FAILURE = "failure"


@dataclass
class Experience:
    experience_type: ExperienceType = ExperienceType.SUCCESS
    context: str = ""
    outcome: str = ""
    confidence: float = 1.0
    tags: list[str] = field(default_factory=list)
    metadata: dict = field(default_factory=dict)
    timestamp: datetime = field(default_factory=datetime.utcnow)
    experience_id: str = "stub-experience"


class _StubConnector:
    def store_experience(self, experience):
        return getattr(experience, "experience_id", "stub-experience")

    def retrieve_experiences(self, **kwargs):
        return []

    def close(self):
        return None


class ExperienceStore:
    def __init__(self, **kwargs):
        self.connector = _StubConnector()

    def search(self, **kwargs):
        return []

    def get_statistics(self):
        return {}
""".strip()
            + "\n",
            encoding="utf-8",
        )
        python_paths.insert(0, str(stubs))
    env["PYTHONPATH"] = os.pathsep.join(python_paths)
    env["SANDBOX_ROOT"] = str(sandbox_root)
    for key, value in case.get("env", {}).items():
        rendered = str(value)
        rendered = rendered.replace("${SANDBOX_ROOT}", str(sandbox_root))
        rendered = rendered.replace("${HOME}", str(home))
        rendered = rendered.replace("${PATH}", env.get("PATH", ""))
        env[str(key)] = rendered
    return env


def run_setup(setup: Any, sandbox_root: Path, env: dict[str, str]) -> None:
    if not setup:
        return

    if isinstance(setup, list):
        script = "\n".join(str(item) for item in setup)
    else:
        script = str(setup)

    subprocess.run(
        ["bash", "-lc", script],
        cwd=sandbox_root,
        env=env,
        text=True,
        capture_output=True,
        check=True,
    )


def run_direct(
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    stdin: str,
    timeout: int,
    sandbox_root: Path,
) -> EngineResult:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            input=stdin,
            text=True,
            capture_output=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as e:
        return EngineResult(
            stdout=e.stdout.decode("utf-8", errors="replace") if e.stdout else "",
            stderr=f"TIMEOUT after {timeout}s: {' '.join(command[:3])}",
            exit_code=124,
            sandbox_root=sandbox_root,
        )
    return EngineResult(
        stdout=result.stdout,
        stderr=result.stderr,
        exit_code=result.returncode,
        sandbox_root=sandbox_root,
    )


def run_observable(
    engine_name: str,
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    stdin: str,
    timeout: int,
    sandbox_root: Path,
    session_name: str | None,
) -> EngineResult:
    if not session_name:
        raise ValueError("observable runs require a tmux session name")
    stdout_log = sandbox_root / f"{engine_name}.stdout.log"
    stderr_log = sandbox_root / f"{engine_name}.stderr.log"
    exit_file = sandbox_root / f"{engine_name}.exit"
    stdin_file = sandbox_root / f"{engine_name}.stdin"
    stdin_file.write_text(stdin, encoding="utf-8")

    command_script = render_tmux_script(command, cwd, env, stdin_file, stdout_log, stderr_log, exit_file)

    if engine_name == "python":
        subprocess.run(["tmux", "new-session", "-d", "-s", session_name, command_script], check=True)
    else:
        subprocess.run(["tmux", "split-window", "-h", "-t", session_name, command_script], check=True)
        subprocess.run(["tmux", "select-layout", "-t", session_name, "even-horizontal"], check=True)

    deadline = time.time() + timeout
    while time.time() < deadline:
        if exit_file.exists():
            break
        time.sleep(0.2)

    if not exit_file.exists():
        raise TimeoutError(f"{engine_name} command timed out: {' '.join(command)}")

    return EngineResult(
        stdout=stdout_log.read_text(encoding="utf-8") if stdout_log.exists() else "",
        stderr=stderr_log.read_text(encoding="utf-8") if stderr_log.exists() else "",
        exit_code=int(exit_file.read_text(encoding="utf-8") or "1"),
        sandbox_root=sandbox_root,
    )


def render_tmux_script(
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    stdin_file: Path,
    stdout_log: Path,
    stderr_log: Path,
    exit_file: Path,
) -> str:
    exports = " ".join(
        f"export {key}={shlex.quote(value)};"
        for key, value in env.items()
        if key in {"HOME", "PYTHONDONTWRITEBYTECODE", "PYTHONPATH", "SANDBOX_ROOT"}
    )
    body = (
        "set -o pipefail;"
        f" cd {shlex.quote(str(cwd))};"
        f" {exports}"
        f" {shlex.join(command)} < {shlex.quote(str(stdin_file))}"
        f" > >(tee {shlex.quote(str(stdout_log))})"
        f" 2> >(tee {shlex.quote(str(stderr_log))} >&2);"
        " status=$?;"
        f" printf '%s' \"$status\" > {shlex.quote(str(exit_file))};"
        " printf '\\n[parity] exit=%s\\n' \"$status\";"
        " exec bash"
    )
    return f"bash -lc {shlex.quote(body)}"


def compare_case(case: dict[str, Any], py: EngineResult, rust: EngineResult) -> tuple[bool, dict[str, Any]]:
    details: dict[str, Any] = {}
    success = True

    for item in case.get("compare", ["stdout", "stderr", "exit_code"]):
        if item == "stdout":
            ok, reason = compare_text(py.stdout, rust.stdout, py.sandbox_root, rust.sandbox_root)
            details["stdout"] = reason
            success &= ok
        elif item == "stderr":
            ok, reason = compare_text(py.stderr, rust.stderr, py.sandbox_root, rust.sandbox_root)
            details["stderr"] = reason
            success &= ok
        elif item == "exit_code":
            ok = py.exit_code == rust.exit_code
            details["exit_code"] = {"python": py.exit_code, "rust": rust.exit_code, "match": ok}
            success &= ok
        elif str(item).startswith("fs:"):
            rel_path = str(item)[3:]
            ok, reason = compare_snapshots(
                snapshot_path(py.sandbox_root / rel_path),
                snapshot_path(rust.sandbox_root / rel_path),
                py.sandbox_root,
                rust.sandbox_root,
            )
            details[f"fs:{rel_path}"] = reason
            success &= ok
        elif str(item).startswith("jsonfs:"):
            rel_path = str(item)[7:]
            ok, reason = compare_json_files(
                py.sandbox_root / rel_path,
                rust.sandbox_root / rel_path,
                py.sandbox_root,
                rust.sandbox_root,
            )
            details[f"jsonfs:{rel_path}"] = reason
            success &= ok
        else:
            raise ValueError(f"Unknown compare target: {item}")

    return success, details


def compare_text(left: str, right: str, left_root: Path, right_root: Path) -> tuple[bool, dict[str, Any]]:
    left = normalize_text(left, left_root)
    right = normalize_text(right, right_root)
    left_json = try_json(left)
    right_json = try_json(right)
    if left_json is not None and right_json is not None:
        ok = left_json == right_json
        return ok, {"mode": "json", "match": ok, "python": left_json, "rust": right_json}

    ok = left == right
    return ok, {"mode": "text", "match": ok, "python": left, "rust": right}


def try_json(value: str) -> Any | None:
    stripped = value.strip()
    if not stripped:
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        return None


def normalize_text(value: str, sandbox_root: Path) -> str:
    normalized = value.replace(str(sandbox_root), "<SANDBOX_ROOT>")
    normalized = normalized.replace(str(sandbox_root / "home"), "<HOME>")
    normalized = re.sub(r"<SANDBOX_ROOT>/tmp/[^\s'\)]+", "<TMPDIR>", normalized)
    normalized = re.sub(r"settings\.json\.backup\.\d+", "settings.json.backup.<TS>", normalized)
    normalized = re.sub(r"install_\d+_backup\.json", "install_<TS>_backup.json", normalized)
    return normalized


def snapshot_path(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False}
    if path.is_file():
        return {
            "exists": True,
            "type": "file",
            "sha256": sha256(path.read_bytes()),
            "text": decode_if_text(path.read_bytes()),
        }
    if path.is_symlink():
        return {"exists": True, "type": "symlink", "target": os.readlink(path)}

    entries: dict[str, Any] = {}
    for child in sorted(path.rglob("*")):
        rel = child.relative_to(path).as_posix()
        if child.is_dir():
            entries[rel] = {"type": "dir"}
        elif child.is_symlink():
            entries[rel] = {"type": "symlink", "target": os.readlink(child)}
        else:
            data = child.read_bytes()
            entries[rel] = {
                "type": "file",
                "sha256": sha256(data),
                "text": decode_if_text(data),
            }

    return {"exists": True, "type": "dir", "entries": entries}


def compare_snapshots(
    left: dict[str, Any],
    right: dict[str, Any],
    left_root: Path,
    right_root: Path,
) -> tuple[bool, dict[str, Any]]:
    left = normalize_snapshot_value(left, left_root)
    right = normalize_snapshot_value(right, right_root)
    ok = left == right
    return ok, {"match": ok, "python": left, "rust": right}


def normalize_snapshot_value(value: Any, sandbox_root: Path) -> Any:
    if isinstance(value, dict):
        if value.get("type") == "file" and isinstance(value.get("text"), str):
            normalized_text = normalize_text(value["text"], sandbox_root)
            normalized = dict(value)
            normalized["text"] = normalized_text
            normalized["sha256"] = sha256(normalized_text.encode("utf-8"))
            return normalized
        return {key: normalize_snapshot_value(item, sandbox_root) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize_snapshot_value(item, sandbox_root) for item in value]
    if isinstance(value, str):
        return normalize_text(value, sandbox_root)
    return value


def compare_json_files(
    left_path: Path,
    right_path: Path,
    left_root: Path,
    right_root: Path,
) -> tuple[bool, dict[str, Any]]:
    left = load_json_path(left_path, left_root)
    right = load_json_path(right_path, right_root)
    ok = left == right
    return ok, {"match": ok, "python": left, "rust": right}


def load_json_path(path: Path, sandbox_root: Path) -> Any:
    if not path.exists():
        return {"exists": False}
    data = json.loads(path.read_text(encoding="utf-8"))
    return normalize_json_value(data, sandbox_root)


def normalize_json_value(value: Any, sandbox_root: Path) -> Any:
    if isinstance(value, dict):
        normalized: dict[str, Any] = {}
        for key, item in value.items():
            if key == "exported_at":
                normalized[key] = "<TIMESTAMP>"
            else:
                normalized[key] = normalize_json_value(item, sandbox_root)
        return normalized
    if isinstance(value, list):
        return [normalize_json_value(item, sandbox_root) for item in value]
    if isinstance(value, str):
        return normalize_text(value, sandbox_root)
    return value


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def decode_if_text(data: bytes) -> str | None:
    if len(data) > 4096:
        return None
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None


def sanitize_name(value: str) -> str:
    return "".join(ch if ch.isalnum() or ch in {"-", "_"} else "-" for ch in value)[:40]


if __name__ == "__main__":
    sys.exit(main())
