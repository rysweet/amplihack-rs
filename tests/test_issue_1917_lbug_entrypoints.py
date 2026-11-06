"""Read-only integration wiring checks; these do not claim native CI evidence."""
from pathlib import Path
import re
import hashlib
import json
import unittest


REPO = Path(__file__).resolve().parents[1]


class EntrypointContracts(unittest.TestCase):
    def test_development_ci_commands_use_shared_integration(self):
        # Check executable command lines, ignoring prose and unrelated tool
        # installation. Container legs must invoke the integration internally.
        command = re.compile(r"^\s*(?:-?\s*run:\s*)?(?:[A-Z_]+=\S+\s+)*(cargo|cross)\s+(build|check|clippy|nextest|test|install)\b(.*)")
        for relative in (".github/workflows/ci.yml",):
            text = (REPO / relative).read_text()
            with self.subTest(file=relative):
                self.assertIn("cargo-with-lbug.sh", text, f"RED: {relative} has no shared integration")
                for number, line in enumerate(text.splitlines(), 1):
                    match = command.match(line)
                    if match and match[2] not in ("install", "build") and "build-support/lbug-cache" not in line:
                        self.fail(f"{relative}:{number}: application command bypasses shared integration: {line.strip()}")

    def test_dev_space_and_standalone_probe_route_builds_through_wrapper(self):
        for relative in ("scripts/dev-space.sh", "scripts/probe-no-python.sh"):
            with self.subTest(file=relative):
                code = "\n".join(line for line in (REPO / relative).read_text().splitlines()
                                 if not line.lstrip().startswith("#"))
                self.assertIn("cargo-with-lbug.sh", code, f"RED: {relative} bypasses the wrapper")

    def test_release_and_distributed_installers_remain_outside_v1(self):
        for relative in (".github/workflows/release.yml", ".github/workflows/publish-snapshot.yml",
                         "uvx/amplihack", "npm/lib/bootstrap.js"):
            self.assertNotIn("cargo-with-lbug.sh", (REPO / relative).read_text())

    def test_exact_deputy_decision_is_retained(self):
        root = REPO / "build-support/lbug-cache"
        decision = json.loads((root / "approval-v1.json").read_text())
        self.assertEqual(decision["id"], 6616249815)
        self.assertEqual(hashlib.sha256((root / "approved-v1.md").read_bytes()).hexdigest(),
                         decision["body_sha256"])

    def test_ci_reaches_the_new_contract_harnesses(self):
        workflow = (REPO / ".github/workflows/ci.yml").read_text()
        self.assertIn("test_issue_1917_lbug_", workflow,
                      "wrapper/entrypoint/Cargo contracts must be executed in CI")
        self.assertIn("npm test", workflow)
        self.assertIn("check-lbug-release-rpath.sh", workflow)

    def test_no_vendor_patch_and_cxx_pin_is_preserved(self):
        import tomllib
        data = tomllib.loads((REPO / "Cargo.toml").read_text())
        self.assertNotIn("lbug", data.get("patch", {}).get("crates-io", {}))
        self.assertEqual(data["workspace"]["dependencies"]["cxx-build"], "=1.0.138")


if __name__ == "__main__":
    unittest.main()
