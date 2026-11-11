//! A serialized single-attempt REST merge, authorized independently of proposals.
use super::{GhRunner,RealGhRunner,gh,evidence::{self,artifacts::{self,Store},schema::*}};
use anyhow::{Result,Context,ensure};
use clap::Subcommand;
use serde_json::{Value,json};

#[derive(Subcommand,Debug)]
pub enum Commands { Execute(evidence::Files), Inspect(evidence::Files),
 /// Launch-side adapter; usable only as the child of the installed privileged shell gate.
 Gate(GateArgs),
}
#[derive(clap::Args,Debug)]
pub struct GateArgs { #[arg(long)] state_dir:std::path::PathBuf, #[arg(long)] dry_run:bool }
fn gate(args:GateArgs)->Result<()> {
 let home=std::env::var_os("AMPLIHACK_HOME").context("AUTHORIZATION_REFUSED: no launch home")?;
 let gate=std::fs::canonicalize(std::path::Path::new(&home).join("amplifier-bundle/tools/autodrive_merge_gate.sh"))?;
 let parent=std::path::PathBuf::from(format!("/proc/{}",unsafe{libc::getppid()}));
 let argv=std::fs::read(parent.join("cmdline"))?;
 let words:Vec<_>=argv.split(|b|*b==0).collect();
 ensure!(words.len()>=4 && words[1]==b"-p" && words[2]==gate.as_os_str().as_encoded_bytes() && std::fs::canonicalize(parent.join("fd/255"))?==gate && std::fs::canonicalize(parent.join("exe"))?==std::fs::canonicalize("/bin/bash")?,"AUTHORIZATION_REFUSED: executor must be called by the installed shell gate");
 ensure!(std::env::var("AMPLIHACK_MERGE_GUARD").is_ok_and(|v|!v.is_empty()),"AUTHORIZATION_REFUSED: merge guard missing");
 let round=artifacts::json(&artifacts::read(&args.state_dir.join("merge-ready-latest.json"))?)?;
 let label=round["round_label"].as_str().context("EVIDENCE_UNSUPPORTED: round label")?;
 let mut r=evidence::launch::template_for(&args.state_dir,label)?;r.operation="execute".into();
 let store=Store::load(&args.state_dir.join("pr-evidence"),&r)?;
 let result=if store.authorization.no_merge {("NO_MERGE".into(),refusal("AUTHORIZATION_REFUSED","no_merge is enabled").1,vec![])}else{
  evidence::local::final_chain(&store,&r)?;
  let evaluator_ref:ArtifactRef=serde_json::from_value(artifacts::json(&artifacts::read(&store.root().join(format!("{label}-evaluator.json")))?)?)?;
  let p=Execute{bundle_ref:serde_json::from_value(round["bundle_ref"].clone())?,evaluator_ref,revalidation_plan:serde_json::from_value(round["revalidation_plan"].clone())?};
  if args.dry_run {("DRY_RUN".into(),refusal("AUTHORIZATION_REFUSED","dry run").1,vec![])}else{execute(&store,&r,&p,&RealGhRunner)?}
 };
 let (status,payload,errors)=result;
 let record=store.register(&r.record("merge",json!({"status":status,"payload":payload,"errors":errors})))?;
 println!("{}",json!({"merge_result":match status.as_str(){"CONFIRMED_MERGED"=>"MERGED","NO_MERGE"=>"NO_MERGE","DRY_RUN"=>"DRY_RUN",_=>"NOT_MERGED"},"status":status,"payload":payload,"errors":errors,"record_ref":record}));Ok(())
}
fn refusal(code:&str,message:&str)->(String,Value,Vec<Error>) {("REFUSED".into(),json!({"attempt_ref":null,"mutation_sent":"NO","platform_state":"UNKNOWN","merge_commit_sha":null,"discrepancies":[]}),vec![Error::new(code,"authorization",message)])}
fn get_pr(r:&Request,runner:&dyn GhRunner)->Result<Value> {
 let endpoint=format!("repos/{}/{}/pulls/{}",r.target.owner,r.target.repo,r.target.pr_number);
 let response=gh::response(runner.run_raw(&["api","--hostname",&r.target.host,"--include","--method","GET",&endpoint])?)?;
 ensure!(response.success && response.status==Some(200),"API_REFUSED: REST confirmation unavailable");let value=artifacts::json(&response.body)?;
 ensure!(value["number"].as_u64()==Some(r.target.pr_number as u64) && value["base"]["repo"]["id"].as_u64()==Some(r.target.repository_id) && value["head"]["repo"]["id"].as_u64()==Some(r.target.head_repository_id),"TARGET_MISMATCH");Ok(value)
}
fn confirmation(store:&Store,r:&Request,attempt:&ArtifactRef,runner:&dyn GhRunner)->Result<(String,Value,Vec<Error>)> {
 let record=store.record(attempt,r,Some("attempt"),false)?;
 let attempt_id=record.payload["attempt_id"].as_str().context("ARTIFACT_UNREGISTERED: attempt id")?;
 ensure!(identifier(attempt_id),"ARTIFACT_UNSAFE: attempt identity");
 let outcome=store.root().join(format!("{attempt_id}-outcome.json"));
 let rejected=if outcome.exists(){let reference:ArtifactRef=serde_json::from_value(artifacts::json(&artifacts::read(&outcome)?)?)?;store.record(&reference,r,Some("attempt-outcome"),false)?.payload["definitively_refused"]==true}else{false};
 let observed=get_pr(r,runner);
 let (status,state,commit)=match observed {
  Ok(p) if p["merged"]==true && p["merge_commit_sha"].as_str().is_some_and(git_sha)=>("CONFIRMED_MERGED","MERGED",p["merge_commit_sha"].clone()),
  Ok(p) if rejected && p["merged"]==false && p["state"]=="open"=>("CONFIRMED_NOT_MERGED","OPEN",Value::Null),
  Ok(p)=>("AMBIGUOUS",if p["state"]=="closed"{"CLOSED"}else{"OPEN"},Value::Null),
  Err(e)=>{tracing::warn!("REST merge confirmation unavailable: {e}");("AMBIGUOUS","UNKNOWN",Value::Null)},
 };
 let errors=if status=="AMBIGUOUS"{vec![Error::new("MERGE_OUTCOME_AMBIGUOUS","merge","Inspect this attempt; an open snapshot does not exclude an in-flight merge")]}else{vec![]};
 Ok((status.into(),json!({"attempt_ref":attempt,"mutation_sent":"UNKNOWN","platform_state":state,"merge_commit_sha":commit,"discrepancies":[]}),errors))
}
fn relevant(e:&evidence::Evidence)->Value {
 let mut facts=vec![];
 for (_,receipt,bodies) in &e.receipts {
  let data:Vec<_>=bodies.iter().map(|body| match receipt.operation.resource {
   Resource::PrMetadata=>json!({"state":body["state"],"draft":body["draft"],"head":body["head"]["sha"],"base":body["base"]["sha"],"mergeable":body["mergeable"],"mergeable_state":body["mergeable_state"],"review_comments":body["review_comments"],"body":body["body"],"title":body["title"]}),
   Resource::CheckRuns|Resource::SuiteCheckRuns=>json!(body["check_runs"].as_array().map(|rows|rows.iter().map(|c|json!({"id":c["id"],"head_sha":c["head_sha"],"name":c["name"],"status":c["status"],"conclusion":c["conclusion"],"app":c["app"]["id"],"suite":c["check_suite"]["id"]})).collect::<Vec<_>>())),
   Resource::CommitStatuses=>json!({"sha":body["sha"],"statuses":body["statuses"].as_array().map(|rows|rows.iter().map(|s|json!({"id":s["id"],"context":s["context"],"state":s["state"]})).collect::<Vec<_>>())}),
   _=>body.clone(),
  }).collect();facts.push(json!({"resource":receipt.operation.resource,"bodies":data}));
 }
 facts.sort_by_key(Value::to_string);json!(facts)
}
pub(crate) fn execute(store:&Store,r:&Request,p:&Execute,runner:&dyn GhRunner)->Result<(String,Value,Vec<Error>)> {
 if store.authorization.no_merge {return Ok(("NO_MERGE".into(),refusal("AUTHORIZATION_REFUSED","no_merge is enabled").1,vec![]));}
 let _lock=store.lock()?;
 let active=store.root().join("active-attempt.json");
 if active.exists(){return Ok(refusal("ATTEMPT_IN_PROGRESS","Inspect the persisted attempt before any further action; this run never replays a merge"));}
 let bundle:Bundle=serde_json::from_value(store.record(&p.bundle_ref,r,Some("bundle"),true)?.payload)?;
 let evaluator:Evaluator=serde_json::from_value(store.record(&p.evaluator_ref,r,Some("evaluator"),true)?.payload)?;
 ensure!(bundle.assessment.merge_ready_verdict==Readiness::MergeReady && evaluator.loop_verdict=="DONE" && evaluator.assessment_ref==p.bundle_ref && evaluator.next_action.kind==ActionKind::Advance,"EVIDENCE_UNSUPPORTED: accepted readiness and matching evaluator DONE required");
 evidence::operations(r,&p.revalidation_plan.operations)?;
 let criteria=&store.authorization.criteria;
 let covered:std::collections::BTreeSet<_>=p.revalidation_plan.coverage.iter().map(|c|c.criterion_id.as_str()).collect();
 if covered.len()!=criteria.len() || covered.len()!=p.revalidation_plan.coverage.len() || !criteria.iter().all(|c|covered.contains(c.as_str())) || p.revalidation_plan.coverage.iter().any(|c|c.operation_ids.is_empty() || c.operation_ids.iter().any(|id|!p.revalidation_plan.operations.iter().any(|op|&op.operation_id==id))) {
  let mut result=refusal("REVALIDATION_INCOMPLETE","The selected plan must cover every mutable criterion");result.0="REASSESSMENT_REQUIRED".into();return Ok(result);
 }
 let original=evidence::Evidence::load(store,r,&bundle.receipt_refs)?;
 // Revalidate accepted citations as well as new evidence; hashes and readiness are enforced again.
 let proposal=Validate{assessment_ref:bundle.assessment_ref.clone(),receipt_refs:bundle.receipt_refs.clone(),supersessions:bundle.supersessions.clone()};
 let (accepted,discrepancies)=super::evidence::validate::validate(store,r,&proposal)?;
 if accepted.is_none(){return Ok(("REASSESSMENT_REQUIRED".into(),json!({"attempt_ref":null,"mutation_sent":"NO","platform_state":"UNKNOWN","merge_commit_sha":null,"discrepancies":discrepancies}),vec![]));}
 let collected=evidence::collect(store,r,&p.revalidation_plan.operations,runner)?;
 let fresh=evidence::Evidence::load(store,r,&collected.refs)?;
 let mut errors=collected.errors;
 for coverage in &p.revalidation_plan.coverage {
  let selected=evidence::Evidence{receipts:fresh.receipts.iter().filter(|(_,receipt,_)|coverage.operation_ids.contains(&receipt.operation.operation_id)).cloned().collect()};
  if let Err(e)=evidence::support(&selected,r,&coverage.criterion_id){errors.push(Error::new("REVALIDATION_CHANGED","revalidation",&e.to_string()));}
 }
 if !errors.is_empty() || relevant(&original)!=relevant(&fresh) {
  return Ok(("REASSESSMENT_REQUIRED".into(),json!({"attempt_ref":null,"mutation_sent":"NO","platform_state":"UNKNOWN","merge_commit_sha":null,"discrepancies":errors,"receipt_refs":collected.refs}),vec![Error::new("REVALIDATION_CHANGED","revalidation","Fresh evidence differs or lacks support; return to assessment")]));
 }
 let attempt_id=format!("attempt-{}",uuid::Uuid::new_v4().simple());
 let attempt=store.register(&r.record("attempt",json!({"attempt_id":attempt_id,"bundle_ref":p.bundle_ref,"evaluator_ref":p.evaluator_ref,"expected_head_sha":r.revision.head_sha,"created_at":chrono::Utc::now().to_rfc3339(),"mutation_sent":"UNKNOWN","status":"PREPARED","revalidation_refs":collected.refs})))?;
 artifacts::write(&active,&serde_json::to_vec(&attempt)?)?;
 let endpoint=format!("repos/{}/{}/pulls/{}/merge",r.target.owner,r.target.repo,r.target.pr_number);
 let sha=format!("sha={}",r.revision.head_sha);
 // Exactly one expected-SHA squash mutation, no admin/bypass flags and no retries.
 let response=runner.run_raw(&["api","--hostname",&r.target.host,"--include","--method","PUT",&endpoint,"-f","merge_method=squash","-f",&sha]).and_then(gh::response);
 let refused=response.as_ref().is_ok_and(|response|matches!(response.status,Some(405|409|422)));
 let outcome=store.register(&r.record("attempt-outcome",json!({"attempt_ref":attempt,"definitively_refused":refused,"http_status":response.as_ref().ok().and_then(|v|v.status)})))?;
 artifacts::write(&store.root().join(format!("{attempt_id}-outcome.json")),&serde_json::to_vec(&outcome)?)?;
 confirmation(store,r,&attempt,runner)
}
pub fn run(command:Commands)->Result<()> {
 let command=match command {Commands::Gate(args)=>return gate(args),other=>other};
 let (op,files)=match command {Commands::Execute(f)=>("execute",f),Commands::Inspect(f)=>("inspect",f),_=>unreachable!()};
 let result=(||{let r=evidence::input(&files,op)?;let store=evidence::store(&r)?;
  let (status,payload,errors)=if op=="execute"{let p:Execute=serde_json::from_value(r.payload.clone())?;execute(&store,&r,&p,&RealGhRunner)?}else{let p:Inspect=serde_json::from_value(r.payload.clone())?;let _lock=store.lock()?;confirmation(&store,&r,&p.attempt_ref,&RealGhRunner)?};
  evidence::output(&files,&r,&store,&status,payload,errors)
 })();if let Err(e)=&result{evidence::diagnostic(e);}result
}
