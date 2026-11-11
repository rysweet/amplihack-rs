//! Launch-side adapters: bootstrap identity, finalize proposals, bind round outputs.
use super::{*,artifacts::{self,Store,Authorization}};
use anyhow::{Result,Context,ensure};
use std::{io::Read,path::{Path,PathBuf}};
use serde::Deserialize;
#[derive(clap::Args,Debug)]
pub struct Session {
 #[arg(long)] pub pr:u32,
 #[arg(long)] pub state_dir:PathBuf,
 #[arg(long)] pub repo:PathBuf,
 #[arg(long)] pub tools:PathBuf,
 #[arg(long)] pub round:String,
 #[arg(long,default_value="true",action=clap::ArgAction::Set)] pub no_merge:bool,
}
#[derive(clap::Args,Debug)]
pub struct Round { #[arg(long)] pub state_dir:PathBuf, #[arg(long)] pub round:String, #[arg(long)] pub record:Option<PathBuf> }
fn root(state:&Path)->PathBuf {state.join("pr-evidence")}
fn template(state:&Path,round:&str)->Result<Request> {ensure!(identifier(round),"INVALID_REQUEST: round");Ok(serde_json::from_value(artifacts::json(&artifacts::read(&root(state).join(format!("{round}-template.json")))?)?)?)}
fn raw_pr(pr:u32)->Result<Value> {
 use crate::commands::pr::{GhRunner,RealGhRunner,gh};
 let path=format!("repos/{{owner}}/{{repo}}/pulls/{pr}");
 let response=gh::response(RealGhRunner.run_raw(&["api","--hostname","github.com","--include","--method","GET",&path])?)?;
 ensure!(response.success && response.status==Some(200),"API_REFUSED: cannot establish PR identity using REST");artifacts::json(&response.body)
}
pub fn session(args:Session)->Result<()> {
 ensure!(identifier(&args.round) && args.pr>0,"INVALID_REQUEST: launch arguments");
 let p=raw_pr(args.pr)?;
 let target=Target{host:"github.com".into(),repository_id:p["base"]["repo"]["id"].as_u64().context("TARGET_MISMATCH")?,owner:p["base"]["repo"]["owner"]["login"].as_str().context("TARGET_MISMATCH")?.into(),repo:p["base"]["repo"]["name"].as_str().context("TARGET_MISMATCH")?.into(),pr_number:args.pr,head_repository_id:p["head"]["repo"]["id"].as_u64().context("TARGET_MISMATCH")?};
 let revision=Revision{head_sha:p["head"]["sha"].as_str().context("REVISION_MISMATCH")?.into(),base_ref:p["base"]["ref"].as_str().context("REVISION_MISMATCH")?.into(),base_sha:p["base"]["sha"].as_str().context("REVISION_MISMATCH")?.into(),worktree_sha:Some(p["head"]["sha"].as_str().context("REVISION_MISMATCH")?.into())};
 let registry=root(&args.state_dir);
 let authorization=if registry.join("authorization.json").exists(){
  let auth:Authorization=serde_json::from_value(artifacts::json(&artifacts::read(&registry.join("authorization.json"))?)?)?;
  ensure!(auth.target==target && (!args.no_merge || auth.no_merge),"AUTHORIZATION_REFUSED: launch binding cannot be widened or disable no_merge");auth
 }else{
  let authorization=Authorization{schema_version:1,run_id:format!("run-{}",uuid::Uuid::new_v4().simple()),target:target.clone(),no_merge:args.no_merge,criteria:(1..=13).map(|n|n.to_string()).collect()};
  let _=Store::initialize(&registry,authorization.clone())?;
  let repo=std::fs::canonicalize(&args.repo)?;let tools=std::fs::canonicalize(&args.tools)?;
  // Only the launcher-selected bundle is executable; never a tools directory in the PR tree.
  ensure!(!tools.starts_with(&repo),"AUTHORIZATION_REFUSED: tools must be outside the worktree under review");
  artifacts::write(&registry.join("local.json"),&serde_json::to_vec(&json!({"repo":repo,"tools":tools}))?)?;
  authorization
 };
 let r=Request{schema_version:1,request_id:format!("request-{}",uuid::Uuid::new_v4().simple()),operation:"collect".into(),run_id:authorization.run_id.clone(),round_id:args.round.clone(),target,revision,predecessor_ref:None,resume_ref:None,payload:json!({"operations":[]})};
 let path=registry.join(format!("{}-template.json",args.round));artifacts::write(&path,&serde_json::to_vec(&r)?)?;
 println!("{}",json!({"pr":args.pr.to_string(),"request_template":path,"registry_dir":registry,"schema_version":1,"head_sha":r.revision.head_sha,"ci_status":"UNAVAILABLE","ci_signal":"unavailable","explanation":"Agent selects complete evidence operations; no readiness judgment was collected"}));Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer { assessment:Assessment,receipt_refs:Vec<ArtifactRef>,supersessions:Vec<Supersession>,revalidation_plan:Plan }
pub fn assess(args:Round)->Result<()> {
 let mut bytes=Vec::new();std::io::stdin().take(artifacts::MAX_BYTES+1).read_to_end(&mut bytes)?;ensure!(bytes.len() as u64<=artifacts::MAX_BYTES,"EXECUTION_LIMIT_REACHED");
 let value=artifacts::json(&bytes).context("EVALUATOR_INVALID: readiness proposal must be a JSON object")?;
 let answer:Answer=serde_json::from_value(value)?;let mut r=template(&args.state_dir,&args.round)?;r.operation="validate".into();
 let store=Store::load(&root(&args.state_dir),&r)?;
 let assessment_ref=store.register(&r.record("assessment",json!(answer.assessment)))?;
 let request=Validate{assessment_ref,receipt_refs:answer.receipt_refs.clone(),supersessions:answer.supersessions};
 let (bundle,discrepancies)=validate::validate(&store,&r,&request)?;
 let ready=bundle.is_some() && answer.assessment.merge_ready_verdict==Readiness::MergeReady;
 let value=json!({"merge_ready_verdict":if ready{"MERGE_READY"}else{"NOT_MERGE_READY"},"bundle_ref":bundle,"receipt_refs":answer.receipt_refs,"revalidation_plan":answer.revalidation_plan,"head_sha":r.revision.head_sha,"discrepancies":discrepancies,"next_action":answer.assessment.next_action,"blocker_count":discrepancies.len(),"downgrade_reason":if discrepancies.is_empty(){""}else{"EVIDENCE_UNSUPPORTED"}});
 artifacts::write(&root(&args.state_dir).join(format!("{}-assessment.json",args.round)),&serde_json::to_vec(&value)?)?;
 println!("{value}");Ok(())
}
pub fn round(args:Round)->Result<()> {
 let r=template(&args.state_dir,&args.round)?;
 let mut value=artifacts::json(&artifacts::read(&root(&args.state_dir).join(format!("{}-assessment.json",args.round)))?)?;
 let qa=artifacts::read(&args.state_dir.join("qa-evidence.json"))?;value["qa_sha256"]=json!(artifacts::digest(&qa));
 value["round_label"]=json!(args.round);value["head_sha"]=json!(r.revision.head_sha);
 value["ci_signal"]=json!(if value["merge_ready_verdict"]=="MERGE_READY"{"GREEN"}else{"ASSESSMENT_BLOCKED"});
 value["actions"]=json!([value["next_action"]]);value["collection_errors"]=json!([]);
 let store=Store::load(&root(&args.state_dir),&r)?;
 let refs:Vec<ArtifactRef>=serde_json::from_value(value["receipt_refs"].clone())?;
 for reference in refs {let receipt:Receipt=serde_json::from_value(store.record(&reference,&r,Some("receipt"),true)?.payload)?;for error in receipt.errors {value["collection_errors"].as_array_mut().context("collection errors")?.push(json!(error));}}
 let record=args.record.context("INVALID_REQUEST: --record required")?;artifacts::write(&record,&serde_json::to_vec(&value)?)?;
 artifacts::write(&PathBuf::from(format!("{}.findings",record.display())),&serde_json::to_vec(&value["discrepancies"])?)?;
 println!("{value}");Ok(())
}
/// Called by the resolver, after actual agent invocation and decision parsing.
pub fn register_health(value:&mut Value)->Result<()> {
 let Ok(record)=std::env::var("LOOP_ROUND_RECORD") else{return Ok(())};
 let record_path=Path::new(&record);let round=artifacts::json(&artifacts::read(record_path)?)?;
 let Some(bundle)=round.get("bundle_ref").filter(|v|!v.is_null()) else{return Ok(())};
 let state=record_path.parent().context("ARTIFACT_UNSAFE")?;let label=round["round_label"].as_str().context("INVALID_REQUEST: round label")?;
 let r=template(state,label)?;let store=Store::load(&root(state),&r)?;
 let bundle_ref:ArtifactRef=serde_json::from_value(bundle.clone())?;let _=store.record(&bundle_ref,&r,Some("bundle"),true)?;
 let token=value["loop_verdict"].as_str().context("EVALUATOR_INVALID")?;
 ensure!(["CONTINUE","DONE","STUCK"].contains(&token),"EVALUATOR_INVALID");
 let evaluator=Evaluator{loop_verdict:token.into(),assessment_ref:bundle_ref,explanation:value["recommended_action"].as_str().unwrap_or("agent loop-health assessment").into(),next_action:serde_json::from_value(value["next_action"].clone()).context("EVALUATOR_INVALID: next_action required")?,recommended_intervention:if token=="STUCK"{Some(value["not_converging"].to_string())}else{None}};
 let reference=store.register(&r.record("evaluator",json!(evaluator)))?;
 artifacts::write(&root(state).join(format!("{label}-evaluator.json")),&serde_json::to_vec(&reference)?)?;value["record_ref"]=json!(reference);Ok(())
}
pub fn template_for(state:&Path,round:&str)->Result<Request>{template(state,round)}
