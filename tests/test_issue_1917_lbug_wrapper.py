"""Executable RED contracts for the real repository wrapper and hook routing.

Run: python3 -m unittest discover -s tests -p test_issue_1917_lbug_wrapper.py -v
Only Cargo/tool boundaries are faked. The wrapper/hook under test is copied
unchanged into an owned source fixture. No app, compiler, profile guard,
credentials, normal Cargo cache, or network client is executed.
"""
import json
import os
from pathlib import Path
import shutil
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parents[1]
WRAPPER = REPO / "scripts/cargo-with-lbug.sh"


class WrapperContracts(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory(prefix="issue-1917-wrapper-")
        self.addCleanup(self.fixture.cleanup)
        self.root = Path(self.fixture.name)
        self.bin = self.root / "tools"
        self.source = self.root / "source tree"
        self.bin.mkdir()
        (self.source / "scripts").mkdir(parents=True)
        for directory in ("home", "cache", "cargo-home", "tmp"):
            (self.root / directory).mkdir(mode=0o700)
        self.log = self.root / "calls.jsonl"
        self.env = {
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(self.root / "home"),
            "CARGO_HOME": str(self.root / "cargo-home"),
            "TMPDIR": str(self.root / "tmp"),
            "XDG_CACHE_HOME": str(self.root / "cache"),
            "AMPLIHACK_LBUG_CACHE_DIR": str(self.root / "cache/lbug"),
            "AMPLIHACK_LBUG_CACHE": "source",
            "AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS": "0",
            "CARGO_BUILD_JOBS": "2",
            "CMAKE_BUILD_PARALLEL_LEVEL": "2",
            "TEST_LOG": str(self.log),
            "TEST_ROOT": str(self.source),
        }
        self.write_tool("cargo", r'''
import json, os, signal, sys, time
record = {"tool": "cargo", "argv": sys.argv[1:], "cwd": os.getcwd(),
          "env": {k: v for k, v in os.environ.items() if k.startswith(("LBUG_", "AMPLIHACK_LBUG_", "CARGO_", "GIT_", "RUSTUP_"))}}
with open(os.environ["TEST_LOG"], "a") as f:
    f.write(json.dumps(record) + "\n")
if os.environ.get("TEST_WAIT") == "1":
    def terminate(*_):
        with open(os.environ["TEST_LOG"], "a") as f:
            f.write(json.dumps({"tool": "cargo-terminated"}) + "\n")
        sys.exit(143)
    signal.signal(signal.SIGTERM, terminate)
    open(os.environ["TEST_READY"], "w").close()
    while True:
        time.sleep(0.05)
sys.stdout.write('{"reason":"compiler-artifact","fixture":true}\n')
sys.stderr.write("cargo fixture stderr\n")
sys.exit(int(os.environ.get("TEST_STATUS", "0")))
''')
        self.write_tool("git", 'import os\nprint(os.environ["TEST_ROOT"])\n')
        for tool in ("curl", "wget", "gh", "cmake", "c++", "g++", "clang++", "rustc", "rustup"):
            self.write_tool(tool, f'import sys\nsys.stderr.write("forbidden tool: {tool}\\n")\nsys.exit(91)\n')

    def write_tool(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n{body}")
        path.chmod(0o700)

    def wrapper(self):
        self.assertTrue(WRAPPER.is_file(), "RED: scripts/cargo-with-lbug.sh is not implemented")
        destination = self.source / "scripts/cargo-with-lbug.sh"
        shutil.copy2(WRAPPER, destination)
        # Copy only repository-controlled build support/config, never .git or
        # application runtime/profile fixtures. Cargo remains a recording fake.
        for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
            shutil.copy2(REPO / name, self.source / name)
        for name in ("build-support", ".cargo"):
            shutil.copytree(REPO / name, self.source / name, dirs_exist_ok=True,
                            ignore=shutil.ignore_patterns("target", "Cargo.lock", "__pycache__"))
        return destination

    def run_wrapper(self, args, extra=None):
        env = {**self.env, **(extra or {})}
        return subprocess.run(["/bin/bash", str(self.wrapper()), *args], cwd=self.source,
                              env=env, capture_output=True, timeout=15, check=False)

    def calls(self):
        if not self.log.exists():
            return []
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_preserves_argv_machine_stdout_and_cargo_failure_status(self):
        args = ["test", "--locked", "--target-dir", "a directory", "--", "", "literal;$(touch PWNED)", "--nocapture"]
        result = self.run_wrapper(args, {"TEST_STATUS": "23"})
        self.assertEqual(result.returncode, 23, result.stderr.decode())
        self.assertEqual(result.stdout, b'{"reason":"compiler-artifact","fixture":true}\n')
        self.assertEqual(self.calls()[0]["argv"], args)
        self.assertIn(b"cargo fixture stderr", result.stderr)
        self.assertFalse((self.source / "PWNED").exists())

    def test_source_mode_does_not_bootstrap_or_invent_empty_shared_override(self):
        result = self.run_wrapper(["build", "--release", "--locked"])
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        calls = self.calls()
        self.assertEqual(len(calls), 1, "source mode must not bootstrap a helper")
        self.assertNotIn("LBUG_SHARED", calls[0]["env"])
        self.assertNotIn("LBUG_LIBRARY_DIR", calls[0]["env"])
        self.assertNotIn("LBUG_INCLUDE_DIR", calls[0]["env"])

    def test_explicit_empty_partial_and_shared_overrides_reach_cargo_unchanged(self):
        for override in ({"LBUG_SHARED": ""}, {"LBUG_SHARED": "1"},
                         {"LBUG_LIBRARY_DIR": ""}, {"LBUG_INCLUDE_DIR": "/explicit/include"},
                         {"LBUG_LIBRARY_DIR": "/explicit/lib", "LBUG_INCLUDE_DIR": "/explicit/include"}):
            with self.subTest(override=override):
                result = self.run_wrapper(["build"], {"AMPLIHACK_LBUG_CACHE": "auto", **override})
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                child = self.calls()[-1]["env"]
                self.assertEqual({key: child.get(key) for key in override}, override)

    def test_non_build_commands_never_bootstrap_or_produce(self):
        for args in (["fmt", "--all", "--check"], ["metadata", "--no-deps"],
                     ["install", "kani-verifier@0.67.0", "--locked"]):
            with self.subTest(args=args):
                before = len(self.calls())
                result = self.run_wrapper(args, {"AMPLIHACK_LBUG_CACHE": "auto", "AMPLIHACK_LBUG_PRODUCER": "1"})
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertEqual(self.calls()[before:][0]["argv"], args)
                self.assertEqual(len(self.calls()) - before, 1)

    def test_ci_dispatcher_uses_real_cargo_without_bypassing_fixture_tools(self):
        dispatcher = self.bin / "dispatcher"
        dispatcher.mkdir()
        shim = dispatcher / "cargo"
        shim.write_text('#!/bin/bash\nexec "$TEST_ROOT/scripts/cargo-with-lbug.sh" "$@"\n')
        shim.chmod(0o700)
        args = ["build", "--locked"]
        result = self.run_wrapper(args, {"PATH": f"{dispatcher}:{self.bin}:/usr/bin:/bin",
                                        "AMPLIHACK_LBUG_CARGO_SHIM": str(shim),
                                        "AMPLIHACK_LBUG_REAL_CARGO": str(self.bin / "cargo")})
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(self.calls()[0]["argv"], args)

        # Tests may shadow Cargo intentionally. The dispatcher must honor that
        # tool boundary even though CI's original-Cargo variable is inherited.
        result = self.run_wrapper(args, {"AMPLIHACK_LBUG_CARGO_SHIM": str(shim),
                                        "AMPLIHACK_LBUG_REAL_CARGO": "/not/a/tool"})
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_offline_sources_never_trigger_networked_bootstrap(self):
        for args, overrides in [
            (["build", "--offline"], {}), (["build", "--frozen"], {}),
            (["build"], {"CARGO_NET_OFFLINE": "true"}),
            (["--config", "net.offline=true", "build"], {}),
        ]:
            with self.subTest(args=args, overrides=overrides):
                before = len(self.calls())
                result = self.run_wrapper(args, {"AMPLIHACK_LBUG_CACHE": "auto", **overrides})
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                calls = self.calls()[before:]
                self.assertEqual(calls[-1]["argv"], args)
                for call in calls[:-1]:
                    self.assertTrue("--offline" in call["argv"] or "--frozen" in call["argv"]
                                    or call["env"].get("CARGO_NET_OFFLINE") == "true",
                                    "helper bootstrap must inherit effective offline mode")
                self.assertEqual(calls[-1]["env"].get("RUSTUP_AUTO_INSTALL"), "0")
                self.assertNotIn(b"forbidden tool:", result.stderr)

    def test_preparation_failure_keeps_original_cargo_status_and_target(self):
        args = ["build", "--target", "custom.json", "--target-dir", "owned target", "--jobs", "2"]
        result = self.run_wrapper(args, {"AMPLIHACK_LBUG_CACHE": "auto", "TEST_STATUS": "37"})
        self.assertEqual(result.returncode, 37, result.stderr.decode())
        self.assertEqual(self.calls()[-1]["argv"], args)
        self.assertNotIn("LBUG_LIBRARY_DIR", self.calls()[-1]["env"])

    def test_term_is_forwarded_to_the_active_cargo_child(self):
        wrapper = self.wrapper()
        ready = self.root / "ready"
        child = subprocess.Popen(["/bin/bash", str(wrapper), "build"], cwd=self.source,
                                 env={**self.env, "TEST_WAIT": "1", "TEST_READY": str(ready)},
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        def cleanup():
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.communicate(timeout=5)
        self.addCleanup(cleanup)
        deadline = time.monotonic() + 10
        while not ready.exists() and child.poll() is None and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(ready.exists(), "recording Cargo child never became ready")
        child.send_signal(signal.SIGTERM)  # Signal wrapper PID, not its group.
        child.communicate(timeout=10)
        self.assertIn({"tool": "cargo-terminated"}, self.calls())
        self.assertNotEqual(child.returncode, 0)

    def test_actual_hook_delegates_after_scrubbing_git_and_selecting_owned_target(self):
        hook = self.source / "scripts/git-hook-cargo.sh"
        shutil.copy2(REPO / "scripts/git-hook-cargo.sh", hook)
        spy = self.source / "scripts/cargo-with-lbug.sh"
        spy.write_text("#!/bin/bash\nexec " + shlex.quote(sys.executable) + " - \"$@\" <<'PY'\n" + r'''
import json, os, sys
with open(os.environ["TEST_LOG"], "a") as f:
    f.write(json.dumps({"tool": "wrapper", "argv": sys.argv[1:],
                       "target": os.environ.get("CARGO_TARGET_DIR"),
                       "git_dir": os.environ.get("GIT_DIR")}) + "\n")
os.execvp("cargo", ["cargo", *sys.argv[1:]])
PY
''')
        spy.chmod(0o700)
        for args in (["clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
                     ["test", "--workspace", "--locked", "--no-fail-fast", "--", "--skip", "kuzu"]):
            with self.subTest(args=args):
                result = subprocess.run(["/bin/bash", str(hook), "cargo", *args], cwd=self.source,
                                        env={**self.env, "GIT_DIR": "/fixture/unrelated.git"},
                                        capture_output=True, timeout=15, check=False)
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                spies = [call for call in self.calls() if call["tool"] == "wrapper"]
                self.assertTrue(spies, "RED: hook executes bare Cargo and bypasses managed reuse")
                self.assertEqual(spies[-1]["argv"], args)
                self.assertIsNone(spies[-1]["git_dir"])
                self.assertTrue(Path(spies[-1]["target"]).is_relative_to(self.root))


if __name__ == "__main__":
    unittest.main()
