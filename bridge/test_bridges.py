#!/usr/bin/env python3
"""
Unit tests for all 4 Python bridge scripts.

Run with: python3 -m pytest bridge/test_bridges.py -v
Or:       python3 bridge/test_bridges.py
"""

import io
import json
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

# Ensure bridge/ is on the import path.
_BRIDGE_DIR = Path(__file__).parent
if str(_BRIDGE_DIR) not in sys.path:
    sys.path.insert(0, str(_BRIDGE_DIR))


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _call_bridge_script(script_name: str, input_data: dict) -> tuple[dict, int]:
    """Run a bridge script as a subprocess with JSON IPC. Returns (output, exit_code)."""
    script = _BRIDGE_DIR / script_name
    result = subprocess.run(
        [sys.executable, str(script)],
        input=json.dumps(input_data).encode(),
        capture_output=True,
        timeout=10,
    )
    output = json.loads(result.stdout.decode()) if result.stdout else {}
    return output, result.returncode


def _stdin_from(data: dict):
    """Return a BytesIO that looks like sys.stdin for direct module calls."""
    return io.TextIOWrapper(io.BytesIO(json.dumps(data).encode()))


# ---------------------------------------------------------------------------
# Tests: reflection.py
# ---------------------------------------------------------------------------

class TestReflectionBridge(unittest.TestCase):
    """Tests for bridge/reflection.py — StopHook._run_reflection_sync()."""

    def test_invalid_json_returns_failure(self):
        """Bridge must return failure JSON when stdin is not valid JSON."""
        script = _BRIDGE_DIR / "reflection.py"
        result = subprocess.run(
            [sys.executable, str(script)],
            input=b"not-json",
            capture_output=True,
            timeout=10,
        )
        output = json.loads(result.stdout.decode())
        self.assertEqual(result.returncode, 1)
        self.assertFalse(output["success"])
        self.assertIn("error", output)

    def test_import_error_returns_failure(self):
        """Bridge must return {success: false} when amplihack is not installed."""
        import reflection  # type: ignore[import]

        # Patch to simulate missing amplihack.
        with patch.dict("sys.modules", {"amplihack": None, "amplihack.hooks.stop": None}):
            with patch("sys.stdin", _stdin_from({"session_id": "test"})):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = reflection.main()
                    output = json.loads(mock_out.getvalue())
        self.assertEqual(code, 1)
        self.assertFalse(output["success"])

    def test_successful_reflection(self):
        """Bridge must return {success: true, template: str} on success."""
        import reflection  # type: ignore[import]

        mock_module = MagicMock()
        mock_module.run_claude_reflection.return_value = "## Reflection\n\nGood work."

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.hooks": MagicMock(),
            "amplihack.hooks.stop": mock_module,
        }):
            with patch("sys.stdin", _stdin_from({"session_id": "s1", "project_path": "/tmp"})):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = reflection.main()
                    output = json.loads(mock_out.getvalue())

        self.assertEqual(code, 0)
        self.assertTrue(output["success"])
        self.assertIn("template", output)
        self.assertIn("Reflection", output["template"])

    def test_empty_reflection_result_is_failure(self):
        """Bridge must return failure when run_claude_reflection returns empty."""
        import reflection  # type: ignore[import]

        mock_module = MagicMock()
        mock_module.run_claude_reflection.return_value = ""

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.hooks": MagicMock(),
            "amplihack.hooks.stop": mock_module,
        }):
            with patch("sys.stdin", _stdin_from({})):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = reflection.main()
                    output = json.loads(mock_out.getvalue())

        self.assertEqual(code, 1)
        self.assertFalse(output["success"])
        self.assertEqual(output.get("reason"), "empty result")

    def test_transcript_loading(self):
        """Bridge loads JSONL transcript and passes conversation to reflection."""
        import reflection  # type: ignore[import]

        captured = {}
        mock_module = MagicMock()

        def capture_call(session_dir, project_path, conversation):
            captured["conversation"] = conversation
            return "## Done"

        mock_module.run_claude_reflection.side_effect = capture_call

        with tempfile.NamedTemporaryFile(suffix=".jsonl", mode="w", delete=False) as f:
            f.write(json.dumps({"type": "user", "message": "hello"}) + "\n")
            f.write(json.dumps({"type": "assistant", "message": "world"}) + "\n")
            transcript_path = f.name

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.hooks": MagicMock(),
            "amplihack.hooks.stop": mock_module,
        }):
            with patch("sys.stdin", _stdin_from({"transcript_path": transcript_path})):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = reflection.main()

        self.assertEqual(code, 0)
        self.assertEqual(len(captured["conversation"]), 2)
        self.assertEqual(captured["conversation"][0]["role"], "user")
        self.assertEqual(captured["conversation"][1]["role"], "assistant")


# ---------------------------------------------------------------------------
# Tests: session_start.py
# ---------------------------------------------------------------------------

