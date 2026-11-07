#!/usr/bin/env python3
"""Additional conservative checks. Run only through an authorised deterministic step.

These tests supplement the unchanged 64 common tests. Their presence is not
execution evidence. They use the common independent linked fixtures and their
byte-preserving snapshot oracle. They do not invoke a model or installed actor.
"""
import importlib.util
import os
from pathlib import Path
import shlex
import sys
import unittest

spec = importlib.util.spec_from_file_location('checkpoint_common', Path(__file__).with_name('test-checkpoint-scope.py'))
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)


class ConservativeBoundaryTests(unittest.TestCase):
    def test_duplicate_strict_paths_refuse_without_index_write(self):
        fixture = common.GitFixture(self)
        fixture.write(b'source.txt', b'task change\n')
        before = fixture.snapshot()
        result = fixture.stage([b'source.txt', b'source.txt'])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(fixture.snapshot(), before)

    def test_assume_unchanged_and_skip_worktree_are_not_authority(self):
        for flag in ('--assume-unchanged', '--skip-worktree'):
            with self.subTest(flag=flag):
                fixture = common.GitFixture(self)
                fixture.write(b'source.txt', b'task change\n')
                fixture.git(fixture.task, 'update-index', flag, '--', 'unrelated.txt')
                before = fixture.snapshot()
                result = common.PhaseProjection(fixture, common.PHASES[0]).run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(fixture.snapshot(), before)

    def test_same_bytes_replacement_record_cannot_replace_file_identity(self):
        fixture = common.GitFixture(self)
        fixture.write(b'source.txt', b'task change\n')
        program = '''
import json, os
from pathlib import Path
root = Path(os.environ['SCOPE_FIXTURE_ROOT'])
for path in root.joinpath('private').rglob('authority.json'):
    raw = path.read_bytes()
    value = json.loads(raw)
    if 'approved_paths' in value:
        replacement = path.with_name('replacement.json')
        replacement.write_bytes(raw)
        os.replace(replacement, path)
'''
        fixture.hook(extra=shlex.quote(sys.executable) + ' -c ' + shlex.quote(program) + '\n')
        result = common.PhaseProjection(fixture, common.PHASES[0]).run()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotEqual(fixture.git(fixture.task, 'rev-parse', 'HEAD').strip().decode(), fixture.base)
        self.assertEqual(fixture.git(fixture.remote, 'rev-parse', 'refs/heads/' + fixture.branch).strip().decode(), fixture.base)

    def test_internal_symlink_parent_also_requires_external_preparation(self):
        fixture = common.GitFixture(self)
        fixture.write(b'directory/source', b'task change\n')
        (fixture.task / 'alias').symlink_to('directory', target_is_directory=True)
        before = fixture.snapshot()
        result = fixture.stage([b'alias/source'])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(fixture.snapshot(), before)


if __name__ == '__main__':
    unittest.main(verbosity=2)
