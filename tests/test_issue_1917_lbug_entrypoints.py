"""Read-only integration wiring checks; these do not claim native CI evidence."""
from pathlib import Path
import re
import tomllib
import unittest


REPO = Path(__file__).resolve().parents[1]


class EntrypointContracts(unittest.TestCase):
    def test_application_ci_release_and_snapshot_commands_use_shared_integration(self):
        # Check executable command lines, ignoring prose and unrelated tool
        # installation. Container legs must invoke the integration internally.
        command = re.compile(r"^\s*(?:-?\s*run:\s*)?(?:[A-Z_]+=\S+\s+)*(cargo|cross)\s+(build|check|clippy|nextest|test|install)\b(.*)")
        for relative in (".github/workflows/ci.yml", ".github/workflows/release.yml",
                         ".github/workflows/publish-snapshot.yml"):
            text = (REPO / relative).read_text()
            with self.subTest(file=relative):
                self.assertIn("cargo-with-lbug.sh", text, f"RED: {relative} has no shared integration")
                for number, line in enumerate(text.splitlines(), 1):
                    match = command.match(line)
                    if match and (match[2] != "install" or "--path" in match[3]):
                        self.fail(f"{relative}:{number}: application command bypasses shared integration: {line.strip()}")

    def test_dev_space_and_standalone_probe_route_builds_through_wrapper(self):
        for relative in ("scripts/dev-space.sh", "scripts/probe-no-python.sh"):
            with self.subTest(file=relative):
                code = "\n".join(line for line in (REPO / relative).read_text().splitlines()
                                 if not line.lstrip().startswith("#"))
                self.assertIn("cargo-with-lbug.sh", code, f"RED: {relative} bypasses the wrapper")

    def test_uvx_uses_one_checkout_for_both_binaries_without_bash4_readarray(self):
        code = "\n".join(line for line in (REPO / "uvx/amplihack").read_text().splitlines()
                         if not line.lstrip().startswith("#"))
        self.assertIn("cargo-with-lbug.sh", code, "RED: uvx uses cargo install --git")
        self.assertNotRegex(code, r"\b(?:readarray|mapfile)\b", "macOS Bash 3.2 has neither builtin")
        self.assertNotRegex(code, r"cargo\s+install[^\n]*--git", "selected source must carry its helper and patch")
        self.assertIn("amplihack-hooks", code)

    def test_helper_bootstrap_has_no_application_or_third_party_dependencies(self):
        manifest = REPO / "build-support/lbug-cache/Cargo.toml"
        self.assertTrue(manifest.is_file(), "RED: dependency-free helper manifest is missing")
        data = tomllib.loads(manifest.read_text())
        self.assertIs(data["package"].get("autotests"), False,
                      "component modules are reached through the isolated contract harness")
        self.assertFalse(data.get("dependencies"), "helper must bootstrap without a Cargo registry")
        self.assertFalse(data.get("build-dependencies"))
        for target in data.get("target", {}).values():
            self.assertFalse(target.get("dependencies"))
            self.assertFalse(target.get("build-dependencies"))

    def test_ci_reaches_the_new_contract_harnesses(self):
        workflow = (REPO / ".github/workflows/ci.yml").read_text()
        self.assertIn("build-support/lbug-cache/tests/contract-harness/Cargo.toml", workflow,
                      "RED: component contracts must be executed in CI")
        self.assertIn("test_issue_1917_lbug_", workflow,
                      "RED: wrapper/entrypoint child-process contracts must be executed in CI")

    def test_root_patch_selects_vendored_lbug_and_keeps_cxx_build_pin(self):
        data = tomllib.loads((REPO / "Cargo.toml").read_text())
        self.assertEqual(data.get("patch", {}).get("crates-io", {}).get("lbug", {}).get("path"),
                         "vendor/lbug", "RED: Cargo still selects upstream's unpatched build script")
        self.assertEqual(data["workspace"]["dependencies"]["cxx-build"], "=1.0.138")


if __name__ == "__main__":
    unittest.main()
