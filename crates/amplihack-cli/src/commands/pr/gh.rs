//! Single-shot GitHub transport. No retry, API selection or readiness policy.
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
#[derive(Debug,Clone)]
pub struct RawOutput { pub stdout:Vec<u8>, pub success:bool, pub exit_code:Option<i32> }
pub struct Response { pub body:Vec<u8>, pub status:Option<u16>, pub headers:BTreeMap<String,String>, pub success:bool, pub exit_code:Option<i32> }
pub fn response(out:RawOutput)->Result<Response> {
 ensure!(out.stdout.len() as u64<=super::evidence::artifacts::MAX_BYTES,"EXECUTION_LIMIT_REACHED: response size");
 let split=out.stdout.windows(4).position(|b|b==b"\r\n\r\n").map(|i|(i,4))
  .or_else(||out.stdout.windows(2).position(|b|b==b"\n\n").map(|i|(i,2)));
 let mut status=None; let mut headers=BTreeMap::new(); let mut body=out.stdout.clone();
 if let Some((i,n))=split {
  let head=std::str::from_utf8(&out.stdout[..i])?;
  if head.starts_with("HTTP/") {
   status=head.lines().next().and_then(|l|l.split_whitespace().nth(1)).and_then(|s|s.parse().ok());
   ensure!(status.is_some(),"RESPONSE_MALFORMED: HTTP status");
   for line in head.lines().skip(1) { if let Some((key,value))=line.split_once(':') {
    let key=key.trim().to_ascii_lowercase(); if ["link","retry-after","x-ratelimit-remaining","x-ratelimit-reset","x-github-request-id"].contains(&key.as_str()) { headers.insert(key,value.trim().to_owned()); }
   }}
   body=out.stdout[i+n..].into();
  }
 }
 Ok(Response{body,status,headers,success:out.success,exit_code:out.exit_code})
}
