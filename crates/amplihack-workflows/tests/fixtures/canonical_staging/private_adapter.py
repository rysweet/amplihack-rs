#!/usr/bin/env python3
"""Private launcher protocol double. Never executes prompts or stages files."""
import json
import os
from pathlib import Path
import sys

assert sys.argv[1:4] == ["codex", "--", "exec"], sys.argv[1:4]
assert "--output-last-message" in sys.argv
sys.stdin.read()  # Consume Codex's actual stdin transport, treating it as data.
cwd = Path.cwd().resolve()
if os.environ["AMPLIHACK_STAGING_NOOP"] != "1":
    for path, body in {
        ".amplihack/indexing.pid": "4242\n",
        ".claude/runtime/sessions.jsonl": '{"fixture":true}\n',
        ".amplihack/tracked.json": '{"modified":true}\n',
    }.items():
        dest = cwd / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(body)
    if os.environ["AMPLIHACK_STAGING_MUTATE"] == "1":
        (cwd / "source.txt").write_text("deferred source modification\n")
        (cwd / "new source.txt").write_text("deferred new source\n")
response = json.dumps({"status": "OK", "fixture": True})
output = Path(sys.argv[sys.argv.index("--output-last-message") + 1])
output.write_text(response)
trace_path = Path(os.environ["AMPLIHACK_STAGING_TRACE"])
previous = json.loads(trace_path.read_text()) if trace_path.exists() else {}
executions = previous.get("executions", [])
executions.append({"cwd": str(cwd), "completed": True})
trace_path.write_text(json.dumps({
    "completed": True, "cwd": str(cwd), "output_written": True,
    "noop": os.environ["AMPLIHACK_STAGING_NOOP"] == "1",
    "mutation": os.environ["AMPLIHACK_STAGING_MUTATE"] == "1",
    "executions": executions,
}))
print(response)
