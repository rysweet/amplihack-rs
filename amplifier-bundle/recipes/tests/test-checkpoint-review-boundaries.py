#!/usr/bin/env python3
"""Read-only review counterexamples with task-owned fixtures only."""
import importlib.util
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parents[3]
HERE = SOURCE / 'amplifier-bundle/recipes/tests'
sys.path.insert(0, str(HERE))
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value
common = module('additional_common', HERE / 'test-checkpoint-scope.py')

class AdditionalReviewBoundaries(unittest.TestCase):
    def test_early_sync_preservation_audit_accepts_only_exact_owner_sequence(self):
        audit=module('early_sync_preservation_audit',HERE/'canonical_staging_checkpoint_audit.py')
        text=(common.RECIPES/'workflow-finalize.yaml').read_text()
        current=common.literal(common.blocks(text)['step-20-sync-pr-head'],'command')
        original=common.literal(common.blocks(common.index_source('amplifier-bundle/recipes/workflow-finalize.yaml'))['step-20-sync-pr-head'],'command')
        self.assertEqual(audit.restored_early_sync_command(current,original),original)
        raw = audit.restored_early_sync_asset_text('workflow-finalize', text)
        baseline = common.index_source('amplifier-bundle/recipes/workflow-finalize.yaml')
        for sid in ('finalize-terminal-state','step-20-sync-pr-head'):
            self.assertEqual(common.blocks(raw)[sid],common.blocks(baseline)[sid])
        comment='  # probe accepted as ahead and set a missing upstream. Executed failures stop.\n'
        for altered in (text.replace(comment,comment.rstrip('\n')+' changed\n',1),text.replace(comment,comment+'\n'+comment,1)):
            with self.assertRaises(AssertionError): audit.restored_early_sync_asset_text('workflow-finalize',altered)
        mutations=[
            current.replace('--strict-fetch --probe-origin-ref','--strict-fetch',1),
            current.replace('--strict-fetch --probe-origin-ref','--probe-origin-ref',1),
            current.replace('exit "$sync_rc"','exit 0',1),
            current.replace('"$sync_output_file" >&2 || redaction_rc=$?','"$sync_output_file" || redaction_rc=$?',1),
            current.replace(audit.EARLY_SYNC_STRICT_CALL,audit.EARLY_SYNC_STRICT_CALL*2,1),
            current+'git push origin main\n',
            current.replace('cd -P "$wt"','cd -P "$wt"; git add -A',1),
        ]
        for altered in mutations:
            self.assertNotEqual(altered,current)
            with self.assertRaises(AssertionError): audit.restored_early_sync_command(altered,original)

    def test_unvalidated_record_cannot_write_on_unauthorised_refusal(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.write(b'unrelated.txt', b'pre-existing unauthorised staging\n')
                f.git(f.task, 'add', '--', 'unrelated.txt')
                f.env['WF_CHECKPOINT_RECORD'] = str(f.task / 'unvalidated-record.json')
                before = f.snapshot()
                result = common.PhaseProjection(f, phase).capture_scope()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before, 'Refusal must not write observations through unvalidated authority.')

    def test_fetch_terminal_79_remains_terminal(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'authorised source\n')
        f.hook()
        f.hook(name='pre-push')
        f.executable(f.bin / 'git', 'if [ "${1-}" = fetch ]; then exit 79; fi\nexec ' + shlex.quote(f.git_binary) + ' "$@"\n')
        before_remote = f.git(f.remote, 'show-ref')
        result = common.PhaseProjection(f, common.PHASES[0]).run()
        self.assertEqual(result.returncode, 79, 'A terminal fetch must not continue into publication.')
        self.assertEqual(f.git(f.remote, 'show-ref'), before_remote)

    def test_original_control_inventory_accepts_only_authorised_delta(self):
        harness = module('pragmatic_inventory', HERE / 'test-pragmatic-auto-stage.py')
        class ReachedNativeExecution(Exception):
            pass
        with tempfile.TemporaryDirectory(prefix='checkpoint-inventory-', dir=os.environ['TMPDIR']) as temporary:
            directory = Path(temporary)
            baseline = directory / 'baseline'
            baseline.mkdir()
            for name in harness.ASSETS:
                raw = subprocess.check_output(['git', 'show', '606f3c746e51b8ef66d75380b41791b9f3a9ed8e:amplifier-bundle/recipes/' + name + '.yaml'], cwd=SOURCE)
                (baseline / (name + '.yaml')).write_bytes(raw)
            fixture = harness.Fixtures.__new__(harness.Fixtures)
            fixture.args = SimpleNamespace(baseline=baseline, candidate=common.RECIPES)
            fixture.report = directory / 'report'
            fixture.report.mkdir()
            fixture.receipts = []
            def no_native_execution(*args, **kwargs):
                raise ReachedNativeExecution()
            fixture.agent_case = no_native_execution
            with self.assertRaises(ReachedNativeExecution):
                fixture.agents()
            import json
            planned = json.loads((fixture.report / 'planned-cases.json').read_text())
            self.assertEqual(len(planned['population']), 40)
            self.assertEqual(fixture.receipts, [], 'This control validates inventory preparation only, not native execution.')

    def test_auxiliary_inventory_rejects_changed_wrappers_bodies_and_fields(self):
        harness = module('pragmatic_mutation_inventory', HERE / 'test-pragmatic-auto-stage.py')
        import canonical_staging_checkpoint_audit as audit
        from unittest.mock import patch
        class ReachedNativeExecution(Exception):
            pass
        with tempfile.TemporaryDirectory(prefix='checkpoint-auxiliary-', dir=os.environ['TMPDIR']) as temporary:
            directory = Path(temporary)
            baseline, candidate = directory / 'baseline', directory / 'candidate'
            baseline.mkdir()
            candidate.mkdir()
            for name in harness.ASSETS:
                raw = subprocess.check_output(['git', 'show', '606f3c746e51b8ef66d75380b41791b9f3a9ed8e:amplifier-bundle/recipes/' + name + '.yaml'], cwd=SOURCE)
                (baseline / (name + '.yaml')).write_bytes(raw)
                (candidate / (name + '.yaml')).write_bytes((common.RECIPES / (name + '.yaml')).read_bytes())
            def prepare(label):
                fixture = harness.Fixtures.__new__(harness.Fixtures)
                fixture.args = SimpleNamespace(baseline=baseline, candidate=candidate)
                fixture.report = directory / label
                fixture.report.mkdir()
                fixture.receipts = []
                def no_native_execution(*args, **kwargs):
                    raise ReachedNativeExecution()
                fixture.agent_case = no_native_execution
                return fixture
            valid = prepare('legitimate')
            with self.assertRaises(ReachedNativeExecution):
                valid.agents()
            self.assertEqual(len(__import__('json').loads((valid.report / 'planned-cases.json').read_text())['population']), 40)
            self.assertEqual(valid.receipts, [])
            for asset, (sid, helper, wrapper) in audit.AUXILIARY_EXTRACTIONS.items():
                path = candidate / (asset + '.yaml')
                original = path.read_text()
                block = harness.blocks(original)[sid]
                changed = block.replace('    command: |\n', '    command: |\n      false\n', 1)
                self.assertNotEqual(changed, block, 'The negative must change its intended command.')
                path.write_text(original.replace(block, changed, 1))
                with self.assertRaises((AssertionError, ValueError)):
                    prepare(asset + '-wrapper').agents()
                path.write_text(original)
                marker = '  - id: "' + sid + '"\n'
                changed = original.replace(marker, marker + '    invented_field: "unapproved"\n', 1)
                self.assertNotEqual(changed, original)
                path.write_text(changed)
                with self.assertRaises((AssertionError, ValueError)):
                    prepare(asset + '-field').agents()
                path.write_text(original)
                real_read = Path.read_bytes
                helper_path = SOURCE / 'amplifier-bundle/tools' / helper
                def changed_body(selected):
                    raw = real_read(selected)
                    return raw + b'# unapproved mutation\n' if selected == helper_path else raw
                with patch.object(Path, 'read_bytes', changed_body):
                    with self.assertRaises((AssertionError, ValueError)):
                        prepare(asset + '-body').agents()

    def test_bounded_projection_reaches_hooks_with_independent_clean_task(self):
        projection = module("native_checkpoint_projection", HERE / "test-pragmatic-checkpoints.py")
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                # The parent has unrelated staging; the independently created
                # linked task has none. Permission names only source.txt.
                f.write(b"unrelated.txt", b"parent staged user content\n", f.caller)
                f.git(f.caller, "add", "--", "unrelated.txt")
                f.write(b"unrelated.txt", b"parent unstaged user content\n", f.caller)
                parent_index = f.caller / ".git/index"
                parent_before = parent_index.read_bytes()
                self.assertEqual(f.git(f.task, "diff", "--cached", "--name-only"), b"")
                f.write(b"source.txt", b"owned projection source\n")
                f.hook()
                f.hook(name="pre-push")
                evidence = f.root / "projection-evidence"
                evidence.mkdir()
                request = {"asset":phase[0], "context":f.context,
                           "fixture_owner_paths":["source.txt"],
                           "evidence_directory":str(evidence), "shell":f.bash}
                result = projection.bounded_fixture_projection(request, f.env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((f.root / "pre-commit.log").is_file())
                self.assertTrue((f.root / "pre-push.log").is_file())
                self.assertEqual(parent_index.read_bytes(), parent_before)
                self.assertEqual(f.git(f.task, "show", "HEAD:source.txt"), b"owned projection source\n")
                self.assertEqual(f.git(f.task, "rev-parse", "HEAD").strip(),
                                 f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip())

    def test_bounded_projection_refuses_partial_user_state_before_hooks(self):
        projection = module("negative_checkpoint_projection", HERE / "test-pragmatic-checkpoints.py")
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.write(b"unrelated.txt", b"unrelated staged user content\n")
                f.git(f.task, "add", "--", "unrelated.txt")
                f.write(b"unrelated.txt", b"unrelated unstaged user content\n")
                f.write(b"source.txt", b"owned pending source\n")
                f.hook()
                f.hook(name="pre-push")
                evidence = f.root / "negative-projection-evidence"
                evidence.mkdir()
                request = {"asset":phase[0], "context":f.context,
                           "fixture_owner_paths":["source.txt"],
                           "evidence_directory":str(evidence), "shell":f.bash}
                before = f.snapshot()
                result = projection.bounded_fixture_projection(request, f.env)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)
                self.assertFalse((f.root / "pre-commit.log").exists())
                self.assertFalse((f.root / "pre-push.log").exists())

