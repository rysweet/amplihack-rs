//! File-based readiness evidence; agents own API, action and readiness decisions.
pub mod artifacts;
pub mod schema;
mod collect;
mod graphql;
pub(super) mod local;
pub(crate) mod launch;
mod rest;
pub(super) mod validate;
mod wait;

use anyhow::{Result,ensure,Context};
use clap::Subcommand;
use serde_json::{Value,json};
use std::path::PathBuf;
use schema::*;
pub(crate) use collect::collect;
pub(crate) use validate::{Evidence,support};

#[derive(clap::Args,Debug,Clone)]
pub struct Files { #[arg(long)] pub request:PathBuf, #[arg(long)] pub result:PathBuf }
#[derive(Subcommand,Debug)]
pub enum Commands { Collect(Files), Validate(Files), Wait(Files),
 /// Initialize the launch-side registry and return an agent request template.
 Session(launch::Session),
 /// Finalize the agent proposal through support validation.
 Assess(launch::Round),
 /// Write the validated round and its immutable evidence references.
 Round(launch::Round),
 /// Register an assessment proposal or evaluator output (never a receipt).
 Propose(Files),
}
pub(crate) fn input(files:&Files,operation:&str)->Result<Request> {
 let bytes=artifacts::read(&files.request)?;
 let value=artifacts::json(&bytes).context("INVALID_REQUEST")?;
 ensure!(value["schema_version"]==1,"UNSUPPORTED_SCHEMA");
 let r:Request=serde_json::from_value(value).context("INVALID_REQUEST")?;
 ensure!(r.operation==operation && identifier(&r.request_id) && identifier(&r.run_id) && identifier(&r.round_id),"INVALID_REQUEST: operation or identity");
 ensure!(r.target.host=="github.com" && r.target.repository_id>0 && r.target.head_repository_id>0 && r.target.pr_number>0 && identifier(&r.target.owner) && identifier(&r.target.repo.replace('.',"_")),"INVALID_REQUEST: target");
 ensure!(git_sha(&r.revision.head_sha) && git_sha(&r.revision.base_sha) && r.revision.worktree_sha.as_deref().is_none_or(git_sha),"INVALID_REQUEST: revision");
 ensure!(!r.revision.base_ref.is_empty() && r.revision.base_ref.len()<=255 && !r.revision.base_ref.starts_with('-') && !r.revision.base_ref.contains("..") && r.revision.base_ref.bytes().all(|b|b.is_ascii_alphanumeric() || b"_/.-".contains(&b)),"INVALID_REQUEST: base ref");
 for reference in [&r.predecessor_ref,&r.resume_ref].into_iter().flatten() {ensure!(schema::reference(reference),"INVALID_REQUEST: reference");}
 validate_payload(&r)?; Ok(r)
}
fn validate_payload(r:&Request)->Result<()> {
 let parsed:Result<()>= (||{ match r.operation.as_str() {
  "collect"=>{let p:Collect=serde_json::from_value(r.payload.clone())?;operations(r,&p.operations)?;},
  "validate"=>{let p:Validate=serde_json::from_value(r.payload.clone())?;refs(std::iter::once(&p.assessment_ref).chain(&p.receipt_refs))?;for s in &p.supersessions {refs([&s.prior_receipt_ref,&s.replacement_receipt_ref])?;}},
  "wait"=>{let p:Wait=serde_json::from_value(r.payload.clone())?;if let Wake::Event{event_ref}=p.wake{refs([&event_ref])?;}},
  "execute"=>{let p:Execute=serde_json::from_value(r.payload.clone())?;refs([&p.bundle_ref,&p.evaluator_ref])?;for op in &p.revalidation_plan.operations{rest::validate(r,op)?;}refs(p.revalidation_plan.reuse_refs.iter())?;},
  "inspect"=>{let p:Inspect=serde_json::from_value(r.payload.clone())?;refs([&p.attempt_ref])?;},
  "propose"=>{let p:Proposal=serde_json::from_value(r.payload.clone())?;match p.kind.as_str(){"assessment"=>{let _:Assessment=serde_json::from_value(p.value)?;},"evaluator"=>{let e:Evaluator=serde_json::from_value(p.value)?;ensure!(["CONTINUE","DONE","STUCK"].contains(&e.loop_verdict.as_str()),"unknown verdict");},_=>anyhow::bail!("proposal kind")}},
  _=>anyhow::bail!("unknown operation"),
 } Ok(())})();parsed.context("INVALID_REQUEST")
}
fn refs<'a>(refs:impl IntoIterator<Item=&'a ArtifactRef>)->Result<()> {ensure!(refs.into_iter().all(schema::reference),"INVALID_REQUEST: artifact reference");Ok(())}
pub(crate) fn operations(r:&Request,ops:&[Operation])->Result<()> {ensure!(!ops.is_empty() && ops.len()<=100 && artifacts::unique_ids(ops.iter().map(|op|op.operation_id.as_str())),"INVALID_REQUEST: empty or duplicate operations");for op in ops {rest::validate(r,op)?;}Ok(())}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal { kind:String,value:Value }
pub(crate) fn store(r:&Request)->Result<artifacts::Store> {
 let root=std::env::var_os("AMPLIHACK_PR_EVIDENCE_DIR").map(PathBuf::from).context("AUTHORIZATION_REFUSED: no launch registry")?;
 artifacts::Store::load(&root,r).context("AUTHORIZATION_REFUSED: no matching launch authorization")
}
pub(crate) fn output(files:&Files,r:&Request,store:&artifacts::Store,status:&str,payload:Value,errors:Vec<Error>)->Result<()> {
 let record=r.record(&r.operation,json!({"status":status,"payload":payload,"errors":errors}));let reference=store.register(&record)?;
 let mut value=serde_json::to_value(r)?;value["status"]=json!(status);value["payload"]=payload;value["errors"]=json!(errors);value["record_ref"]=json!(reference);
 artifacts::write(&files.result,&serde_json::to_vec(&value)?)?;Ok(())
}
pub(crate) fn diagnostic(error:&anyhow::Error) {
 let message=format!("{error:#}");let codes=["DUPLICATE_JSON_KEY","UNSUPPORTED_SCHEMA","ARTIFACT_UNSAFE","INVALID_REQUEST","AUTHORIZATION_REFUSED","ARTIFACT_HASH_MISMATCH","ARTIFACT_UNREGISTERED","CROSS_RUN_REPLAY","REVISION_MISMATCH","PREDECESSOR_MISMATCH","RESULT_PERSIST_FAILED"];
 let code=codes.iter().find(|c|message.contains(**c)).copied().unwrap_or("INVALID_REQUEST");
 eprintln!("{}",json!({"schema_version":1,"error":Error::new(code,"request",code)}));
}
pub fn run(command:Commands)->Result<()> {
 let command=match command {
  Commands::Session(args)=>return launch::session(args),
  Commands::Assess(args)=>return launch::assess(args),
  Commands::Round(args)=>return launch::round(args),
  other=>other,
 };
 let (operation,files)=match command {Commands::Collect(f)=>("collect",f),Commands::Validate(f)=>("validate",f),Commands::Wait(f)=>("wait",f),Commands::Propose(f)=>("propose",f), _=>unreachable!()};
 let result=(||{let r=input(&files,operation)?;let s=store(&r)?;
  let (status,payload,errors)=match operation {
   "collect"=>{let p:Collect=serde_json::from_value(r.payload.clone())?;let c=collect::collect(&s,&r,&p.operations,&super::RealGhRunner)?;(if c.complete{"COMPLETE"}else{"PARTIAL"},collect::result(&c),c.errors)},
   "validate"=>{let p:Validate=serde_json::from_value(r.payload.clone())?;let (bundle,errors)=validate::validate(&s,&r,&p)?;let verdict=if bundle.is_some(){let record=s.record(&p.assessment_ref,&r,Some("assessment"),true)?;record.payload["merge_ready_verdict"].clone()}else{Value::Null};(if bundle.is_some(){"ACCEPTED"}else{"REJECTED"},json!({"bundle_ref":bundle,"merge_ready_verdict":verdict,"discrepancies":errors}),vec![])},
   "wait"=>{let p:Wait=serde_json::from_value(r.payload.clone())?;let (payload,errors)=wait::wait(&s,&r,&p)?;(if errors.is_empty(){"WOKEN"}else{"INTERRUPTED"},payload,errors)},
   "propose"=>{let p:Proposal=serde_json::from_value(r.payload.clone())?;let reference=s.register(&r.record(&p.kind,p.value))?;("REGISTERED",json!({"proposal_ref":reference}),vec![])},
   _=>unreachable!(),
  };output(&files,&r,&s,status,payload,errors)
 })();if let Err(e)=&result{diagnostic(e);}result
}
