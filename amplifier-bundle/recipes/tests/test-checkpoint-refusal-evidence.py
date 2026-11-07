from pathlib import Path
import importlib.util,sys,json,unittest
from unittest.mock import patch
import copy
import os
import shutil
import shlex
from types import SimpleNamespace
sys.dont_write_bytecode=True
SOURCE=Path(__file__).resolve().parents[3]
HERE=SOURCE/'amplifier-bundle/recipes/tests'
sys.path.insert(0,str(HERE))
def module(name,path):
 spec=importlib.util.spec_from_file_location(name,path); value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
common=module('receipt_common',HERE/'test-checkpoint-scope.py')
evidence=module('receipt_evidence',HERE/'test-pragmatic-evidence.py')
producer=module('receipt_producer',HERE/'test-pragmatic-checkpoints.py')
class RefusalReceiptSchema(unittest.TestCase):
 def test_actual_preserved_refusal_reaches_the_independent_auditor(self):
  f=common.GitFixture(self, branch="task")
  f.write(b'unrelated.txt',b'preexisting staged bytes\n');f.git(f.task,'add','--','unrelated.txt')
  f.write(b'unrelated.txt',b'preexisting partial worktree bytes\n');f.write(b'source.txt',b'authorised checkpoint source\n')
  before=f.snapshot()
  before_worktrees={name:producer.retained_worktree_snapshot(producer.fixture_worktree_snapshot(repo)) for name,repo in [('caller',f.caller),('task',f.task)]}
  before_indexes={}
  for name,repo in [('caller',f.caller),('task',f.task)]:
   index=Path(os.fsdecode(f.git(repo,'rev-parse','--git-path','index').strip()))
   if not index.is_absolute():index=repo/index
   before_indexes[name]=index.read_bytes()
  phase=common.PHASES[0];projection=common.PhaseProjection(f,phase);result=projection.run()
  self.assertNotEqual(result.returncode,0);self.assertEqual(f.snapshot(),before)
  root=f.private/'receipts';root.mkdir();case=root/(phase[1]+'--adversarial');case.mkdir()
  (case/'task').symlink_to(f.task,target_is_directory=True);(case/'caller').symlink_to(f.caller,target_is_directory=True)
  (case/'remote.git').symlink_to(f.remote,target_is_directory=True)
  command=common.literal(projection.current[phase[1]],'command')
  (case/'extracted.sh').write_text(command);(case/'stdout').write_bytes(result.stdout);(case/'stderr').write_bytes(result.stderr)
  authority_entries={'source.txt':{'mode':'100644','contents_base64':common.encoded(b'authorised checkpoint source\n')}}
  history=evidence.publication_history(lambda *args:f.git(f.task,*args),{f.base},authority_entries)
  argv=[f.bash,'-c',projection.render(command)] if hasattr(f,'bash') else ['bash','-c',projection.render(command)]
  row={'asset':phase[0],'step_id':phase[1],'variant':'adversarial','source_asset_hash':common.digest((common.RECIPES/(phase[0]+'.yaml')).read_bytes()),
       'extracted_block_hash':common.digest(command.encode()),'head_before':f.base,'head_after':f.base,
       'publication_history':history,'assertions':{'destination_contract_satisfied':True,'publication_authorised':history['publication_authorised'],'unauthorised_entry_refused':True,'normal_hook_not_reached':True,
       'all_index_bytes_preserved':True,'all_worktrees_preserved':True,'index_entries_preserved':True,'head_preserved':True,'no_commit_created':True},
       'command':argv,'exit_status':result.returncode,'hook_executed':False,'hook_staged_entries':{}}
  row['push_required']=True
  row['destination_tip_before']=f.base
  row['destination_observation']=producer.destination_observation(
      lambda *args:f.git(f.remote,*args),'refs/heads/task',f.base,authority_entries)
  (case/'execution.json').write_text(json.dumps({'command':argv,'exit_status':result.returncode,'stdout_sha256':common.digest(result.stdout),'stderr_sha256':common.digest(result.stderr)}))
  staged_entries={}
  for entry in before['entries'].split(b'\0'):
   if entry:
    header,path=entry.split(b'\t',1);staged_entries[os.fsdecode(path)]=header.decode()
  row['before_staged_entries']=staged_entries;row['after_staged_entries']=copy.deepcopy(staged_entries)
  for number,name in enumerate(('caller','task')):
   (case/(str(number)+'.index.before')).write_bytes(before_indexes[name])
   (case/(str(number)+'.index.after')).write_bytes(before_indexes[name])
  after_worktrees={name:producer.retained_worktree_snapshot(producer.fixture_worktree_snapshot(repo)) for name,repo in [('caller',f.caller),('task',f.task)]}
  (case/'worktrees-before.json').write_text(json.dumps(before_worktrees))
  (case/'worktrees-after.json').write_text(json.dumps(after_worktrees))
  # This control isolates checkpoint receipt validation; normal runner identity
  # qualification is a separate requirement, never asserted by this fixture.
  with patch.object(evidence,'verify_report_identity',return_value=True):
   self.assertTrue(evidence.verify_checkpoint(root,row,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS}))
   # Equal reported booleans are insufficient when the retained bytes differ.
   original=(case/'1.index.after').read_bytes();(case/'1.index.after').write_bytes(original+b'tamper')
   with self.assertRaises(AssertionError):evidence.verify_checkpoint(root,row,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS})
   (case/'1.index.after').write_bytes(original)
   changed=copy.deepcopy(after_worktrees);changed['task'][0]['mode'] ^= 1
   for name in ('worktrees-before.json','worktrees-after.json'):(case/name).write_text(json.dumps(changed))
   with self.assertRaises(AssertionError):evidence.verify_checkpoint(root,row,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS})
   for name in ('worktrees-before.json','worktrees-after.json'):(case/name).write_text(json.dumps(after_worktrees))
   malformed=copy.deepcopy(row);del malformed['assertions']['publication_authorised']
   with self.assertRaises((KeyError,AssertionError)):evidence.verify_checkpoint(root,malformed,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS})
   malformed=copy.deepcopy(row);malformed['step_id']='../outside-assigned-case'
   with self.assertRaises(AssertionError):evidence.verify_checkpoint(root,malformed,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS})


