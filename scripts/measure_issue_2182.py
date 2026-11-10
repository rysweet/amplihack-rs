#!/usr/bin/env python3
"""Run the exact CLI suite with descendant-inclusive Linux I/O accounting.

Run from each revision with equivalent warmed caches and the same filesystem.
The script is a child subreaper: descendants abandoned by Cargo are adopted,
accounted, and waited for too. Linux /proc/PID/io includes waited-for children;
reading each direct child's zombie counters before wait avoids losing those
counters and avoids double counting already reaped nested builds.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    args.scratch.mkdir(parents=True, exist_ok=True)
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "cannot enable descendant accounting")
    env = os.environ.copy()
    original_home = Path.home()
    for key, suffix in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")]:
        env.setdefault(key, str(original_home / suffix))
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    with tempfile.TemporaryDirectory(dir=args.scratch, prefix="issue2182-") as root:
        root = Path(root)
        home = root / "home"
        home.mkdir()
        temp = root / "tmp"
        temp.mkdir()
        env.update(HOME=str(home), TMPDIR=str(temp),
                   AMPLIHACK_HOME=str(home / ".amplihack"),
                   CLAUDE_CONFIG_DIR=str(home / ".claude"),
                   AMPLIHACK_CLAUDE_PROFILE_ROOT=str(home / "profiles"))
        command = ["cargo", "test", "-p", "amplihack-cli"]
        with (args.output / "suite.log").open("wb") as log:
            process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
            root_pid = process.pid
            records = []
            exit_code = None
            while True:
                try:
                    zombie = os.waitid(os.P_ALL, 0, os.WEXITED | os.WNOWAIT)
                except ChildProcessError:
                    break
                pid = zombie.si_pid
                counters = {}
                for line in Path(f"/proc/{pid}/io").read_text().splitlines():
                    key, value = line.split(":", 1)
                    counters[key] = int(value)
                _, status, usage = os.wait4(pid, 0)
                code = os.waitstatus_to_exitcode(status)
                records.append(dict(pid=pid, exit_code=code, io=counters,
                                    output_block_bytes=usage.ru_oublock * 512))
                if pid == root_pid:
                    exit_code = code
                    process.returncode = code
        report = dict(revision=revision, command=command, exit_code=exit_code,
                      filesystem_device=root.stat().st_dev, processes=records,
                      write_bytes=sum(r["io"]["write_bytes"] for r in records),
                      cancelled_write_bytes=sum(r["io"]["cancelled_write_bytes"] for r in records),
                      limitations=["Linux counters are logical kernel accounting, not device writes",
                                   "shared caches and unrelated build activity must be controlled externally",
                                   "no logical staging-volume inference from I/O counters alone"])
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return exit_code if exit_code is not None and exit_code >= 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
