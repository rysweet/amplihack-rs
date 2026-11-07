#!/usr/bin/env python3
"""Focused controls for the three source findings, using task-owned Git fixtures."""
import importlib.util
import contextlib
from pathlib import Path
import unittest
import json
import base64
import os
import shlex
from unittest.mock import patch
import sys
sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('checkpoint_common', SOURCE / 'amplifier-bundle/recipes/tests/test-checkpoint-scope.py')
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)

class PublicationBoundaryTests(unittest.TestCase):
    def test_inherited_configuration_file_refuses_before_mutation(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'task change\n')
                alternate = f.private / 'alternative-git-config'
                alternate.write_text('')
                before = f.snapshot()
                f.env['GIT_CONFIG'] = str(alternate)
                result = common.PhaseProjection(f, phase).capture_scope()
                del f.env['GIT_CONFIG']
                self.assertNotEqual(result.returncode, 0, 'Configuration-view substitution must refuse before capture.')
                self.assertEqual(f.snapshot(), before)

    def test_other_upstream_cannot_define_the_publication_range(self):
        for phase in common.PHASES:
            for mode in ('current', 'upstream'):
                with self.subTest(phase=phase[0], push_default=mode):
                    f = common.GitFixture(self)
                    f.git(f.task, 'push', 'origin', 'HEAD:refs/heads/other-task')
                    f.git(f.task, 'config', 'branch.' + f.branch + '.merge', 'refs/heads/other-task')
                    f.git(f.task, 'config', 'push.default', mode)
                    f.write(b'source.txt', b'task change\n')
                    before = f.snapshot()
                    result = common.PhaseProjection(f, phase).capture_scope()
                    self.assertNotEqual(result.returncode, 0, 'The inspected destination must equal the owner publication branch.')
                    self.assertEqual(f.snapshot(), before)

    def test_shallow_history_refuses_before_capture(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.git(f.task, 'commit', '--allow-empty', '-m', 'real second fixture commit')
                shallow = f.caller / '.git/shallow'
                shallow.write_bytes(f.git(f.task, 'rev-parse', 'HEAD'))
                self.assertEqual(f.git(f.task, 'rev-parse', '--is-shallow-repository').strip(), b'true')
                f.write(b'source.txt', b'task change\n')
                before = f.snapshot()
                result = common.PhaseProjection(f, phase).capture_scope()
                self.assertNotEqual(result.returncode, 0, 'Incomplete parent history must refuse before capture.')
                self.assertEqual(f.snapshot(), before)

    def test_complete_matching_destination_still_captures(self):
        for phase in common.PHASES:
            for mode in ('simple', 'current', 'upstream'):
                with self.subTest(phase=phase[0], push_default=mode):
                    f = common.GitFixture(self)
                    f.git(f.task, 'config', 'push.default', mode)
                    f.write(b'source.txt', b'task change\n')
                    before = f.snapshot()
                    projection = common.PhaseProjection(f, phase)
                    result = projection.capture_scope()
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(projection.capture['publication']['ref'], 'refs/heads/' + f.branch)
                    self.assertEqual(f.snapshot(), before)

class DistinctPublicationDestinationControls(unittest.TestCase):
    def test_distinct_fetch_and_push_urls_preserve_authorised_publication(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                fetch = f.private / 'fetch-only.git'
                f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(fetch)])
                f.git(f.task, 'remote', 'set-url', 'origin', str(fetch))
                f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(f.remote))
                f.write(b'source.txt', b'authorised distinct destination source\n')
                f.hook()
                result = common.PhaseProjection(f, phase).run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch),
                                 f.git(f.task, 'rev-parse', 'HEAD'))
                self.assertEqual(f.git(fetch, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                self.assertTrue((f.root / 'pre-commit.log').exists())

    def test_distinct_fetch_tip_cannot_hide_the_actual_push_range(self):
        f = common.GitFixture(self)
        fetch = f.private / 'fetch-only.git'
        f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(fetch)])
        f.git(f.task, 'remote', 'set-url', 'origin', str(fetch))
        f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(f.remote))
        f.write(b'unrelated.txt', b'unreviewed fetch repository advance\n', f.caller)
        f.git(f.caller, 'add', '--', 'unrelated.txt')
        f.git(f.caller, 'commit', '-m', 'advance only the fetch repository')
        advanced = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.caller, 'push', str(fetch), 'HEAD:refs/heads/' + f.branch)
        f.git(f.task, 'fetch', 'origin')
        f.git(f.task, 'merge', '--ff-only', advanced)
        f.write(b'source.txt', b'authorised task source\n')
        original = f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch)
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertIn(advanced, {row['commit'] for row in projection.capture['outgoing_commits']})
        verdict = projection.verdict()
        self.assertFalse(verdict['approved'])
        before = f.snapshot()
        result = projection.finish(verdict)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'scope review refused or mismatched', result.stderr)
        self.assertEqual(f.snapshot(), before)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), original)

class InactiveReplayConfigurationControls(unittest.TestCase):
    def test_unused_global_filters_and_driver_preserve_a_legitimate_rebase(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'known local source change\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'known source before upstream movement')
        f.write(b'.gitattributes', b'*.bin filter=fixture-lfs\n', f.caller)
        f.write(b'upstream-only.txt', b'normal independent upstream source\n', f.caller)
        f.git(f.caller, 'add', '--', '.gitattributes', 'upstream-only.txt')
        f.git(f.caller, 'commit', '-m', 'upstream change with inactive LFS-style attributes')
        f.git(f.caller, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        marker = f.private / 'inactive-handler-executed'
        handler = f.private / 'inactive-global-handler'
        f.executable(handler, "printf 'unexpected external invocation\\n' > " + shlex.quote(str(marker)) + "\nexit 99\n")
        configuration = f.private / 'synthetic-global-gitconfig'
        configuration.write_text('')
        f.env['GIT_CONFIG_GLOBAL'] = str(configuration)
        for name in ('clean', 'smudge', 'process'):
            f.git(f.task, 'config', '--global', 'filter.fixture-lfs.' + name, shlex.quote(str(handler)))
        f.git(f.task, 'config', '--global', 'merge.unused-driver.driver', shlex.quote(str(handler)))
        f.git(f.task, 'config', '--global', 'merge.renormalize', 'true')
        original_configuration = configuration.read_bytes()
        f.hook(); f.hook(name='pre-push')
        result = common.PhaseProjection(f, common.PHASES[1]).run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(marker.exists(), 'Unused configuration must not be executed by validation.')
        self.assertEqual(configuration.read_bytes(), original_configuration)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch),
                         f.git(f.task, 'rev-parse', 'HEAD'))
        self.assertEqual((f.task / 'source.txt').read_bytes(), b'known local source change\n')
        self.assertEqual((f.task / 'upstream-only.txt').read_bytes(), b'normal independent upstream source\n')
        self.assertTrue((f.root / 'pre-push.log').exists())

class PublicationErrorDisclosure(unittest.TestCase):
    def test_failed_push_errors_do_not_export_disposable_userinfo(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                marker = 'DISPOSABLE_ERROR_SECRET_1554'
                # The real transport is a local, test-owned bare repository.
                # Only its rejecting hook prints a generated URL-shaped value.
                f.executable(f.remote / 'hooks/pre-receive',
                             "printf '%s\\n' 'https://fixture-user:" + marker +
                             "@example.invalid/test.git' >&2\nexit 1\n")
                f.write(b'source.txt', b'authorised source despite transport refusal\n')
                result = common.PhaseProjection(f, phase).run()
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn(marker.encode(), result.stdout + result.stderr)
                self.assertNotIn(b'fixture-user', result.stdout + result.stderr)
                self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'),
                                 b'authorised source despite transport refusal\n')

class FirstPublicationControls(unittest.TestCase):
    def test_first_publication_preserves_original_outcomes_and_normal_hooks(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.git(f.task, 'branch', '--unset-upstream')
                f.git(f.remote, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.git(f.task, 'update-ref', '-d', 'refs/remotes/origin/' + f.branch)
                f.write(b'source.txt', b'first authorised publication\n')
                f.hook()
                result = common.PhaseProjection(f, phase).run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((f.root / 'pre-commit.log').exists())
                if phase[0] == 'workflow-publish':
                    # The retained original step explicitly skips publication
                    # without tracking. Do not invent a first-push obligation.
                    self.assertIn(b'"reason":"no-upstream"', result.stdout)
                    self.assertNotEqual(f.run([f.git_binary, '-C', str(f.remote), 'show-ref', '--verify',
                                               'refs/heads/' + f.branch], check=False).returncode, 0)
                else:
                    self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch),
                                     f.git(f.task, 'rev-parse', 'HEAD'))
                self.assertEqual({p for _, _, paths in f.history() for p in paths}, {b'source.txt'})

    def test_retained_publish_phase_also_skips_without_upstream(self):
        f = common.GitFixture(self)
        f.git(f.task, 'branch', '--unset-upstream')
        f.git(f.remote, 'update-ref', '-d', 'refs/heads/' + f.branch)
        f.git(f.task, 'update-ref', '-d', 'refs/remotes/origin/' + f.branch)
        f.write(b'source.txt', b'original phase fixture source\n')
        f.hook()
        original = common.blocks(common.index_source('amplifier-bundle/recipes/workflow-publish.yaml'))
        command = common.literal(original['step-15-commit-push'], 'command')
        result = f.shell(command)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(b'"reason":"no-upstream"', result.stdout)
        self.assertTrue((f.root / 'pre-commit.log').exists())
        self.assertNotEqual(f.run([f.git_binary, '-C', str(f.remote), 'show-ref', '--verify',
                                   'refs/heads/' + f.branch], check=False).returncode, 0)

    def test_stale_local_destination_cannot_hide_remote_outgoing_paths(self):
        f = common.GitFixture(self)
        f.git(f.task, 'branch', '--unset-upstream')
        f.write(b'unrelated.txt', b'unreviewed existing commit\n', f.caller)
        f.git(f.caller, 'add', '--', 'unrelated.txt')
        f.git(f.caller, 'commit', '-m', 'unreviewed local base movement')
        advanced = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'merge', '--ff-only', advanced)
        f.git(f.task, 'update-ref', 'refs/remotes/origin/' + f.branch, advanced)
        f.write(b'source.txt', b'authorised pending source\n')
        # Prepare the legitimate existing commit before authority is requested.
        # Refusal must preserve it rather than manufacture a later commit.
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'authorised existing source')
        original_remote = f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch)
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertIn(advanced, {row['commit'] for row in projection.capture['outgoing_commits']})
        verdict = projection.verdict()
        self.assertFalse(verdict['approved'])
        before = f.snapshot()
        result = projection.finish(verdict)
        self.assertNotEqual(result.returncode, 0, 'The actual destination range must be checked before push.')
        self.assertEqual(f.snapshot(), before)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), original_remote)
        self.assertIn(b'scope review refused or mismatched', result.stderr)
        # A legitimate local commit may already have happened. Preserve it;
        # refusal is not permission to reset or discard authorised work.
        self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'), b'authorised pending source\n')

