//! Evidence support checks, not readiness or convergence policy.
//! The agent chooses the assessment and citations. Unsupported claims return discrepancies.
use super::{schema::*, artifacts::{self,Store}, rest};
use anyhow::{Result, ensure, Context};
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet};

pub struct Evidence { pub receipts: Vec<(ArtifactRef,Receipt,Vec<Value>)> }
impl Evidence {
 pub fn load(store:&Store,r:&Request,refs:&[ArtifactRef])->Result<Self> {
  let mut receipts=vec![];
  ensure!(artifacts::unique_ids(refs.iter().map(|r|r.artifact_id.as_str())),"EVIDENCE_UNSUPPORTED: duplicate receipt");
  for reference in refs {
   let record=store.record(reference,r,Some("receipt"),true)?;
   let receipt:Receipt=serde_json::from_value(record.payload)?;
   rest::validate(r,&receipt.operation)?;
   let mut bodies=vec![];
   for page in &receipt.pages { if let Some(reference)=&page.response_ref { bodies.push(artifacts::json(&store.get(reference,Some("response"))?)?); } }
   receipts.push((reference.clone(),receipt,bodies));
  }
  Ok(Self{receipts})
 }
 pub fn bodies(&self,resource:Resource)->Result<Vec<&Value>> {
  let matching:Vec<_>=self.receipts.iter().filter(|(_,r,_)|r.operation.resource==resource && r.complete && r.errors.is_empty()).collect();
  ensure!(matching.len()==1,"EVIDENCE_UNSUPPORTED: need exactly one complete {resource:?} receipt");
  let (_,receipt,bodies)=matching[0];
  ensure!(!bodies.is_empty() && bodies.len()==receipt.pages.len() && receipt.pages.iter().all(|p|p.http_status.is_some_and(|s|(200..300).contains(&s)) && p.process_exit_code==Some(0)),"EVIDENCE_UNSUPPORTED: incomplete {resource:?}");
  Ok(bodies.iter().collect())
 }
 pub fn rows(&self,resource:Resource)->Result<Vec<&Value>> {
  let mut rows=vec![];
  for body in self.bodies(resource)? { rows.extend(rest::rows(resource,body).context("EVIDENCE_UNSUPPORTED: missing rows")?); }
  Ok(rows)
 }
 fn cite(&self,refs:&[FactRef])->Result<()> {
  ensure!(!refs.is_empty(),"EVIDENCE_UNSUPPORTED: success requires citations");
  for reference in refs {
   let (_,receipt,bodies)=self.receipts.iter().find(|(r,_,_)|r==&reference.receipt_ref).context("EVIDENCE_UNSUPPORTED: uncited receipt")?;
   let index=reference.fact_id.strip_prefix("page-").and_then(|s|s.parse::<usize>().ok()).context("EVIDENCE_UNSUPPORTED: unknown fact")?;
   ensure!(receipt.complete && receipt.errors.is_empty() && index>0 && index<=bodies.len(),"EVIDENCE_UNSUPPORTED: incomplete fact");
  } Ok(())
 }
}
#[derive(Default)]
struct Policy { checks:Vec<(String,Option<u64>)>, approvals:u64, conversations:bool, strict:bool }
fn policy(e:&Evidence)->Result<Policy> {
 let mut p=Policy::default();
 let classic=e.bodies(Resource::BranchProtection);
 if let Ok(bodies)=classic {
  let body=bodies[0];
  if let Some(checks)=body.get("required_status_checks").filter(|v|!v.is_null()) {
   p.strict=checks["strict"].as_bool().context("EVIDENCE_UNSUPPORTED: strict policy unknown")?;
   if let Some(rows)=checks["checks"].as_array() { for c in rows { p.checks.push((c["context"].as_str().context("EVIDENCE_UNSUPPORTED: required context")?.into(),c["app_id"].as_u64())); } }
   else { for c in checks["contexts"].as_array().context("EVIDENCE_UNSUPPORTED: required contexts unknown")? { p.checks.push((c.as_str().context("EVIDENCE_UNSUPPORTED: context type")?.into(),None)); } }
  }
  if let Some(review)=body.get("required_pull_request_reviews").filter(|v|!v.is_null()) {
   p.approvals=review["required_approving_review_count"].as_u64().context("EVIDENCE_UNSUPPORTED: review count")?;
   ensure!(review["require_code_owner_reviews"].as_bool()==Some(false) && review.get("require_last_push_approval").is_none_or(|v|v.as_bool()==Some(false)),"EVIDENCE_UNSUPPORTED: code owner/last push requirements need authoritative evidence");
  }
  p.conversations=body["required_conversation_resolution"]["enabled"].as_bool().unwrap_or(false);
  for field in ["required_signatures","required_deployments"] { ensure!(body.get(field).is_none_or(|v|v.is_null() || v["enabled"]==false),"EVIDENCE_UNSUPPORTED: unsupported classic requirement {field}"); }
 }else{
  let branch=e.bodies(Resource::BranchMetadata)?;
  ensure!(branch[0]["protected"].as_bool()==Some(false),"EVIDENCE_UNSUPPORTED: protection unreadable");
 }
 for rule in e.rows(Resource::ActiveRules)? {
  match rule["type"].as_str().context("EVIDENCE_UNSUPPORTED: rule type")? {
   "required_status_checks"=>{
    let params=&rule["parameters"];
    p.strict|=params["strict_required_status_checks_policy"].as_bool().context("EVIDENCE_UNSUPPORTED: strict rule")?;
    for check in params["required_status_checks"].as_array().context("EVIDENCE_UNSUPPORTED: required checks")? {
     p.checks.push((check["context"].as_str().context("EVIDENCE_UNSUPPORTED: check context")?.into(),check["integration_id"].as_u64()));
    }
   },
   "pull_request"=>{
    let params=&rule["parameters"];
    p.approvals=p.approvals.max(params["required_approving_review_count"].as_u64().context("EVIDENCE_UNSUPPORTED: approvals")?);
    ensure!(params["require_code_owner_review"].as_bool()==Some(false) && params["require_last_push_approval"].as_bool()==Some(false),"EVIDENCE_UNSUPPORTED: code owner/last push approvals need authoritative evidence");
    p.conversations|=params["required_review_thread_resolution"].as_bool().context("EVIDENCE_UNSUPPORTED: thread rule")?;
   },
   // These rules constrain writers; the accepted current-head PR and server still enforce them.
   "deletion"|"non_fast_forward"|"creation"|"update"=>(),
   other=>anyhow::bail!("EVIDENCE_UNSUPPORTED: rule {other} requires additional authoritative support (direct merge queue is never bypassed)"),
  }
 }
 Ok(p)
}
fn pr<'a>(e:&'a Evidence,r:&Request)->Result<&'a Value> {
 let bodies=e.bodies(Resource::PrMetadata)?; ensure!(bodies.len()==1,"EVIDENCE_UNSUPPORTED: multiple PR bodies");
 let p=bodies[0];
 ensure!(p["number"].as_u64()==Some(r.target.pr_number as u64) && p["base"]["repo"]["id"].as_u64()==Some(r.target.repository_id) && p["head"]["repo"]["id"].as_u64()==Some(r.target.head_repository_id),"TARGET_MISMATCH");
 ensure!(p["head"]["sha"].as_str()==Some(&r.revision.head_sha) && p["base"]["sha"].as_str()==Some(&r.revision.base_sha) && p["base"]["ref"].as_str()==Some(&r.revision.base_ref),"REVISION_MISMATCH"); Ok(p)
}
fn ci(e:&Evidence,r:&Request,p:&Policy)->Result<()> {
 let checks=e.rows(Resource::CheckRuns)?; let statuses=e.rows(Resource::CommitStatuses)?;
 for b in e.bodies(Resource::CommitStatuses)? {ensure!(b["sha"].as_str()==Some(&r.revision.head_sha),"REVISION_MISMATCH: status commit");}
 ensure!(!checks.is_empty() || !statuses.is_empty(),"EVIDENCE_UNSUPPORTED: both CI families empty");
 let suites=e.rows(Resource::CheckSuites)?;
 ensure!(suites.len()<1000,"EVIDENCE_UNSUPPORTED: source suite limit; request each suite explicitly");
 let mut latest=BTreeMap::new();
 for c in checks {
  ensure!(c["head_sha"].as_str()==Some(&r.revision.head_sha),"REVISION_MISMATCH: check commit");
  let suite=c["check_suite"]["id"].as_u64().context("EVIDENCE_UNSUPPORTED: suite identity")?;
  ensure!(suites.iter().any(|s|s["id"].as_u64()==Some(suite) && s["head_sha"].as_str()==Some(&r.revision.head_sha)),"EVIDENCE_UNSUPPORTED: unaccounted suite");
  let app=c["app"]["id"].as_u64().context("EVIDENCE_UNSUPPORTED: app identity")?;
  let name=c["name"].as_str().context("EVIDENCE_UNSUPPORTED: check name")?;
  let id=c["id"].as_u64().context("EVIDENCE_UNSUPPORTED: check identity")?;
  let key=(name,app,suite);
  if latest.get(&key).is_none_or(|(old,_)|id>*old) {latest.insert(key,(id,c));}
 }
 for (_,c) in latest.values() {ensure!(c["status"]=="completed" && c["conclusion"]=="success","EVIDENCE_UNSUPPORTED: check pending, failed or not successful");}
 let mut latest_status=BTreeMap::new();
 for s in statuses {let context=s["context"].as_str().context("EVIDENCE_UNSUPPORTED: status context")?;let id=s["id"].as_u64().context("EVIDENCE_UNSUPPORTED: status identity")?;
  if latest_status.get(context).is_none_or(|(old,_)|id>*old) {latest_status.insert(context,(id,s));}}
 for (_,s) in latest_status.values() {ensure!(s["state"]=="success","EVIDENCE_UNSUPPORTED: status pending or failed");}
 for (name,app) in &p.checks {ensure!(latest.iter().any(|((n,a,_),(_,c))|*n==name && app.is_none_or(|id|id==*a) && c["conclusion"]=="success") || (app.is_none() && latest_status.contains_key(name.as_str())),"EVIDENCE_UNSUPPORTED: required context/app missing");}
 Ok(())
}
fn reviews(e:&Evidence,r:&Request,p:&Policy)->Result<()> {
 let mut latest=BTreeMap::new();
 for review in e.rows(Resource::Reviews)? {
  let user=review["user"]["id"].as_u64().context("EVIDENCE_UNSUPPORTED: reviewer identity")?;
  let id=review["id"].as_u64().context("EVIDENCE_UNSUPPORTED: review identity")?;
  if latest.get(&user).is_none_or(|(old,_)|id>*old) {latest.insert(user,(id,review));}
 }
 ensure!(latest.values().all(|(_,v)|v["state"]!="CHANGES_REQUESTED"),"EVIDENCE_UNSUPPORTED: changes requested");
 let approvals=latest.values().filter(|(_,v)|v["state"]=="APPROVED" && v["commit_id"].as_str()==Some(&r.revision.head_sha)).count();
 ensure!(approvals as u64>=p.approvals,"EVIDENCE_UNSUPPORTED: required current-head approvals missing"); Ok(())
}
fn threads(e:&Evidence,r:&Request)->Result<()> {
 if let Ok(bodies)=e.bodies(Resource::ReviewThreads) {
  for body in bodies { for thread in body["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"].as_array().context("EVIDENCE_UNSUPPORTED: thread list")? {ensure!(thread["isResolved"]==true,"EVIDENCE_UNSUPPORTED: unresolved review thread");} }
 }else{
  // REST comments cannot establish resolution. Complete absence is supportable.
  let p=pr(e,r)?;
  ensure!(p["review_comments"].as_u64()==Some(0) && e.rows(Resource::ReviewComments)?.is_empty(),"EVIDENCE_UNSUPPORTED: inline activity requires authoritative thread evidence");
  let _=e.rows(Resource::Reviews)?;
 }
 Ok(())
}
pub fn support(e:&Evidence,r:&Request,id:&str)->Result<()> {
 let p=pr(e,r)?;
 ensure!(p["state"]=="open" && p["draft"]==false,"EVIDENCE_UNSUPPORTED: PR closed or draft");
 match id {
  "4"=>ci(e,r,&policy(e)?),
  "6"=>reviews(e,r,&policy(e)?),
  "7"=>{ensure!(p["mergeable"]==true && p["mergeable_state"]=="clean","EVIDENCE_UNSUPPORTED: mergeability unknown or not clean");Ok(())},
  "8"=>{let policy=policy(e)?;ci(e,r,&policy)?;reviews(e,r,&policy)?;if policy.conversations {threads(e,r)?;} Ok(())},
  "10"=>threads(e,r),
  "5"|"9"|"11"|"12"=>{ensure!(p["body"].as_str().is_some_and(|s|!s.trim().is_empty()) && p["title"].as_str().is_some_and(|s|!s.trim().is_empty()),"EVIDENCE_UNSUPPORTED: missing reviewable metadata");Ok(())},
  // Local QA, reviewed ranges, documentation and scope use trusted local receipts.
  "1"|"2"|"3"|"13"=>{
   let bodies=e.bodies(Resource::LocalEvidence)?;let local=bodies[0];
   ensure!(local["head_sha"].as_str()==Some(&r.revision.head_sha),"REVISION_MISMATCH: local evidence");
   if id=="1" {ensure!(local["qa_supported"]==true,"EVIDENCE_UNSUPPORTED: QA did not pass");}
   if id=="3" {ensure!(local["crusty_supported"]==true,"EVIDENCE_UNSUPPORTED: Crusty range or followups unverified");}
   ensure!(local["worktree_clean"]==true && local["base_sha"].as_str()==Some(&r.revision.base_sha),"EVIDENCE_UNSUPPORTED: local synchronization");
   Ok(())
  },
  _=>anyhow::bail!("EVIDENCE_UNSUPPORTED: unknown criterion"),
 }
}
pub fn validate(store:&Store,r:&Request,proposal:&Validate)->Result<(Option<ArtifactRef>,Vec<Error>)> {
 let record=store.record(&proposal.assessment_ref,r,Some("assessment"),true)?;
 let assessment:Assessment=serde_json::from_value(record.payload)?;
 let evidence=Evidence::load(store,r,&proposal.receipt_refs)?;
 let mut discrepancies=vec![];
 let configured:BTreeSet<_>=store.authorization.criteria.iter().map(String::as_str).collect();
 let supplied:BTreeSet<_>=assessment.criteria.iter().map(|c|c.criterion_id.as_str()).collect();
 if configured!=supplied || supplied.len()!=assessment.criteria.len() {discrepancies.push(Error::new("CRITERIA_INCOMPLETE","validation","Every configured criterion must occur exactly once"));}
 for s in &proposal.supersessions {
  let prior=store.record(&s.prior_receipt_ref,r,Some("receipt"),false)?;
  let old:Receipt=serde_json::from_value(prior.payload)?;
  let replacement=evidence.receipts.iter().find(|(id,_,_)|id==&s.replacement_receipt_ref);
  if old.complete || old.errors.is_empty() || replacement.is_none_or(|(_,receipt,_)|!receipt.complete) || s.fact_ids.is_empty() { discrepancies.push(Error::new("EVIDENCE_UNSUPPORTED","validation","Supersession requires a failed collection and complete replacement; actual results remain accounted")); }
 }
 for c in &assessment.criteria {
  let ready=assessment.merge_ready_verdict==Readiness::MergeReady;
  if ready && c.disposition!=Disposition::Satisfied {discrepancies.push(Error::new("EVIDENCE_UNSUPPORTED","validation","Readiness requires supported satisfaction of every configured criterion"));continue;}
  if matches!(c.disposition,Disposition::Satisfied|Disposition::NotApplicable) {
   let checked=evidence.cite(&c.fact_refs).and_then(|()| {
    // Restrict support to cited receipts; unrelated green evidence cannot validate a claim.
    let cited=Evidence{receipts:evidence.receipts.iter().filter(|(id,_,_)|c.fact_refs.iter().any(|f|&f.receipt_ref==id)).cloned().collect()};
    support(&cited,r,&c.criterion_id)
   });
   if let Err(error)=checked {discrepancies.push(Error::new("EVIDENCE_UNSUPPORTED","validation",&format!("criterion {}: {error}",c.criterion_id)));}
  }else if c.explanation.trim().is_empty() {discrepancies.push(Error::new("EVIDENCE_UNSUPPORTED","validation","Blocked or unknown criterion needs an explanation"));}
 }
 if !discrepancies.is_empty() {return Ok((None,discrepancies));}
 let bundle=Bundle{assessment_ref:proposal.assessment_ref.clone(),receipt_refs:proposal.receipt_refs.clone(),supersessions:proposal.supersessions.clone(),assessment};
 Ok((Some(store.register(&r.record("bundle",json!(bundle)))?),discrepancies))
}
