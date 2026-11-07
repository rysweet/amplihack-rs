#!/usr/bin/env python3
"""Supplement the frozen common tests for the existing #1762 alias contract.

Execution belongs to an authorised deterministic recipe step under the frozen
ordinary Mac ancestry observer. Writing these tests is not execution evidence.
The common test bytes and all its assertions remain unchanged. This separate
fixture supplies synthetic valid Git identities without reading a profile or
changing the production identity resolver. It launches no model or provider.
The inherited shell projection remains diagnostic, not native acceptance.
"""
import importlib.util
from pathlib import Path
import sys
import unittest


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "checkpoint_common", Path(__file__).with_name("test-checkpoint-scope.py"))
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)


class AliasFixture(common.GitFixture):
    def __init__(self, case):
        super().__init__(case)
        self.publication_branch = self.branch
        alias = self.publication_branch + "--wt-1"
        self.git(self.task, "branch", "-m", alias)
        # Match step 04: the caller holds the target branch, while the linked
        # task has an alias tracking that same target on origin.
        self.git(self.caller, "switch", self.publication_branch)
        self.git(self.task, "branch", "--set-upstream-to=origin/" + self.publication_branch)
        self.branch = alias
        self.context["worktree_setup"]["local_branch"] = alias
        for prefix in ("GIT_", "AMPLIHACK_GIT_"):
            for role in ("AUTHOR", "COMMITTER"):
                self.env[prefix + role + "_NAME"] = "Checkpoint Synthesis"
                self.env[prefix + role + "_EMAIL"] = "checkpoint-synthesis@example.org"
        self.sync_context()

    def remote_head(self):
        return self.git(self.remote, "rev-parse", "refs/heads/" + self.publication_branch).strip().decode()


