//! Basic usage example for `amplihack-session`.
//!
//! Mirrors `examples/basic_usage.py` from the deleted Python tree.

use amplihack_session::{SessionConfig, SessionError, SessionToolkit, quick_session};
use serde_json::json;

fn main() -> Result<(), SessionError> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime = tmp.path().join("runtime");

    // 1. Manual toolkit lifecycle.
    let mut toolkit = SessionToolkit::new(&runtime, true, "INFO")?;
    let cfg = SessionConfig {
        session_id: Some("demo-1".into()),
        ..SessionConfig::default()
    };
    let id = toolkit.create_session("hello-world", Some(cfg), Some(json!({"owner": "demo"})))?;
    println!("created session: {id}");

    if let Some(s) = toolkit.manager_mut().get_session(&id) {
        s.start();
        let _ = s.execute_command("greet", None, json!({"who": "world"}))?;
        s.save_checkpoint();
        println!("statistics: {}", s.get_statistics());
        s.stop();
    }
    toolkit.save_current()?;

    // 2. RAII helper.
    let total = quick_session("counted", |tk, sid| {
        let session = tk.manager_mut().get_session(sid).expect("session");
        for i in 0..3 {
            let _ = session.execute_command(&format!("step-{i}"), None, json!({}))?;
        }
        Ok::<_, SessionError>(session.state.command_count)
    })?;
    println!("quick_session ran {total} commands");

    Ok(())
}
