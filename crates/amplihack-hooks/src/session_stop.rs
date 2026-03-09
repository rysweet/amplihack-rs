//! Session stop hook: stores session memory via Python SDK bridge.
//!
//! This is the simplest hook — it delegates to a Python bridge script
//! for MemoryCoordinator.store() and outputs `{}`.

use crate::protocol::{FailurePolicy, Hook};
use amplihack_state::PythonBridge;
use amplihack_types::HookInput;
use serde_json::Value;
use std::time::Duration;

/// Embedded Python bridge script for memory storage.
const MEMORY_STORE_BRIDGE: &str = r#"
import sys
import json

try:
    input_data = json.load(sys.stdin)
    action = input_data.get("action", "store")
    session_id = input_data.get("session_id", "")
    transcript_path = input_data.get("transcript_path", "")

    from amplihack.memory.coordinator import MemoryCoordinator
    coordinator = MemoryCoordinator()
    coordinator.store(session_id=session_id, transcript_path=transcript_path)
    result = {"stored": True, "memories_count": 0}
    json.dump(result, sys.stdout)
except Exception as e:
    json.dump({"stored": False, "error": str(e)}, sys.stdout)
    sys.exit(1)
"#;

pub struct SessionStopHook;

impl Hook for SessionStopHook {
    fn name(&self) -> &'static str {
        "session_stop"
    }

    fn failure_policy(&self) -> FailurePolicy {
        // Don't block session exit on memory store failure.
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (session_id, transcript_path) = match input {
            HookInput::SessionStop {
                session_id,
                transcript_path,
                ..
            } => (session_id, transcript_path),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        let bridge_input = serde_json::json!({
            "action": "store",
            "session_id": session_id.unwrap_or_default(),
            "transcript_path": transcript_path.map(|p| p.display().to_string()).unwrap_or_default(),
        });

        match PythonBridge::call(MEMORY_STORE_BRIDGE, &bridge_input, Duration::from_secs(15)) {
            Ok(result) => {
                let stored = result
                    .get("stored")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !stored {
                    let error = result
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    tracing::error!("Memory store failed: {}", error);
                }
            }
            Err(e) => {
                tracing::error!("Memory store bridge error: {}", e);
                // Don't block session exit.
            }
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_unknown_events() {
        let hook = SessionStopHook;
        let result = hook.process(HookInput::Unknown).unwrap();
        assert!(result.as_object().unwrap().is_empty());
    }
}
