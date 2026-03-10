#!/usr/bin/env python3
"""
Bridge: UserPromptSubmitHook.process — injects memory context into user prompts.

Calls MemoryCoordinator.inject_memory_for_agents_sync().

IPC CONTRACT (machine-readable):
{
  "bridge_id": "user_prompt_submit",
  "hook": "UserPromptSubmitHook.process",
  "transport": "subprocess JSON IPC",
  "input_schema": {
    "type": "object",
    "required": [],
    "properties": {
      "action":     {"type": "string", "enum": ["inject_memory"],
                     "description": "Operation to perform"},
      "session_id": {"type": "string", "description": "Claude session identifier"},
      "prompt":     {"type": "string", "description": "User prompt text to inject context into"}
    }
  },
  "output_schema_success": {
    "type": "object",
    "required": ["injected_context", "memory_keys_used"],
    "properties": {
      "injected_context":  {"type": "string",
                            "description": "Context string to prepend to prompt"},
      "memory_keys_used":  {"type": "array", "items": {"type": "string"},
                            "description": "Memory keys that contributed to context"}
    },
    "exit_code": 0
  },
  "output_schema_failure": {
    "type": "object",
    "required": ["injected_context"],
    "properties": {
      "injected_context": {"type": "string", "const": ""},
      "error":            {"type": "string", "description": "Error message"}
    },
    "exit_code": 1
  },
  "timeout_seconds": 5,
  "error_codes": {
    "0": "Success — context written to stdout",
    "1": "Failure — error details in output JSON, empty context used"
  }
}
"""

import sys
import json


def inject_memory(session_id: str, prompt: str) -> tuple[dict, int]:
    """Inject memory context for a user prompt. Returns (result_dict, exit_code)."""
    try:
        from amplihack.memory.coordinator import MemoryCoordinator  # type: ignore[import]

        coordinator = MemoryCoordinator()
        context = coordinator.inject_memory_for_agents_sync(
            session_id=session_id,
            prompt=prompt,
        )
        return {"injected_context": context or "", "memory_keys_used": []}, 0
    except ImportError as e:
        return {
            "injected_context": "",
            "error": f"amplihack not installed: {e}",
        }, 1
    except Exception as e:
        return {"injected_context": "", "error": str(e)}, 1


def main() -> int:
    """Run user_prompt_submit bridge. Returns exit code."""
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as e:
        json.dump(
            {"injected_context": "", "error": f"Invalid JSON input: {e}"},
            sys.stdout,
        )
        return 1

    session_id = input_data.get("session_id", "")
    prompt = input_data.get("prompt", "")

    result, code = inject_memory(session_id, prompt)
    json.dump(result, sys.stdout)
    return code


if __name__ == "__main__":
    sys.exit(main())