class UntrackedPublicationRange(unittest.TestCase):
 def test_existing_destination_is_not_replaced_by_a_moved_base(self):
  f=common.GitFixture(self)
  f.git(f.task,'branch','--unset-upstream')
  f.write(b'unrelated.txt',b'previously unreviewed base advance\n',f.caller)
  f.git(f.caller,'add','--','unrelated.txt')
  f.git(f.caller,'commit','-m','independent base advance after task setup')
  advanced=f.git(f.caller,'rev-parse','HEAD').strip().decode()
  f.git(f.caller,'push','origin','fixture-base')
  f.git(f.task,'merge','--ff-only',advanced)
  f.write(b'source.txt',b'owned source change\n')
  destination=f.git(f.remote,'rev-parse','refs/heads/'+f.branch).strip().decode()
  actual_range=f.git(f.task,'rev-list',destination+'..HEAD').splitlines()
  projection=common.PhaseProjection(f,common.PHASES[0])
  result=projection.capture_scope()
  self.assertEqual(result.returncode,0,result.stderr)
  captured={row['commit'] for row in projection.capture['outgoing_commits']}
  record={'source_helper_sha256':common.digest((common.TOOLS/'workflow_publish_checkpoint.sh').read_bytes()),
          'destination_head':destination,'actual_outgoing':[x.decode() for x in actual_range],
          'captured_outgoing':sorted(captured),'captured_publication':projection.capture['publication'],
          'owner_paths':[common.encoded(b'source.txt')],'base_ref':f.context['worktree_setup']['base_ref']}
  self.assertEqual(captured,{x.decode() for x in actual_range},
                   'An existing owner destination must define the complete outgoing range when tracking is absent.')

class CaptureUrlDisclosure(unittest.TestCase):
 def test_capture_does_not_export_disposable_url_credentials(self):
  f=common.GitFixture(self)
  # Real Git queries only an owned file-protocol bare repository. Generated
  # userinfo remains part of the resolved URI, never an actual credential.
  # GitFixture restricts transport to file; no external request is possible.
  marker='DISPOSABLE_TEST_PASSWORD_1554'
  f.git(f.task,'remote','set-url','origin','file://fixture-user:'+marker+'@localhost'+str(f.remote))
  f.write(b'source.txt',b'owned source\n')
  projection=common.PhaseProjection(f,common.PHASES[0])
  result=projection.capture_scope()
  self.assertEqual(result.returncode,0,result.stderr)
  record={'exit_code':result.returncode,'disposable_marker_in_public_stdout':marker.encode() in result.stdout,
          'publication':projection.capture['publication'],'source_helper_sha256':common.digest((common.TOOLS/'workflow_publish_checkpoint.sh').read_bytes()),
          'scope':'Generated URI userinfo and actual private file transport only; no network or real credential access.'}
  self.assertNotIn(marker.encode(),result.stdout,'The public model capture must not disclose URL credentials.')

class ExternalReplayDriver(unittest.TestCase):
 def test_replay_does_not_execute_a_repository_merge_driver(self):
  f=common.GitFixture(self)
  f.write(b'source.txt',b'known original source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','known local change')
  old=f.git(f.task,'rev-parse','HEAD').strip().decode()
  f.write(b'source.txt',b'conflicting upstream source\n',f.caller)
  f.write(b'.gitattributes',b'source.txt merge=fixture-driver\n',f.caller)
  f.git(f.caller,'add','--','source.txt','.gitattributes');f.git(f.caller,'commit','-m','fixture upstream with driver attribute')
  upstream=f.git(f.caller,'rev-parse','HEAD').strip().decode()
  f.git(f.task,'checkout','-b','fixture-rewritten',upstream)
  f.write(b'source.txt',b'incorrect rewritten source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','incorrect rewritten fixture')
  rewritten=f.git(f.task,'rev-parse','HEAD').strip().decode()
  marker=f.task/'outside-review-scope.txt'
  driver=f.private/'repository-merge-driver'
  f.executable(driver,"printf 'fixture driver executed\\n' > "+shlex.quote(str(marker))+"\nexit 1\n")
  f.git(f.task,'config','merge.fixture-driver.driver',shlex.quote(str(driver))+' %O %A %B')
  before=f.snapshot()
  helper=common.TOOLS/'workflow_publish_checkpoint.sh'
  body=helper.read_text().split("<<'CHECKPOINT_PY'\n",1)[1].split('\ntry:\n    action =',1)[0]
  namespace={};exec(compile(body,str(helper),'exec'),namespace)
  private=f.private/'driver-replay-authority';private.mkdir(mode=0o700)
  namespace['root']=str(f.task);namespace['observation_directory']=private
  with patch.dict(os.environ,f.env,clear=True):
   with self.assertRaises(namespace['Refusal']):
    namespace['verify_replay'](f.base,old,upstream,rewritten)
  record={'driver_executed':marker.exists(),'worktree_preserved':before['tree']==f.snapshot()['tree'],
          'normal_index_preserved':before['raw_index']==f.snapshot()['raw_index'],
          'helper_sha256':common.digest(helper.read_bytes()),
          'scope':'Actual Git with an intentionally generated private merge driver; no external service, credential or profile.'}
  self.assertFalse(marker.exists(),'Replay validation must refuse unsupported external drivers before invocation.')
  self.assertEqual(f.snapshot(),before)

class ExternalReplayFilter(unittest.TestCase):
 def test_replay_does_not_execute_a_repository_clean_filter(self):
  f=common.GitFixture(self)
  f.write(b'source.txt',b'known original source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','known local change')
  old=f.git(f.task,'rev-parse','HEAD').strip().decode()
  f.write(b'source.txt',b'conflicting upstream source\n',f.caller)
  f.write(b'.gitattributes',b'source.txt filter=fixture-clean\n',f.caller)
  f.git(f.caller,'add','--','source.txt','.gitattributes');f.git(f.caller,'commit','-m','fixture upstream with driver attribute')
  upstream=f.git(f.caller,'rev-parse','HEAD').strip().decode()
  f.git(f.task,'checkout','-b','fixture-rewritten',upstream)
  f.write(b'source.txt',b'incorrect rewritten source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','incorrect rewritten fixture')
  rewritten=f.git(f.task,'rev-parse','HEAD').strip().decode()
  marker=f.task/'outside-review-scope.txt'
  driver=f.private/'repository-clean-filter'
  f.executable(driver,"printf 'fixture driver executed\\n' > "+shlex.quote(str(marker))+"\ncat\n")
  f.git(f.task,'config','filter.fixture-clean.clean',shlex.quote(str(driver)))
  f.git(f.task,'config','merge.renormalize','true')
  before=f.snapshot()
  helper=common.TOOLS/'workflow_publish_checkpoint.sh'
  body=helper.read_text().split("<<'CHECKPOINT_PY'\n",1)[1].split('\ntry:\n    action =',1)[0]
  namespace={};exec(compile(body,str(helper),'exec'),namespace)
  private=f.private/'driver-replay-authority';private.mkdir(mode=0o700)
  namespace['root']=str(f.task);namespace['observation_directory']=private
  with patch.dict(os.environ,f.env,clear=True):
   with self.assertRaises(namespace['Refusal']):
    namespace['verify_replay'](f.base,old,upstream,rewritten)
  record={'driver_executed':marker.exists(),'worktree_preserved':before['tree']==f.snapshot()['tree'],
          'normal_index_preserved':before['raw_index']==f.snapshot()['raw_index'],
          'helper_sha256':common.digest(helper.read_bytes()),
          'scope':'Actual Git with an intentionally generated private merge driver; no external service, credential or profile.'}
  self.assertFalse(marker.exists(),'Replay validation must refuse unsupported external filters before invocation.')
  self.assertEqual(f.snapshot(),before)

class ExternalReplayProcessFilter(unittest.TestCase):
 def test_replay_does_not_execute_a_repository_process_filter(self):
  f=common.GitFixture(self)
  f.write(b'source.txt',b'known original source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','known local change')
  old=f.git(f.task,'rev-parse','HEAD').strip().decode()
  f.write(b'source.txt',b'conflicting upstream source\n',f.caller)
  f.write(b'.gitattributes',b'source.txt filter=fixture-clean\n',f.caller)
  f.git(f.caller,'add','--','source.txt','.gitattributes');f.git(f.caller,'commit','-m','fixture upstream with driver attribute')
  upstream=f.git(f.caller,'rev-parse','HEAD').strip().decode()
  f.git(f.task,'checkout','-b','fixture-rewritten',upstream)
  f.write(b'source.txt',b'incorrect rewritten source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','incorrect rewritten fixture')
  rewritten=f.git(f.task,'rev-parse','HEAD').strip().decode()
  marker=f.task/'outside-review-scope.txt'
  driver=f.private/'repository-clean-filter'
  f.executable(driver,"printf 'fixture driver executed\\n' > "+shlex.quote(str(marker))+"\nexit 1\n")
  f.git(f.task,'config','filter.fixture-clean.process',shlex.quote(str(driver)))
  f.git(f.task,'config','merge.renormalize','true')
  before=f.snapshot()
  helper=common.TOOLS/'workflow_publish_checkpoint.sh'
  body=helper.read_text().split("<<'CHECKPOINT_PY'\n",1)[1].split('\ntry:\n    action =',1)[0]
  namespace={};exec(compile(body,str(helper),'exec'),namespace)
  private=f.private/'driver-replay-authority';private.mkdir(mode=0o700)
  namespace['root']=str(f.task);namespace['observation_directory']=private
  with patch.dict(os.environ,f.env,clear=True):
   with self.assertRaises(namespace['Refusal']):
    namespace['verify_replay'](f.base,old,upstream,rewritten)
  record={'driver_executed':marker.exists(),'worktree_preserved':before['tree']==f.snapshot()['tree'],
          'normal_index_preserved':before['raw_index']==f.snapshot()['raw_index'],
          'helper_sha256':common.digest(helper.read_bytes()),
          'scope':'Actual Git with an intentionally generated private merge driver; no external service, credential or profile.'}
  self.assertFalse(marker.exists(),'Replay validation must refuse unsupported external filters before invocation.')
  self.assertEqual(f.snapshot(),before)

class ExternalReplaySmudgeFilter(unittest.TestCase):
 def test_replay_does_not_execute_a_repository_smudge_filter(self):
  f=common.GitFixture(self)
  f.write(b'source.txt',b'known original source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','known local change')
  old=f.git(f.task,'rev-parse','HEAD').strip().decode()
  f.write(b'source.txt',b'conflicting upstream source\n',f.caller)
  f.write(b'.gitattributes',b'source.txt filter=fixture-clean\n',f.caller)
  f.git(f.caller,'add','--','source.txt','.gitattributes');f.git(f.caller,'commit','-m','fixture upstream with driver attribute')
  upstream=f.git(f.caller,'rev-parse','HEAD').strip().decode()
  f.git(f.task,'checkout','-b','fixture-rewritten',upstream)
  f.write(b'source.txt',b'incorrect rewritten source\n')
  f.git(f.task,'add','--','source.txt');f.git(f.task,'commit','-m','incorrect rewritten fixture')
  rewritten=f.git(f.task,'rev-parse','HEAD').strip().decode()
  marker=f.task/'outside-review-scope.txt'
  driver=f.private/'repository-clean-filter'
  f.executable(driver,"printf 'fixture driver executed\\n' > "+shlex.quote(str(marker))+"\ncat\n")
  f.git(f.task,'config','filter.fixture-clean.smudge',shlex.quote(str(driver)))
  f.git(f.task,'config','merge.renormalize','true')
  before=f.snapshot()
  helper=common.TOOLS/'workflow_publish_checkpoint.sh'
  body=helper.read_text().split("<<'CHECKPOINT_PY'\n",1)[1].split('\ntry:\n    action =',1)[0]
  namespace={};exec(compile(body,str(helper),'exec'),namespace)
  private=f.private/'driver-replay-authority';private.mkdir(mode=0o700)
  namespace['root']=str(f.task);namespace['observation_directory']=private
  with patch.dict(os.environ,f.env,clear=True):
   with self.assertRaises(namespace['Refusal']):
    namespace['verify_replay'](f.base,old,upstream,rewritten)
  record={'driver_executed':marker.exists(),'worktree_preserved':before['tree']==f.snapshot()['tree'],
          'normal_index_preserved':before['raw_index']==f.snapshot()['raw_index'],
          'helper_sha256':common.digest(helper.read_bytes()),
          'scope':'Actual Git with an intentionally generated private merge driver; no external service, credential or profile.'}
  self.assertFalse(marker.exists(),'Replay validation must refuse unsupported external filters before invocation.')
  self.assertEqual(f.snapshot(),before)

