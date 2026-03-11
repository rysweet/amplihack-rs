#!/usr/bin/env python3
"""
Bridge: SessionStartHook.process — retrieves memory context and checks version.

Calls MemoryCoordinator.get_context() and optionally check_version_mismatch().

IPC CONTRACT (machine-readable):
{
  "bridge_id": "session_start",
  "hook": "SessionStartHook.process",
  "transport": "subprocess JSON IPC",
  "input_schema": {
    "type": "object",
    "required": [],
    "properties": {
      "action":       {"type": "string", "enum": ["get_context", "check_version"],
                       "description": "Which sub-operation to perform"},
      "session_id":   {"type": "string", "description": "Claude session identifier"},
      "project_path": {"type": "string", "description": "Absolute path to project root"}
    }
  },
  "output_schema_get_context": {
    "type": "object",
    "required": ["context", "memories"],
    "properties": {
      "context":  {"type": "string", "description": "Memory context to inject"},
      "memories": {"type": "array",  "items": {"type": "string"}},
      "error":    {"type": "string", "description": "Error message if partial failure"}
    },
    "exit_code_success": 0,
    "exit_code_failure": 1
  },
  "output_schema_check_version": {
    "type": "object",
    "required": ["mismatch"],
    "properties": {
      "mismatch": {"type": "boolean", "description": "True if version mismatch detected"},
      "message":  {"type": "string",  "description": "Human-readable mismatch message"},
      "error":    {"type": "string",  "description": "Error message if check failed"}
    },
    "exit_code_success": 0,
    "exit_code_failure": 1
  },
  "timeout_seconds": 10,
  "error_codes": {
    "0": "Success",
    "1": "Failure — error details in output JSON"
  }
}
"""

import sys
import json


def get_context(session_id: str, project_path: str) -> tuple[dict, int]:
    """Retrieve memory context for session start. Returns (result_dict, exit_code)."""
    try:
        from amplihack.memory.coordinator import MemoryCoordinator  # type: ignore[import]

        coordinator = MemoryCoordinator()
        context = coordinator.get_context(
            session_id=session_id,
            project_path=project_path,
        )
        return {"context": context or "", "memories": []}, 0
    except ImportError as e:
        return {"context": "", "error": f"amplihack not installed: {e}"}, 1
    except Exception as e:
        return {"context": "", "error": str(e)}, 1


def check_version(project_path: str) -> dict:
    """Check for amplihack version mismatches. Returns result dict."""
    try:
        from amplihack.version import check_version_mismatch  # type: ignore[import]

        result = check_version_mismatch(project_path)
        return result or {"mismatch": False}
    except ImportError as e:
        return {"mismatch": False, "error": f"amplihack not installed: {e}"}
    except Exception as e:
        return {"mismatch": False, "error": str(e)}


def main() -> int:
    """Run session_start bridge. Returns exit code."""
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as e:
        json.dump({"context": "", "error": f"Invalid JSON input: {e}"}, sys.stdout)
        return 1

    action = input_data.get("action", "get_context")
    session_id = input_data.get("session_id", "")
    project_path = input_data.get("project_path", "")

    if action == "check_version":
        result = check_version(project_path)
        json.dump(result, sys.stdout)
        return 1 if "error" in result else 0

    # Default: get_context
    result, code = get_context(session_id, project_path)
    json.dump(result, sys.stdout)
    return code


if __name__ == "__main__":
    sys.exit(main())
