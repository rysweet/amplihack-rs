#!/usr/bin/env python3
"""Independent, executable index checks for Refs #1554 and Refs #1914.

Run with --report DIRECTORY --runner /absolute/path/to/recipe-runner-rs.
Python and Ruby's standard YAML library are sufficient. No model is invoked.
Evidence is retained; a failed acceptance check is never converted to a pass.
"""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from canonical_staging_publication import commit_objects, publication_history

ROOT = Path(__file__).resolve().parents[3]
ASSETS = ROOT / "amplifier-bundle/recipes"
PHASES = (
    "workflow-prep", "workflow-design", "workflow-tdd", "workflow-refactor-review",
    "workflow-precommit-test", "workflow-publish", "workflow-pr-review",
    "workflow-finalize", "autodrive-crusty-round",
)
NESTED = "workflow-terminal-adjudication"
BASELINE = "606f3c746e51b8ef66d75380b41791b9f3a9ed8e"
MUTATORS = {
    "step-05-architecture", "step-06-documentation", "step-06c-documentation-refinement",
    "step-07-write-tests", "step-08-implement", "step-08b-integration",
    "step-09-refactor", "step-09b-optimize", "step-11b-implement-feedback",
    "step-12-run-precommit", "step-13-local-testing", "step-14-bump-version",
    "step-16b-outside-in-fix-loop", "step-17e-address-blocking-issues",
    "step-18b-implement-feedback", "step-20-final-cleanup", "step-20c-quality-audit",
    "step-22-ensure-mergeable", "step-04-address-concerns",
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def executable_identity(path):
    """Identify tools without reading outside the authorised supporting root."""
    path = Path(path).resolve()
    version = subprocess.run([str(path), "--version"], capture_output=True, check=True)
    return {"path": str(path), "version": version.stdout.decode(errors="replace").strip(),
            "sha256": digest(path.read_bytes()) if path.is_relative_to("/Users/ryan/src/bb") else None,
            "digest_policy": "Binary reads are restricted to /Users/ryan/src/bb; version probes are permitted."}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=True) + "\n")


def run(argv, cwd, env, check=True):
    result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and result.returncode:
        raise RuntimeError(f"{argv!r}: exit {result.returncode}: "
                           + result.stderr.decode(errors="replace"))
    return result


def git(repo, env, *args):
    return run(["git", "--literal-pathspecs", "-C", repo, *args], repo, env).stdout