class UncapturedCommit(unittest.TestCase):
    def test_a_known_existing_commit_is_captured_against_the_actual_destination(self):
        f = common.GitFixture(self)
        f.git(f.task, 'branch', '--unset-upstream')
        f.write(b'source.txt', b'previously existing source history\n', f.caller)
        f.git(f.caller, 'add', '--', 'source.txt')
        f.git(f.caller, 'commit', '-m', 'existing prior source commit')
        advanced = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'merge', '--ff-only', advanced)
        f.git(f.task, 'update-ref', 'refs/remotes/origin/' + f.branch, advanced)
        f.write(b'source.txt', b'current reviewed source content\n')
        f.hook()
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertIn(advanced, {row['commit'] for row in projection.capture['outgoing_commits']})
        result = projection.finish(projection.verdict())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((f.root / 'pre-commit.log').exists())
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch),
                         f.git(f.task, 'rev-parse', 'HEAD'))

    def test_a_known_existing_commit_needs_capture_even_if_its_path_is_allowed(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'known earlier source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'known earlier source')
        known = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        f.write(b'source.txt', b'current reviewed source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'current reviewed source')
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertNotIn(known, {row['commit'] for row in projection.capture['outgoing_commits']})
        verdict = projection.verdict()
        self.assertTrue(verdict['approved'])
        # A real destination transition exposes the known commit only after
        # capture. Local tracking alone is deliberately left unchanged.
        f.git(f.remote, 'update-ref', 'refs/heads/' + f.branch, f.base)
        before = f.snapshot()
        result = projection.finish(verdict)
        self.assertNotEqual(result.returncode, 0, 'An allowed path does not qualify an omitted existing commit.')
        self.assertIn(b'initial candidates changed', result.stderr)
        self.assertEqual(f.snapshot(), before)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)


class AbsentDestinationAfterAdmissionControls(unittest.TestCase):
    def delete_destination_in_normal_hook(self, fixture, advance_prepared_base_to=None):
        # Git sets repository selectors for hooks. Clear only those fixture
        # selectors so this command addresses the explicitly named bare remote.
        command = ('env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR -u GIT_INDEX_FILE '
                   + shlex.quote(str(fixture.git_binary)) + ' --git-dir='
                   + shlex.quote(str(fixture.remote)) + ' update-ref -d refs/heads/'
                   + fixture.branch + '\n')
        if advance_prepared_base_to is not None:
            base_ref = 'refs/heads/' + fixture.context['worktree_setup']['base_branch']
            command += ('env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR -u GIT_INDEX_FILE '
                        + shlex.quote(str(fixture.git_binary)) + ' --git-dir='
                        + shlex.quote(str(fixture.remote)) + ' update-ref '
                        + shlex.quote(base_ref) + ' ' + shlex.quote(advance_prepared_base_to) + '\n')
        fixture.hook(extra=command)

    def test_publish_refuses_newly_exposed_known_commit_after_hook_deletes_destination(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'known earlier source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'known earlier source')
        known = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        f.write(b'source.txt', b'current reviewed source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'current reviewed source')
        initial_head = f.git(f.task, 'rev-parse', 'HEAD')
        f.write(b'source.txt', b'new authorised pending source\n')
        self.delete_destination_in_normal_hook(f)
        projection = common.PhaseProjection(f, common.PHASES[0])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertNotIn(known, {row['commit'] for row in projection.capture['outgoing_commits']})
        result = projection.finish(projection.verdict())
        self.assertNotEqual(result.returncode, 0, 'Deleted destinations must not expose unreviewed existing commits.')
        # The operation callback propagates failure without promising the
        # private inspection diagnostic as public stderr. Verify its actual
        # publication outcome and retained authorised commit independently.
        self.assertNotIn(b'"pushed":"true"', result.stdout)
        self.assertNotEqual(f.git(f.task, 'rev-parse', 'HEAD'), initial_head,
                            'An authorised local commit must be preserved after publication refusal.')
        self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'), b'new authorised pending source\n')
        self.assertTrue((f.root / 'pre-commit.log').exists())
        self.assertNotEqual(f.run([f.git_binary, '-C', str(f.remote), 'show-ref', '--verify',
                                  'refs/heads/' + f.branch], check=False).returncode, 0)

    def test_moving_prepared_base_cannot_hide_known_uncaptured_history(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'known earlier source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'known earlier source')
        known = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        f.write(b'source.txt', b'current reviewed source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'current reviewed source')
        f.write(b'source.txt', b'new authorised pending source\n')
        self.delete_destination_in_normal_hook(f, advance_prepared_base_to=known)
        projection = common.PhaseProjection(f, common.PHASES[0])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertEqual(projection.capture['publication']['prepared_base_oid'], f.base)
        self.assertNotIn(known, {row['commit'] for row in projection.capture['outgoing_commits']})
        result = projection.finish(projection.verdict())
        self.assertNotEqual(result.returncode, 0, 'A tracking-ref advance cannot renew captured commit authority.')
        self.assertEqual(f.git(f.task, 'rev-parse', f.context['worktree_setup']['base_ref']).strip().decode(), known)
        self.assertEqual(projection.capture['publication']['prepared_base_oid'], f.base)
        self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'), b'new authorised pending source\n')
        self.assertTrue((f.root / 'pre-commit.log').exists())
        self.assertNotEqual(f.run([f.git_binary, '-C', str(f.remote), 'show-ref', '--verify',
                                  'refs/heads/' + f.branch], check=False).returncode, 0)

    def test_publish_can_restore_an_absent_destination_when_all_existing_history_was_captured(self):
        f = common.GitFixture(self)
        f.write(b'source.txt', b'authorised new publication\n')
        self.delete_destination_in_normal_hook(f)
        projection = common.PhaseProjection(f, common.PHASES[0])
        self.assertEqual(projection.capture_scope().returncode, 0)
        result = projection.finish(projection.verdict())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((f.root / 'pre-commit.log').exists())
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch),
                         f.git(f.task, 'rev-parse', 'HEAD'))


class IndependentPublicationEvidenceControls(unittest.TestCase):
    def test_real_destination_distinguishes_local_commit_from_actual_publication(self):
        file = SOURCE / 'amplifier-bundle/recipes/tests/canonical_staging_publication.py'
        specification = importlib.util.spec_from_file_location('destination_oracle', file)
        oracle = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(oracle)
        f = common.GitFixture(self)
        f.write(b'source.txt', b'authorised checkpoint source\n')
        f.git(f.task, 'add', '--', 'source.txt')
        f.git(f.task, 'commit', '-m', 'local source without publication')
        head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        expected = {'source.txt': {'mode': '100644', 'contents_base64':
                    base64.b64encode(b'authorised checkpoint source\n').decode()}}
        inspect = lambda: oracle.destination_observation(
            lambda *args: f.git(f.remote, *args), 'refs/heads/' + f.branch, head, expected)
        missing = inspect()
        self.assertFalse(missing['publication_observed'])
        self.assertFalse(missing['matches_local_head'])
        f.git(f.task, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
        published = inspect()
        self.assertTrue(published['publication_observed'])
        self.assertEqual(published['entries']['source.txt']['contents_base64'],
                         expected['source.txt']['contents_base64'])


class ActualDestinationAndAncestryControls(unittest.TestCase):
    def test_grafted_ancestry_refuses_before_capture_or_mutation(self):
        for mode in ('environment', 'repository'):
            with self.subTest(mode=mode):
                f = common.GitFixture(self)
                f.write(b'unrelated.txt', b'unapproved transient source\n')
                f.git(f.task, 'add', '--', 'unrelated.txt')
                f.git(f.task, 'commit', '-m', 'unapproved transient source')
                hidden = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                f.write(b'unrelated.txt', b'initial\n')
                f.write(b'source.txt', b'owned final source\n')
                f.git(f.task, 'add', '--', 'unrelated.txt', 'source.txt')
                f.git(f.task, 'commit', '-m', 'restore transient and change owned source')
                plain = common.PhaseProjection(f, common.PHASES[0])
                result = plain.capture_scope()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(hidden, [row['commit'] for row in plain.capture['outgoing_commits']])
                self.assertFalse(plain.verdict()['approved'])
                head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                if mode == 'environment':
                    graft = f.private / 'grafts'
                    f.env['GIT_GRAFT_FILE'] = str(graft)
                else:
                    metadata = Path(os.fsdecode(f.git(f.task, 'rev-parse', '--git-common-dir').strip()))
                    if not metadata.is_absolute():
                        metadata = f.task / metadata
                    graft = metadata / 'info/grafts'
                    graft.parent.mkdir(exist_ok=True)
                graft.write_text(head + ' ' + f.base + '\n')
                before = f.snapshot()
                result = common.PhaseProjection(f, common.PHASES[0]).capture_scope()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)

    def test_already_pushed_uses_the_actual_destination_and_normal_push(self):
        for state in ('matching', 'behind', 'absent'):
            with self.subTest(destination=state):
                f = common.GitFixture(self)
                destination = f.root / 'private-push.git'
                if state != 'matching':
                    f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(destination)])
                    if state == 'absent':
                        f.git(destination, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.hook()
                f.hook(name='pre-push')
                f.write(b'source.txt', b'owned final source\n')
                f.git(f.task, 'add', '--', 'source.txt')
                f.git(f.task, 'commit', '-m', 'owned source with normal commit hook')
                f.git(f.task, 'push', 'origin', f.branch)
                previous_hooks = (f.root / 'pre-push.log').read_bytes()
                if state != 'matching':
                    f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(destination))
                projection = common.PhaseProjection(f, common.PHASES[0])
                captured = projection.capture_scope()
                self.assertEqual(captured.returncode, 0, captured.stderr)
                if state != 'matching':
                    self.assertIn(f.git(f.task, 'rev-parse', 'HEAD').strip().decode(),
                                  [row['commit'] for row in projection.capture['outgoing_commits']])
                result = projection.finish(projection.verdict())
                self.assertEqual(result.returncode, 0, result.stderr)
                outcome = json.loads(result.stdout)
                self.assertIn(outcome['pushed'], (True, 'true'))
                actual = destination if state != 'matching' else f.remote
                self.assertEqual(f.git(actual, 'rev-parse', 'refs/heads/' + f.branch),
                                 f.git(f.task, 'rev-parse', 'HEAD'))
                self.assertEqual((f.root / 'pre-push.log').read_bytes(),
                                 previous_hooks + (b'hook\n' if state != 'matching' else b''))
                self.assertEqual(outcome['reason'], 'already-pushed' if state == 'matching' else 'pushed')
                self.assertTrue((f.root / 'pre-commit.log').is_file())


_CAPTURE_TRANSFORM_PROCESS = r'''import sys,pathlib
marker=pathlib.Path(sys.argv[1])
with marker.open('ab') as out:out.write(b'started\n')
r=sys.stdin.buffer;w=sys.stdout.buffer
def packet():
 h=r.read(4)
 if not h:return None
 n=int(h,16)
 return b'' if n==0 else r.read(n-4)
def group():
 values=[]
 while True:
  v=packet()
  if v is None:return None
  if v==b'':return values
  values.append(v)
def write(data):w.write(('%04x'%(len(data)+4)).encode()+data)
def flush():w.write(b'0000');w.flush()
assert group()==[b'git-filter-client\n',b'version=2\n']
write(b'git-filter-server\n');write(b'version=2\n');flush()
capabilities=group();assert b'capability=clean\n' in capabilities
write(b'capability=clean\n');flush()
while True:
 headers=group()
 if headers is None:break
 chunks=group();assert chunks is not None
 assert b'command=clean\n' in headers
 write(b'status=success\n');flush()
 for chunk in chunks:write(chunk.upper())
 flush();flush()
'''

class CaptureFilterExecutionControls(unittest.TestCase):
    observations = []

    def record(self, value):
        self.observations.append(value)
        target = os.environ.get('STAGING_CAPTURE_FILTER_OBSERVATION')
        if target:
            Path(target).write_text(json.dumps(self.observations, indent=2) + '\n')

    def fixture(self, kind, referenced=True, attribute_path='source.txt'):
        f = common.GitFixture(self)
        # The fixture baseline declares attributes before operator commands are
        # registered. Task permission remains the independent literal source.txt.
        f.own([b'source.txt'])
        pattern = attribute_path if referenced else 'absent-filter-fixture'
        f.write(b'.gitattributes', (pattern + ' filter=capture-fixture\n').encode())
        f.git(f.task, 'add', '--', '.gitattributes')
        f.git(f.task, 'commit', '-m', 'independent fixture attribute baseline')
        f.git(f.task, 'push', 'origin', f.branch)
        marker, handler = f.private / 'operator-called', f.private / 'operator-filter.py'
        if kind == 'process':
            body = _CAPTURE_TRANSFORM_PROCESS
        else:
            body = ("import pathlib,sys\n"
                    "with pathlib.Path(sys.argv[1]).open('ab') as marker: marker.write(b'called\\n')\n"
                    "sys.stdout.buffer.write(sys.stdin.buffer.read().upper())\n")
        handler.write_text(body)
        command = shlex.join([sys.executable, str(handler), str(marker)])
        f.git(f.task, 'config', 'filter.capture-fixture.' + kind, command)
        f.git(f.task, 'config', 'filter.capture-fixture.required', 'true')
        return f, marker, command

    def state(self, f):
        index = Path(os.fsdecode(f.git(f.task, 'rev-parse', '--git-path', 'index').strip()))
        if not index.is_absolute():
            index = f.task / index
        # No diff/status query here: this independent preservation check must
        # itself avoid invoking the operator before the capture under test.
        return (index.read_bytes(), f.git(f.task, 'rev-parse', 'HEAD'),
                (f.task / 'source.txt').read_bytes(), (f.task / 'unrelated.txt').read_bytes())

    def test_capture_then_finite_authority_preserves_real_transforming_stage(self):
        for kind in ('clean', 'process'):
            for referenced in (False, True):
                with self.subTest(handler=kind, referenced=referenced):
                    f, marker, command = self.fixture(kind, referenced)
                    payload = b'owned filtered source\n'
                    f.write(b'source.txt', payload)
                    before = self.state(f)
                    projection = common.PhaseProjection(f, common.PHASES[0])
                    captured = projection.capture_scope()
                    self.record({'kind': kind, 'referenced': referenced, 'phase': 'capture',
                                 'exit_code': captured.returncode, 'operator_invoked': marker.exists(),
                                 'state_preserved': self.state(f) == before})
                    self.assertEqual(captured.returncode, 0, captured.stderr)
                    self.assertFalse(marker.exists())
                    self.assertEqual(self.state(f), before)
                    self.assertIn('qualified identity', projection.capture['inspection_policy'])
                    verdict = projection.verdict()
                    self.assertTrue(verdict['approved'])
                    f.context['checkpoint_scope_review'] = verdict
                    f.sync_context()
                    # These are the actual admission and strict-stage helpers.
                    # The real operator configuration is retained throughout.
                    shell = ('set -euo pipefail\n. "$1"\nworkflow_checkpoint_enter publish\n'
                             'workflow_checkpoint_stage\n'
                             + ('test -s "$2"\n' if referenced else 'test ! -e "$2"\n')
                             + 'git show :source.txt\n')
                    staged = f.shell(shell, str(common.TOOLS / 'workflow_publish_checkpoint.sh'), str(marker))
                    expected = payload.upper() if referenced else payload
                    self.record({'kind': kind, 'referenced': referenced, 'phase': 'authorised-stage',
                                 'exit_code': staged.returncode, 'operator_invoked': marker.exists(),
                                 'exact_index_blob': staged.stdout == expected,
                                 'normal_configuration_preserved': f.git(f.task, 'config', '--get', 'filter.capture-fixture.' + kind).strip().decode() == command})
                    self.assertEqual(staged.returncode, 0, staged.stderr)
                    self.assertEqual(staged.stdout, expected)
                    self.assertEqual(f.staged_paths(), {b'source.txt'})
                    self.assertEqual((f.task / 'source.txt').read_bytes(), payload)
                    self.assertEqual(marker.exists(), referenced)
                    self.assertEqual(f.git(f.task, 'config', '--get', 'filter.capture-fixture.' + kind).strip().decode(), command)
                    self.assertEqual(f.git(f.task, 'config', '--get', 'filter.capture-fixture.required').strip(), b'true')

    def test_unowned_filter_candidate_never_gains_path_permission(self):
        for kind in ('clean', 'process'):
            with self.subTest(handler=kind):
                f, marker, command = self.fixture(kind, attribute_path='unrelated.txt')
                f.write(b'source.txt', b'owned source\n')
                f.write(b'unrelated.txt', b'unowned filtered source\n')
                before = self.state(f)
                projection = common.PhaseProjection(f, common.PHASES[0])
                captured = projection.capture_scope()
                self.assertEqual(captured.returncode, 0, captured.stderr)
                verdict = projection.verdict()
                self.assertFalse(verdict['approved'])
                refused = projection.finish(verdict)
                self.record({'kind': kind, 'phase': 'unowned-refusal', 'exit_code': refused.returncode,
                             'operator_invoked': marker.exists(), 'state_preserved': self.state(f) == before})
                self.assertNotEqual(refused.returncode, 0)
                self.assertFalse(marker.exists())
                self.assertEqual(self.state(f), before)

    def test_filter_normalised_file_is_an_explicit_conservative_candidate(self):
        for kind in ('clean', 'process'):
            with self.subTest(handler=kind):
                f, marker, command = self.fixture(kind)
                payload = b'normalised fixture bytes\n'
                f.write(b'source.txt', payload)
                f.git(f.task, 'add', '--', 'source.txt')
                f.git(f.task, 'commit', '-m', 'independently prepared normalised baseline')
                f.git(f.task, 'push', 'origin', f.branch)
                self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'), payload.upper())
                os.utime(f.task / 'source.txt', ns=(1700000000000000000, 1700000000000000000))
                index_path = Path(os.fsdecode(f.git(f.task, 'rev-parse', '--git-path', 'index').strip()))
                if not index_path.is_absolute():
                    index_path = f.task / index_path
                index_before_normal = common.digest(index_path.read_bytes())
                normal = f.run([f.git_binary, '-C', str(f.task), 'diff', '--quiet'], env=f.env, check=False)
                self.assertEqual(normal.returncode, 0, normal.stderr)
                index_after_normal = common.digest(index_path.read_bytes())
                # Dirty only metadata after the independent logical-cleanliness
                # control, so that control cannot prepare away the tested state.
                os.utime(f.task / 'source.txt', ns=(1600000000000000000, 1600000000000000000))
                marker.unlink(missing_ok=True)
                projection = common.PhaseProjection(f, common.PHASES[0])
                captured = projection.capture_scope()
                self.assertEqual(captured.returncode, 0, captured.stderr)
                paths = {base64.b64decode(row['path_b64']) for row in projection.capture['paths']}
                self.record({'kind': kind, 'phase': 'conservative-candidate', 'capture_exit': captured.returncode,
                             'normal_git_logically_clean': normal.returncode == 0,
                             'index_before_normal_sha256': index_before_normal,
                             'index_after_normal_sha256': index_after_normal,
                             'working_bytes_unchanged': (f.task / 'source.txt').read_bytes() == payload,
                             'metadata_changed_after_normal_control': True,
                             'source_is_candidate': b'source.txt' in paths, 'operator_invoked_during_capture': marker.exists()})
                self.assertEqual(captured.returncode, 0, captured.stderr)
                self.assertIn(b'source.txt', paths)
                self.assertFalse(marker.exists())

class OwnerOutgoingOnlyControls(unittest.TestCase):
    """Decision 17.116: real outgoing history, without unreachable-object authority."""

    @staticmethod
    def record(row):
        destination = os.environ.get('STAGING_OUTGOING_OBSERVATION')
        if destination:
            path = Path(destination)
            rows = json.loads(path.read_text()) if path.exists() else []
            rows.append(row)
            path.write_text(json.dumps(rows, indent=2) + '\n')

    def test_all_nine_wildcard_fetch_cases_publish(self):
        for phase in common.PHASES:
            for mode in ('no-sibling', 'allowed-sibling', 'unrelated-sibling'):
                with self.subTest(phase=phase[0], mode=mode):
                    f = common.GitFixture(self)
                    sibling = None
                    if mode != 'no-sibling':
                        producer = f.private / 'independent-producer'
                        f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(f.remote), str(producer)])
                        name = b'source.txt' if mode == 'allowed-sibling' else b'unrelated.txt'
                        f.write(name, b'independent remote sibling\n', producer)
                        f.git(producer, 'add', '--', name.decode())
                        f.git(producer, 'commit', '-m', 'independent sibling')
                        sibling = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                        f.git(producer, 'push', 'origin', 'HEAD:refs/heads/independent-sibling')
                        self.assertNotIn(sibling, f.commits())
                    f.write(b'source.txt', b'authorised task change\n')
                    f.hook(); f.hook(name='pre-push')
                    projection = common.PhaseProjection(f, phase)
                    self.assertEqual(projection.capture_scope().returncode, 0)
                    self.assertNotIn(sibling, {row['commit'] for row in projection.capture['outgoing_commits']})
                    result = projection.finish(projection.verdict())
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), f.git(f.task, 'rev-parse', 'HEAD'))
                    if sibling:
                        self.assertIn(sibling, f.commits())
                    self.record({'control': 'nine-siblings', 'phase': phase[0], 'mode': mode,
                                 'exit_code': result.returncode, 'sibling': sibling,
                                 'published_tip': f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).decode().strip()})

    def namespace(self, f):
        helper = (common.TOOLS / 'workflow_publish_checkpoint.sh').read_text()
        code = helper.split("<<'CHECKPOINT_PY'\n", 1)[1].split('\nCHECKPOINT_PY\n', 1)[0]
        definitions = code.split('\ntry:\n    action = sys.argv[1]', 1)[0]
        namespace = {'__name__': 'owned_checkpoint_control'}
        exec(compile(definitions, 'actual-checkpoint-functions', 'exec'), namespace)
        namespace['context']('publish')
        return namespace

    def test_forged_tracking_and_fetch_head_never_shrink_real_outgoing_history(self):
        for kind in ('authorised', 'commit', 'merge', 'rebase'):
            with self.subTest(history=kind):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'authorised initial candidate\n')
                projection = common.PhaseProjection(f, common.PHASES[0])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict()
                self.assertTrue(verdict['approved'])
                saved = json.loads(Path(projection.capture['capture_file']).read_text())
                record = {'binding': saved['binding'], 'capture': saved,
                          'approved_paths': verdict['approved_paths']}
                f.git(f.task, 'add', '--', 'source.txt')
                f.git(f.task, 'commit', '-m', 'authorised local candidate')
                if kind == 'merge':
                    f.git(f.task, 'checkout', '-b', 'fixture-side', f.base)
                if kind != 'authorised':
                    f.write(b'unrelated.txt', b'unauthorised outgoing source\n')
                    f.git(f.task, 'add', '--', 'unrelated.txt')
                    f.git(f.task, 'commit', '-m', 'unauthorised outgoing candidate')
                if kind == 'merge':
                    side = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                    f.git(f.task, 'checkout', f.branch)
                    f.git(f.task, 'merge', '--no-ff', side, '-m', 'merge the unauthorised candidate')
                elif kind == 'rebase':
                    tree = f.git(f.task, 'rev-parse', f.base + '^{tree}').strip().decode()
                    onto = f.git(f.task, 'commit-tree', tree, '-p', f.base, data=b'independent empty parent\n').strip().decode()
                    f.git(f.task, 'rebase', '--onto', onto, f.base)
                head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                f.git(f.task, 'update-ref', 'refs/remotes/origin/' + f.branch, head)
                fetch_head = Path(os.fsdecode(f.git(f.task, 'rev-parse', '--git-path', 'FETCH_HEAD').strip()))
                if not fetch_head.is_absolute():
                    fetch_head = f.task / fetch_head
                fetch_head.write_text(head + '\t\tforged fixture tracking\n')
                self.assertEqual(f.git(f.task, 'rev-list', '@{u}..HEAD').strip(), b'')
                before = common.PrivateHistoryTests.metadata(self, f)
                with patch.dict(os.environ, f.env, clear=True), contextlib.chdir(f.task):
                    namespace = self.namespace(f)
                    publication = namespace['publication'](record['binding'], record['capture']['publication'])
                    actual = namespace['outgoing'](publication)
                    self.assertIn(head, {row['commit'] for row in actual})
                    if kind == 'authorised':
                        namespace['verify_publication_destination'](record, publication)
                    else:
                        self.assertTrue(any(common.encoded(b'unrelated.txt') in row['paths'] for row in actual))
                        with self.assertRaises(namespace['Refusal']):
                            namespace['verify_publication_destination'](record, publication)
                self.assertEqual(common.PrivateHistoryTests.metadata(self, f), before)
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                self.record({'control': 'forged-tracking', 'history': kind,
                             'remote_tip': f.base, 'local_tip': head, 'actual_outgoing': [row['commit'] for row in actual]})

    def test_extra_push_refs_and_options_refuse_before_transport(self):
        for args in (['--all', 'origin'], ['--mirror', 'origin'], ['--tags', 'origin'],
                     ['origin', 'HEAD:task/checkpoint', 'HEAD:another-task'],
                     ['origin', 'HEAD:another-task'], ['origin', 'fixture-base:task/checkpoint']):
            with self.subTest(arguments=args):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'authorised source\n'); f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, common.PHASES[0])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                f.context['checkpoint_scope_review'] = verdict; f.sync_context()
                refs = f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)')
                body = ('set -euo pipefail\n. "$1"\nshift\nworkflow_checkpoint_enter publish\n'
                        'workflow_checkpoint_stage\ngit commit -m "authorised source"\ngit push "$@"\n')
                result = f.shell(body, str(common.TOOLS / 'workflow_publish_checkpoint.sh'), *args)
                self.assertEqual(result.returncode, 78, result.stderr)
                self.assertEqual(f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)'), refs)
                self.assertFalse((f.root / 'pre-push.log').exists())
                self.assertNotEqual(f.git(f.task, 'rev-parse', 'HEAD').strip().decode(), f.base)

    def test_configured_additional_push_behaviour_refuses_before_mutation(self):
        for key, value in (('push.followTags', 'true'), ('remote.origin.mirror', 'true'),
                           ('remote.origin.push', 'HEAD:refs/heads/another-task'),
                           ('push.recurseSubmodules', 'on-demand'), ('push.recurseSubmodules', 'only')):
            with self.subTest(configuration=key, value=value):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'authorised source\n')
                f.git(f.task, 'config', key, value)
                before = common.PrivateHistoryTests.metadata(self, f)
                result = common.PhaseProjection(f, common.PHASES[0]).capture_scope()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(common.PrivateHistoryTests.metadata(self, f), before)

    def test_new_task_ref_excludes_all_advertised_remote_ancestry(self):
        for phase in common.PHASES[1:]:
            for object_format in ('sha1', 'sha256'):
                with self.subTest(phase=phase[0], object_format=object_format):
                    f = common.GitFixture(self, object_format=object_format)
                    f.own([b'source.txt', b'unrelated.txt'])
                    f.git(f.caller, 'checkout', '-b', 'remote-side', f.base)
                    f.write(b'unrelated.txt', b'already owned by remote history\n', f.caller)
                    f.git(f.caller, 'add', '--', 'unrelated.txt')
                    f.git(f.caller, 'commit', '-m', 'remote side commit')
                    side = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
                    f.git(f.caller, 'push', 'origin', 'HEAD:refs/heads/remote-side')
                    f.write(b'source.txt', b'authorised local source\n')
                    f.git(f.task, 'add', '--', 'source.txt'); f.git(f.task, 'commit', '-m', 'local task commit')
                    f.git(f.task, 'merge', '--no-ff', side, '-m', 'authorised local merge')
                    merge = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                    f.git(f.task, 'push', 'origin', ':refs/heads/' + f.branch)
                    f.git(f.task, 'branch', '--unset-upstream')
                    f.hook(); f.hook(name='pre-push')
                    projection = common.PhaseProjection(f, phase)
                    self.assertEqual(projection.capture_scope().returncode, 0)
                    rows = projection.capture['outgoing_commits']
                    self.assertNotIn(side, {row['commit'] for row in rows})
                    self.assertIn(merge, {row['commit'] for row in rows})
                    result = projection.finish(projection.verdict())
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), f.git(f.task, 'rev-parse', 'HEAD'))
                    self.record({'control': 'new-ref-merge', 'phase': phase[0], 'format': object_format,
                                 'already_remote': side, 'checked_local': [row['commit'] for row in rows]})


    def test_cleanup_sync_nested_fetch_preserves_failed_partial_and_terminal_status(self):
        for mode in ('failed', 'failed-and-advertisement', 'partial', 'terminal'):
            with self.subTest(mode=mode):
                f = common.GitFixture(self)
                imported = None
                if mode == 'partial':
                    producer = f.private / 'sync-fetch-producer'
                    f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(f.remote), str(producer)])
                    f.write(b'unrelated.txt', b'independent advertised remote advance\n', producer)
                    f.git(producer, 'add', '--', 'unrelated.txt')
                    f.git(producer, 'commit', '-m', 'remote advance downloaded by sync')
                    imported = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                    f.git(producer, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
                    self.assertNotIn(imported, f.commits())
                f.write(b'source.txt', b'authorised cleanup source\n')
                f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, common.PHASES[2])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                remote_before = f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)')
                self.fault_git(f, mode, 'fetch')
                result = projection.finish(verdict)
                self.assertEqual(result.returncode, 79 if mode == 'terminal' else 7, result.stderr)
                records = [json.loads(line) for line in (f.private / 'transport-statuses.jsonl').read_text().splitlines()]
                intended = [row for row in records if row.get('intended')]
                self.assertEqual(len(intended), 1)
                self.assertEqual(intended[0]['operation'], 'fetch')
                self.assertFalse(any(row['operation'] in ('pull', 'push') for row in records))
                self.assertFalse((f.root / 'pre-push.log').exists())
                self.assertEqual(f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)'), remote_before)
                self.assertIn(b'controlled fixture failure after actual fetch writes' if mode == 'partial' else b'controlled fixture fetch failure', result.stderr)
                self.assertEqual(f.git(f.task, 'show', 'HEAD:source.txt'), b'authorised cleanup source\n')
                if imported:
                    self.assertIn(imported, f.commits(), 'Actual partial fetch objects must remain observable.')
                    self.assertEqual(f.git(f.task, 'rev-parse', 'refs/remotes/origin/' + f.branch).strip().decode(), imported)
                if mode == 'failed-and-advertisement':
                    self.assertIn(b'publication destination inspection failed', result.stderr)
                self.record({'control':'nested-sync-fetch', 'mode':mode, 'exit_code':result.returncode,
                             'imported':imported, 'transport':records})

    def forged_shell_case(self, mutant=None):
        f = common.GitFixture(self)
        f.write(b'unrelated.txt', b'existing remote history outside task scope\n')
        f.git(f.task, 'add', '--', 'unrelated.txt')
        f.git(f.task, 'commit', '-m', 'genuinely published unrelated history')
        previous = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.task, 'push', 'origin', f.branch)
        f.git(f.task, 'fetch', 'origin', f.branch)
        self.assertEqual(f.git(f.task, 'rev-parse', 'FETCH_HEAD').strip().decode(), previous)
        f.write(b'source.txt', b'authorised source\n'); f.hook(); f.hook(name='pre-push')
        projection = common.PhaseProjection(f, common.PHASES[0])
        self.assertEqual(projection.capture_scope().returncode, 0)
        verdict = projection.verdict(); self.assertTrue(verdict['approved'])
        self.assertNotIn(previous, {row['commit'] for row in projection.capture['outgoing_commits']})
        f.context['checkpoint_scope_review'] = verdict; f.sync_context()
        helper = common.TOOLS / 'workflow_publish_checkpoint.sh'
        if mutant:
            tools = f.private / 'range-mutant-tools'; tools.mkdir()
            source = helper.read_text()
            original = ("    tips = ([pub['destination_tip']] if pub['destination_tip'] else\n"
                        "            sorted(set(remote_advertisements[pub['url_sha256']].values())))")
            self.assertEqual(source.count(original), 1)
            replacement = ("    tips = [pub['upstream']]" if mutant == 'tracking' else
                           "    tips = [textgit('rev-parse', '--verify', 'FETCH_HEAD^{commit}')]")
            source = source.replace(original, replacement, 1)
            helper = tools / helper.name; helper.write_text(source)
            (tools / 'workflow_stage_changes.sh').write_bytes((common.TOOLS / 'workflow_stage_changes.sh').read_bytes())
        body = ('set -euo pipefail\n. "$1"\nworkflow_checkpoint_enter publish\n'
                'workflow_checkpoint_stage\ngit commit -m "authorised source"\n'
                'command git --git-dir "$2" update-ref "refs/heads/$3" "$4" "$5"\n'
                'fetch_head=$(command git rev-parse --git-path FETCH_HEAD)\n'
                'printf "%s\\t\\tgenuine prior tracking tip\\n" "$5" > "$fetch_head"\n'
                'printf "%s\\n%s\\n" "$WF_CHECKPOINT_HEAD" "$WF_CHECKPOINT_UPSTREAM" > "$SCOPE_FIXTURE_ROOT/private/held-values"\n'
                'command git rev-list "@{u}..HEAD" > "$SCOPE_FIXTURE_ROOT/private/apparent-range"\n'
                'git push origin "HEAD:$3"\n')
        result = f.shell(body, str(helper), str(f.remote), f.branch, f.base, previous)
        self.record({'control':'forged-shell-attempt', 'mutant':mutant,
                     'checkpoint_exit':result.returncode, 'stderr':result.stderr.decode(errors='replace')})
        head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
        self.assertEqual((f.private / 'held-values').read_text().splitlines(), [head, previous])
        self.assertEqual((f.private / 'apparent-range').read_text().splitlines(), [head])
        self.assertEqual(f.git(f.task, 'show', 'HEAD:unrelated.txt'), b'existing remote history outside task scope\n')
        return f, result, head, previous

    def assert_forged_shell_refusal(self, f, result):
        self.assertEqual(result.returncode, 78, result.stderr)
        self.assertIn(b'outgoing history exceeds reviewed scope', result.stderr)
        self.assertNotIn(b'unexpected HEAD transition', result.stderr)
        self.assertNotIn(b'upstream changed outside a known fetch operation', result.stderr)
        self.assertFalse((f.root / 'pre-push.log').exists())
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)

    def test_shell_push_refuses_forged_tracking_and_fetch_head_with_78(self):
        f, result, head, previous = self.forged_shell_case()
        self.assert_forged_shell_refusal(f, result)
        self.record({'control':'shell-forged-tracking', 'exit_code':result.returncode,
                     'held_head':head, 'held_upstream':previous, 'remote_tip_after_rewind':f.base,
                     'refusal':result.stderr.decode()})

    def test_forged_shell_oracle_kills_both_local_range_exclusion_mutants(self):
        for mutant in ('tracking', 'fetch-head'):
            with self.subTest(mutant=mutant):
                f, result, head, previous = self.forged_shell_case(mutant)
                with self.assertRaises(AssertionError):
                    self.assert_forged_shell_refusal(f, result)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((f.root / 'pre-push.log').exists())
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), head)
                self.assertEqual(f.git(f.remote, 'merge-base', '--is-ancestor', previous, head, check=False).returncode, 0)
                self.record({'control':'range-exclusion-mutant-killed', 'mutant':mutant,
                             'checkpoint_exit':result.returncode, 'published_tip':head,
                             'unapproved_history_republished':previous})

    def test_transport_zero_malformed_advertisement_refuses_before_push(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'authorised source\n'); f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, phase)
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                remote_before = f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)')
                self.fault_git(f, 'malformed-advertisement', 'fetch' if phase[0] == 'workflow-publish' else 'pull')
                result = projection.finish(verdict)
                self.assertEqual(result.returncode, 78, result.stderr)
                records = [json.loads(line) for line in (f.private / 'transport-statuses.jsonl').read_text().splitlines()]
                malformed = [row for row in records if row.get('malformed')]
                self.assertEqual(len(malformed), 1); self.assertEqual(malformed[0]['reported_exit'], 0)
                self.assertFalse(any(row['operation'] == 'push' for row in records))
                self.assertFalse((f.root / 'pre-push.log').exists())
                self.assertEqual(f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)'), remote_before)
                self.assertIn(b'malformed publication advertisement', result.stderr)
                self.record({'control':'malformed-advertisement', 'phase':phase[0], 'exit_code':result.returncode})


    def test_cleanup_sync_policy_skips_only_current_same_destination_absence(self):
        for route in ('same-absent', 'same-rewrite', 'distinct-absent', 'push-rewrite-distinct', 'stale-absence', 'alias-existing'):
            with self.subTest(route=route):
                f = common.GitFixture(self)
                destination = f.remote
                target_branch = f.branch
                if route in ('distinct-absent','push-rewrite-distinct'):
                    destination = f.private / 'distinct-push.git'
                    f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(destination)])
                    if route == 'distinct-absent':
                        f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(destination))
                    else:
                        f.git(f.task, 'config', 'url.'+str(destination)+'.pushInsteadOf', str(f.remote))
                if route != 'alias-existing':
                    f.git(destination, 'update-ref', '-d', 'refs/heads/' + target_branch)
                    f.git(f.task, 'branch', '--unset-upstream')
                else:
                    alias=target_branch+'--wt-1';f.git(f.task,'branch','-m',alias);f.branch=alias
                    f.context['worktree_setup'].update(branch_name=target_branch,local_branch=alias)
                if route == 'same-rewrite':
                    f.git(f.task,'config','url.'+str(f.remote)+'.insteadOf','fixture-origin:repo')
                    f.git(f.task,'remote','set-url','origin','fixture-origin:repo')
                f.sync_context()
                f.write(b'source.txt', b'authorised first publication\n')
                f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, common.PHASES[2])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                if route != 'alias-existing':
                    self.assertIsNone(projection.capture['publication']['destination_tip'])
                self.fault_git(f, 'create-ref-after-commit' if route == 'stale-absence' else 'record-only', 'fetch')
                result = projection.finish(verdict)
                self.assertEqual(result.returncode, 0, result.stderr)
                records = [json.loads(line) for line in (f.private / 'transport-statuses.jsonl').read_text().splitlines()]
                fetches = [row for row in records if row['operation'] == 'fetch']
                events = []
                for name in f.private.glob('checkpoint-scope-*/operations.jsonl'):
                    events.extend(json.loads(line) for line in name.read_text().splitlines())
                policies = [row for row in events if row.get('boundary') == 'sync-fetch-policy']
                skips = [row for row in events if row.get('boundary') == 'operation-skipped']
                self.assertEqual(len(policies), 1)
                self.assertEqual(policies[0]['ref'], 'refs/heads/' + target_branch)
                if route in ('same-absent','same-rewrite'):
                    self.assertEqual(policies[0]['mode'], 'skip-absent')
                    self.assertTrue(policies[0]['same_destination']); self.assertTrue(policies[0]['current_ref_absent'])
                    self.assertEqual(fetches, []); self.assertEqual(len(skips), 1)
                else:
                    self.assertEqual(policies[0]['mode'], 'fetch')
                    self.assertEqual(len(fetches), 1); self.assertEqual(fetches[0]['native_exit'], 0)
                    self.assertEqual(skips, [])
                    if route in ('distinct-absent','push-rewrite-distinct'):
                        self.assertFalse(policies[0]['same_destination'])
                        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                    else:
                        self.assertFalse(policies[0]['current_ref_absent'])
                        if route == 'stale-absence':
                            self.assertTrue(any(row['operation'] == 'create-task-ref' for row in records))
                head = f.git(f.task, 'rev-parse', 'HEAD').strip().decode()
                self.assertEqual(f.git(destination, 'rev-parse', 'refs/heads/' + target_branch).strip().decode(), head)
                self.assertTrue((f.root / 'pre-push.log').exists())
                self.record({'control':'first-publication-sync-policy', 'route':route,
                             'policy':policies[0], 'transport':records, 'published_tip':head})

    def test_cleanup_sync_policy_rejects_invalid_bound_operation_tokens(self):
        for case in ('flag-type', 'operation', 'callback', 'reference', 'invocation', 'substituted-file', 'authority'):
            with self.subTest(case=case):
                f = common.GitFixture(self)
                f.git(f.remote, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.git(f.task, 'branch', '--unset-upstream')
                f.write(b'source.txt', b'authorised source remains unstaged\n')
                f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, common.PHASES[2])
                self.assertEqual(projection.capture_scope().returncode, 0)
                f.context['checkpoint_scope_review'] = projection.verdict(); f.sync_context()
                tamper = f.private / 'tamper-operation.py'
                tamper.write_text("""import sys,json,pathlib,hashlib,uuid
value=json.loads(sys.argv[1]);kind=sys.argv[2];authority=pathlib.Path(sys.argv[3]);path=pathlib.Path(value['path'])
token=json.loads(path.read_text())
if kind=='authority': authority.write_bytes(authority.read_bytes()+b'\\n')
elif kind=='substituted-file':
 other=path.with_name('operation-'+uuid.uuid4().hex+'.json');other.write_bytes(path.read_bytes());value['path']=str(other)
else:
 if kind=='flag-type': token['cleanup_sync']['current_absent']='true'
 elif kind=='operation': token['cleanup_sync']['operation']='push'
 elif kind=='callback': token['cleanup_sync']['callback']='unrelated_callback'
 elif kind=='reference': token['cleanup_sync']['ref']='refs/heads/unrelated'
 elif kind=='invocation': token['invocation']='different-invocation'
 raw=json.dumps(token,sort_keys=True,separators=(',',':')).encode();path.write_bytes(raw);value['sha256']=hashlib.sha256(raw).hexdigest()
print(json.dumps(value,separators=(',',':')))
""")
                before = f.snapshot(); refs = f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)')
                body = ('set -euo pipefail\n. "$1"\nworkflow_checkpoint_enter cleanup\n'
                        'token=$(workflow_checkpoint_python before sync workflow_checkpoint_sync_body "$2")\n'
                        'workflow_checkpoint_python sync-fetch-mode "$token"\n'
                        'changed=$(python3 -I "$3" "$token" "$4" "$WF_CHECKPOINT_RECORD")\n'
                        'workflow_checkpoint_python sync-fetch-mode "$changed"\n')
                result = f.shell(body, str(common.TOOLS / 'workflow_publish_checkpoint.sh'), f.branch, str(tamper), case)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertEqual(result.stdout.strip(), b'skip-absent', 'The unmodified bound token must first be accepted.')
                self.assertIn(b'checkpoint scope refusal:', result.stderr)
                self.assertEqual(f.snapshot(), before)
                self.assertEqual(f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)'), refs)
                self.assertFalse((f.root / 'pre-commit.log').exists()); self.assertFalse((f.root / 'pre-push.log').exists())


    def test_earlier_recipe_sync_stops_before_cleanup_on_every_executed_failure(self):
        for mode in ('failed', 'partial', 'terminal', 'early-malformed', 'early-unreadable', 'early-terminal', 'healthy', 'first-publication'):
            with self.subTest(mode=mode):
                f = common.GitFixture(self)
                imported = None
                if mode == 'partial':
                    producer = f.private / 'early-partial-producer'
                    f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(f.remote), str(producer)])
                    f.write(b'unrelated.txt', b'actual early partial fetch objects\n', producer)
                    f.git(producer, 'add', '--', 'unrelated.txt'); f.git(producer, 'commit', '-m', 'early remote advance')
                    imported = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                    f.git(producer, 'push', 'origin', 'HEAD:refs/heads/' + f.branch)
                    self.assertNotIn(imported, f.commits())
                if mode == 'first-publication':
                    f.git(f.remote, 'update-ref', '-d', 'refs/heads/' + f.branch)
                    f.git(f.task, 'branch', '--unset-upstream')
                # Dirty/open-PR lookup can leave these fields empty too. They
                # deliberately stay identical in present and absent cases.
                f.context['terminal_state'] = {'pr_number':'', 'pr_url':'', 'terminal_success':'false', 'should_finalize':'true'}
                f.write(b'source.txt', b'authorised cleanup after early sync\n');f.hook();f.hook(name='pre-push')
                projection = common.PhaseProjection(f, common.PHASES[2])
                self.assertEqual(projection.capture_scope().returncode, 0)
                f.context['checkpoint_scope_review'] = projection.verdict();f.sync_context()
                before = f.snapshot(); refs = f.git(f.remote, 'for-each-ref', '--format=%(refname) %(objectname)')
                self.fault_git(f, 'record-only' if mode in ('healthy','first-publication') else mode, 'fetch')
                early = projection.render(common.literal(projection.current['step-20-sync-pr-head'], 'command'))
                cleanup = projection.render(common.literal(projection.current[projection.checkpoint], 'command'))
                # The actual earlier recipe command executes in a subshell;
                # ordinary Bash failure propagation must prevent the real later
                # checkpoint command, not merely a marker-only substitute.
                body = ('set -euo pipefail\n(\n' + early + '\n)\n'
                        'printf "entered\\n" > "$SCOPE_FIXTURE_ROOT/private/cleanup-entered"\n' + cleanup)
                result = f.shell(body)
                records = [json.loads(line) for line in (f.private / 'transport-statuses.jsonl').read_text().splitlines()]
                queries = [row for row in records if row['operation']=='origin-existence']
                self.assertEqual(len(queries), 1, 'Only the earlier opt-in mode adds an existence read.')
                fetches = [row for row in records if row['operation']=='fetch']
                if mode in ('healthy','first-publication'):
                    self.assertEqual(result.returncode,0,result.stderr)
                    self.assertTrue((f.private/'cleanup-entered').exists());self.assertTrue((f.root/'pre-push.log').exists())
                    self.assertEqual(f.git(f.remote,'rev-parse','refs/heads/'+f.branch),f.git(f.task,'rev-parse','HEAD'))
                    if mode=='first-publication':
                        self.assertEqual(queries[0]['reported_exit'],0);self.assertEqual(fetches,[])
                        self.assertIn(b'origin task ref is absent',result.stderr)
                    else:
                        self.assertEqual(queries[0]['reported_exit'],0);self.assertEqual(len(fetches),2)
                else:
                    expected=79 if mode in ('terminal','early-terminal') else 78 if mode=='early-malformed' else 11 if mode=='early-unreadable' else 7
                    self.assertEqual(result.returncode,expected,result.stderr)
                    self.assertFalse((f.private/'cleanup-entered').exists())
                    self.assertFalse(any(row['operation'] in ('pull','push') for row in records))
                    self.assertFalse((f.root/'pre-commit.log').exists());self.assertFalse((f.root/'pre-push.log').exists())
                    self.assertEqual(f.git(f.remote,'for-each-ref','--format=%(refname) %(objectname)'),refs)
                    after=f.snapshot()
                    for key in ('raw_index','entries','blobs','tree','head','shared_indexes','remote'):
                        self.assertEqual(after[key],before[key])
                    if mode.startswith('early-'):
                        self.assertEqual(fetches,[])
                        self.assertIn(b'malformed origin task-ref advertisement' if mode=='early-malformed' else b'controlled origin existence inspection failure',result.stderr)
                    else:
                        self.assertEqual(len(fetches),1)
                        self.assertIn(b'controlled fixture failure after actual fetch writes' if mode=='partial' else b'controlled fixture fetch failure',result.stderr)
                    if imported:
                        self.assertIn(imported,f.commits())
                        self.assertEqual(f.git(f.task,'rev-parse','refs/remotes/origin/'+f.branch).strip().decode(),imported)
                self.record({'control':'earlier-finalization-sync','mode':mode,'exit_code':result.returncode,'transport':records,'imported':imported})

    def test_generic_sync_default_remains_best_effort_without_existence_read(self):
        for mode in ('failed','terminal'):
            with self.subTest(mode=mode):
                f=common.GitFixture(self);self.fault_git(f,mode,'fetch');before=f.snapshot()
                result=f.run([f.bash,str(common.TOOLS/'workflow_pr_head_sync.sh')],cwd=f.task,check=False)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn(b'could not fetch origin/',result.stderr)
                rows=[json.loads(line) for line in (f.private/'transport-statuses.jsonl').read_text().splitlines()]
                self.assertEqual([x['operation'] for x in rows],['fetch'])
                self.assertEqual(f.snapshot(),before)

    def fault_git(self, f, mode, target):
        script = '''#!@PYTHON@
import json, os, pathlib, subprocess, sys
real = @REAL@
mode = @MODE@
target = @TARGET@
root = pathlib.Path(@ROOT@)
marker = root / 'fetch-completed'
log = root / 'transport-statuses.jsonl'
args = sys.argv[1:]
op = next((x for x in args if x in ('fetch', 'pull', 'push', 'ls-remote', 'commit')), None)
def record(row):
    with log.open('a') as stream: stream.write(json.dumps(row) + '\\n')
if op == 'ls-remote' and '--get-url' not in args and args[-1:] == ['origin']:
    if mode in ('early-malformed', 'early-unreadable', 'early-terminal'):
        rc = 0 if mode == 'early-malformed' else 79 if mode == 'early-terminal' else 11
        if rc == 0: sys.stdout.write('malformed-origin-advertisement\\n')
        else: print('controlled origin existence inspection failure', file=sys.stderr)
        record({'operation':'origin-existence', 'reported_exit':rc, 'injected':True})
        sys.exit(rc)
    result = subprocess.run([real, *args], capture_output=True)
    sys.stdout.buffer.write(result.stdout);sys.stderr.buffer.write(result.stderr)
    record({'operation':'origin-existence', 'reported_exit':result.returncode})
    sys.exit(result.returncode)
if op == 'commit' and mode == 'create-ref-after-commit':
    result = subprocess.run([real, *args])
    if result.returncode == 0 and not (root / 'ref-created').exists():
        ref = 'refs/heads/' + @BRANCH@
        subprocess.run([real, '--git-dir', str(root.parent / 'remote.git'), 'update-ref', ref, @BASE@], check=True)
        (root / 'ref-created').write_text('created after actual commit')
        record({'operation':'create-task-ref', 'tip':@BASE@})
    sys.exit(result.returncode)
if op in ('fetch', 'pull') and op != target:
    result = subprocess.run([real, *args])
    record({'operation':op, 'intended':False, 'native_exit':result.returncode, 'reported_exit':result.returncode})
    sys.exit(result.returncode)
if op == target:
    if mode in ('failed', 'failed-and-advertisement', 'terminal'):
        if mode == 'failed-and-advertisement': marker.write_text('failed fetch')
        rc = 79 if mode == 'terminal' else 7
        print('controlled fixture fetch failure', file=sys.stderr)
        record({'operation':op, 'intended':True, 'native_exit':None, 'reported_exit':rc})
        sys.exit(rc)
    result = subprocess.run([real, *args])
    rc = result.returncode
    if rc == 0: marker.write_text('success')
    if mode == 'partial' and rc == 0:
        print('controlled fixture failure after actual fetch writes', file=sys.stderr)
        record({'operation':op, 'intended':True, 'native_exit':0, 'reported_exit':7})
        sys.exit(7)
    record({'operation':op, 'intended':True, 'native_exit':rc, 'reported_exit':rc})
    sys.exit(rc)
if op == 'ls-remote' and '--get-url' not in args and marker.exists() and mode in ('advertisement', 'advertisement-terminal', 'failed-and-advertisement'):
    rc = 79 if mode == 'advertisement-terminal' else 11
    print('controlled unreadable advertisement', file=sys.stderr)
    record({'operation':'advertisement', 'reported_exit':rc})
    sys.exit(rc)
if op == 'ls-remote' and '--get-url' not in args and marker.exists() and mode == 'malformed-advertisement':
    sys.stdout.write('not-an-advertisement\\n')
    record({'operation':'advertisement', 'reported_exit':0, 'malformed':True})
    sys.exit(0)
if op == 'push': record({'operation':'push'})
os.execv(real, [real, *args])
'''
        for key, value in (('@PYTHON@', sys.executable), ('@REAL@', repr(f.git_binary)),
                           ('@MODE@', repr(mode)), ('@TARGET@', repr(target)), ('@ROOT@', repr(str(f.private))),
                           ('@BRANCH@', repr(f.branch)), ('@BASE@', repr(f.base))):
            script = script.replace(key, value)
        path = f.bin / 'git'; path.write_text(script); path.chmod(0o755)

    def test_failed_partial_fetch_and_unreadable_advertisement_stop_before_push(self):
        for phase in common.PHASES:
            for mode in ('failed', 'failed-and-advertisement', 'partial', 'terminal', 'advertisement', 'advertisement-terminal'):
                with self.subTest(phase=phase[0], failure=mode):
                    f = common.GitFixture(self)
                    f.write(b'source.txt', b'authorised source survives failure\n')
                    f.hook(); f.hook(name='pre-push')
                    partial_sibling = None
                    if mode == 'partial':
                        producer = f.private / 'partial-fetch-producer'
                        f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(f.remote), str(producer)])
                        f.write(b'unrelated.txt', b'actual objects retained after reported transport failure\n', producer)
                        f.git(producer, 'add', '--', 'unrelated.txt'); f.git(producer, 'commit', '-m', 'partial-fetch sibling')
                        partial_sibling = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                        f.git(producer, 'push', 'origin', 'HEAD:refs/heads/partial-sibling')
                        self.assertNotIn(partial_sibling, f.commits())
                    projection = common.PhaseProjection(f, phase)
                    self.assertEqual(projection.capture_scope().returncode, 0, 'Real preparation must finish before fault injection.')
                    verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                    target = 'fetch' if phase[0] == 'workflow-publish' else 'pull'
                    self.fault_git(f, mode, target)
                    result = projection.finish(verdict)
                    self.assertEqual(result.returncode, 79 if mode in ('terminal', 'advertisement-terminal') else 78 if mode == 'advertisement' else 7,
                                     result.stderr)
                    statuses = [json.loads(line) for line in (f.private / 'transport-statuses.jsonl').read_text().splitlines()]
                    intended = [row for row in statuses if row.get('intended') is True]
                    self.assertEqual(len(intended), 1, 'The chosen first-failure stop targets exactly one normal operation.')
                    self.assertEqual(intended[0]['operation'], target)
                    operation_events = []
                    for observation_file in f.private.glob('checkpoint-scope-*/operations.jsonl'):
                        self.assertFalse(observation_file.is_symlink())
                        operation_events.extend(json.loads(line) for line in observation_file.read_text().splitlines())
                    if mode != 'terminal':
                        self.assertTrue(any(row.get('boundary') == 'operation-returned' and
                                            row.get('operation') == target and
                                            row.get('command_exit') == intended[0]['reported_exit']
                                            for row in operation_events), 'The real command status must remain in the operation evidence.')
                    if mode == 'failed-and-advertisement':
                        self.assertTrue(any(row.get('boundary') == 'refusal' and
                                            'publication destination inspection failed (exit 11)' in row.get('reason', '')
                                            for row in operation_events), 'Retain the additional post-inspection refusal.')
                        self.assertIn(b'publication destination inspection failed (exit 11)', result.stderr)
                    self.assertFalse(any(row['operation'] == 'push' for row in statuses))
                    self.assertFalse((f.root / 'pre-push.log').exists())
                    self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                    self.assertNotEqual(f.git(f.task, 'rev-parse', 'HEAD').strip().decode(), f.base)
                    self.assertEqual((f.task / 'source.txt').read_bytes(), b'authorised source survives failure\n')
                    self.assertEqual(f.staged_paths(), set())
                    if partial_sibling:
                        self.assertIn(partial_sibling, f.commits(), 'The real fetched objects must remain after the reported partial failure.')
                        self.assertEqual(intended[0]['native_exit'], 0)
                        self.assertEqual(intended[0]['reported_exit'], 7)
                    if mode.startswith('advertisement'):
                        self.assertTrue(any(row.get('reported_exit') == 0 for row in statuses if row['operation'] in ('fetch', 'pull')))
                        self.assertIn(b'publication destination inspection failed (exit ' + (b'79' if mode == 'advertisement-terminal' else b'11') + b')', result.stderr)
                    else:
                        self.assertIn(b'controlled fixture', result.stdout + result.stderr)
                    self.record({'control': 'transport-failure', 'phase': phase[0], 'mode': mode,
                                 'exit_code': result.returncode, 'statuses': statuses})


    def test_sibling_created_after_capture_does_not_change_task_permission(self):
        for phase in common.PHASES:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                f.write(b'source.txt', b'authorised pending source\n'); f.hook()
                projection = common.PhaseProjection(f, phase)
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict(); self.assertTrue(verdict['approved'])
                producer = f.private / 'later-producer'
                f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(f.remote), str(producer)])
                f.write(b'unrelated.txt', b'new unrelated remote history\n', producer)
                f.git(producer, 'add', '--', 'unrelated.txt'); f.git(producer, 'commit', '-m', 'late sibling')
                sibling = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                f.git(producer, 'push', 'origin', 'HEAD:refs/heads/late-sibling')
                self.assertNotIn(sibling, f.commits())
                result = projection.finish(verdict)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(sibling, f.commits())
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), f.git(f.task, 'rev-parse', 'HEAD'))

    def test_new_ref_uses_distinct_push_advertisement_and_known_noncommit_tags(self):
        for phase in common.PHASES[1:]:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                push = f.private / 'distinct-push.git'
                f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(push)])
                f.git(push, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(push))
                f.git(f.task, 'branch', '--unset-upstream')
                f.own([b'source.txt', b'unrelated.txt'])
                f.git(f.caller, 'checkout', '-b', 'distinct-remote-side', f.base)
                f.write(b'unrelated.txt', b'only the push destination owns this history\n', f.caller)
                f.git(f.caller, 'add', '--', 'unrelated.txt'); f.git(f.caller, 'commit', '-m', 'push-only side history')
                side = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
                f.git(f.caller, 'push', str(push), 'HEAD:refs/heads/distinct-remote-side')
                f.git(f.task, 'merge', '--ff-only', side)
                blob = f.git(f.task, 'hash-object', '-w', '--stdin', data=b'known noncommit tag target\n').strip().decode()
                f.git(f.task, 'tag', '-a', 'known-blob-tag', blob, '-m', 'known annotated noncommit tag')
                f.git(f.task, 'push', str(push), 'refs/tags/known-blob-tag')
                f.write(b'source.txt', b'authorised first publication\n'); f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, phase)
                self.assertEqual(projection.capture_scope().returncode, 0)
                self.assertIsNone(projection.capture['publication']['destination_tip'])
                self.assertNotIn(side, {row['commit'] for row in projection.capture['outgoing_commits']})
                self.assertNotEqual(f.git(f.remote, 'show-ref', '--verify', 'refs/heads/distinct-remote-side', check=False).returncode, 0)
                result = projection.finish(projection.verdict())
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(f.git(push, 'rev-parse', 'refs/heads/' + f.branch), f.git(f.task, 'rev-parse', 'HEAD'))
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                self.record({'control': 'distinct-new-ref-noncommit-tag', 'phase': phase[0], 'exit_code': result.returncode})

    def test_unavailable_distinct_push_ancestry_refuses_without_hydration_or_fallback(self):
        for kind in ('commit', 'noncommit-tag'):
            with self.subTest(unavailable=kind):
                f = common.GitFixture(self)
                push = f.private / 'unknown-push.git'
                f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(push)])
                f.git(push, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(push))
                f.git(f.task, 'branch', '--unset-upstream')
                producer = f.private / 'independent-push-writer'
                f.run([f.git_binary, 'clone', '--branch', 'fixture-base', str(push), str(producer)])
                if kind == 'commit':
                    f.write(b'unrelated.txt', b'unavailable remote ancestry\n', producer)
                    f.git(producer, 'add', '--', 'unrelated.txt'); f.git(producer, 'commit', '-m', 'unavailable remote commit')
                    oid = f.git(producer, 'rev-parse', 'HEAD').strip().decode()
                    f.git(producer, 'push', 'origin', 'HEAD:refs/heads/unknown-side')
                else:
                    blob = f.git(producer, 'hash-object', '-w', '--stdin', data=b'unavailable tag blob\n').strip().decode()
                    f.git(producer, 'tag', '-a', 'unknown-blob', blob, '-m', 'unavailable noncommit tag')
                    oid = f.git(producer, 'rev-parse', 'refs/tags/unknown-blob').strip().decode()
                    f.git(producer, 'push', 'origin', 'refs/tags/unknown-blob')
                missing = f.git(f.task, 'cat-file', '-e', oid, check=False)
                self.assertNotEqual(missing.returncode, 0)
                f.write(b'source.txt', b'authorised pending source\n')
                before = common.PrivateHistoryTests.metadata(self, f)
                result = common.PhaseProjection(f, common.PHASES[1]).capture_scope()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'advertised ancestry is unavailable locally', result.stderr)
                self.assertEqual(common.PrivateHistoryTests.metadata(self, f), before)
                self.assertNotEqual(f.git(f.task, 'cat-file', '-e', oid, check=False).returncode, 0)
                self.record({'control': 'conservative-unavailable-ancestry', 'kind': kind,
                             'exit_code': result.returncode, 'implementation_unchanged': True,
                             'limitation': 'No extra fetch or prepared-base/tracking exclusion is used.'})

    def test_remote_merge_does_not_grant_unapproved_path_authority(self):
        f = common.GitFixture(self)
        f.git(f.caller, 'checkout', '-b', 'already-remote-side', f.base)
        f.write(b'unrelated.txt', b'already remote source\n', f.caller)
        f.git(f.caller, 'add', '--', 'unrelated.txt'); f.git(f.caller, 'commit', '-m', 'remote source')
        side = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
        f.git(f.caller, 'push', 'origin', 'HEAD:refs/heads/already-remote-side')
        f.git(f.task, 'commit', '--allow-empty', '-m', 'local parent')
        f.git(f.task, 'merge', '--no-ff', side, '-m', 'local merge without path permission')
        f.write(b'source.txt', b'only this path has owner permission\n')
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertFalse(projection.verdict()['approved'])
        before = common.PrivateHistoryTests.metadata(self, f)
        result = projection.finish(projection.verdict())
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(common.PrivateHistoryTests.metadata(self, f), before)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)


    def test_new_ref_excludes_the_remotes_detached_advertised_head(self):
        f = common.GitFixture(self)
        f.git(f.remote, 'update-ref', '--no-deref', 'HEAD', f.base)
        f.git(f.remote, 'update-ref', '-d', 'refs/heads/fixture-base')
        f.git(f.remote, 'update-ref', '-d', 'refs/heads/' + f.branch)
        f.git(f.task, 'branch', '--unset-upstream')
        advertisement = f.git(f.remote, 'ls-remote', str(f.remote))
        self.assertIn(f.base.encode() + b'\tHEAD', advertisement)
        self.assertNotIn(b'refs/heads/', advertisement)
        f.write(b'source.txt', b'authorised new task ref\n'); f.hook(); f.hook(name='pre-push')
        projection = common.PhaseProjection(f, common.PHASES[1])
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertEqual(projection.capture['outgoing_commits'], [])
        result = projection.finish(projection.verdict())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch), f.git(f.task, 'rev-parse', 'HEAD'))


    def test_cleanup_distinct_fetch_merge_rebase_retains_unsupported_refusal(self):
        for phase in common.PHASES[2:]:
            with self.subTest(phase=phase[0]):
                f = common.GitFixture(self)
                push = f.private / 'distinct-push.git'
                f.run([f.git_binary, 'clone', '--bare', str(f.remote), str(push)])
                f.git(push, 'update-ref', '-d', 'refs/heads/' + f.branch)
                f.git(f.task, 'remote', 'set-url', '--push', 'origin', str(push))
                f.git(f.task, 'branch', '--unset-upstream')
                f.own([b'source.txt', b'unrelated.txt'])
                f.git(f.caller, 'checkout', '-b', 'distinct-remote-side', f.base)
                f.write(b'unrelated.txt', b'only the push destination owns this history\n', f.caller)
                f.git(f.caller, 'add', '--', 'unrelated.txt'); f.git(f.caller, 'commit', '-m', 'push-only side history')
                side = f.git(f.caller, 'rev-parse', 'HEAD').strip().decode()
                f.git(f.caller, 'push', str(push), 'HEAD:refs/heads/distinct-remote-side')
                f.git(f.task, 'merge', '--no-ff', side, '-m', 'local merge of advertised push history')
                blob = f.git(f.task, 'hash-object', '-w', '--stdin', data=b'known noncommit tag target\n').strip().decode()
                f.git(f.task, 'tag', '-a', 'known-blob-tag', blob, '-m', 'known annotated noncommit tag')
                f.git(f.task, 'push', str(push), 'refs/tags/known-blob-tag')
                f.write(b'source.txt', b'authorised first publication\n'); f.hook(); f.hook(name='pre-push')
                projection = common.PhaseProjection(f, phase)
                self.assertEqual(projection.capture_scope().returncode, 0)
                self.assertIsNone(projection.capture['publication']['destination_tip'])
                self.assertNotIn(side, {row['commit'] for row in projection.capture['outgoing_commits']})
                self.assertNotEqual(f.git(f.remote, 'show-ref', '--verify', 'refs/heads/distinct-remote-side', check=False).returncode, 0)
                result = projection.finish(projection.verdict())
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'rewritten history changes the known commit count', result.stderr)
                self.assertNotEqual(f.git(push, 'show-ref', '--verify', 'refs/heads/' + f.branch, check=False).returncode, 0)
                self.assertEqual(f.git(f.remote, 'rev-parse', 'refs/heads/' + f.branch).strip().decode(), f.base)
                self.record({'control': 'unsupported-distinct-route-merge-rebase', 'phase': phase[0], 'exit_code': result.returncode})



if __name__ == '__main__':
    unittest.main(verbosity=2)