class CurrentProducerAuditorControls(unittest.TestCase):
 def fixture(self):
  f=common.GitFixture(self)
  report=f.private/'current-producer-report';report.mkdir()
  # This control executes the actual checkpoint producer. Its native runner
  # constructor is not needed; native identity remains a separate requirement.
  fixture=producer.support.Fixtures.__new__(producer.support.Fixtures)
  fixture.report=report;fixture.bin=f.bin
  fixture.env={key:value for key,value in f.env.items() if not key.startswith('RECIPE_VAR_')}
  artifact=f.private/'declared-artifact-check'
  f.executable(artifact,"[ \"$1 $2\" = 'hygiene artifact-guard' ] && exit 0\nprintf 'Unexpected fixture dependency invocation\n' >&2\nexit 93\n")
  jq=shutil.which('jq',path=f.env['PATH']);self.assertIsNotNone(jq)
  args=SimpleNamespace(bundle=SOURCE,shell=Path(f.bash),amplihack=artifact,jq=Path(jq));fixture.args=args
  return f,fixture,args

 def verify(self,fixture,row):
  # Only the unrelated native runner identity boundary is mocked. Git,
  # snapshots, producer schema and independent receipt checks remain real.
  with patch.object(evidence,'verify_report_identity',return_value=True):
   return evidence.verify_checkpoint(fixture.report,row,{'source_root':str(SOURCE),'checkpoints':producer.CHECKPOINTS})

 def test_current_producer_refusal_reaches_the_independent_auditor(self):
  f,fixture,args=self.fixture()
  asset,sid,kind=producer.CHECKPOINTS[2]
  row=producer.checkpoint(fixture,args,asset,sid,kind,'adversarial')
  self.assertEqual(row['acceptance_result'],'passed',row)
  self.assertTrue(self.verify(fixture,row))
  malformed=copy.deepcopy(row);del malformed['assertions']['destination_contract_satisfied']
  with self.assertRaises((KeyError,AssertionError)):self.verify(fixture,malformed)
  malformed=copy.deepcopy(row);malformed['destination_observation']['tip']='0'*40
  with self.assertRaises(AssertionError):self.verify(fixture,malformed)
  malformed=copy.deepcopy(row);malformed['destination_observation']['entries']['source.txt']['contents_base64']=common.encoded(b'forged destination bytes\n')
  with self.assertRaises(AssertionError):self.verify(fixture,malformed)

 def test_real_publication_and_local_only_receipts_reach_the_auditor(self):
  for index in (3,0):
   with self.subTest(site=producer.CHECKPOINTS[index][1]):
    f,fixture,args=self.fixture()
    asset,sid,kind=producer.CHECKPOINTS[index]
    row=producer.checkpoint(fixture,args,asset,sid,kind,'normal')
    self.assertEqual(row['acceptance_result'],'passed',row)
    self.assertTrue(self.verify(fixture,row))
    self.assertIs(row['push_required'],index==3)
    self.assertIs(row['destination_observation']['publication_observed'],index==3)

 def test_independent_auditor_rejects_truthfully_reported_unauthorised_history(self):
  f,fixture,args=self.fixture()
  original_run=fixture.run;injected=[]
  def intercepted(argv,cwd=None,env=None,check=True):
   result=original_run(argv,cwd=cwd,env=env,check=check)
   if not injected and len(argv)==2 and str(argv[1]).endswith('/rendered.sh'):
    # Simulate untrusted checkpoint output before the producer measures it.
    # These real local Git operations test the independent auditor, not a
    # production checkpoint permission bypass.
    repo=Path(argv[1]).parent/'task'
    (repo/'unauthorised.txt').write_bytes(b'Unauthorised private fixture source\n')
    fixture.git(repo,'add','--','unauthorised.txt')
    fixture.git(repo,'commit','-m','Disposable unauthorised-history control')
    fixture.git(repo,'push','origin','HEAD:refs/heads/task')
    injected.append(fixture.git(repo,'rev-parse','HEAD').decode().strip())
   return result
  fixture.run=intercepted
  asset,sid,kind=producer.CHECKPOINTS[3]
  row=producer.checkpoint(fixture,args,asset,sid,kind,'normal')
  self.assertTrue(injected,row)
  self.assertEqual(row['exit_status'],0,row)
  self.assertEqual(row['acceptance_result'],'failed',row)
  self.assertFalse(row['publication_history']['publication_authorised'])
  self.assertFalse(row['assertions']['new_staging_authorised'])
  self.assertTrue(row['destination_observation']['publication_observed'])
  with self.assertRaises(AssertionError):self.verify(fixture,row)

 def test_independent_auditor_rejects_a_real_omitted_push(self):
  f,fixture,args=self.fixture()
  original_run=fixture.run;shim=f.bin/'git';omitted=f.private/'omitted-push.log'
  def intercepted(argv,cwd=None,env=None,check=True):
   if len(argv)==2 and str(argv[1]).endswith('/rendered.sh'):
    shim.write_text('#!'+f.bash+'\nset -eu\noriginal=("$@")\ncommand=\nwhile [ "$#" -gt 0 ]; do\n case "$1" in -C|--git-dir|--work-tree) shift 2;; --literal-pathspecs|--no-pager) shift;; --git-dir=*|--work-tree=*) shift;; *) command="$1"; break;; esac\ndone\nif [ "$command" = push ]; then printf \"omitted push\\n\" >> '+shlex.quote(str(omitted))+'; exit 0; fi\nexec '+shlex.quote(str(f.git_binary))+' "${original[@]}"\n')
    shim.chmod(0o700)
   return original_run(argv,cwd=cwd,env=env,check=check)
  fixture.run=intercepted
  asset,sid,kind=producer.CHECKPOINTS[3]
  row=producer.checkpoint(fixture,args,asset,sid,kind,'normal')
  self.assertTrue(omitted.exists(),row)
  self.assertEqual(row['exit_status'],0,row)
  self.assertFalse(row['assertions']['destination_contract_satisfied'])
  self.assertEqual(row['acceptance_result'],'failed')
  with self.assertRaises(AssertionError):self.verify(fixture,row)
  # Changing only the producer's claimed booleans must not hide the actual
  # unchanged bare reference. The independent query still rejects it.
  forged=copy.deepcopy(row);forged['assertions']['destination_contract_satisfied']=True
  forged['destination_observation']['matches_local_head']=True
  forged['destination_observation']['publication_observed']=True
  with self.assertRaises(AssertionError):self.verify(fixture,forged)

if __name__=='__main__':unittest.main(verbosity=2)
