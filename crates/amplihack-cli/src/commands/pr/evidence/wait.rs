//! Execute an agent-selected deadline. No default delay or fallback strategy.
use super::{schema::*,artifacts::Store};
use anyhow::{Result,ensure};
use serde_json::json;
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
pub fn wait(store:&Store,r:&Request,request:&Wait)->Result<(serde_json::Value,Vec<Error>)> {
 ensure!(!request.reason.trim().is_empty() && !request.recollect_operation_ids.is_empty(),"INVALID_REQUEST: a wait requires a reason and recollection plan");
 let deadline=match &request.wake {
  Wake::Deadline{at}=>chrono::DateTime::parse_from_rfc3339(at)?.with_timezone(&chrono::Utc),
  Wake::Event{event_ref}=>{
   // No event source is registered by this launcher. Invented events never become paths.
   let record=store.record(event_ref,r,Some("event"),true);
   let code=if record.is_ok(){"EVENT_SOURCE_UNAVAILABLE"}else{"EVENT_NOT_REGISTERED"};
   return Ok((json!({"elapsed_ms":0,"wake_reason":"EXECUTION_CONTROL","event_ref":event_ref}),vec![Error::new(code,"wait","No registered event adapter is available; choose another action explicitly")]));
  }
 };
 let stopped=Arc::new(AtomicBool::new(false));
 let mut signals=vec![];
 for signal in [signal_hook::consts::SIGINT,signal_hook::consts::SIGTERM] {signals.push(signal_hook::flag::register(signal,Arc::clone(&stopped))?);}
 let started=Instant::now();
 while chrono::Utc::now()<deadline && !stopped.load(Ordering::Relaxed) {
  let remaining=(deadline-chrono::Utc::now()).to_std().unwrap_or_default();
  std::thread::sleep(remaining.min(Duration::from_millis(100)));
 }
 for signal in signals {signal_hook::low_level::unregister(signal);}
 let interrupted=stopped.load(Ordering::Relaxed);
 Ok((json!({"elapsed_ms":started.elapsed().as_millis(),"wake_reason":if interrupted{"CANCELLED"}else{"DEADLINE"},"event_ref":null}),if interrupted{vec![Error::new("CANCELLED","execution","Agent-selected wait interrupted")]}else{vec![]}))
}
