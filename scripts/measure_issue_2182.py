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
import sys
import tempfile


def account(command, env, log):
    """Read each direct/adopted zombie once, before reaping it."""
    process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
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
        if pid == process.pid:
            exit_code = code
            process.returncode = code
    return exit_code, records


def self_test():
    # Distinct payload sizes make missing and duplicate accounting visible.
    program = r'''
import os, sys
from pathlib import Path
root = Path(sys.argv[1])
def write(n):
    with (root / str(n)).open("wb", buffering=0) as f:
        data = bytes([n]) * (n * 1024 * 1024)
        while data:
            data = data[os.write(f.fileno(), data):]
        os.fsync(f.fileno())
write(1)
child = os.fork()
if child == 0:
    write(2)
    grandchild = os.fork()
    if grandchild == 0:
        write(3)
        os._exit(0)
    os.waitpid(grandchild, 0)
    os._exit(0)
os.waitpid(child, 0)
launcher = os.fork()
if launcher == 0:
    if os.fork() == 0:
        write(4)
    os._exit(0)
os.waitpid(launcher, 0)
'''
    with tempfile.TemporaryDirectory(prefix="issue2182-accounting-") as root:
        with open(os.devnull, "wb") as log:
            code, records = account([sys.executable, "-c", program, root], os.environ.copy(), log)
        expected = 10 * 1024 * 1024
        observed = sum(r["io"]["wchar"] for r in records)
        assert code == 0 and all(r["exit_code"] == 0 for r in records), records
        assert observed == expected, (expected, observed, records)
        assert sum(p.stat().st_size for p in Path(root).iterdir()) == expected
        print(json.dumps(dict(expected_wchar=expected, observed_wchar=observed, processes=records), indent=2))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scratch", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "cannot enable descendant accounting")
    if args.self_test:
        return self_test()
    if args.output is None or args.scratch is None:
        parser.error("--output and --scratch are required for suite measurements")
    args.output.mkdir(parents=True, exist_ok=False)
    args.scratch.mkdir(parents=True, exist_ok=True)
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
            exit_code, records = account(command, env, log)
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
