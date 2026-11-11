//! Typed endpoints. Caller text never becomes a URL, method or executable.
use super::schema::*;
use anyhow::{Result,ensure};
pub fn encoded(s:&str)->String { s.bytes().map(|b|if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() }else{format!("%{b:02X}")}).collect() }
pub fn endpoint(r:&Request,op:&Operation)->String {
 let root=format!("repos/{}/{}",r.target.owner,r.target.repo); let sha=&r.revision.head_sha; let pr=r.target.pr_number; let base=encoded(&r.revision.base_ref);
 match op.resource {
  Resource::PrMetadata=>format!("{root}/pulls/{pr}"),
  Resource::CheckRuns=>format!("{root}/commits/{sha}/check-runs"),
  Resource::CommitStatuses=>format!("{root}/commits/{sha}/status"),
  Resource::CheckSuites=>format!("{root}/commits/{sha}/check-suites"),
  Resource::SuiteCheckRuns=>format!("{root}/check-suites/{}/check-runs",op.parameters.suite_id.unwrap_or(0)),
  Resource::BranchProtection=>format!("{root}/branches/{base}/protection"),
  Resource::BranchMetadata=>format!("{root}/branches/{base}"),
  Resource::ActiveRules=>format!("{root}/rules/branches/{base}"),
  Resource::Reviews=>format!("{root}/pulls/{pr}/reviews"),
  Resource::ReviewComments=>format!("{root}/pulls/{pr}/comments"),
  Resource::ReviewThreads|Resource::LocalEvidence=>String::new(),
 }
}
pub fn validate(r:&Request,op:&Operation)->Result<()> {
 ensure!(identifier(&op.operation_id),"INVALID_REQUEST: operation id");
 ensure!((1..=100).contains(&op.pagination.per_page) && (1..=1000).contains(&op.pagination.max_pages),"INVALID_REQUEST: pagination bounds");
 let p=&op.parameters;
 let needs_sha=matches!(op.resource,Resource::CheckRuns|Resource::CommitStatuses|Resource::CheckSuites|Resource::SuiteCheckRuns);
 ensure!(if needs_sha{p.sha.as_deref()==Some(&r.revision.head_sha)}else{p.sha.is_none()},"INVALID_REQUEST: commit subject");
 ensure!(if op.resource==Resource::SuiteCheckRuns{p.suite_id.is_some_and(|id|id>0)}else{p.suite_id.is_none()},"INVALID_REQUEST: suite id");
 ensure!(if matches!(op.resource,Resource::BranchProtection|Resource::BranchMetadata){p.base_ref.as_deref()==Some(&r.revision.base_ref)}else{p.base_ref.is_none()},"INVALID_REQUEST: base ref");
 ensure!((op.api==Api::Graphql)==(op.resource==Resource::ReviewThreads),"INVALID_REQUEST: unsupported API/resource combination"); Ok(())
}
/// Only accept pagination links for the same resource (including its repository-ID alias).
/// We construct the next URL ourselves, never execute an untrusted Link value.
pub fn next(headers:&std::collections::BTreeMap<String,String>,r:&Request,endpoint:&str,page:u32,per_page:u32)->Result<bool> {
 let Some(link)=headers.get("link") else{return Ok(false)};
 let root=format!("repos/{}/{}",r.target.owner,r.target.repo);
 let alias=endpoint.replacen(&root,&format!("repositories/{}",r.target.repository_id),1);
 let mut has_next=false;
 for part in link.split(',') { let part=part.trim(); let Some((url,rel))=part.split_once('>') else {anyhow::bail!("PAGINATION_INCOMPLETE: malformed link")};
  let url=url.strip_prefix('<').ok_or_else(||anyhow::anyhow!("PAGINATION_INCOMPLETE: malformed URL"))?;
  let prefix=format!("https://api.{}/",r.target.host); let path=url.strip_prefix(&prefix).ok_or_else(||anyhow::anyhow!("PAGINATION_INCOMPLETE: cross-host link"))?;
  let (path,query)=path.split_once('?').ok_or_else(||anyhow::anyhow!("PAGINATION_INCOMPLETE: missing page"))?;
  ensure!(path==endpoint || path==alias,"PAGINATION_INCOMPLETE: expanded scope");
  let mut next_page=None;
  for item in query.split('&') { let (key,value)=item.split_once('=').ok_or_else(||anyhow::anyhow!("PAGINATION_INCOMPLETE"))?; match key {
   "page"=>{ensure!(next_page.is_none(),"PAGINATION_INCOMPLETE");next_page=Some(value.parse::<u32>()?);},
   "per_page"=>ensure!(value.parse::<u32>()?==per_page,"PAGINATION_INCOMPLETE: page size changed"),
   _=>anyhow::bail!("PAGINATION_INCOMPLETE: unknown query field")
  }}
  if rel.contains("rel=\"next\"") { ensure!(!has_next && next_page==Some(page+1),"PAGINATION_INCOMPLETE: invalid next page"); has_next=true; }
 }
 Ok(has_next)
}
pub fn rows(resource:Resource,body:&serde_json::Value)->Option<&Vec<serde_json::Value>> { match resource {
 Resource::CheckRuns|Resource::SuiteCheckRuns=>body.get("check_runs")?.as_array(),
 Resource::CheckSuites=>body.get("check_suites")?.as_array(),
 Resource::CommitStatuses=>body.get("statuses")?.as_array(),
 Resource::ActiveRules|Resource::Reviews|Resource::ReviewComments=>body.as_array(),
 _=>None,
}}
