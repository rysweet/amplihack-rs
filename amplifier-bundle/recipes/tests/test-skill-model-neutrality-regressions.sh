#!/usr/bin/env bash
# Offline neutrality guard integration fixtures.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
python3 - "$repo_root" <<'PY'
"""Offline guard regressions; fixtures respect the caller's TMPDIR."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest
import sys

REPO = Path(sys.argv.pop(1))
BASE = Path(tempfile.gettempdir())


class NeutralityGuardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        BASE.mkdir(parents=True, exist_ok=True)
        cls.temp = tempfile.TemporaryDirectory(dir=BASE)
        cls.repo = Path(cls.temp.name)
        for tree in ('amplifier-bundle/skills', 'docs/claude/skills'):
            shutil.copytree(REPO / tree, cls.repo / tree, symlinks=True)
        cls.guard = cls.repo / 'amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh'
        cls.guard.parent.mkdir(parents=True)
        shutil.copyfile(REPO / cls.guard.relative_to(cls.repo), cls.guard)
        audit = cls.guard.with_name('skill-model-neutrality-audit.md')
        shutil.copyfile(REPO / audit.relative_to(cls.repo), audit)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def run_guard(self):
        return subprocess.run(['bash', str(self.guard)], cwd=BASE, text=True,
                              capture_output=True, env=dict(os.environ, TMPDIR=str(BASE)))

    def test_neutral_tree_from_unrelated_directory(self):
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('130 skills; 860 text files', result.stdout)

    def test_execution_pin_in_supporting_file_and_mirror(self):
        for tree in ('amplifier-bundle/skills', 'docs/claude/skills'):
            path = self.repo / tree / 'poet-analyst/regression.md'
            try:
                path.write_text('Haiku is a poetic form.\nmodel: sonnet\n')
                result = self.run_guard()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(str(path.relative_to(self.repo)) + ':2:', result.stderr)
            finally:
                path.unlink()

    def test_frontmatter_override_and_inventory_drift(self):
        path = self.repo / 'amplifier-bundle/skills/regression/SKILL.md'
        path.parent.mkdir()
        try:
            path.write_text('---\nname: regression\nmodel: runtime-default\n---\n')
            result = self.run_guard()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('execution model override in frontmatter', result.stderr)
            self.assertIn('Expected 130 skills; scanned 131', result.stderr)
        finally:
            shutil.rmtree(path.parent)

    def test_generic_provider_recommendations(self):
        path = self.repo / 'amplifier-bundle/skills/poet-analyst/regression.md'
        try:
            for line in ('Recommended model: Claude', 'default_model = "GPT"',
                         'Use the Claude model for this skill',
                         'primary_model: sonnet', 'fallback_model = "haiku"',
                         'run --model claude', '/model gpt'):
                path.write_text(line + '\n')
                result = self.run_guard()
                self.assertNotEqual(result.returncode, 0, line)
                self.assertIn(str(path.relative_to(self.repo)) + ':1:', result.stderr)
        finally:
            path.unlink()

    def test_same_count_inventory_replacement(self):
        original = self.repo / 'amplifier-bundle/skills/poet-analyst/SKILL.md'
        replacement = original.with_name('RENAMED.md')
        added = self.repo / 'amplifier-bundle/skills/unreviewed/SKILL.md'
        try:
            original.rename(replacement)
            added.parent.mkdir()
            added.write_text('---\nname: unreviewed\n---\n')
            result = self.run_guard()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Reviewed skill inventory changed', result.stderr)
        finally:
            replacement.rename(original)
            shutil.rmtree(added.parent)

    def test_removed_configuration_validation_is_rejected(self):
        path = self.repo / 'docs/claude/skills/microsoft-agent-framework/examples/04-basic-agent.cs'
        original = path.read_text()
        try:
            path.write_text(original.replace('string.IsNullOrWhiteSpace(value)', 'value == null'))
            result = self.run_guard()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('missing blank configuration rejection', result.stderr)
        finally:
            path.write_text(original)


if __name__ == '__main__':
    unittest.main()

PY
