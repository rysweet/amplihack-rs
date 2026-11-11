//! Execute only the operations and pagination selected by the agent.
use super::{schema::*, artifacts::Store, rest, graphql};
use crate::commands::pr::{GhRunner, gh};
use anyhow::Result;
use serde_json::json;

pub struct Collected { pub refs:Vec<ArtifactRef>, pub errors:Vec<Error>, pub complete:bool }
pub fn collect(store:&Store,r:&Request,operations:&[Operation],runner:&dyn GhRunner)->Result<Collected> {
 let mut result=Collected{refs:Vec::new(),errors:Vec::new(),complete:true};
 for op in operations {
  rest::validate(r,op)?;
  let mut receipt=Receipt{operation:op.clone(),collected_at:chrono::Utc::now().to_rfc3339(),pages:vec![],complete:false,errors:vec![]};
  if op.resource==Resource::LocalEvidence {
   match super::local::snapshot(store,r) {
    Ok(value)=>{ let response_ref=store.put("response",&serde_json::to_vec(&value)?)?;
     receipt.pages.push(Page{response_ref:Some(response_ref),http_status:Some(200),response_headers:Default::default(),process_exit_code:Some(0)});receipt.complete=true; },
    Err(error)=>receipt.errors.push(Error::new("EVIDENCE_UNSUPPORTED","collection",&error.to_string())),
   }
   result.complete &=receipt.complete;result.errors.extend(receipt.errors.clone());
   result.refs.push(store.register(&r.record("receipt",serde_json::to_value(receipt)?))?);continue;
  }
  let endpoint=rest::endpoint(r,op); let mut cursor=None; let mut received=0usize; let mut total=None;
  for page in 1..=op.pagination.max_pages {
   let mut args=vec!["api".to_owned(),"--hostname".into(),r.target.host.clone(),"--include".into(),"--method".into(),"GET".into(),"-H".into(),"Accept: application/vnd.github+json".into(),"-H".into(),"X-GitHub-Api-Version: 2022-11-28".into()];
   if op.api==Api::Rest {
    args.push(format!("{endpoint}?per_page={}&page={page}",op.pagination.per_page));
   }else{
    args[5]="POST".into(); args.push("graphql".into());
    for (key,value) in [("query",graphql::THREADS.to_owned()),("owner",r.target.owner.clone()),("repo",r.target.repo.clone())] {args.push("-f".into());args.push(format!("{key}={value}"));}
    args.push("-F".into());args.push(format!("pr={}",r.target.pr_number));
    if let Some(c)=&cursor {args.push("-f".into());args.push(format!("cursor={c}"));}
   }
   let response=runner.run_raw(&args.iter().map(String::as_str).collect::<Vec<_>>()).and_then(gh::response);
   let response=match response {Ok(v)=>v,Err(e)=>{receipt.errors.push(Error::new("TRANSPORT_FAILED","collection",&format!("GitHub operation could not produce a response: {e}")));break}};
   let body_ref=store.put("response",&response.body)?;
   let value=super::artifacts::json(&response.body);
   receipt.pages.push(Page{response_ref:Some(body_ref),http_status:response.status,response_headers:response.headers.clone(),process_exit_code:response.exit_code});
   if !response.success || !response.status.is_some_and(|s|(200..300).contains(&s)) {
    let code=if response.status==Some(429) || (response.status==Some(403) && String::from_utf8_lossy(&response.body).to_lowercase().contains("rate limit")){"RATE_LIMITED"}else{"API_REFUSED"};
    receipt.errors.push(Error::new(code,"collection","GitHub refused the selected operation; no fallback or retry was attempted"));break;
   }
   let value=match value {Ok(v)=>v,Err(_)=>{receipt.errors.push(Error::new("RESPONSE_MALFORMED","collection","Response is not valid unique-key JSON"));break}};
   let more=if op.api==Api::Graphql {
    if value.get("errors").is_some() {receipt.errors.push(Error::new("GRAPHQL_PARTIAL","collection","GraphQL returned errors"));break;}
    let pr=&value["data"]["repository"]["pullRequest"];
    if pr["headRefOid"].as_str()!=Some(&r.revision.head_sha) {receipt.errors.push(Error::new("REVISION_MISMATCH","collection","Thread evidence is bound to another head"));break;}
    let info=&pr["reviewThreads"]["pageInfo"];
    let Some(more)=info["hasNextPage"].as_bool() else {receipt.errors.push(Error::new("RESPONSE_MALFORMED","collection","Missing GraphQL page information"));break};
    if !pr["reviewThreads"]["nodes"].is_array(){receipt.errors.push(Error::new("RESPONSE_MALFORMED","collection","Missing thread nodes"));break;}
    if more { let next=info["endCursor"].as_str(); if next.is_none() || next==cursor.as_deref() {receipt.errors.push(Error::new("PAGINATION_INCOMPLETE","collection","Missing or repeated cursor"));break;} cursor=next.map(str::to_owned); } more
   }else{
    if let Some(rows)=rest::rows(op.resource,&value) {
     received+=rows.len();
     if let Some(n)=value.get("total_count").and_then(|v|v.as_u64()) { if total.is_some_and(|old|old!=n) {receipt.errors.push(Error::new("PAGINATION_INCOMPLETE","collection","Total changed during collection"));break;} total=Some(n); }
    }else if !matches!(op.resource,Resource::PrMetadata|Resource::BranchProtection|Resource::BranchMetadata) || !value.is_object() {
     receipt.errors.push(Error::new("RESPONSE_MALFORMED","collection","Missing expected resource shape"));break;
    }
    match rest::next(&response.headers,r,&endpoint,page,op.pagination.per_page){Ok(v)=>v,Err(e)=>{receipt.errors.push(Error::new("PAGINATION_INCOMPLETE","collection",&e.to_string()));break}}
   };
   if !more {
    if total.is_some_and(|n|n!=received as u64){receipt.errors.push(Error::new("PAGINATION_INCOMPLETE","collection","Enumerated rows do not match reported total"));}
    else {receipt.complete=true;}
    break;
   }
   if op.pagination.scope==Scope::Single || page==op.pagination.max_pages {receipt.errors.push(Error::new("PAGINATION_INCOMPLETE","collection","Requested collection ended with pages remaining"));break;}
  }
  for e in &mut receipt.errors {e.operation_id=Some(op.operation_id.clone());}
  result.complete &=receipt.complete;
  result.errors.extend(receipt.errors.clone());
  result.refs.push(store.register(&r.record("receipt",serde_json::to_value(receipt)?))?);
 }
 Ok(result)
}
pub fn result(collected:&Collected)->serde_json::Value {json!({"receipt_refs":collected.refs,"completeness":if collected.complete{"COMPLETE"}else{"PARTIAL"}})}
