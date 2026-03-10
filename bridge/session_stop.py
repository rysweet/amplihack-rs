#!/usr/bin/env python3
"""
Bridge: session_stop.main — stores session memory via MemoryCoordinator.store().

IPC CONTRACT (machine-readable):
{
  "bridge_id": "session_stop",
  "hook": "session_stop.main / SessionStopHook.process",
  "transport": "subprocess JSON IPC",
  "input_schema": {
    "type": "object",
    "required": [],
    "properties": {
      "action":          {"type": "string", "enum": ["store"],
                          "description": "Operation to perform (currently only 'store')"},
      "session_id":      {"type": "string", "description": "Claude session identifier"},
      "transcript_path": {"type": "string",
                          "description": "Path to JSONL session transcript file"}
    }
  },
  "output_schema_success": {
    "type": "object",
    "required": ["stored"],
    "properties": {
      "stored":         {"type": "boolean", "const": true},
      "memories_count": {"type": "integer",
                         "description": "Number of memories stored (0 if unknown)"}
    },
    "exit_code": 0
  },
  "output_schema_failure": {
    "type": "object",
    "required": ["stored"],
    "properties": {
      "stored": {"type": "boolean", "const": false},
      "error":  {"type": "string", "description": "Error message"}
    },
    "exit_code": 1
  },
  "timeout_seconds": 30,
  "error_codes": {
    "0": "Success — memory stored",
    "1": "Failure — error details in output JSON"
  }
}
"""

import sys
import json


def store_memory(session_id: str, transcript_path: str) -> tuple[dict, int]:
    """Store session memory. Returns (result_dict, exit_code)."""
    try:
        from amplihack.memory.coordinator import MemoryCoordinator  # type: ignore[import]

        coordinator = MemoryCoordinator()
        coordinator.store(session_id=session_id, transcript_path=transcript_path)
        return {"stored": True, "memories_count": 0}, 0
    except ImportError as e:
        return {"stored": False, "error": f"amplihack not installed: {e}"}, 1
    except Exception as e:
        return {"stored": False, "error": str(e)}, 1


def main() -> int:
    """Run session_stop bridge. Returns exit code."""
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as e:
        json.dump({"stored": False, "error": f"Invalid JSON input: {e}"}, sys.stdout)
        return 1

    session_id = input_data.get("session_id", "")
    transcript_path = input_data.get("transcript_path", "")

    result, code = store_memory(session_id, transcript_path)
    json.dump(result, sys.stdout)
    return code


if __name__ == "__main__":
    sys.exit(main())
