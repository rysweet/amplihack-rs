#!/usr/bin/env python3
"""Controls for projection, evidence tampering, publication history and receipt finalisation.

Set CANONICAL_STAGING_REQUIREMENTS_FILE and CANONICAL_STAGING_RUNNER to run
against the complete task contract and real installed runner. No model is called.
"""
import argparse
import base64
import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


fixture = module("fixture", "test-pragmatic-auto-stage.py")
evidence = module("evidence", "test-pragmatic-evidence.py")
checkpoints = module("checkpoints", "test-pragmatic-checkpoints.py")
from canonical_staging_publication import commit_objects, publication_history


class HarnessControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="harness-controls-", dir=os.environ["TMPDIR"])
        cls.root = Path(cls.temp.name)
        cls.requirements = Path(os.environ["CANONICAL_STAGING_REQUIREMENTS_FILE"])
        cls.baseline = cls.root / "baseline"
        cls.baseline.mkdir()
        for name in fixture.ASSETS:
            raw = subprocess.check_output(["git", "show", "606f3c746e51b8ef66d75380b41791b9f3a9ed8e:amplifier-bundle/recipes/" + name + ".yaml"], cwd=ROOT)
            (cls.baseline / (name + ".yaml")).write_bytes(raw)
        cls.args = argparse.Namespace(requirements_file=cls.requirements, report=cls.root / "real",
            tmpdir=cls.root, target=cls.root, runner=Path(os.environ["CANONICAL_STAGING_RUNNER"]),
            baseline=cls.baseline, candidate=HERE.parent, materialise_working_dir=True)
        cls.f = fixture.Fixtures(cls.args)
        cls.f.agent_case("workflow-publish", ["step-14-bump-version"], "green")
        cls.row = cls.f.receipts[-1]
        assert cls.row["accepted"], cls.row
        # The expected manifest is supplied independently of evidence mutations.
        cls.source = cls.root / "authoritative-source"
        for path in ("amplifier-bundle/recipes/workflow-publish.yaml", "amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py"):
            dest = cls.source / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, dest)
        cls.authority = {"source_root": str(cls.source), "baseline_root": str(cls.baseline),
            "files": {str(p.relative_to(cls.source)): fixture.digest(p.read_bytes()) for p in cls.source.rglob("*") if p.is_file()},
            "baseline_assets": {"workflow-publish": fixture.digest((cls.baseline / "workflow-publish.yaml").read_bytes())},
            "requirements_file": str(cls.requirements), "requirements_sha256": fixture.digest(cls.requirements.read_bytes()),
            "materialise_working_dir": True, "runner": {k: json.loads((cls.f.report / "identity.json").read_text())[k] for k in ("runner_path", "runner_version", "runner_sha256")},
            "cases": {cls.row["case_id"]: {"asset": "workflow-publish", "steps": ["step-14-bump-version"], "variant": "green", "mode": "mutation", "shared": False}}}

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_genuine_receipt_and_authority_are_accepted(self):
        self.assertTrue(evidence.authority_checks(self.authority))
        self.assertTrue(evidence.verify_case(self.f.report, self.row, self.authority))

    def test_all_original_prompt_scalars_preserve_fields(self):
        count = 0
        additional = 0
        for name in fixture.ASSETS:
            text = (HERE.parent / (name + ".yaml")).read_text()
            original = fixture.blocks((self.baseline / (name + ".yaml")).read_text())
            for sid, block in fixture.blocks(text).items():
                if "    agent:" in block:
                    recipe = text.split("steps:\n", 1)[0] + "steps:\n" + block
                    fixed = fixture.append_requirements(recipe, self.f.requirements)
                    self.assertIn(self.f.requirements, fixture.parse_yaml(fixed)["steps"][0]["prompt"])
                    if sid in original:
                        count += 1
                    else:
                        self.assertTrue(sid.endswith('-scope-review'))
                        additional += 1
        self.assertEqual(count, 40)
        self.assertEqual(additional, 3)

    def test_candidate_tamper_with_rehashed_receipt_is_rejected(self):
        path = self.source / "amplifier-bundle/recipes/workflow-publish.yaml"
        old = path.read_bytes()
        try:
            path.write_bytes(old + b"\n# forged candidate\n")
            forged = copy.deepcopy(self.row)
            forged["candidate_hash"] = fixture.digest(path.read_bytes())
            forged["source_hash"] = forged["candidate_hash"]
            self.assertFalse(evidence.authority_checks(self.authority))
            with self.assertRaises(AssertionError):
                evidence.verify_case(self.f.report, forged, self.authority)
        finally:
            path.write_bytes(old)

    def test_baseline_source_tamper_with_rehashed_receipt_is_rejected(self):
        path = self.baseline / "workflow-publish.yaml"
        old = path.read_bytes()
        try:
            path.write_bytes(old + b"\n# forged source\n")
            forged = copy.deepcopy(self.row)
            forged["baseline_hash"] = fixture.digest(path.read_bytes())
            with self.assertRaises(AssertionError):
                evidence.verify_case(self.f.report, forged, self.authority)
        finally:
            path.write_bytes(old)

    def test_projection_tamper_with_rehashed_receipt_is_rejected(self):
        path = self.f.report / self.row["case_id"] / "recipe.yaml"
        old = path.read_bytes()
        try:
            path.write_bytes(old.replace(b"auto_stage: false", b"auto_stage: true", 1))
            forged = copy.deepcopy(self.row)
            forged["projection_hash"] = fixture.digest(path.read_bytes())
            with self.assertRaises(AssertionError):
                evidence.verify_case(self.f.report, forged, self.authority)
        finally:
            path.write_bytes(old)

    def test_requirements_tamper_with_rehashed_receipt_is_rejected(self):
        path = self.f.report / "requirements-unchanged.json"
        old = path.read_bytes()
        try:
            path.write_bytes(b"{}\n")
            forged = copy.deepcopy(self.row)
            forged["projection_substitutions"]["requirements_sha256"] = fixture.digest(path.read_bytes())
            with self.assertRaises(AssertionError):
                evidence.verify_case(self.f.report, forged, self.authority)
        finally:
            path.write_bytes(old)

    def test_runner_identity_tamper_is_rejected(self):
        path = self.f.report / "identity.json"
        old = path.read_bytes()
        try:
            changed = json.loads(old)
            changed["runner_version"] = "forged runner 99\n"
            fixture.save(path, changed)
            with self.assertRaises(AssertionError):
                evidence.verify_case(self.f.report, self.row, self.authority)
        finally:
            path.write_bytes(old)

    def test_dropped_repository_and_false_observations_are_rejected(self):
        forged = copy.deepcopy(self.row)
        forged["resolved_index_paths"] = dict(list(forged["resolved_index_paths"].items())[:1])
        self.assertFalse(evidence.verify_case(self.f.report, forged, self.authority))
        path = self.f.report / self.row["case_id"] / "task linked" / "indexing.pid"
        old = path.read_bytes()
        try:
            path.write_bytes(b"forged metadata\n")
            self.assertFalse(evidence.verify_case(self.f.report, self.row, self.authority))
        finally:
            path.write_bytes(old)

    def test_hook_population_requires_every_pair_once(self):
        rows = [{"asset": asset, "step_id": sid, "variant": variant, "hook_executed": True,
                 "assertions": {"rejecting_hook_prevents_commit": True, "rejecting_hook_propagates_failure": True}}
                for asset, sid, _ in checkpoints.CHECKPOINTS for variant in ("normal", "adversarial", "tracked-runtime", "reject")]
        self.assertTrue(evidence.verify_hook_population(rows, checkpoints.CHECKPOINTS))
        self.assertFalse(evidence.verify_hook_population(rows[:-1], checkpoints.CHECKPOINTS))
        self.assertFalse(evidence.verify_hook_population(rows[:-1] + [rows[0]], checkpoints.CHECKPOINTS))
        self.assertFalse(evidence.verify_hook_population([r for r in rows if r["variant"] == "adversarial"] * 4, checkpoints.CHECKPOINTS))
        rows[0]["hook_executed"] = False
        self.assertFalse(evidence.verify_hook_population(rows, checkpoints.CHECKPOINTS))

    def test_hook_log_index_output_and_execution_tampering_are_rejected(self):
        case = self.root / "hook-evidence"
        case.mkdir()
        (case / "hook.log").write_text("hook executed\n")
        (case / "hook.index-entries").write_bytes(b"100644 abc 0\tsource.txt\0")
        (case / "stdout").write_bytes(b"checkpoint output\n")
        (case / "stderr").write_bytes(b"")
        row = {"command": ["fixture-shell"], "exit_status": 0, "hook_executed": True,
               "hook_staged_entries": {"source.txt": "100644 abc 0"}, "variant": "normal",
               "publication_history": {"created_commits": ["created-commit"]}}
        fixture.save(case / "execution.json", {"command": row["command"], "exit_status": 0,
                     "stdout_sha256": fixture.digest((case / "stdout").read_bytes()), "stderr_sha256": fixture.digest(b"")})
        self.assertTrue(evidence.verify_hook_evidence(case, row))
        for filename, changed in (("hook.log", b"forged hook\n"), ("hook.index-entries", b""), ("stdout", b"forged output")):
            path = case / filename
            old = path.read_bytes()
            try:
                path.write_bytes(changed)
                with self.assertRaises(AssertionError):
                    evidence.verify_hook_evidence(case, row)
            finally:
                path.write_bytes(old)
        row["exit_status"] = 1
        with self.assertRaises(AssertionError):
            evidence.verify_hook_evidence(case, row)

    def test_successful_process_with_malformed_output_retains_execution(self):
        runner = self.root / "malformed-runner"
        runner.write_text("#!/bin/sh\nprintf 'not JSON\\n'\nexit 0\n")
        runner.chmod(0o755)
        args = copy.copy(self.args)
        args.runner, args.report = runner, self.root / "malformed"
        f = fixture.Fixtures(args)
        f.agent_case("workflow-publish", ["step-14-bump-version"], "green")
        row = f.receipts[-1]
        self.assertFalse(row["accepted"])
        self.assertTrue(row["finalized"])
        self.assertEqual(row["exit_status"], 0)
        self.assertEqual(row["failure_phase"], "output_parsing")
        self.assertEqual(row["command"][0], str(runner))
        case = f.report / row["case_id"]
        self.assertEqual((case / "stdout").read_bytes(), b"not JSON\n")
        self.assertTrue((case / "stderr").exists())
        self.assertTrue((case / "0.index.after").exists())

    def test_every_commit_deletion_mode_and_reset_are_inspected(self):
        repo = self.root / "history"
        self.f.seed(repo)
        git = lambda *args: self.f.git(repo, *args)
        baseline = commit_objects(git)
        authority = {"source.txt": {"mode": "100644", "contents_base64": base64.b64encode(b"authorised\n").decode()}}
        forbidden = repo / ".onboarding-evidence/checkpoint.json"
        forbidden.parent.mkdir()
        forbidden.write_text("private content\n")
        git("add", "--all")
        git("commit", "-m", "forbidden then deleted")
        git("rm", "--", str(forbidden.relative_to(repo)))
        git("commit", "-m", "delete forbidden")
        (repo / "source.txt").write_text("authorised\n")
        git("add", "--all")
        git("commit", "-m", "authorised final content")
        history = publication_history(git, baseline, authority)
        self.assertFalse(history["publication_authorised"])
        self.assertEqual(len(history["created_commits"]), 3)
        deleted = [r for r in history["transitions"] if r["after"] is None]
        self.assertEqual(deleted[0]["before"]["mode"], "100644")
        clean_base = commit_objects(git)
        (repo / "source.txt").chmod(0o755)
        git("add", "--all")
        git("commit", "-m", "wrong executable mode")
        history = publication_history(git, clean_base, authority)
        self.assertFalse(history["publication_authorised"])
        self.assertEqual(history["transitions"][0]["after"]["mode"], "100755")
        wrong_commit = git("rev-parse", "HEAD").strip().decode()
        git("reset", "--hard", "HEAD~1")  # Task-private fixture only.
        history = publication_history(git, clean_base, authority)
        self.assertIn(wrong_commit, history["created_commits"])
        self.assertFalse(history["publication_authorised"])


if __name__ == "__main__":
    unittest.main()
