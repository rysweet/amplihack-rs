#!/usr/bin/env python3
"""
Bridge: StopHook reflection — calls amplihack.hooks.stop.run_claude_reflection.

IPC CONTRACT (machine-readable):
{
  "bridge_id": "reflection",
  "hook": "StopHook._run_reflection_sync",
  "transport": "subprocess JSON IPC",
  "input_schema": {
    "type": "object",
    "required": [],
    "properties": {
      "session_id":      {"type": "string", "description": "Claude session identifier"},
      "project_path":    {"type": "string", "description": "Absolute path to project root"},
      "transcript_path": {"type": "string", "description": "Path to JSONL session transcript"},
      "session_dir":     {"type": "string", "description": "Path to session log directory"}
    }
  },
  "output_schema_success": {
    "type": "object",
    "required": ["success"],
    "properties": {
      "success":  {"type": "boolean", "const": true},
      "template": {"type": "string", "description": "Markdown reflection template"}
    }
  },
  "output_schema_failure": {
    "type": "object",
    "required": ["success"],
    "properties": {
      "success": {"type": "boolean", "const": false},
      "reason":  {"type": "string"},
      "error":   {"type": "string"}
    },
    "exit_code": 1
  },
  "timeout_seconds": 30,
  "error_codes": {
    "0": "Success — reflection template written to stdout",
    "1": "Failure — error details in output JSON"
  }
}
"""

import sys
import json
import os


def main() -> int:
    """Run reflection bridge. Returns exit code."""
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as e:
        json.dump({"success": False, "error": f"Invalid JSON input: {e}"}, sys.stdout)
        return 1

    session_id = input_data.get("session_id", "")
    project_path = input_data.get("project_path", "")
    transcript_path = input_data.get("transcript_path", "")
    session_dir = input_data.get("session_dir", "")

    try:
        from amplihack.hooks.stop import run_claude_reflection  # type: ignore[import]

        # Load conversation from JSONL transcript.
        conversation = []
        if transcript_path and os.path.exists(transcript_path):
            with open(transcript_path, "r") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        entry = json.loads(line)
                        if entry.get("type") in ("user", "assistant") and "message" in entry:
                            msg = entry["message"]
                            if isinstance(msg, str):
                                conversation.append({"role": entry["type"], "content": msg})
                            elif isinstance(msg, list):
                                text = " ".join(
                                    b.get("text", "")
                                    for b in msg
                                    if isinstance(b, dict) and b.get("type") == "text"
                                )
                                if text:
                                    conversation.append(
                                        {"role": entry["type"], "content": text}
                                    )
                    except json.JSONDecodeError:
                        continue

        result = run_claude_reflection(session_dir, project_path, conversation)
        if result:
            json.dump({"success": True, "template": result}, sys.stdout)
            return 0
        else:
            json.dump({"success": False, "reason": "empty result"}, sys.stdout)
            return 1
    except ImportError as e:
        json.dump({"success": False, "error": f"amplihack not installed: {e}"}, sys.stdout)
        return 1
    except Exception as e:
        json.dump({"success": False, "error": str(e)}, sys.stdout)
        return 1


if __name__ == "__main__":
    sys.exit(main())
