#!/usr/bin/env python3
"""Actual-runner staging regressions; no models, credentials or new dependencies.

Run with --baseline and --candidate recipe directories, --report (a NEW directory),
and --runner (the actual recipe-runner-rs executable). Reports retain all failures.
This intentionally accepts only the canonical, quoted step layout. The runner,
not this fixture, parses and executes YAML and decides whether to stage changes.
"""

import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


ASSETS = (
    "autodrive-crusty-round", "workflow-prep", "workflow-design", "workflow-tdd",
    "workflow-refactor-review", "workflow-precommit-test", "workflow-publish",
    "workflow-pr-review", "workflow-finalize",
)
USER = "user partially staged\tfile.txt"
METADATA = (".amplihack/new\nnote.json", ".claude/runtime/new.json", "indexing.pid")
TRACKED_RUNTIME = ".amplihack/tracked.json"
MUTATING = {
    "step-05-architecture", "step-06-documentation", "step-06c-documentation-refinement",
    "step-07-write-tests", "step-08-implement", "step-08b-integration", "step-09-refactor",
    "step-09b-optimize", "step-11b-implement-feedback", "step-12-run-precommit",
    "step-13-local-testing", "step-14-bump-version", "step-16b-outside-in-fix-loop",
    "step-17e-address-blocking-issues", "step-18b-implement-feedback",
    "step-20-final-cleanup", "step-20c-quality-audit", "step-22-ensure-mergeable",
    "step-04-address-concerns",
}
ADAPTER = '''#!/usr/bin/python3
import json, os, pathlib, sys
if sys.argv[1:2] != ["claude"]:
    sys.exit(96)
root = pathlib.Path.cwd()
if os.environ["V2_MODE"] != "noop":
    for name in json.loads(os.environ["V2_METADATA"]):
        p = root / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("visible fixture metadata\\n")
    (root / ".amplihack/tracked.json").write_text("modified runtime\\n")
    if os.environ["V2_MODE"] == "mutation":
        (root / "source.txt").write_text("deferred source change\\n")
record = {"cwd": str(root.resolve()), "success": True,
          "mode": os.environ["V2_MODE"], "boundary": sys.argv[1:4]}
with open(os.environ["V2_ADAPTER_RECEIPT"], "a") as receipt:
    receipt.write(json.dumps(record) + "\\n")
print(json.dumps({"status": "OK", "fixture": True}))
'''


def digest(data):
    return hashlib.sha256(data).hexdigest()


def binary_digest(path):
    path = Path(path).resolve()
    return digest(path.read_bytes()) if path.is_relative_to("/Users/ryan/src/bb") else None


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def parse_yaml(text):
    result = subprocess.run(["ruby", "-e", 'require "yaml"; require "json"; puts JSON.generate(YAML.load(STDIN.read))'],
                            input=text.encode(), capture_output=True)
    if result.returncode:
        raise ValueError("projection YAML: " + result.stderr.decode(errors="replace"))
    return json.loads(result.stdout)


def append_requirements(recipe, requirements):
    """Preserve each literal scalar's own indentation, including eight-space prompts."""
    original = parse_yaml(recipe)
    lines = recipe.splitlines(keepends=True)
    for start in reversed([i for i, line in enumerate(lines) if re.fullmatch(r"    prompt: \|\n", line)]):
        end = start + 1
        while end < len(lines) and (not lines[end].strip() or len(lines[end]) - len(lines[end].lstrip(" ")) > 4):
            end += 1
        body = lines[start + 1:end]
        indent = min(len(line) - len(line.lstrip(" ")) for line in body if line.strip())
        addition = "\n" + " " * indent + "Complete supplied requirements, unchanged:\n"
        addition += "".join(" " * indent + line + "\n" for line in requirements.splitlines())
        lines[start + 1:end] = ["".join(body).rstrip("\n") + "\n" + addition]
    projected = "".join(lines)
    parsed = parse_yaml(projected)
    expected = json.loads(json.dumps(original))
    for step in expected["steps"]:
        if "prompt" in step:
            step["prompt"] += "\nComplete supplied requirements, unchanged:\n" + requirements
    if parsed != expected:
        raise ValueError("Projection changed fields beyond the exact requirements addition")
    return projected