class TestSessionStartBridge(unittest.TestCase):
    """Tests for bridge/session_start.py — SessionStartHook.process()."""

    def test_invalid_json_returns_empty_context(self):
        """Bridge must return {context: ''} when stdin is not valid JSON."""
        script = _BRIDGE_DIR / "session_start.py"
        result = subprocess.run(
            [sys.executable, str(script)],
            input=b"not-json",
            capture_output=True,
            timeout=10,
        )
        output = json.loads(result.stdout.decode())
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output.get("context", ""), "")
        self.assertIn("error", output)

    def test_get_context_import_error(self):
        """Bridge returns {context: '', error: ...} when amplihack not installed."""
        import session_start  # type: ignore[import]

        with patch.dict("sys.modules", {
            "amplihack": None,
            "amplihack.memory": None,
            "amplihack.memory.coordinator": None,
        }):
            result, code = session_start.get_context("s1", "/tmp")

        self.assertEqual(result["context"], "")
        self.assertIn("error", result)

    def test_get_context_success(self):
        """Bridge returns {context: str, memories: []} on success."""
        import session_start  # type: ignore[import]

        mock_coordinator = MagicMock()
        mock_coordinator.get_context.return_value = "Previous session context here."
        mock_module = MagicMock()
        mock_module.MemoryCoordinator.return_value = mock_coordinator

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.memory": MagicMock(),
            "amplihack.memory.coordinator": mock_module,
        }):
            result, code = session_start.get_context("s1", "/tmp")

        self.assertEqual(code, 0)
        self.assertEqual(result["context"], "Previous session context here.")
        self.assertEqual(result["memories"], [])

    def test_check_version_no_mismatch(self):
        """Bridge returns {mismatch: false} when versions match."""
        import session_start  # type: ignore[import]

        mock_version_module = MagicMock()
        mock_version_module.check_version_mismatch.return_value = {"mismatch": False}

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.version": mock_version_module,
        }):
            result = session_start.check_version("/tmp")

        self.assertFalse(result["mismatch"])

    def test_action_routing(self):
        """main() routes 'check_version' action to version check."""
        import session_start  # type: ignore[import]

        with patch.object(session_start, "check_version", return_value={"mismatch": False}) as mock_cv:
            with patch("sys.stdin", _stdin_from({"action": "check_version", "project_path": "/x"})):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = session_start.main()
                    output = json.loads(mock_out.getvalue())

        mock_cv.assert_called_once_with("/x")
        self.assertFalse(output["mismatch"])


# ---------------------------------------------------------------------------
# Tests: session_stop.py
# ---------------------------------------------------------------------------

class TestSessionStopBridge(unittest.TestCase):
    """Tests for bridge/session_stop.py — session_stop.main()."""

    def test_invalid_json_returns_failure(self):
        """Bridge must return {stored: false} when stdin is not valid JSON."""
        script = _BRIDGE_DIR / "session_stop.py"
        result = subprocess.run(
            [sys.executable, str(script)],
            input=b"not-json",
            capture_output=True,
            timeout=10,
        )
        output = json.loads(result.stdout.decode())
        self.assertEqual(result.returncode, 1)
        self.assertFalse(output["stored"])
        self.assertIn("error", output)

    def test_store_import_error(self):
        """Bridge returns {stored: false, error: ...} when amplihack not installed."""
        import session_stop  # type: ignore[import]

        with patch.dict("sys.modules", {
            "amplihack": None,
            "amplihack.memory": None,
            "amplihack.memory.coordinator": None,
        }):
            result, code = session_stop.store_memory("s1", "/tmp/transcript.jsonl")

        self.assertFalse(result["stored"])
        self.assertIn("error", result)
        self.assertEqual(code, 1)

    def test_store_success(self):
        """Bridge returns {stored: true, memories_count: 0} on success."""
        import session_stop  # type: ignore[import]

        mock_coordinator = MagicMock()
        mock_coordinator.store.return_value = None
        mock_module = MagicMock()
        mock_module.MemoryCoordinator.return_value = mock_coordinator

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.memory": MagicMock(),
            "amplihack.memory.coordinator": mock_module,
        }):
            result, code = session_stop.store_memory("s1", "/tmp/t.jsonl")

        self.assertTrue(result["stored"])
        self.assertEqual(result["memories_count"], 0)
        self.assertEqual(code, 0)

    def test_main_calls_store(self):
        """main() calls store_memory with correct args from JSON input."""
        import session_stop  # type: ignore[import]

        with patch.object(
            session_stop, "store_memory",
            return_value=({"stored": True, "memories_count": 0}, 0),
        ) as mock_store:
            with patch("sys.stdin", _stdin_from({
                "session_id": "abc",
                "transcript_path": "/t.jsonl",
            })):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = session_stop.main()

        mock_store.assert_called_once_with("abc", "/t.jsonl")
        self.assertEqual(code, 0)


# ---------------------------------------------------------------------------
# Tests: user_prompt_submit.py
# ---------------------------------------------------------------------------