class SynthesisAliasTests(unittest.TestCase):
    def assert_source_only_commit(self, fixture):
        head = fixture.git(fixture.task, "rev-parse", "HEAD").strip().decode()
        self.assertNotEqual(head, fixture.base)
        self.assertEqual(fixture.staged_paths(), set())
        history = fixture.history()
        self.assertTrue(history)
        for commit, parent, paths in history:
            self.assertEqual(paths, {b"source.txt"})
        self.assertEqual(fixture.git(fixture.task, "show", "HEAD:source.txt"), b"task change\n")
        self.assertEqual(fixture.git(fixture.task, "symbolic-ref", "--short", "HEAD").strip().decode(), fixture.branch)
        # Publishing from the alias must never create an origin alias branch.
        aliases = fixture.git(fixture.remote, "for-each-ref", "--format=%(refname)",
                              "refs/heads/" + fixture.branch)
        self.assertEqual(aliases, b"")
        return head

    def test_all_three_alias_checkpoints_publish_with_normal_hooks(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                fixture.hook()
                fixture.hook(name="pre-push")
                result = common.PhaseProjection(fixture, phase).run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(fixture.remote_head(), self.assert_source_only_commit(fixture))
                self.assertTrue((fixture.root / "pre-commit.log").is_file())
                self.assertTrue((fixture.root / "pre-push.log").is_file())

    def test_missing_local_tracking_ref_keeps_the_original_alias_publication(self):
        # The owner and tracking configuration predate the missing cached ref.
        # Review and cleanup retain their original first-publication fallback.
        for phase in common.PHASES[1:]:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.context["worktree_setup"]["base_ref"] = "refs/remotes/origin/fixture-base"
                fixture.sync_context()
                fixture.git(fixture.task, "update-ref", "-d", "refs/remotes/origin/" + fixture.publication_branch)
                fixture.write(b"source.txt", b"task change\n")
                fixture.hook()
                fixture.hook(name="pre-push")
                result = common.PhaseProjection(fixture, phase).run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(fixture.remote_head(), self.assert_source_only_commit(fixture))
                self.assertTrue((fixture.root / "pre-commit.log").is_file())
                self.assertTrue((fixture.root / "pre-push.log").is_file())

    def test_alias_capture_binds_both_local_and_publication_branches(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                projection = common.PhaseProjection(fixture, phase)
                result = projection.capture_scope()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(projection.capture["binding"]["branch"], fixture.branch)
                self.assertEqual(projection.capture["binding"]["publication_branch"], fixture.publication_branch)
                self.assertEqual(projection.capture["publication"]["ref"], "refs/heads/" + fixture.publication_branch)

    def test_missing_empty_or_malformed_alias_authority_preserves_entry_state(self):
        for phase in common.PHASES:
            for value in (None, "", False, [], "other--wt-1"):
                with self.subTest(phase=phase[0], local_branch=value):
                    fixture = AliasFixture(self)
                    fixture.write(b"source.txt", b"task change\n")
                    if value is None:
                        del fixture.context["worktree_setup"]["local_branch"]
                    else:
                        fixture.context["worktree_setup"]["local_branch"] = value
                    fixture.sync_context()
                    before = fixture.snapshot()
                    result = common.PhaseProjection(fixture, phase).run()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(fixture.snapshot(), before)

    def test_alias_tracking_mismatch_refuses_without_mutation(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                fixture.git(fixture.task, "config", "branch." + fixture.branch + ".merge", "refs/heads/fixture-base")
                before = fixture.snapshot()
                result = common.PhaseProjection(fixture, phase).run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.snapshot(), before)

    def test_alias_owner_branch_change_after_capture_refuses(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                projection = common.PhaseProjection(fixture, phase)
                result = projection.capture_scope()
                self.assertEqual(result.returncode, 0, result.stderr)
                verdict = projection.verdict()
                fixture.context["worktree_setup"]["branch_name"] = "other"
                fixture.sync_context()
                before = fixture.snapshot()
                result = projection.finish(verdict)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.snapshot(), before)

    def test_unrelated_pre_existing_alias_staging_is_preserved(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                fixture.write(b"unrelated.txt", b"user staged\n")
                fixture.git(fixture.task, "add", "--", "unrelated.txt")
                fixture.write(b"unrelated.txt", b"user unstaged\n")
                before = fixture.snapshot()
                result = common.PhaseProjection(fixture, phase).run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.snapshot(), before)

    def test_rejecting_alias_hooks_preserve_authorised_staging(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                fixture.hook(reject=True)
                result = common.PhaseProjection(fixture, phase).run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.git(fixture.task, "rev-parse", "HEAD").strip().decode(), fixture.base)
                self.assertEqual(fixture.remote_head(), fixture.base)
                self.assertEqual(fixture.staged_paths(), {b"source.txt"})
                self.assertEqual((fixture.task / "source.txt").read_bytes(), b"task change\n")
                self.assertTrue((fixture.root / "pre-commit.log").is_file())

    def test_failed_alias_push_retains_the_actual_local_commit(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                fixture = AliasFixture(self)
                fixture.write(b"source.txt", b"task change\n")
                fixture.hook()
                fixture.remote_reject()
                result = common.PhaseProjection(fixture, phase).run()
                self.assertNotEqual(result.returncode, 0)
                self.assert_source_only_commit(fixture)
                self.assertEqual(fixture.remote_head(), fixture.base)


class SynthesisLiteralTests(unittest.TestCase):
    def test_different_leaf_spelling_cannot_stage_a_reviewed_file(self):
        fixture = common.GitFixture(self)
        fixture.write(b"source.txt", b"task change\n")
        before = fixture.snapshot()
        result = fixture.stage([b"SOURCE.TXT"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(fixture.snapshot(), before)

    def test_different_parent_spelling_cannot_stage_a_literal_child(self):
        fixture = common.GitFixture(self)
        fixture.write(b"ExactParent/child", b"task change\n")
        before = fixture.snapshot()
        result = fixture.stage([b"exactparent/child"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(fixture.snapshot(), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
