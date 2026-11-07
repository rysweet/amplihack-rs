#!/usr/bin/env python3
"""Supplementary scope and independent second-parent controls."""
import importlib.util
from pathlib import Path
import sys
import os
import unittest
sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('parent_common', SOURCE / 'amplifier-bundle/recipes/tests/test-checkpoint-scope.py')
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)

class ScopeAndParentControls(unittest.TestCase):
    def test_forbidden_source_is_detected_in_every_git_state(self):
        for state in ('working', 'staged', 'staged-and-hidden', 'committed', 'committed-and-deleted'):
            with self.subTest(state=state):
                f = common.GitFixture(self)
                f.write(b'src/runner/mod.rs', b'forbidden runtime source\n')
                if state != 'working':
                    f.git(f.task, 'add', '--', 'src/runner/mod.rs')
                if state == 'staged-and-hidden':
                    (f.task / 'src/runner/mod.rs').unlink()
                if state in ('committed', 'committed-and-deleted'):
                    f.git(f.task, 'commit', '-m', 'forbidden fixture source')
                if state == 'committed-and-deleted':
                    f.git(f.task, 'rm', '--', 'src/runner/mod.rs')
                    f.git(f.task, 'commit', '-m', 'hide forbidden fixture source')
                previous_root = common.ROOT
                previous_baseline = getattr(common, 'SCOPE_AUDIT_BASELINE', None)
                common.ROOT = f.task
                common.SCOPE_AUDIT_BASELINE = f.base
                try:
                    with self.assertRaises(AssertionError):
                        case = common.RecipeContractTests('test_no_additional_cargo_runtime_runner_or_consensus_changes')
                        case.test_no_additional_cargo_runtime_runner_or_consensus_changes()
                finally:
                    common.ROOT = previous_root
                    if previous_baseline is None:
                        del common.SCOPE_AUDIT_BASELINE
                    else:
                        common.SCOPE_AUDIT_BASELINE = previous_baseline

    def test_complete_ancestry_is_required_even_with_a_hydrated_base(self):
        for mode in ('shallow', 'unrelated-root', 'grafted', 'replacement'):
            with self.subTest(ancestry=mode):
                f = common.GitFixture(self)
                f.write(b'src/runner/mod.rs', b'forbidden transient source\n')
                f.git(f.task, 'add', '--', 'src/runner/mod.rs')
                f.git(f.task, 'commit', '-m', 'forbidden transient source')
                f.git(f.task, 'rm', '--', 'src/runner/mod.rs')
                f.git(f.task, 'commit', '-m', 'remove forbidden transient source')
                head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                metadata = Path(os.fsdecode(f.git(f.task, 'rev-parse', '--git-common-dir').strip()))
                if not metadata.is_absolute():
                    metadata = f.task / metadata
                if mode == 'shallow':
                    (metadata / 'shallow').write_text(head + '\n')
                    self.assertEqual(f.git(f.task, 'rev-parse', '--is-shallow-repository').strip(), b'true')
                elif mode == 'grafted':
                    (metadata / 'info').mkdir(exist_ok=True)
                    (metadata / 'info/grafts').write_text(head + ' ' + f.base + '\n')
                elif mode == 'replacement':
                    f.git(f.task, 'replace', '--graft', head, f.base)
                else:
                    f.git(f.task, 'checkout', '--orphan', 'fixture-unrelated-root')
                    f.git(f.task, 'commit', '-m', 'unrelated root with identical final tree')
                self.assertEqual(f.git(f.task, 'cat-file', '-t', f.base).strip(), b'commit')
                previous_root, previous_base = common.ROOT, common.SCOPE_AUDIT_BASELINE
                before = f.snapshot()
                common.ROOT, common.SCOPE_AUDIT_BASELINE = f.task, f.base
                try:
                    with self.assertRaises(AssertionError):
                        case = common.RecipeContractTests('test_no_additional_cargo_runtime_runner_or_consensus_changes')
                        case.test_no_additional_cargo_runtime_runner_or_consensus_changes()
                finally:
                    common.ROOT, common.SCOPE_AUDIT_BASELINE = previous_root, previous_base
                self.assertEqual(f.snapshot(), before)

    def test_only_the_excluded_second_parent_reveals_unknown_source(self):
        f = common.GitFixture(self)
        f.write(b'unrelated.txt', b'upstream-only content\n', f.caller)
        f.git(f.caller, 'add', '--', 'unrelated.txt')
        f.git(f.caller, 'commit', '-m', 'independent upstream second parent')
        second = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.caller, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        f.git(f.task, 'fetch', 'origin')
        f.write(b'source.txt', b'authorised first-parent content\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'authorised first parent')
        first = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'merge', '--strategy=ours', '--no-ff', second, '-m', 'merge with independent second-parent difference')
        head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        self.assertEqual(f.git(f.task, 'diff-tree', '--no-commit-id', '--name-only', '-r', first, head), b'')
        self.assertIn(b'unrelated.txt', f.git(f.task, 'diff-tree', '--no-commit-id', '--name-only', '-r', second, head))
        outgoing = f.git(f.task, 'rev-list', '@{u}..HEAD').splitlines()
        self.assertNotIn(second.encode(), outgoing)
        self.assertIn(first.encode(), outgoing)
        before = f.snapshot()
        result = common.PhaseProjection(f, common.PHASES[0]).run()
        self.assertNotEqual(result.returncode, 0, 'The second-parent difference requires owner authority.')
        self.assertEqual(f.snapshot(), before)

    def test_exact_approved_cargo_input_is_permitted(self):
        f = common.GitFixture(self)
        for path in ('Cargo.lock', 'crates/amplihack-workflows/Cargo.toml'):
            f.write(path, (SOURCE / path).read_bytes())
            f.git(f.task, 'add', '--', path)
        previous_root = common.ROOT
        previous_baseline = getattr(common, 'SCOPE_AUDIT_BASELINE', None)
        common.ROOT = f.task
        common.SCOPE_AUDIT_BASELINE = f.base
        try:
            case = common.RecipeContractTests('test_no_additional_cargo_runtime_runner_or_consensus_changes')
            case.test_no_additional_cargo_runtime_runner_or_consensus_changes()
        finally:
            common.ROOT = previous_root
            if previous_baseline is None:
                del common.SCOPE_AUDIT_BASELINE
            else:
                common.SCOPE_AUDIT_BASELINE = previous_baseline

    def test_additional_cargo_edge_is_never_hidden_by_staging_or_commit(self):
        for state in ('working', 'staged', 'staged-and-hidden', 'committed', 'committed-and-hidden', 'committed-and-reverted'):
            with self.subTest(state=state):
                f = common.GitFixture(self)
                for path in ('Cargo.lock', 'crates/amplihack-workflows/Cargo.toml'):
                    f.write(path, (SOURCE / path).read_bytes())
                    f.git(f.task, 'add', '--', path)
                f.git(f.task, 'commit', '-m', 'exact approved fixture Cargo input')
                path = f.task / 'crates/amplihack-workflows/Cargo.toml'
                text = path.read_text()
                needle = 'amplihack-git = { workspace = true }'
                self.assertEqual(text.count(needle), 1)
                path.write_text(text.replace(needle, needle + '\nunapproved = "99.0"'))
                if state != 'working':
                    f.git(f.task, 'add', '--', 'crates/amplihack-workflows/Cargo.toml')
                if state.startswith('committed'):
                    f.git(f.task, 'commit', '-m', 'unapproved fixture dependency edge')
                if state.endswith('hidden') or state == 'committed-and-reverted':
                    path.write_bytes((SOURCE / 'crates/amplihack-workflows/Cargo.toml').read_bytes())
                if state == 'committed-and-reverted':
                    f.git(f.task, 'add', '--', 'crates/amplihack-workflows/Cargo.toml')
                    f.git(f.task, 'commit', '-m', 'restore approved fixture dependency bytes')
                previous_root = common.ROOT
                previous_baseline = getattr(common, 'SCOPE_AUDIT_BASELINE', None)
                common.ROOT = f.task
                common.SCOPE_AUDIT_BASELINE = f.base
                try:
                    with self.assertRaises(AssertionError):
                        case = common.RecipeContractTests('test_no_additional_cargo_runtime_runner_or_consensus_changes')
                        case.test_no_additional_cargo_runtime_runner_or_consensus_changes()
                finally:
                    common.ROOT = previous_root
                    if previous_baseline is None:
                        del common.SCOPE_AUDIT_BASELINE
                    else:
                        common.SCOPE_AUDIT_BASELINE = previous_baseline

if __name__ == '__main__':
    unittest.main(verbosity=2)