def blocks(text):
    """Slice exact canonical blocks, refusing duplicate IDs or unknown layout."""
    starts = list(re.finditer(r'^  - id: "([^"\n]+)"\n', text, re.M))
    if len(starts) != len(re.findall(r'^  - id:', text, re.M)):
        raise ValueError("Unsupported canonical step layout")
    end = re.search(r'^output:', text, re.M)
    result = {}
    for n, start in enumerate(starts):
        stop = starts[n + 1].start() if n + 1 < len(starts) else (end.start() if end else len(text))
        if start[1] in result:
            raise ValueError("Duplicate step ID")
        result[start[1]] = text[start.start():stop]
    return result


def scalar(block, key):
    match = re.search(r'^    ' + re.escape(key) + r': (.+)$', block, re.M)
    if match is None:
        return None
    return json.loads(match[1])


class Fixtures:
    def __init__(self, args):
        self.args = args
        self.requirements_bytes = args.requirements_file.read_bytes()
        self.requirements = self.requirements_bytes.decode("utf-8")
        if not self.requirements.strip() or not self.requirements.endswith("\n"):
            raise ValueError("Requirements must be nonempty UTF-8 with a final newline")
        self.report = args.report.resolve()
        self.report.mkdir(parents=True, exist_ok=False)
        self.bin = self.report / "bin"
        self.bin.mkdir()
        (self.report / "requirements-unchanged.json").write_bytes(self.requirements_bytes)
        adapter = self.bin / "amplihack"
        adapter.write_text(ADAPTER)
        adapter.chmod(0o755)
        home = self.report / "home"
        home.mkdir()
        # An explicit clean environment excludes inherited Git selectors and credentials.
        self.env = {
            "HOME": str(home), "PATH": str(self.bin) + ":/usr/bin:/bin",
            "TMPDIR": str(args.tmpdir.resolve()), "LC_ALL": "C", "RUST_LOG": "warn",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_TERMINAL_PROMPT": "0", "V2_METADATA": json.dumps(METADATA),
            "CARGO_TARGET_DIR": str(args.target.resolve()), "CARGO_BUILD_JOBS": "2",
        }
        args.tmpdir.mkdir(parents=True, exist_ok=True)
        self.runner = args.runner.resolve()
        self.receipts = []
        save(self.report / "identity.json", {
            "runner_path": str(self.runner), "runner_sha256": binary_digest(self.runner),
            "requirements_sha256": digest(self.requirements.encode()),
            "runner_version": self.run([str(self.runner), "--version"]).stdout.decode(),
            "test_sha256": digest(Path(__file__).read_bytes()),
            "platform": sys.platform, "nice": os.getpriority(os.PRIO_PROCESS, 0),
            "environment": self.env, "global_auto_stage": True,
        })

    def carry_requirements(self, recipe):
        """Append the complete task contract to every projected agent prompt."""
        return append_requirements(recipe, self.requirements)

    def run(self, cmd, cwd=None, check=True, env=None):
        started = time.perf_counter()
        result = subprocess.run(cmd, cwd=cwd, env=env or self.env, capture_output=True)
        result.elapsed_seconds = time.perf_counter() - started
        if check and result.returncode:
            raise RuntimeError((cmd, result.returncode, result.stderr.decode(errors="replace")))
        return result

    def git(self, repo, *args):
        return self.run(["git", "-C", str(repo), "--literal-pathspecs", *args]).stdout

    def seed(self, path):
        path.mkdir()
        self.git(path, "init", "-b", "fixture")
        for key, value in (("user.name", "Staging Fixture"), ("user.email", "fixture@example.invalid"),
                           ("core.filemode", "true"), ("commit.gpgsign", "false")):
            self.git(path, "config", "--local", key, value)
        for name in (USER, "source.txt", "delete me.txt", "rename\nold.txt", TRACKED_RUNTIME):
            dest = path / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text("baseline\n")
        (path / "symlink").symlink_to("source.txt")
        self.git(path, "add", "--all")
        self.git(path, "commit", "-m", "fixture baseline")

    def partial(self, path):
        (path / USER).write_text("user staged content\n")
        (path / USER).chmod(0o755)
        self.git(path, "add", "--", USER)
        (path / USER).write_text("user unstaged content\n")
        (path / USER).chmod(0o644)
        self.git(path, "rm", "--", "delete me.txt")
        self.git(path, "mv", "--", "rename\nold.txt", ":(glob)* renamed\tfile.txt")

    def snapshot_paths(self, repos):
        return {str(repo): Path(os.fsdecode(self.git(repo, "rev-parse", "--path-format=absolute", "--git-path", "index")).strip()) for repo in repos}

    @staticmethod
    def raw(paths):
        # No Git subprocess may run between agent return and these reads.
        return {repo: path.read_bytes() if path.exists() else None for repo, path in paths.items()}

    def agent_case(self, asset, ids, variant, mode="mutation", shared=False):
        started = time.perf_counter()
        name = asset + "--" + "+".join(ids) + "--" + variant + ("--shared" if shared else "")
        case = self.report / name
        case.mkdir()
        receipt = {"case_id": name, "planned_variant": variant, "status": "attempted"}
        self.receipts.append(receipt)
        try:
            caller, review, target = (case / p for p in ("caller", "review", "task linked"))
            self.seed(caller)
            self.seed(review)
            self.git(caller, "worktree", "add", "-b", "task", str(target))
            if shared:
                review = caller
            repos = list(dict.fromkeys((caller, review, target)))
            for repo in repos:
                self.partial(repo)
            baseline = (self.args.baseline / (asset + ".yaml")).read_text()
            candidate = (self.args.candidate / (asset + ".yaml")).read_text()
            original = baseline if variant == "red" else candidate
            selected = blocks(original)
            step_text = "".join(selected[sid] for sid in ids)
            if variant == "removal":
                assert len(ids) == 1 and step_text.count("    auto_stage: false\n") == 1
                step_text = step_text.replace("    auto_stage: false\n", "", 1)
            prefix = original.split("steps:\n", 1)[0]
            recipe = prefix + "steps:\n" + step_text
            projection = case / "recipe.yaml"
            context = {
                "repo_path": str(target), "review_dir": str(review),
                "worktree_setup": {"worktree_path": str(target)},
                "clarified_requirements": {"requires_integration_work": "true", "requires_api_design": "true", "requires_database_design": "true"},
                "resume_checkpoint": "", "goal_already_met": "false",
                "terminal_state": {"terminal_success": "false", "should_publish": "true", "should_finalize": "true", "should_run_ci_wait": "true"},
                "crusty_verdict": {"crusty_verdict": "CONCERNS", "concern_count": 1},
                "crusty_round_context": {"pr": "1", "head_sha": "fixture", "resolved_concerns": ""},
            }
            materialised = {}
            if self.args.materialise_working_dir:
                for template, directory in (("{{review_dir}}", review), ("{{repo_path}}", target), ("{{worktree_setup.worktree_path}}", target)):
                    field = "    working_dir: " + json.dumps(template)
                    if field in recipe:
                        recipe = recipe.replace(field, "    working_dir: " + json.dumps(str(directory)))
                        materialised[template] = str(directory)
            recipe = self.carry_requirements(recipe)
            projection.write_text(recipe)
            env = dict(self.env, V2_MODE=mode, V2_ADAPTER_RECEIPT=str(case / "adapter.jsonl"))
            command = [str(self.runner), str(projection), "-C", str(caller), "--agent-binary", "claude", "--output-format", "json"]
            for key, value in context.items():
                command += ["-c", key + "=" + (json.dumps(value) if isinstance(value, dict) else value)]
            paths = self.snapshot_paths(repos)
            entries = {str(repo): self.git(repo, "ls-files", "--stage", "-z") for repo in repos}
            blobs = {str(repo): self.git(repo, "show", ":" + USER) for repo in repos}
            before = self.raw(paths)
            receipt.update({"command": command, "working_directory": str(caller),
                            "source_hash": digest(original.encode()), "baseline_hash": digest(baseline.encode()),
                            "candidate_hash": digest(candidate.encode()), "projection_hash": digest(recipe.encode()),
                            "execution_state": "running"})
            save(case / "receipt.json", receipt)
            receipt["fixture_seconds"] = time.perf_counter() - started
            result = self.run(command, cwd=caller, env=env, check=False)
            after = self.raw(paths)
            (case / "stdout").write_bytes(result.stdout)
            (case / "stderr").write_bytes(result.stderr)
            receipt.update({"execution_state": "returned", "exit_status": result.returncode,
                            "runner_seconds": result.elapsed_seconds,
                            "stdout_sha256": digest(result.stdout), "stderr_sha256": digest(result.stderr)})
            save(case / "receipt.json", receipt)
            # Raw snapshots are retained before any refresh-capable diagnostics.
            for n, repo in enumerate(repos):
                for phase, data in (("before", before), ("after", after)):
                    content = data[str(repo)]
                    if content is not None:
                        (case / (str(n) + ".index." + phase)).write_bytes(content)
            (case / "stdout").write_bytes(result.stdout)
            (case / "stderr").write_bytes(result.stderr)
            adapter = [json.loads(line) for line in (case / "adapter.jsonl").read_text().splitlines()] if (case / "adapter.jsonl").exists() else []
            try:
                outcome = json.loads(result.stdout)
            except (ValueError, UnicodeError) as error:
                receipt["failure_phase"] = "output_parsing"
                raise ValueError("Runner output parsing failed") from error
            expected_dirs = []
            for sid in ids:
                wd = scalar(selected[sid], "working_dir")
                expected_dirs.append(str(review if wd == "{{review_dir}}" else target if wd in ("{{repo_path}}", "{{worktree_setup.worktree_path}}") else caller))
            execution_ok = result.returncode == 0 and outcome["success"] and len(adapter) == len(ids) and [r["cwd"] for r in adapter] == expected_dirs and all(r["success"] for r in adapter)
            assertions = {"adapter_execution": execution_ok, "index_bytes": all(before[k] == after[k] for k in paths)}
            assertions["staged_entries_and_modes"] = all(entries[str(repo)] == self.git(repo, "ls-files", "--stage", "-z") for repo in repos)
            assertions["staged_user_blob"] = all(blobs[str(repo)] == self.git(repo, "show", ":" + USER) for repo in repos)
            assertions["metadata_visible"] = mode == "noop" or all((Path(repo) / p).is_file() for repo in expected_dirs for p in METADATA)
            assertions["mutation_source_visible"] = mode != "mutation" or all((Path(repo) / "source.txt").read_text() == "deferred source change\n" for repo in expected_dirs)
            assertions["metadata_untracked"] = mode == "noop" or all(set(os.fsdecode(x) for x in self.git(Path(repo), "ls-files", "--others", "--exclude-standard", "-z", "--", *METADATA).split(b"\0") if x) == set(METADATA) for repo in expected_dirs)
            assertions["deferred_changes_unstaged"] = mode == "noop" or all(TRACKED_RUNTIME in set(os.fsdecode(x) for x in self.git(Path(repo), "diff", "--name-only", "-z").split(b"\0") if x) and (mode != "mutation" or "source.txt" in set(os.fsdecode(x) for x in self.git(Path(repo), "diff", "--name-only", "-z").split(b"\0") if x)) for repo in expected_dirs)
            if variant in ("red", "removal"):
                accepted = execution_ok and assertions["metadata_visible"] and assertions["mutation_source_visible"] and not assertions["index_bytes"] and not assertions["staged_entries_and_modes"] and not assertions["metadata_untracked"]
                terminal = "valid intended integrity failure" if accepted else "invalid control"
            else:
                accepted = all(assertions.values())
                terminal = "passed" if accepted else "failed integrity acceptance"
            receipt.update({"status": "completed", "accepted": accepted, "terminal_outcome": terminal,
                            "assertions": assertions, "exit_status": result.returncode, "command": command,
                            "source_hash": digest(original.encode()), "baseline_hash": digest(baseline.encode()),
                            "candidate_hash": digest(candidate.encode()), "projection_hash": digest(recipe.encode()),
                            "resolved_index_paths": {k: str(v) for k, v in paths.items()},
                            "index_presence": {k: {"before": before[k] is not None, "after": after[k] is not None} for k in paths},
                            "staged_entries_before_base64": {k: base64.b64encode(v).decode() for k, v in entries.items()},
                            "adapter": adapter, "projection_substitutions": {"context": context, "omitted_steps": [sid for sid in selected if sid not in ids], "original_prompt_and_condition": "preserved; complete supplied requirements appended to prompts", "requirements_sha256": digest(self.requirements.encode()), "materialised_working_dir": materialised, "directory_limitation": "The pinned runner stages with the raw working_dir template; materialisation changes that behaviour. Native controls must be assessed separately.", "removed_auto_stage": variant == "removal", "global_auto_stage": "enabled"}})
        except Exception as error:
            receipt.update({"accepted": False, "terminal_outcome": "invalid control", "error": repr(error),
                            "failure_phase": receipt.get("failure_phase", "execution_or_setup")})
        receipt["finalized"] = True
        receipt["total_seconds"] = time.perf_counter() - started
        receipt["evidence_seconds"] = max(0, receipt["total_seconds"] - receipt.get("fixture_seconds", 0) - receipt.get("runner_seconds", 0))
        save(case / "receipt.json", receipt)
        save(self.report / "results.json", self.receipts)
        print(name + ": " + receipt["terminal_outcome"], flush=True)

    def agents(self):
        from canonical_staging_checkpoint_audit import ADDITIONAL_CONTROLS, AUXILIARY_EXTRACTIONS, CHECKPOINTS, without_checkpoint_delta, restored_early_sync_asset_text
        population = []
        for asset in ASSETS:
            baseline = (self.args.baseline / (asset + ".yaml")).read_text()
            candidate = (self.args.candidate / (asset + ".yaml")).read_text()
            # Parsed preservation checks the actual candidate below. Raw-block
            # comparison restores only the exact authorised early call/comment.
            original, fixed = blocks(baseline), blocks(restored_early_sync_asset_text(asset, candidate))
            previous, current = parse_yaml(baseline), parse_yaml(candidate)
            expected = copy.deepcopy(previous)
            for step in expected["steps"]:
                if "agent" in step:
                    step["auto_stage"] = False
            if without_checkpoint_delta(asset, current, previous) != expected:
                raise ValueError((asset, "unapproved phase change or missing original opt-out"))
            # The normaliser validates the two exact additions and the complete
            # extraction. The original population remains independently counted.
            fixed = {sid: block for sid, block in fixed.items() if sid in original}
            if original.keys() != fixed.keys():
                raise ValueError("Step membership changed")
            for sid, block in original.items():
                if re.search(r'^    agent:', block, re.M):
                    if scalar(fixed[sid], "auto_stage") is not False:
                        raise ValueError((asset, sid, "missing opt-out"))
                    if fixed[sid].replace("    auto_stage: false\n", "", 1) != block:
                        raise ValueError((asset, sid, "non-policy change"))
                    population.append((asset, sid))
                elif asset in CHECKPOINTS and sid == CHECKPOINTS[asset][0]:
                    continue  # The complete extraction was validated above.
                elif asset in AUXILIARY_EXTRACTIONS and sid == AUXILIARY_EXTRACTIONS[asset][0]:
                    # The exact wrapper, complete original command and all
                    # other step fields were independently validated above.
                    continue
                elif fixed[sid] != block:
                    raise ValueError((asset, sid, "checkpoint changed"))
        # Count is an independent expectation; membership is discovered from assets.
        if len(population) != 40:
            raise ValueError(("Reconcile changed inventory", len(population)))
        save(self.report / "planned-cases.json", {"population": population, "variants": ["red", "green", "removal"], "extra": ["disabled-noop", "crusty-shared-review", "crusty-shared-fixer", "tdd-sequence"], "additional_controls": ADDITIONAL_CONTROLS})
        self.agent_case("autodrive-crusty-round", ["step-02-crusty-review"], "disabled-noop", "noop")
        for asset, sid in population:
            mode = "mutation" if sid in MUTATING else "readonly"
            for variant in ("red", "green", "removal"):
                self.agent_case(asset, [sid], variant, mode)
        self.agent_case("autodrive-crusty-round", ["step-02-crusty-review"], "green", "readonly", True)
        self.agent_case("autodrive-crusty-round", ["step-04-address-concerns"], "green", "mutation", True)
        self.agent_case("workflow-tdd", ["step-07-write-tests", "step-08-implement", "step-08c-work-verifier", "step-08b-integration"], "sequence")
        expected = len(population) * 3 + 4
        complete = len(self.receipts) == expected and all(r.get("accepted") for r in self.receipts)
        save(self.report / "acceptance.json", {"bounded_regressions": "passed" if complete else "failed", "expected": expected, "observed": len(self.receipts), "delivery": "not established", "issues": ["Refs #1554", "Refs #1914"], "unsupported": ["absent, alternate, split, sparse and unmerged indexes", "concurrent Git writers", "interrupted adapters", "non-Unix executable and symlink semantics"], "remaining_gates": ["independent inventory reconciliation", "checkpoint publication safety", "critical consensus branches and review", "exact final-candidate review", "resolved native/proof qualification procedure", "natural full QA and normal hooks", "required CI", "supported-platform actual auto-drive", "separately authorised owner integration"]})
        return complete


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("baseline", "candidate", "report", "runner", "tmpdir", "target"):
        parser.add_argument("--" + flag, required=True, type=Path)
    parser.add_argument("--requirements-file", required=True, type=Path)
    parser.add_argument("--materialise-working-dir", action="store_true", help="Record and materialise directory templates; this is projected evidence, not native-template acceptance")
    args = parser.parse_args()
    fixtures = Fixtures(args)
    return 0 if fixtures.agents() else 1


if __name__ == "__main__":
    sys.exit(main())