class TestUserPromptSubmitBridge(unittest.TestCase):
    """Tests for bridge/user_prompt_submit.py — UserPromptSubmitHook.process()."""

    def test_invalid_json_returns_failure(self):
        """Bridge must return {injected_context: ''} when stdin is not valid JSON."""
        script = _BRIDGE_DIR / "user_prompt_submit.py"
        result = subprocess.run(
            [sys.executable, str(script)],
            input=b"not-json",
            capture_output=True,
            timeout=10,
        )
        output = json.loads(result.stdout.decode())
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output.get("injected_context", ""), "")
        self.assertIn("error", output)

    def test_inject_import_error(self):
        """Bridge returns {injected_context: '', error: ...} when amplihack not installed."""
        import user_prompt_submit  # type: ignore[import]

        with patch.dict("sys.modules", {
            "amplihack": None,
            "amplihack.memory": None,
            "amplihack.memory.coordinator": None,
        }):
            result, code = user_prompt_submit.inject_memory("s1", "hello")

        self.assertEqual(result["injected_context"], "")
        self.assertIn("error", result)
        self.assertEqual(code, 1)

    def test_inject_success(self):
        """Bridge returns {injected_context: str, memory_keys_used: []} on success."""
        import user_prompt_submit  # type: ignore[import]

        mock_coordinator = MagicMock()
        mock_coordinator.inject_memory_for_agents_sync.return_value = "Relevant context."
        mock_module = MagicMock()
        mock_module.MemoryCoordinator.return_value = mock_coordinator

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.memory": MagicMock(),
            "amplihack.memory.coordinator": mock_module,
        }):
            result, code = user_prompt_submit.inject_memory("s1", "my prompt")

        self.assertEqual(result["injected_context"], "Relevant context.")
        self.assertEqual(result["memory_keys_used"], [])
        self.assertEqual(code, 0)

    def test_empty_context_is_success(self):
        """Bridge returns exit 0 even when injected_context is empty string."""
        import user_prompt_submit  # type: ignore[import]

        mock_coordinator = MagicMock()
        mock_coordinator.inject_memory_for_agents_sync.return_value = None
        mock_module = MagicMock()
        mock_module.MemoryCoordinator.return_value = mock_coordinator

        with patch.dict("sys.modules", {
            "amplihack": MagicMock(),
            "amplihack.memory": MagicMock(),
            "amplihack.memory.coordinator": mock_module,
        }):
            result, code = user_prompt_submit.inject_memory("s1", "prompt")

        self.assertEqual(result["injected_context"], "")
        self.assertEqual(code, 0)

    def test_main_calls_inject(self):
        """main() calls inject_memory with correct args from JSON input."""
        import user_prompt_submit  # type: ignore[import]

        with patch.object(
            user_prompt_submit, "inject_memory",
            return_value=({"injected_context": "ctx", "memory_keys_used": []}, 0),
        ) as mock_inject:
            with patch("sys.stdin", _stdin_from({
                "session_id": "sid",
                "prompt": "what should I do?",
            })):
                with patch("sys.stdout", new_callable=io.StringIO) as mock_out:
                    code = user_prompt_submit.main()

        mock_inject.assert_called_once_with("sid", "what should I do?")
        self.assertEqual(code, 0)


# ---------------------------------------------------------------------------
# Contract validation tests
# ---------------------------------------------------------------------------

class TestContractCompleteness(unittest.TestCase):
    """Validate that every bridge script has the required contract docstring."""

    BRIDGE_SCRIPTS = [
        "reflection.py",
        "session_start.py",
        "session_stop.py",
        "user_prompt_submit.py",
    ]

    def test_each_script_has_contract_docstring(self):
        """Every bridge script must have a machine-readable IPC CONTRACT section."""
        for name in self.BRIDGE_SCRIPTS:
            path = _BRIDGE_DIR / name
            content = path.read_text()
            self.assertIn("IPC CONTRACT", content, f"{name} missing IPC CONTRACT")
            self.assertIn("input_schema", content, f"{name} missing input_schema")
            self.assertIn("output_schema", content, f"{name} missing output_schema")
            self.assertIn("timeout_seconds", content, f"{name} missing timeout_seconds")

    def test_each_script_has_main_function(self):
        """Every bridge script must have a main() function."""
        for name in self.BRIDGE_SCRIPTS:
            path = _BRIDGE_DIR / name
            content = path.read_text()
            self.assertIn("def main(", content, f"{name} missing main()")
            self.assertIn('if __name__ == "__main__"', content, f"{name} missing __main__ guard")

    def test_each_script_is_valid_python(self):
        """Every bridge script must parse as valid Python 3."""
        import ast
        for name in self.BRIDGE_SCRIPTS:
            path = _BRIDGE_DIR / name
            source = path.read_text()
            try:
                ast.parse(source)
            except SyntaxError as e:
                self.fail(f"{name} has syntax error: {e}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
