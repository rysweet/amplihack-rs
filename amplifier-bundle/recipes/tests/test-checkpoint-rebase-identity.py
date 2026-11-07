#!/usr/bin/env python3
"""Actual private rebase counterexample for whitespace-insensitive identity."""
import ast
import importlib.util
import json
import os
from pathlib import Path
import shlex
import sys
import unittest
sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('rebase_common', SOURCE / 'amplifier-bundle/recipes/tests/test-checkpoint-scope.py')
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)

class RebaseByteIdentity(unittest.TestCase):
    def test_changed_indentation_cannot_impersonate_the_known_replay(self):
        original = 'def pick(flag):\n    if flag:\n        return 1\n    return 2\n'
        altered = 'def pick(flag):\n    if flag:\n        return 1\n        return 2\n'
        self.assertNotEqual(ast.dump(ast.parse(original)), ast.dump(ast.parse(altered)))
        f = common.GitFixture(self)
        f.write(b'source.txt', original.encode())
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'known local Python change')
        old_head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.write(b'upstream-only.txt', b'independent upstream\n', f.caller)
        f.git(f.caller, 'add', '--', 'upstream-only.txt')
        f.git(f.caller, 'commit', '-m', 'independent upstream advance')
        f.git(f.caller, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        before_remote = f.git(f.remote, 'show-ref')
        f.hook()
        f.hook(name='pre-push')
        payload = f.private / 'altered-python-source'
        payload.write_text(altered)
        real = shlex.quote(f.git_binary)
        f.executable(f.bin / 'git', 'if [ "${1-}" = pull ]; then\n  ' + real + ' "$@"\n  cp ' + shlex.quote(str(payload)) + ' source.txt\n  ' + real + ' add -- source.txt\n  ' + real + ' commit --amend --no-edit\n  exit 0\nfi\nexec ' + real + ' "$@"\n')
        result = common.PhaseProjection(f, common.PHASES[1]).run()
        head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        def patch_id(oid):
            parent = f.git(f.task, 'rev-parse', oid + '^').strip().decode()
            diff = f.git(f.task, 'diff-tree', '--no-ext-diff', '--no-renames', '-p', '--binary', parent, oid)
            return f.git(f.task, 'patch-id', '--stable', data=diff).split()[0].decode()
        record = {'exit_code':result.returncode,'old_head':old_head,'actual_head':head,'old_patch_id':patch_id(old_head),'actual_patch_id':patch_id(head),'actual_source':(f.task/'source.txt').read_text(),'remote_changed':f.git(f.remote,'show-ref')!=before_remote,'stdout':result.stdout.decode(errors='replace'),'stderr':result.stderr.decode(errors='replace'),'scope':'Private actual Git rebase and synthetic fixture hooks only; no project-hook or native runner acceptance.'}
        Path(os.environ.get('STAGING_REBASE_OBSERVATION', str(f.root / 'rebase-observation.json'))).write_text(json.dumps(record,indent=2)+'\n')
        self.assertEqual(record['old_patch_id'], record['actual_patch_id'], 'This control must exercise the patch-id collision.')
        self.assertEqual((f.task / 'source.txt').read_text(), altered, 'Refusal must preserve the actual resulting local work.')
        self.assertNotEqual(result.returncode, 0, 'Significant whitespace changes are not the known authorised replay.')
        self.assertEqual(f.git(f.remote, 'show-ref'), before_remote)

    def test_replay_object_writes_are_isolated_from_user_state(self):
        from unittest.mock import patch
        f = common.GitFixture(self)
        f.write(b"source.txt", b"known local source\n")
        f.git(f.task, "add", "--", "source.txt")
        f.git(f.task, "commit", "-m", "known local fixture change")
        old_head = f.git(f.task, "rev-parse", "HEAD").strip().decode()
        f.write(b"upstream-only.txt", b"independent upstream\n", f.caller)
        f.git(f.caller, "add", "--", "upstream-only.txt")
        f.git(f.caller, "commit", "-m", "independent fixture upstream")
        upstream = f.git(f.caller, "rev-parse", "HEAD").strip().decode()
        # Construct the incorrect rewritten result without ever producing the
        # expected combined tree in the repository's object store.
        f.git(f.task, "checkout", "-b", "fixture-rewritten", upstream)
        f.write(b"source.txt", b"different local source\n")
        f.git(f.task, "add", "--", "source.txt")
        f.git(f.task, "commit", "-m", "incorrect rewritten fixture change")
        rewritten = f.git(f.task, "rev-parse", "HEAD").strip().decode()
        before = f.snapshot()
        objects_before = f.git(f.task, "cat-file", "--batch-all-objects", "--batch-check=%(objectname) %(objecttype)")
        helper = common.TOOLS / "workflow_publish_checkpoint.sh"
        body = helper.read_text().split("<<'CHECKPOINT_PY'\n", 1)[1].split("\ntry:\n    action =", 1)[0]
        namespace = {}
        exec(compile(body, str(helper), "exec"), namespace)
        private = f.private / "isolated-replay-authority"
        private.mkdir(mode=0o700)
        namespace["root"] = str(f.task)
        namespace["observation_directory"] = private
        with patch.dict(os.environ, f.env, clear=True):
            with self.assertRaises(namespace["Refusal"]):
                namespace["verify_replay"](f.base, old_head, upstream, rewritten)
        self.assertEqual(f.snapshot(), before, "Validation must preserve the actual user index, blobs, worktree and refs.")
        self.assertEqual(f.git(f.task, "cat-file", "--batch-all-objects", "--batch-check=%(objectname) %(objecttype)"),
                         objects_before, "Replay validation must not add objects to the real repository.")
        events = [json.loads(line) for line in (private / "operations.jsonl").read_text().splitlines()]
        outputs = [entry for entry in events if entry["boundary"] == "lossless-replay-output"]
        self.assertEqual(len(outputs), 1)
        entry = outputs[0]
        directory = Path(entry["directory"])
        self.assertTrue(directory.resolve().is_relative_to(private.resolve()))
        self.assertTrue(any(name.startswith("objects/") and name != "objects/info/alternates"
                            for name in entry["private_object_files"]), "This control must produce actual private objects.")
        for name, expected in entry["private_object_files"].items():
            relative = Path(name)
            self.assertFalse(relative.is_absolute() or ".." in relative.parts)
            target = directory / relative
            self.assertTrue(target.resolve().is_relative_to(private.resolve()) and not target.is_symlink())
            self.assertEqual(common.digest(target.read_bytes()), expected)
        record = {"helper_sha256":common.digest(helper.read_bytes()), "normal_object_store_unchanged":True,
                  "index_worktree_blobs_and_refs_unchanged":True, "private_output":entry,
                  "scope":"Production replay function with real Git and synthetic task-owned graph; admission and full workflow acceptance are separate."}
        destination = Path(os.environ.get("STAGING_REPLAY_ISOLATION", str(f.root / "replay-isolation.json")))
        destination.write_text(json.dumps(record, indent=2) + "\n")

if __name__ == '__main__':
    unittest.main(verbosity=2)
