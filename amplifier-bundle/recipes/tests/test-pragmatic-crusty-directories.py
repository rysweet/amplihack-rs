#!/usr/bin/env python3
"""Crusty boundary checks with an unrelated caller and a linked target.

This supplements the shared-repository fixture. It uses a second raw-index
oracle, opens the resolved index after execution and reads it before Git
diagnostics. Directory materialisation remains an explicit projection.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("staging_fixture", Path(__file__).with_name("test-pragmatic-auto-stage.py"))
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def raw(path):
    try:
        with open(path, "rb", buffering=0) as stream:
            chunks = []
            while True:
                chunk = stream.read(8192)
                if not chunk:
                    return b"".join(chunks)
                chunks.append(chunk)
    except FileNotFoundError:
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("baseline", "candidate", "report", "runner", "tmpdir", "target"):
        parser.add_argument("--" + flag, required=True, type=Path)
    parser.add_argument("--requirements-file", required=True, type=Path)
    args = parser.parse_args()
    fixture = support.Fixtures(args)
    results = []
    support.save(fixture.report / "planned-cases.json", {"steps": ["step-02-crusty-review", "step-04-address-concerns"], "variants": ["red", "green", "removal"]})
    for sid in ("step-02-crusty-review", "step-04-address-concerns"):
        for variant in ("red", "green", "removal"):
            case = fixture.report / (sid + "--" + variant)
            case.mkdir()
            caller, main, target = (case / name for name in ("unrelated caller", "target main", "target linked"))
            fixture.seed(caller)
            fixture.seed(main)
            fixture.git(main, "worktree", "add", "-b", "target", str(target))
            repos = [caller, main, target]
            for repo in repos:
                fixture.partial(repo)
            contexts = {"review_dir": str(main), "repo_path": str(target), "crusty_verdict": {"crusty_verdict": "CONCERNS", "concern_count": 1}, "crusty_round_context": {"pr": "1", "head_sha": "fixture", "resolved_concerns": ""}}
            asset = (args.baseline if variant == "red" else args.candidate) / "autodrive-crusty-round.yaml"
            text = asset.read_text()
            block = support.blocks(text)[sid]
            if variant == "removal":
                assert block.count("    auto_stage: false\n") == 1
                block = block.replace("    auto_stage: false\n", "", 1)
            execution_dir = main if sid == "step-02-crusty-review" else target
            template = "{{review_dir}}" if sid == "step-02-crusty-review" else "{{repo_path}}"
            block = block.replace("    working_dir: " + json.dumps(template), "    working_dir: " + json.dumps(str(execution_dir)))
            recipe = text.split("steps:\n", 1)[0] + "steps:\n" + block
            recipe = fixture.carry_requirements(recipe)
            (case / "recipe.yaml").write_text(recipe)
            mode = "readonly" if sid == "step-02-crusty-review" else "mutation"
            env = dict(fixture.env, V2_MODE=mode, V2_ADAPTER_RECEIPT=str(case / "adapter.jsonl"))
            cmd = [str(fixture.runner), str(case / "recipe.yaml"), "-C", str(caller), "--agent-binary", "claude", "--output-format", "json"]
            for key, value in contexts.items():
                cmd += ["-c", key + "=" + (json.dumps(value) if isinstance(value, dict) else value)]
            paths = [Path(os.fsdecode(fixture.git(repo, "rev-parse", "--path-format=absolute", "--git-path", "index")).strip()) for repo in repos]
            entries_before = [fixture.git(repo, "ls-files", "--stage", "-z") for repo in repos]
            before = [raw(path) for path in paths]
            proc = subprocess.run(cmd, cwd=caller, env=env, capture_output=True)
            after = [raw(path) for path in paths]
            for i in range(3):
                (case / (str(i) + ".before.index")).write_bytes(before[i])
                (case / (str(i) + ".after.index")).write_bytes(after[i])
            (case / "stdout").write_bytes(proc.stdout)
            (case / "stderr").write_bytes(proc.stderr)
            adapters = [json.loads(line) for line in (case / "adapter.jsonl").read_text().splitlines()]
            execution = proc.returncode == 0 and json.loads(proc.stdout)["success"] and len(adapters) == 1 and adapters[0]["success"] and adapters[0]["cwd"] == str(execution_dir)
            byte_equal = before == after
            entries_equal = entries_before == [fixture.git(repo, "ls-files", "--stage", "-z") for repo in repos]
            metadata_visible = all((execution_dir / p).is_file() for p in support.METADATA)
            source_visible = mode != "mutation" or (execution_dir / "source.txt").read_text() == "deferred source change\n"
            untracked = {os.fsdecode(p) for p in fixture.git(execution_dir, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0") if p}
            accepted = execution and metadata_visible and source_visible and (byte_equal and entries_equal and set(support.METADATA) <= untracked if variant == "green" else not byte_equal and not entries_equal and not set(support.METADATA) <= untracked)
            record = {"case_id": case.name, "accepted": accepted, "exit_status": proc.returncode, "adapter": adapters, "command": cmd, "index_paths": [str(p) for p in paths], "index_bytes_equal": byte_equal, "staged_blobs_and_modes_equal": entries_equal, "metadata_visible": metadata_visible, "source_visible": source_visible, "metadata_untracked": set(support.METADATA) <= untracked, "source_asset_sha256": hashlib.sha256(text.encode()).hexdigest(), "projected_recipe_sha256": hashlib.sha256(recipe.encode()).hexdigest(), "projection_substitutions": {template: str(execution_dir)}, "caller_common_dir": os.fsdecode(fixture.git(caller, "rev-parse", "--git-common-dir")).strip(), "target_common_dir": os.fsdecode(fixture.git(target, "rev-parse", "--path-format=absolute", "--git-common-dir")).strip()}
            support.save(case / "receipt.json", record)
            results.append(record)
            support.save(fixture.report / "results.json", results)
            print(case.name + ": " + str(accepted), flush=True)
    return 0 if len(results) == 6 and all(r["accepted"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