def load_yaml(raw):
    # Parse the real asset, including quoted scalars, multiline commands and defaults.
    result = subprocess.run(
        ["ruby", "-e", 'require "yaml"; require "json"; puts JSON.generate(YAML.load(STDIN.read))'],
        input=raw, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    return json.loads(result.stdout)


def source(name, baseline=False):
    path = ASSETS / (name + ".yaml")
    if baseline:
        return subprocess.check_output(["git", "show", BASELINE + ":" + str(path.relative_to(ROOT))], cwd=ROOT)
    return path.read_bytes()


def owner(name, step_id):
    if step_id not in MUTATORS:
        return None
    if name in ("workflow-design", "workflow-tdd"):
        return "workflow-tdd/checkpoint-after-implementation"
    if name == "workflow-refactor-review":
        return name + "/checkpoint-after-review-feedback"
    if name == "workflow-precommit-test" or step_id == "step-14-bump-version":
        return "workflow-publish/step-15-commit-push"
    if step_id == "step-16b-outside-in-fix-loop":
        return name + "/step-16b-outside-in-fix-loop (own explicit commit)"
    if name == "workflow-pr-review":
        return name + "/step-18c-push-feedback-changes"
    if step_id in ("step-20-final-cleanup", "step-20c-quality-audit"):
        return name + "/step-20b-push-cleanup"
    if name == "autodrive-crusty-round":
        return name + "/step-04-address-concerns (own explicit commit)"
    return "UNVERIFIED: CI agent illustrates commit/push; no later recipe source checkpoint or verified publication authority"


def inventory(report):
    from canonical_staging_checkpoint_audit import ADDITIONAL_CONTROLS, CHECKPOINTS, without_checkpoint_delta
    rows, recipes, baseline = [], {}, {}
    for name in (*PHASES, NESTED):
        raw, old = source(name), source(name, True)
        current, previous = load_yaml(raw), load_yaml(old)
        recipes[name], baseline[name] = current, previous
        expected = copy.deepcopy(previous)
        for step in expected["steps"]:
            if "agent" in step and name in PHASES:
                step["auto_stage"] = False
        normalised = without_checkpoint_delta(name, current, previous)
        assert normalised == expected, f"Out-of-scope change or missing opt-out: {name}"
        old_steps = {s["id"]: s for s in previous["steps"]}
        for step in current["steps"]:
            if "agent" not in step:
                continue
            sid = step["id"]
            if name in CHECKPOINTS and sid == CHECKPOINTS[name][0] + '-scope-review':
                continue  # Separately counted below; none of the original 40 is removed.
            rows.append({
                "asset": str((ASSETS / (name + ".yaml")).relative_to(ROOT)),
                "asset_hash": digest(raw), "step_id": sid,
                "direct_or_transitive": "transitive" if name == NESTED else "direct",
                "permitted_behaviour": "source mutation" if sid in MUTATORS else "analysis/review output",
                "prompt": step.get("prompt"), "agent": step["agent"],
                "execution_directory": step.get("working_dir", "<runner caller directory>"),
                "condition": step.get("condition"), "context": current.get("context", {}),
                "declared_auto_stage": step.get("auto_stage"),
                "effective_auto_stage": step.get("auto_stage", True),
                "baseline_effective_auto_stage": old_steps[sid].get("auto_stage", True),
                "authorised_edit": name in PHASES,
                "staging_owner": owner(name, sid), "commit_owner": owner(name, sid),
                "directory_limitation": "Caller may differ from checkpoint worktree" if
                    sid in MUTATORS and "working_dir" not in step else None,
            })
    # Reconcile membership from the top-level recipe rather than supplied counts.
    top = load_yaml(source("default-workflow"))
    phases = {s["recipe"] for s in top["steps"] if "recipe" in s}
    assert phases == (set(PHASES) - {"autodrive-crusty-round"}) | {"workflow-worktree"}
    assert not any("agent" in s for s in load_yaml(source("workflow-worktree"))["steps"])
    nested_calls = [(name, s["id"]) for name, doc in recipes.items()
                    for s in doc["steps"] if s.get("recipe") == NESTED]
    assert nested_calls, "Nested terminal adjudication must be accounted for"
    write_json(report / "inventory.json", {"steps": rows, "nested_calls": nested_calls,
               "doc_review_checkpoint_is_source_owner": False,
               "checkpoint_delta_controls": ADDITIONAL_CONTROLS,
               "checkpoint_delta_control_count": 4,
               "original_native_controls_unverified": 25,
               "runner_issue_201_resolved": False})
    return rows, recipes, baseline


ADAPTER = '''#!/usr/bin/env python3
import json, os
from pathlib import Path
p = Path.cwd()
mode = os.environ["MINIMALIST_MODE"]
if mode != "noop":
    (p / ".amplihack").mkdir(exist_ok=True)
    (p / ".amplihack/indexing.pid").write_text("12345\\n")
if mode == "mutation":
    (p / "source.txt").write_text("deferred source change\\n")
    (p / ".claude/runtime/tracked.json").write_text("tracked runtime changed\\n")
with open(os.environ["MINIMALIST_ADAPTER_RECEIPT"], "a") as f:
    f.write(json.dumps({"cwd": str(p.resolve()), "mode": mode, "executed": True}) + "\\n")
print('{"status":"OK","requires_integration_work":"true"}')
'''


def environment(case):
    home = case / "fixture-home"
    home.mkdir()
    # Explicit allowlist: no ambient credentials, Git selectors or shell startup scripts.
    return {"PATH": os.environ["PATH"], "HOME": str(home), "LANG": "C", "LC_ALL": "C",
            "TMPDIR": os.environ["TMPDIR"], "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0",
            "GIT_PAGER": "cat", "PAGER": "cat", "AMPLIHACK_MAX_DEPTH": "8",
            "AMPLIHACK_RATELIMIT_MAX_RETRIES": "0"}


def initialise(repo, env, runtime=True):
    repo.mkdir()
    git(repo, env, "init", "-b", "fixture")
    git(repo, env, "config", "user.name", "Fixture")
    git(repo, env, "config", "user.email", "fixture@example.invalid")
    git(repo, env, "config", "core.filemode", "true")
    git(repo, env, "config", "commit.gpgsign", "false")
    for name in ("user.sh", "source.txt", "deleted.txt", "old name.txt"):
        (repo / name).write_text("base\\n")
    (repo / "user.sh").chmod(0o755)
    if runtime:
        (repo / ".claude/runtime").mkdir(parents=True)
        (repo / ".claude/runtime/tracked.json").write_text("base runtime\n")
    git(repo, env, "add", "-A")
    git(repo, env, "commit", "-m", "fixture base")


def partial(repo, env, edges=False):
    (repo / "user.sh").write_text("user staged contents\n")
    git(repo, env, "add", "--", "user.sh")
    (repo / "user.sh").write_text("user unstaged contents\n")
    (repo / "user.sh").chmod(0o644)
    if edges:
        git(repo, env, "rm", "--", "deleted.txt")
        git(repo, env, "mv", "--", "old name.txt", "renamed\nname.txt")
        for name in ("-leading", "a\tb", ":(glob)*", "unicodé"):
            (repo / name).write_text("staged unusual\n")
            git(repo, env, "add", "--", name)
            (repo / name).write_text("unstaged unusual\n")
        (repo / "link").symlink_to("source.txt")
        git(repo, env, "add", "--", "link")


def setup(case, edges=False, runtime=True):
    env = environment(case)
    caller, review, base, target = (case / n for n in ("caller", "review", "base", "task-worktree"))
    for repo in (caller, review, base):
        initialise(repo, env, runtime)
    git(base, env, "worktree", "add", "-b", "task", target)
    for repo in (caller, review, base, target):
        partial(repo, env, edges)
    return env, {"caller": caller, "review": review, "base": base, "target": target}


def snapshot(repos, env, case):
    before = {}
    for label, repo in repos.items():
        index = Path(git(repo, env, "rev-parse", "--path-format=absolute", "--git-path", "index").decode().strip())
        entries = git(repo, env, "ls-files", "--stage", "-z")
        raw = index.read_bytes() if index.exists() else None
        assert raw is not None
        (case / (label + ".before.index")).write_bytes(raw)
        (case / (label + ".before.entries")).write_bytes(entries)
        before[label] = (index, raw, entries)
    return before


def after_bytes(before, case):
    # This is deliberately the first operation after the runner/checkpoint returns.
    after = {label: index.read_bytes() if index.exists() else None
             for label, (index, _, _) in before.items()}
    for label, raw in after.items():
        if raw is not None:
            (case / (label + ".after.index")).write_bytes(raw)
    return after


def agent_case(args, row, recipe, variant, suffix="", edges=False, noop=False, shared=False):
    case = Path(tempfile.mkdtemp(prefix=variant + "-" + row["step_id"] + suffix + "-", dir=args.report))
    env, repos = setup(case, edges)
    step = copy.deepcopy(next(s for s in recipe["steps"] if s["id"] == row["step_id"]))
    if variant == "removal":
        del step["auto_stage"]
    if noop:
        step["auto_stage"] = False
    context = copy.deepcopy(recipe.get("context", {}))
    context.update(repo_path=str(repos["target"]), review_dir=str(repos["base"] if shared else repos["review"]),
                   worktree_setup={"worktree_path": str(repos["target"])},
                   clarified_requirements={"requires_api_design": "true", "requires_database_design": "true",
                                           "requires_integration_work": "true"},
                   resume_checkpoint="", goal_already_met="false",
                   terminal_state={"terminal_success": "false", "should_finalize": "true",
                                   "should_run_ci_wait": "true", "should_publish": "true"},
                   crusty_verdict={"crusty_verdict": "CONCERNS"})
    wd = step.get("working_dir", "")
    assert wd in ("", "{{review_dir}}", "{{repo_path}}", "{{worktree_setup.worktree_path}}"), wd
    expected = Path(context["review_dir"]) if wd == "{{review_dir}}" else repos["target"] if wd else repos["caller"]
    if args.resolve_directories and wd:
        step["working_dir"] = str(expected)
    # Keep conditions and directory templates intact; replace only the model payload.
    original_prompt = step["prompt"]
    step["prompt"] = "Task-private adapter integrity fixture. No models or credentials."
    if args.requirements_file:
        step["prompt"] += "\n\nComplete supplied requirements, unchanged:\n" + args.requirements_text
    projection = {"name": "minimalist-" + row["step_id"], "version": "1.0.0",
                  "context": context, "steps": [step]}
    for key in ("recursion", "default_step_timeout"):
        if key in recipe:
            projection[key] = recipe[key]
    recipe_file = case / "projected.yaml"
    write_json(recipe_file, projection)  # JSON is a YAML subset consumed by the actual runner.
    adapter = case / "private-launcher"
    adapter.write_text(ADAPTER)
    adapter.chmod(0o755)
    env.update(AMPLIHACK_LAUNCHER_BINARY=str(adapter), MINIMALIST_ADAPTER_RECEIPT=str(case / "adapter.jsonl"),
               MINIMALIST_MODE="noop" if noop else "mutation" if row["step_id"] in MUTATORS else "review")
    before = snapshot(repos, env, case)
    command = [args.runner, recipe_file, "-C", repos["caller"], "--agent-binary", "claude", "--output-format", "json"]
    result = run(command, repos["caller"], env, False)
    raw_after = after_bytes(before, case)
    (case / "stdout").write_bytes(result.stdout)
    (case / "stderr").write_bytes(result.stderr)
    adapter_rows = [json.loads(s) for s in (case / "adapter.jsonl").read_text().splitlines()] if (case / "adapter.jsonl").exists() else []
    checks = {"runner_success": result.returncode == 0,
              "adapter_executed_at_expected_directory": bool(adapter_rows) and all(r["cwd"] == str(expected.resolve()) for r in adapter_rows),
              "raw_index_bytes_equal": all(raw_after[k] == v[1] for k, v in before.items()),
              "staged_blobs_and_modes_equal": all(git(repos[k], env, "ls-files", "--stage", "-z") == v[2] for k, v in before.items())}
    if not noop:
        metadata = expected / ".amplihack/indexing.pid"
        checks["metadata_created"] = metadata.is_file()
        checks["metadata_visible_and_untracked"] = metadata.is_file() and b".amplihack/indexing.pid\0" in git(expected, env, "ls-files", "--others", "--exclude-standard", "-z")
    if env["MINIMALIST_MODE"] == "mutation":
        checks["source_mutation_created"] = (expected / "source.txt").read_text() == "deferred source change\n"
        checks["tracked_runtime_mutation_created"] = (expected / ".claude/runtime/tracked.json").read_text() == "tracked runtime changed\n"
        checks["deferred_source_and_tracked_runtime_unstaged"] = checks["source_mutation_created"] and all(
            p.encode() + b"\0" in git(expected, env, "diff", "--name-only", "-z")
            for p in ("source.txt", ".claude/runtime/tracked.json"))
    valid = checks["runner_success"] and checks["adapter_executed_at_expected_directory"] and all(
        checks[k] for k in ("metadata_created", "source_mutation_created", "tracked_runtime_mutation_created") if k in checks)
    intended_failure = valid and not checks["raw_index_bytes_equal"] and not checks["staged_blobs_and_modes_equal"] and not checks.get("metadata_visible_and_untracked", True)
    accepted = all(checks.values()) if variant == "green" else intended_failure
    receipt = {"case_id": case.name, "variant": variant, "step_id": row["step_id"], "asset": row["asset"],
               "command": [str(x) for x in command], "working_directory": str(repos["caller"]),
               "expected_execution_directory": str(expected), "adapter_evidence": adapter_rows,
               "resolved_indexes": {k: str(v[0]) for k, v in before.items()},
               "projection_hash": digest(recipe_file.read_bytes()),
               "source_asset_hash": digest(source(Path(row["asset"]).stem, variant == "baseline")),
               "projection_substitutions": {"prompt": {"original_sha256": digest(original_prompt.encode()), "replacement": step["prompt"]},
                    "context": context, "other_steps": "Omitted; no checkpoint, model or external service is executed.",
                    "directory_resolution": {"original": wd or None, "projected": step.get("working_dir"),
                        "template_fidelity": not args.resolve_directories or not wd,
                        "limitation": "Literal substitution isolates opt-out behaviour. It does not establish native template staging behaviour." if args.resolve_directories and wd else None},
                    "preserved": ["agent", "condition", "parse_json", "output", "continue_on_error", "recursion"]},
               "selected_environment": env, "exit_status": result.returncode, "assertion_results": checks,
               "valid_control": intended_failure if variant != "green" else None,
               "accepted": accepted, "terminal_outcome": "PASS" if accepted else "FAIL"}
    write_json(case / "receipt.json", receipt)
    print(case.name, receipt["terminal_outcome"], checks, flush=True)
    return receipt


def checkpoint_case(args, recipes, name, sid, reject=False, runtime=False):
    case = Path(tempfile.mkdtemp(prefix="checkpoint-" + sid + ("-reject-" if reject else "-success-"), dir=args.report))
    try:
        receipt = _checkpoint_case(args, recipes, name, sid, reject, runtime, case)
    except Exception as error:
        receipt = json.loads((case / "execution.json").read_bytes()) if (case / "execution.json").exists() else {}
        receipt.update({"case_id": case.name, "accepted": False, "failure_phase": "checkpoint_diagnostics_or_setup", "error": repr(error)})
    receipt["finalized"] = True
    write_json(case / "receipt.json", receipt)
    return receipt


def _checkpoint_case(args, recipes, name, sid, reject, runtime, case):
    env, repos = setup(case, edges=True, runtime=runtime)
    target = repos["target"]
    step = next(s for s in recipes[name]["steps"] if s["id"] == sid)
    if "command" in step:
        command = step["command"]
        substitutions = {}
    else:
        # The Crusty prompt contains exactly one executable commit block.
        blocks = re.findall(r"```bash\n(.*?)\n\s*```", step["prompt"], re.S)
        assert len(blocks) == 1
        command = blocks[0]
        substitutions = {"{{repo_path}}": str(target), "{{autodrive_launch_tools}}": str(ROOT / "amplifier-bundle/tools"), "{{pr_number}}": "7"}
        for key, value in substitutions.items():
            command = command.replace(key, value)
    (case / "extracted-command.sh").write_text(command)
    bin_dir = case / "private-bin"
    bin_dir.mkdir()
    allocator = shutil.which("gmktemp")
    if platform.system() == "Darwin" and allocator:
        # BSD mktemp -t selects a macOS private directory outside the task sandbox.
        # The existing GNU allocator honours the task's TMPDIR; no staging code changes.
        (bin_dir / "mktemp").symlink_to(allocator)
    # Only provider metadata is synthetic. Git staging, commits, pushes and helpers are real.
    gh = bin_dir / "gh"
    gh.write_text('#!/bin/sh\n[ "$*" = "pr view 7 --json headRefName --jq .headRefName" ] || exit 90\nprintf "task\\n"\n')
    gh.chmod(0o755)
    refusal = bin_dir / "refuse-models"
    refusal.write_text('#!/bin/sh\nprintf "fixture refuses model invocation\\n" >&2\nexit 126\n')
    refusal.chmod(0o755)
    env.update(PATH=str(bin_dir) + ":" + env["PATH"], AMPLIHACK_HOME=str(ROOT),
               REPO_PATH=str(repos["base"]), REVIEW_DIR=str(repos["base"]),
               AUTODRIVE_LAUNCH_TOOLS=str(ROOT / "amplifier-bundle/tools"),
               RECIPE_VAR_worktree_setup=json.dumps({"worktree_path": str(target)}),
               AMPLIHACK_GIT_AUTHOR_NAME="Fixture", AMPLIHACK_GIT_AUTHOR_EMAIL="fixture@example.com",
               AMPLIHACK_GIT_COMMITTER_NAME="Fixture", AMPLIHACK_GIT_COMMITTER_EMAIL="fixture@example.com",
               AMPLIHACK_BIN=str(refusal), MINIMALIST_HOOK_LOG=str(case / "hook.jsonl"))
    remote = case / "local-remote.git"
    run(["git", "init", "--bare", remote], case, env)
    git(target, env, "remote", "add", "origin", str(remote))
    git(target, env, "push", "-u", "origin", "task")
    hook_dir = Path(git(target, env, "rev-parse", "--path-format=absolute", "--git-path", "hooks").decode().strip())
    hook_dir.mkdir(exist_ok=True)
    hook = hook_dir / "pre-commit"
    # A fatal final line exercises the existing helper's non-repair failure route.
    # A separate transport refusal prevents any unexpected route from invoking a model.
    hook.write_text('#!/bin/sh\ngit ls-files --stage -z > "$MINIMALIST_HOOK_LOG.index"\nprintf "hook invoked\\n" >> "$MINIMALIST_HOOK_LOG"\n' +
                    ('echo "fatal: fixture hook rejection" >&2\nexit 1\n' if reject else 'exit 0\n'))
    hook.chmod(0o755)
    (target / "source.txt").write_text("authorised source\n")
    (target / ".amplihack").mkdir()
    (target / ".amplihack/indexing.pid").write_text("12345\n")
    (target / ".onboarding-evidence").mkdir()
    (target / ".onboarding-evidence/checkpoint.json").write_text('{"runtime":true}\n')
    if runtime:
        (target / ".claude/runtime/tracked.json").write_text("runtime changed\n")
    initial_head = git(target, env, "rev-parse", "HEAD").decode().strip()
    before = snapshot(repos, env, case)
    objects_before = commit_objects(lambda *argv: git(target, env, *argv))
    invocation = ["bash", "-c", command]
    write_json(case / "execution.json", {"command": invocation, "execution_state": "running"})
    result = run(invocation, repos["caller"], env, False)
    raw_after = after_bytes(before, case)
    (case / "stdout").write_bytes(result.stdout)
    (case / "stderr").write_bytes(result.stderr)
    write_json(case / "execution.json", {"command": invocation, "exit_status": result.returncode, "execution_state": "returned"})
    head = git(target, env, "rev-parse", "HEAD").decode().strip()
    authority = {"source.txt": {"mode": "100644", "contents_base64": "YXV0aG9yaXNlZCBzb3VyY2UK"}}
    history = publication_history(lambda *argv: git(target, env, *argv), objects_before, authority)
    committed = [r["path"] for r in history["transitions"]]
    content = history["transitions"]
    at_hook = (case / "hook.jsonl.index").read_bytes() if (case / "hook.jsonl.index").exists() else None
    def entries(raw):
        return {r.split(b"\t", 1)[1]: r.split(b"\t", 1)[0] for r in (raw or b"").split(b"\0") if r}
    old_entries, new_entries = entries(before["target"][2]), entries(at_hook)
    allow_stage = [b"source.txt"]
    allow_publish = {"source.txt": "authorised source\n"}
    newly_staged = [p for p in new_entries if new_entries[p] != old_entries.get(p)]
    checks = {
        "hook_invoked": (case / "hook.jsonl").is_file(),
        "intended_source_staged": b"source.txt" in newly_staged,
        "newly_staged_paths_authorised": at_hook is not None and set(newly_staged) <= set(allow_stage),
        "unrelated_staged_blobs_and_modes_preserved": at_hook is not None and all(
            new_entries.get(p) == v for p, v in old_entries.items() if p != b"source.txt"),
        "runtime_metadata_not_staged": at_hook is not None and not any(
            p.startswith((b".amplihack/", b".onboarding-evidence/", b".claude/runtime/")) for p in new_entries),
        "all_committed_paths_and_contents_authorised": history["publication_authorised"],
        "rejecting_hook_prevents_commit_and_propagates_failure": ((case / "hook.jsonl").is_file() and head == initial_head and not history["created_commits"] and result.returncode != 0) if reject else None,
        "successful_hook_allows_checkpoint_completion": (head != initial_head and result.returncode == 0) if not reject else None,
    }
    receipt = {"case_id": case.name, "source_asset_hash": digest(source(name)),
               "extracted_block_hash": digest(command.encode()), "command": ["bash", "-c", command],
               "selected_environment": env, "working_directory": str(repos["caller"]),
               "interpolation": substitutions, "exit_status": result.returncode,
               "initial_head": initial_head, "resulting_head": head,
               "allowed_staging_paths": [os.fsdecode(p) for p in allow_stage],
               "allowed_commit_paths_and_contents": allow_publish,
               "newly_staged_paths": [os.fsdecode(p) for p in newly_staged],
               "committed_paths": committed, "committed_contents": content,
               "publication_history": history, "publication_entry_authority": authority,
               "resolved_indexes": {k: str(v[0]) for k, v in before.items()},
               "residual_index_changed": {k: raw_after[k] != v[1] for k, v in before.items()},
               "checks": checks, "accepted": all(v for v in checks.values() if v is not None),
               "residual_differences": ["Synthetic repository, task identity, local bare remote and hooks.",
                   "Provider metadata is fixed to PR 7 on branch task; no provider request is made.",
                   "Full workflow and agent prompts are omitted; relevant bash shell settings and helper resolution remain unchanged.",
                   "On Darwin the pre-existing GNU mktemp is selected through task-private PATH so -t honours assigned TMPDIR; installed tools remain unchanged.",
                   "The rejecting hook ends with fatal:, selecting the real helper's non-repair route. Repair-route acceptance is unexecuted.",
                   "Any unexpected repair transport is refused with exit 126; no model is permitted."],
               "tracked_runtime_case": runtime}
    write_json(case / "receipt.json", receipt)
    print(case.name, "PASS" if receipt["accepted"] else "FAILED ACCEPTANCE", checks, flush=True)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--runner", required=True)
    parser.add_argument("--probe", action="store_true")
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--only-checkpoints", action="store_true")
    selection.add_argument("--agents-only", action="store_true")
    parser.add_argument("--requirements-file", type=Path, required=True)
    parser.add_argument("--resolve-directories", action="store_true",
                        help="Also record the limitation: replace directory templates with their declared context values.")
    args = parser.parse_args()
    args.requirements_bytes = args.requirements_file.read_bytes()
    args.requirements_text = args.requirements_bytes.decode("utf-8")
    if not args.requirements_text.strip():
        raise ValueError("Complete requirements must be nonempty")
    args.report = args.report.resolve()
    args.report.mkdir(parents=True, exist_ok=True)
    args.runner = str(Path(args.runner).resolve())
    os.environ.setdefault("TMPDIR", "/Users/ryan/src/bb/worker-logs/canonical-default-staging-1554/tmp")
    Path(os.environ["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    rows, recipes, baselines = inventory(args.report)
    required = [r for r in rows if r["authorised_edit"]]
    identity = {"head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip(),
                "assets": {n: digest(source(n)) for n in (*PHASES, NESTED)},
                "test_sha256": digest(Path(__file__).read_bytes()),
                "baseline_head": BASELINE, "runner": executable_identity(args.runner),
                "requirements_sha256": digest(args.requirements_bytes),
                "directory_projection": "resolved literal values" if args.resolve_directories else "original templates",
                "platform": platform.platform(), "global_automatic_staging": "enabled (no opt-out flag)"}
    write_json(args.report / "candidate.json", identity)
    plan = [{"asset": r["asset"], "step_id": r["step_id"], "variant": v}
            for r in required for v in ("baseline", "green", "removal")]
    write_json(args.report / "plan.json", plan)
    receipts = []
    for row in ([] if args.only_checkpoints else required):
        name = Path(row["asset"]).stem
        for variant in ("baseline", "green", "removal"):
            receipts.append(agent_case(args, row, baselines[name] if variant == "baseline" else recipes[name], variant))
        if args.probe:
            break
    # Bookkeeping attribution and broader literal-path coverage are separate cases.
    row = next(r for r in required if r["step_id"] == "step-08-implement")
    if not args.only_checkpoints:
        receipts.append(agent_case(args, row, recipes["workflow-tdd"], "green", "-noop", noop=True))
        receipts.append(agent_case(args, row, recipes["workflow-tdd"], "green", "-edges", edges=True))
        for shared_row in (r for r in required if Path(r["asset"]).stem == "autodrive-crusty-round"):
            receipts.append(agent_case(args, shared_row, recipes["autodrive-crusty-round"], "green", "-shared-repository", shared=True))
    checkpoints = []
    if not args.probe and not args.agents_only:
        for name, sid in (("workflow-tdd", "checkpoint-after-implementation"),
                          ("workflow-refactor-review", "checkpoint-after-review-feedback"),
                          ("autodrive-crusty-round", "step-04-address-concerns")):
            for reject in (False, True):
                checkpoints.append(checkpoint_case(args, recipes, name, sid, reject))
        checkpoints.append(checkpoint_case(args, recipes, "workflow-tdd", "checkpoint-after-implementation", runtime=True))
    core = [r for r in receipts if "-noop-" not in r["case_id"] and "-edges-" not in r["case_id"] and "-shared-repository-" not in r["case_id"]]
    agent_complete = not args.probe and len(core) == len(plan)
    checkpoint_complete = len(checkpoints) == 7
    write_json(args.report / "summary.json", {"candidate": identity, "planned": len(plan),
               "executed_core": len(core), "receipts": receipts,
               "complete": agent_complete,
               "agent_cases_requested": not args.only_checkpoints,
               "checkpoint_cases_requested": not args.agents_only,
               "checkpoint_cases_complete": checkpoint_complete,
               "all_executed_cases_accepted": all(r["accepted"] for r in receipts),
               "unsupported": ["absent, alternate, split, sparse and unmerged indexes",
                               "concurrent Git activity", "interrupted adapters", "non-UTF-8 filenames"],
               "checkpoint_receipts": checkpoints,
               "checkpoint_acceptance": "passed" if checkpoints and all(r["accepted"] for r in checkpoints) else "failed or unexecuted",
               "delivery_acceptance": "incomplete; these fixtures establish no delivery"})
    complete = (args.only_checkpoints or agent_complete) and (args.agents_only or checkpoint_complete)
    return 0 if complete and all(r["accepted"] for r in receipts + checkpoints) else 1


if __name__ == "__main__":
    sys.exit(main())