class OriginalLinkedDiagnosticControls(unittest.TestCase):
    def test_publish_retains_the_original_main_checkout_refusal_before_authority(self):
        f = common.GitFixture(self)
        f.context['worktree_setup']['worktree_path'] = str(f.caller)
        f.sync_context()
        # Match the existing integration fixture: it has no scope authority
        # or parent task input, but must still reach the original #1528 refusal.
        f.env.pop('RECIPE_VAR_task_description', None)
        f.env.pop('TASK_DESCRIPTION', None)
        before = f.snapshot()
        original_caller_head = f.git(f.caller, 'rev-parse', 'HEAD')
        projection = common.PhaseProjection(f, common.PHASES[0])
        result = projection.execute(common.PHASES[0][1])
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any(line.startswith(b'ERROR: step-15-commit-push: ') and
                            line.endswith(b' is not the top of a linked task worktree; refusing to stage or commit; never commits in REPO_PATH (#1528)')
                            for line in result.stderr.splitlines()), result.stderr)
        self.assertEqual(f.snapshot(), before)
        self.assertEqual(f.git(f.caller, 'rev-parse', 'HEAD'), original_caller_head)

class ExistingCleanupSkipControls(unittest.TestCase):
    def test_original_skip_locations_preserve_state_without_scope_permission(self):
        for kind in ('unset', 'gone', 'non-directory', 'non-worktree', 'main-checkout'):
            with self.subTest(location=kind):
                f = common.GitFixture(self)
                locations = {'gone': f.private / 'gone', 'non-directory': f.private / 'file',
                             'non-worktree': f.private / 'directory', 'main-checkout': f.caller}
                (f.private / 'file').write_text('test-owned non-directory')
                (f.private / 'directory').mkdir()
                if kind == 'unset':
                    f.context.pop('worktree_setup')
                    f.env.pop('RECIPE_VAR_worktree_setup', None)
                else:
                    f.context['worktree_setup']['worktree_path'] = str(locations[kind])
                f.sync_context()
                log = f.private / 'forbidden-skip-runtime.log'
                f.executable(f.bin / 'amplihack', "printf 'unexpected runtime call\\n' >> " + shlex.quote(str(log)) + "\nexit 99\n")
                task = f.task
                before_task = f.snapshot()
                f.task = f.caller
                before_caller = f.snapshot()
                f.task = task
                original = common.blocks(common.index_source('amplifier-bundle/recipes/workflow-finalize.yaml'))
                projection = common.PhaseProjection(f, common.PHASES[2])
                original_result = f.shell(projection.render(common.literal(original[common.PHASES[2][1]], 'command')))
                self.assertEqual(original_result.returncode, 0, original_result.stderr)
                result = projection.run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, original_result.stdout)
                self.assertEqual(result.stderr, original_result.stderr)
                self.assertIn(b'WARNING: step-20b ', result.stderr)
                self.assertIn(b'skipping cleanup commit/push', result.stderr)
                self.assertIsNone(projection.capture)
                self.assertIs(projection.skipped['admitted'], False)
                self.assertNotIn(common.scalar(projection.current[projection.reviewer], 'output'), f.context)
                self.assertFalse(log.exists(), 'An existing skip must not reach runtime cleanup or model dispatch.')
                self.assertFalse((f.root / 'pre-commit.log').exists())
                self.assertEqual(f.snapshot(), before_task)
                f.task = f.caller
                self.assertEqual(f.snapshot(), before_caller)
                f.task = task

    def test_selector_refusal_precedes_even_a_cleanup_skip(self):
        f = common.GitFixture(self)
        f.context['worktree_setup']['worktree_path'] = str(f.private / 'gone')
        f.sync_context()
        log = f.private / 'forbidden-git-inspection.log'
        f.executable(f.bin / 'git', "printf 'unexpected Git inspection\\n' >> " + shlex.quote(str(log)) + "\nexit 99\n")
        f.env.update(GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='core.hooksPath', GIT_CONFIG_VALUE_0=str(f.private))
        result = common.PhaseProjection(f, common.PHASES[2]).capture_scope()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'unsupported inherited Git selector', result.stderr)
        self.assertFalse(log.exists())

if __name__ == '__main__':
    unittest.main(verbosity=2)
