//! Preserve existing QA/manifests/Crusty contracts as tool-produced observations.
use super::{artifacts::{self,Store},schema::*};
use anyhow::{Result,Context,ensure};
use serde_json::{Value,json};
use std::{path::Path, process::{Command,Stdio},io::Write};
fn git(repo:&Path,args:&[&str])->Result<String> {
 let mut command=amplihack_git::command_in(repo);command.args(args).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env("GIT_NO_REPLACE_OBJECTS","1");
 let out=crate::util::run_output_with_timeout(command,std::time::Duration::from_secs(60))?;
 ensure!(out.status.success(),"EVIDENCE_UNSUPPORTED: git observation failed");Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn manifest(state:&Path,name:&str)->Result<Value> {
 let rows=String::from_utf8(artifacts::read(&state.join(format!("{name}-records.tsv")))?)?;
 let row=rows.lines().last().context("EVIDENCE_UNSUPPORTED: empty manifest")?;
 let parts:Vec<_>=row.split('\t').collect();ensure!(parts.len()==3,"EVIDENCE_UNSUPPORTED: invalid manifest row");
 let file=parts[1];ensure!(!file.starts_with('.') && file.bytes().all(|b|b.is_ascii_alphanumeric() || b"-_.".contains(&b)),"ARTIFACT_UNSAFE: manifest filename");
 let bytes=artifacts::read(&state.join(file))?;
 let mut cmd=amplihack_git::command_in(state);cmd.args(["hash-object","--no-filters","--stdin"]).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
 let mut child=cmd.spawn()?;child.stdin.take().context("hash stdin")?.write_all(&bytes)?;let out=child.wait_with_output()?;
 ensure!(out.status.success() && String::from_utf8(out.stdout)?.trim()==parts[2],"ARTIFACT_HASH_MISMATCH: loop manifest");
 ensure!(bytes==artifacts::read(&state.join(format!("{name}-latest.json")))?,"ARTIFACT_HASH_MISMATCH: latest round");artifacts::json(&bytes)
}
pub fn snapshot(store:&Store,r:&Request)->Result<Value> {
 let state=store.root().parent().context("ARTIFACT_UNSAFE: registry parent")?;
 let config:Value=artifacts::json(&artifacts::read(&store.root().join("local.json"))?)?;
 let repo=Path::new(config["repo"].as_str().context("EVIDENCE_UNSUPPORTED: missing worktree")?);
 let tools=Path::new(config["tools"].as_str().context("EVIDENCE_UNSUPPORTED: missing launch tools")?);
 let head=git(repo,&["rev-parse","HEAD"])?;ensure!(head==r.revision.head_sha,"REVISION_MISMATCH: worktree head");
 let worktree_clean=git(repo,&["status","--porcelain"])?.is_empty();
 let qa_bytes=artifacts::read(&state.join("qa-evidence.json"))?;let qa=artifacts::json(&qa_bytes)?;
 let mut qa_supported=qa["qa_status"]=="PASS" && qa["head_sha"].as_str()==Some(&head);
 // Gadugi's applicability is measured by the existing launch-side trust helper.
 let mut cmd=Command::new("/bin/bash");cmd.arg("-p").arg("-c").arg(". \"$1/autodrive_state.sh\" && . \"$1/autodrive_trust.sh\" && autodrive_gadugi_required \"$2\"").arg("local-evidence").arg(tools).arg(repo).env("AUTODRIVE_QA_BASE",&r.revision.base_ref).env("AUTODRIVE_QA_HEAD",&head).env_remove("BASH_ENV").env_remove("ENV");
 let out=crate::util::run_output_with_timeout(cmd,std::time::Duration::from_secs(60))?;
 ensure!(out.status.success(),"EVIDENCE_UNSUPPORTED: QA applicability unreadable");
 let gadugi_required=String::from_utf8(out.stdout)?.trim()!="false";
 qa_supported &= if gadugi_required {qa["gadugi_status"]=="PASS" && qa["gadugi_scenario_count"].as_str().and_then(|s|s.parse::<usize>().ok()).or_else(||qa["gadugi_scenario_count"].as_u64().map(|n|n as usize)).is_some_and(|n|n>0)}else{qa["gadugi_status"]=="NOT_REQUIRED" || qa["gadugi_status"]=="PASS"};
 let reviewed=manifest(state,"crusty")?;
 let reviewed_head=reviewed["reviewed_head_sha"].as_str().context("EVIDENCE_UNSUPPORTED: reviewed head")?;ensure!(git_sha(reviewed_head),"EVIDENCE_UNSUPPORTED: reviewed SHA");
 let phases=String::from_utf8(artifacts::read(&state.join("phases.tsv"))?)?;
 let mut crusty_supported=phases.lines().any(|line|line.starts_with("crusty-loop\t")) && matches!(reviewed["crusty_verdict"].as_str(),Some("CLEAN"|"MINOR_ONLY"));
 // These existing helpers preserve reviewed-range and per-finding followup protections.
 for script in ["range","followups"] {
  let mut command=Command::new("/bin/bash");command.arg("-p");
  if script=="range" {command.arg("-c").arg(". \"$1/autodrive_state.sh\" && . \"$1/autodrive_trust.sh\" && autodrive_crusty_range \"$2\" \"$3\" \"$4\" \"$5\"").arg("local-range").arg(tools).arg(repo).arg(reviewed_head).arg(&head).arg(&r.revision.base_sha);}
  else{command.arg(tools.join("autodrive_followups.sh")).args(["gate","--state-dir"]).arg(state).arg("--pr").arg(r.target.pr_number.to_string());}
  command.current_dir(repo).env_remove("BASH_ENV").env_remove("ENV");let out=crate::util::run_output_with_timeout(command,std::time::Duration::from_secs(60))?;
  crusty_supported &= out.status.success() && (script!="range" || String::from_utf8(out.stdout)?.trim()=="ok");
 }
 Ok(json!({"head_sha":head,"base_sha":r.revision.base_sha,"worktree_clean":worktree_clean,"qa_supported":qa_supported,"qa_sha256":artifacts::digest(&qa_bytes),"gadugi_required":gadugi_required,"crusty_supported":crusty_supported,"reviewed_head_sha":reviewed_head,"review_record_sha256":artifacts::digest(&serde_json::to_vec(&reviewed)?)}))
}
/// Final local gate still requires the loop-written QA chain.
pub fn final_chain(store:&Store,r:&Request)->Result<()> {
 let state=store.root().parent().context("ARTIFACT_UNSAFE")?;
 let record=manifest(state,"merge-ready")?;
 ensure!(record["head_sha"].as_str()==Some(&r.revision.head_sha) && record["merge_ready_verdict"]=="MERGE_READY","REVISION_MISMATCH: final round");
 // The round includes the registered exact-byte QA digest in addition to its legacy hash.
 let qa=artifacts::read(&state.join("qa-evidence.json"))?;
 ensure!(record["qa_sha256"].as_str()==Some(&artifacts::digest(&qa)),"ARTIFACT_HASH_MISMATCH: final QA chain");Ok(())
}
