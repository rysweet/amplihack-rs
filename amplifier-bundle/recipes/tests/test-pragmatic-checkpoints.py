#!/usr/bin/env python3
"""Execute unchanged checkpoint blocks with real helpers and isolated Git hooks.

The bundle argument must hold verified baseline recipes and their unchanged
tools. This script deliberately reports unsafe checkpoints as failures. It
does not repair them or authorise existing staged entries for publication.
"""

import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
import sys
import time
import textwrap
from canonical_staging_publication import commit_objects, publication_history, destination_observation


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("staging_fixture", Path(__file__).with_name("test-pragmatic-auto-stage.py"))
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)

CHECKPOINTS = (
    ("workflow-tdd", "checkpoint-after-implementation", "command"),
    ("workflow-refactor-review", "checkpoint-after-review-feedback", "command"),
    ("workflow-publish", "step-15-commit-push", "command"),
    ("workflow-publish", "step-16b-outside-in-fix-loop", "fence"),
    ("workflow-pr-review", "step-18c-push-feedback-changes", "command"),
    ("workflow-finalize", "step-20b-push-cleanup", "command"),
    ("autodrive-crusty-round", "step-04-address-concerns", "fence"),
)


def extract(text, sid, kind):
    block = support.blocks(text)[sid]
    if kind == "command":
        match = re.search(r'^    command: \|\n((?:      .*\n|\n)+)', block, re.M)
        if not match:
            raise ValueError((sid, "missing literal command"))
        return "".join(line[6:] if line.startswith("      ") else line for line in match[1].splitlines(True))
    matches = list(re.finditer(r'^([ ]+)```bash\n(.*?)^\1```', block, re.M | re.S))
    if len(matches) != 1:
        raise ValueError((sid, "ambiguous shell fence"))
    return textwrap.dedent(matches[0][2])


def entries(fixture, repo):
    result = {}
    for entry in fixture.git(repo, "ls-files", "--stage", "-z").split(b"\0"):
        if entry:
            header, path = entry.split(b"\t", 1)
            result[os.fsdecode(path)] = header.decode()
    return result


def fixture_worktree_snapshot(root):
    result = {}
    for folder, directories, files in os.walk(root, followlinks=False):
        if Path(folder) == root and ".git" in directories:
            directories.remove(".git")
        for name in directories[:] + files:
            path = Path(folder) / name
            relative = os.fsencode(path.relative_to(root))
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode):
                result[relative] = (stat.S_IMODE(info.st_mode), "symlink", os.readlink(os.fsencode(path)))
                if name in directories:
                    directories.remove(name)
            elif stat.S_ISDIR(info.st_mode):
                result[relative] = (stat.S_IMODE(info.st_mode), "directory")
            elif stat.S_ISREG(info.st_mode):
                result[relative] = (stat.S_IMODE(info.st_mode), "file", path.read_bytes())
            else:
                raise ValueError("Unsupported test-owned worktree object")
    return result


def retained_worktree_snapshot(snapshot):
    """Encode the complete test-owned snapshot without losing filename bytes."""
    return [{"path_b64":base64.b64encode(path).decode("ascii"), "mode":entry[0], "kind":entry[1],
             "contents_b64":base64.b64encode(entry[2]).decode("ascii") if len(entry) == 3 else None}
            for path, entry in sorted(snapshot.items())]


def checkpoint(fixture, args, asset, sid, kind, variant):
    case = fixture.report / (sid + "--" + variant)
    case.mkdir()
    receipt = {"asset": asset, "step_id": sid, "variant": variant, "acceptance_result": "failed"}
    try:
        caller, target, remote = (case / name for name in ("caller", "task", "remote.git"))
        fixture.seed(caller)
        fixture.git(caller, "config", "--local", "user.email", "fixture@example.org")
        fixture.run(["git", "init", "--bare", str(remote)])
        fixture.git(caller, "remote", "add", "origin", str(remote))
        fixture.git(caller, "push", "-u", "origin", "fixture")
        fixture.git(caller, "worktree", "add", "-b", "task", str(target))
        fixture.git(target, "push", "-u", "origin", "task")
        fixture.git(caller, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/fixture")
        if variant in ("adversarial", "tracked-runtime"):
            fixture.partial(target)
            for name in support.METADATA:
                path = target / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("observable runtime metadata\n")
            if variant == "tracked-runtime":
                (target / support.TRACKED_RUNTIME).write_text("modified tracked runtime\n")
        (target / "source.txt").write_text("authorised checkpoint source\n")
        fixture.git(target, "config", "core.hooksPath", str(case / "hooks"))
        hooks = case / "hooks"
        hooks.mkdir()
        hook = hooks / "pre-commit"
        hook.write_text("#!/bin/sh\nprintf 'hook executed\\n' >> " + shlex.quote(str(case / "hook.log"))
                        + "\ngit ls-files --stage -z > " + shlex.quote(str(case / "hook.index-entries"))
                        + "\ngit diff --cached --name-only --no-renames -z > " + shlex.quote(str(case / "hook.staged-paths"))
                        + "\nexit " + ("1" if variant == "reject" else "0") + "\n")
        hook.chmod(0o755)
        # This is a network service fixture. It cannot invoke gh authentication.
        gh = fixture.bin / "gh"
        gh.write_text("#!/bin/sh\nif [ \"$*\" = 'pr view 1 --json headRefName --jq .headRefName' ]; then printf 'task\\n'; else exit 97; fi\n")
        gh.chmod(0o755)
        # The genuine guard runs. Hook repair cannot launch models in this fixture.
        gate = fixture.bin / "amplihack"
        gate.write_text("#!/bin/sh\nif [ \"$1\" = hygiene ]; then exec " + shlex.quote(str(args.amplihack.resolve())) + " \"$@\"; fi\nprintf 'fixture refuses model or repair dispatch\\n' >&2\nexit 98\n")
        gate.chmod(0o755)
        jq = fixture.bin / "jq"
        if not jq.exists():
            jq.symlink_to(args.jq.resolve())
        text = (args.bundle / "amplifier-bundle/recipes" / (asset + ".yaml")).read_text()
        command = extract(text, sid, kind)
        (case / "extracted.sh").write_text(command)
        substitutions = {"{{repo_path}}": str(target), "{{autodrive_launch_tools}}": str(args.bundle / "amplifier-bundle/tools"), "{{pr_number}}": "1"}
        rendered = command
        for key, value in substitutions.items():
            rendered = rendered.replace(key, value)
        (case / "rendered.sh").write_text(rendered)
        env = dict(fixture.env, AMPLIHACK_HOME=str(args.bundle), REPO_PATH=str(caller),
                   RECIPE_VAR_worktree_setup=json.dumps({"worktree_path": str(target), "branch_name": "task"}),
                   AMPLIHACK_BIN=str(gate), REMOTE_HOST_TYPE="other", ISSUE_NUMBER="1554",
                   AMPLIHACK_GIT_AUTHOR_NAME="Staging Fixture", AMPLIHACK_GIT_AUTHOR_EMAIL="fixture@example.org",
                   AMPLIHACK_GIT_COMMITTER_NAME="Staging Fixture", AMPLIHACK_GIT_COMMITTER_EMAIL="fixture@example.org",
                   TASK_DESCRIPTION="fix source staging fixture", RECIPE_VAR_version_bump='{"change_classification":"PATCH"}')
        before = entries(fixture, target)
        before_changed = {os.fsdecode(p) for p in fixture.git(target, "diff", "--cached", "--name-only", "--no-renames", "-z").split(b"\0") if p}
        head = fixture.git(target, "rev-parse", "HEAD").strip()
        remote_before = fixture.git(remote, "rev-parse", "refs/heads/task").strip()
        paths = fixture.snapshot_paths([caller, target])
        objects_before = commit_objects(lambda *argv: fixture.git(target, *argv))
        index_before = fixture.raw(paths)
        worktrees_before = {str(repo):fixture_worktree_snapshot(repo) for repo in (caller, target)}
        support.save(case / "worktrees-before.json", {name:retained_worktree_snapshot(worktrees_before[str(repo)])
                     for name, repo in (("caller",caller),("task",target))})
        invocation = [str(args.shell.resolve()), str(case / "rendered.sh")]
        receipt.update({"command": invocation, "execution_state": "running"})
        support.save(case / "receipt.json", receipt)
        bounded_assets = {"workflow-publish": "step-15-commit-push",
                          "workflow-pr-review": "step-18c-push-feedback-changes",
                          "workflow-finalize": "step-20b-push-cleanup"}
        if bounded_assets.get(asset) == sid:
            owner = "fixture owner: change only literal source.txt"
            context = {"repo_path":str(caller), "task_description":owner, "final_requirements":owner,
                       "worktree_setup":{"worktree_path":str(target), "branch_name":"task",
                                         "base_ref":"origin/fixture", "base_branch":"fixture", "created":True},
                       "version_bump":{"change_classification":"PATCH"}, "pr_number":"1"}
            env["TASK_DESCRIPTION"] = owner
            request = {"asset":asset, "context":context, "fixture_owner_paths":["source.txt"],
                       "evidence_directory":str(case), "shell":str(args.shell.resolve())}
            support.save(case / "scope-projection-request.json", request)
            result = bounded_fixture_projection(request, env)
        else:
            result = fixture.run(invocation, cwd=target if kind == "fence" else caller, env=env, check=False)
        index_after = fixture.raw(paths)
        worktrees_after = {str(repo):fixture_worktree_snapshot(repo) for repo in (caller, target)}
        support.save(case / "worktrees-after.json", {name:retained_worktree_snapshot(worktrees_after[str(repo)])
                     for name, repo in (("caller",caller),("task",target))})
        (case / "stdout").write_bytes(result.stdout)
        (case / "stderr").write_bytes(result.stderr)
        receipt.update({"exit_status": result.returncode, "execution_state": "returned",
                        "runner_seconds": result.elapsed_seconds})
        support.save(case / "execution.json", {"command": invocation, "exit_status": result.returncode,
                     "stdout_sha256": support.digest(result.stdout), "stderr_sha256": support.digest(result.stderr)})
        support.save(case / "receipt.json", receipt)
        for n, repo in enumerate((caller, target)):
            (case / (str(n) + ".index.before")).write_bytes(index_before[str(repo)])
            (case / (str(n) + ".index.after")).write_bytes(index_after[str(repo)])
        (case / "stdout").write_bytes(result.stdout)
        (case / "stderr").write_bytes(result.stderr)
        after = entries(fixture, target)
        now = fixture.git(target, "rev-parse", "HEAD").strip()
        authority = {"source.txt": {"mode": "100644", "contents_base64": base64.b64encode(b"authorised checkpoint source\n").decode()}}
        history = publication_history(lambda *argv: fixture.git(target, *argv), objects_before, authority)
        destination = destination_observation(lambda *argv: fixture.git(remote, *argv),
                                              "refs/heads/task", now.decode(), authority)
        push_required = asset not in ("workflow-tdd", "workflow-refactor-review")
        requires_publication = variant == "normal" and push_required
        destination_contract = (destination["publication_observed"] and destination["tip"] != remote_before.decode()
                                if requires_publication else destination["tip"] == remote_before.decode())
        committed = history["transitions"]
        # A hook snapshots what was staged even when a successful commit cleared it.
        # Committed deltas retain publication violations rather than legitimising them.
        hook_seen = (case / "hook.log").exists()
        stage_allowed = {"source.txt"}
        commit_allowed = {"source.txt": base64.b64encode(b"authorised checkpoint source\n").decode()}
        hook_staged = {os.fsdecode(p) for p in (case / "hook.staged-paths").read_bytes().split(b"\0") if p} if hook_seen else set()
        hook_entries = {}
        if hook_seen:
            for entry in (case / "hook.index-entries").read_bytes().split(b"\0"):
                if entry:
                    header, path = entry.split(b"\t", 1)
                    hook_entries[os.fsdecode(path)] = header.decode()
        new_entries = {p for p, value in hook_entries.items() if before.get(p) != value}
        index_new_paths = set(after) - set(before)
        overwritten = [p for p in before if p != "source.txt" and before[p] != hook_entries.get(p)]
        assertions = {"destination_contract_satisfied": destination_contract,
                      "normal_hook_executed": hook_seen,
                      "unrelated_staged_entries_preserved": not overwritten,
                      "new_staging_authorised": new_entries <= stage_allowed,
                      "runtime_not_indexed": not any(p in after for p in support.METADATA) and (variant != "tracked-runtime" or before.get(support.TRACKED_RUNTIME) == after.get(support.TRACKED_RUNTIME)),
                      "publication_authorised": history["publication_authorised"],
                      "source_committed": any(r["path"] == "source.txt" and r["after"] is not None for r in committed),
                      "rejecting_hook_prevents_commit": variant != "reject" or (now == head and not history["created_commits"]),
                      "rejecting_hook_propagates_failure": variant != "reject" or result.returncode != 0}
        if variant in ("adversarial", "tracked-runtime"):
            # These cases contain unrelated staging. They must refuse before
            # hooks, source staging or publication rather than being made positive.
            assertions = {"destination_contract_satisfied":destination_contract,
                          "publication_authorised":history["publication_authorised"],
                          "unauthorised_entry_refused":result.returncode != 0,
                          "normal_hook_not_reached":not hook_seen,
                          "all_index_bytes_preserved":index_before == index_after,
                          "all_worktrees_preserved":worktrees_before == worktrees_after,
                          "index_entries_preserved":before == after,
                          "head_preserved":head == now,
                          "no_commit_created":not history["created_commits"]}
            accepted = all(assertions.values())
        else:
            accepted = (all(value for key, value in assertions.items() if key != "source_committed") if variant == "reject" else result.returncode == 0 and all(assertions.values()))
        receipt.update({"acceptance_result": "passed" if accepted else "failed", "exit_status": result.returncode,
                        "assertions": assertions, "command": [str(args.shell.resolve()), str(case / "rendered.sh")],
                        "source_asset_hash": support.digest(text.encode()), "extracted_block_hash": support.digest(command.encode()),
                        "rendered_block_hash": support.digest(rendered.encode()), "interpolation": substitutions,
                        "selected_environment": env, "staging_allowlist": sorted(stage_allowed), "publication_allowlist": commit_allowed,
                        "index_paths": {k: str(v) for k, v in paths.items()}, "newly_staged_paths": sorted(new_entries),
                        "new_index_paths": sorted(index_new_paths), "hook_staged_paths": sorted(hook_staged),
                        "overwritten_unrelated_entries": overwritten, "committed_paths_and_contents": committed,
                        "publication_history": history, "publication_entry_authority": authority,
                        "destination_observation": destination, "push_required": push_required,
                        "destination_tip_before": remote_before.decode(),
                        "before_staged_entries": before, "after_staged_entries": after,
                        "hook_staged_entries": hook_entries,
                        "head_before": head.decode(), "head_after": now.decode(), "hook_executed": hook_seen,
                        "metadata_before": variant in ("adversarial", "tracked-runtime"),
                        "metadata_after": {p: (target / p).exists() for p in support.METADATA},
                        "residual_differences": ["Full preceding workflow omitted; entry conditions assumed true.", "Fixture Git identity, local bare remote and controlled normal hook.", "Mock gh returns only the fixture PR head branch; no credentials.", "Real hygiene guard and unchanged source helpers. Repair/evaluator dispatch is refused before model execution.", "The checkpoint's own cleanup and Git diagnostics are unchanged; checkpoint index snapshots are after that boundary.", "Explicit installed Bash, Git and mktemp executables are selected in the private PATH. Their identities are retained. No complete Linux acceptance is claimed."]})
    except Exception as error:
        receipt.update({"terminal_outcome": "setup or checkpoint execution failure", "error": repr(error)})
    receipt["finalized"] = True
    support.save(case / "receipt.json", receipt)
    print(sid + " " + variant + ": " + receipt["acceptance_result"], flush=True)
    return receipt


def bounded_fixture_projection(request, env):
    """Project the three real checkpoints with independent synthetic owner input.

    This is test preparation, not a model scope decision or a runtime API.
    Actual checkpoint helpers, hygiene, commits and hooks remain unchanged.
    """
    import unittest
    specification = importlib.util.spec_from_file_location(
        "checkpoint_projection_contract", Path(__file__).with_name("test-checkpoint-scope.py"))
    contract = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(contract)
    phase = next((row for row in contract.PHASES if row[0] == request["asset"]), None)
    if phase is None:
        raise ValueError("Only the three bounded checkpoint fixtures use this projection.")
    evidence = Path(request["evidence_directory"])
    if not evidence.is_dir():
        raise ValueError("The assigned fixture evidence directory must exist.")
    class FixtureProjection:
        case = unittest.TestCase()
        def __init__(self):
            self.context = request["context"]
            self.task = Path(self.context["worktree_setup"]["worktree_path"])
            self.branch = self.context["worktree_setup"]["branch_name"]
            self.owner = self.context["task_description"]
            self.allowed = {os.fsencode(path) for path in request["fixture_owner_paths"]}
            self.env = dict(env)
            self.trace = []
            self.sync_context()
        def sync_context(self):
            for key, value in self.context.items():
                self.env["RECIPE_VAR_" + key] = json.dumps(value) if isinstance(value, (dict, list, bool)) else str(value)
        def git(self, repo, *args):
            result = subprocess.run(["git", "-C", str(repo), *args], env=self.env, capture_output=True)
            self.case.assertEqual(result.returncode, 0, result.stderr)
            return result.stdout
        def shell(self, command):
            number = len(self.trace)
            result = subprocess.run([request["shell"], "-c", command], cwd=self.task,
                                    env=self.env, capture_output=True)
            stem = evidence / ("scope-step-" + str(number))
            stem.with_suffix(".sh").write_text(command)
            stem.with_suffix(".stdout").write_bytes(result.stdout)
            stem.with_suffix(".stderr").write_bytes(result.stderr)
            self.trace.append({"command_sha256":support.digest(command.encode()), "exit_code":result.returncode})
            return result
    fixture = FixtureProjection()
    started = time.perf_counter()
    result = contract.PhaseProjection(fixture, phase).run()
    result.elapsed_seconds = time.perf_counter() - started
    support.save(evidence / "scope-projection.json", {
        "asset":request["asset"], "fixture_owner_paths":request["fixture_owner_paths"],
        "owner_task":fixture.owner, "steps":fixture.trace,
        "source_sha256":support.digest(Path(__file__).read_bytes()),
        "limitations":["Synthetic finite owner permission replaces the model review only.",
                       "This does not qualify native implicit staging or establish delivery acceptance."]})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("bundle", "report", "runner", "amplihack", "jq", "tmpdir", "target", "shell", "git", "mktemp"):
        parser.add_argument("--" + flag, required=True, type=Path)
    parser.add_argument("--requirements-file", required=True, type=Path)
    args = parser.parse_args()
    fixture = support.Fixtures(args)
    binaries = {}
    for name, path in (("bash", args.shell), ("git", args.git), ("mktemp", args.mktemp)):
        (fixture.bin / name).symlink_to(path.resolve())
        binaries[name] = {"path": str(path.resolve()), "sha256": support.binary_digest(path),
                          "version": fixture.run([str(path.resolve()), "--version"]).stdout.decode(errors="replace")}
    support.save(fixture.report / "selected-binaries.json", binaries)
    support.save(fixture.report / "planned-cases.json", {"checkpoints": CHECKPOINTS, "variants": ["normal", "adversarial", "tracked-runtime", "reject"]})
    receipts = []
    for asset, sid, kind in CHECKPOINTS:
        for variant in ("normal", "adversarial", "tracked-runtime", "reject"):
            receipts.append(checkpoint(fixture, args, asset, sid, kind, variant))
            support.save(fixture.report / "results.json", receipts)
    return 0 if len(receipts) == 28 and all(r["acceptance_result"] == "passed" for r in receipts) else 1


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--bounded-fixture-projection":
        request = json.loads(Path(sys.argv[2]).read_text())
        result = bounded_fixture_projection(request, dict(os.environ))
        sys.stdout.buffer.write(result.stdout)
        sys.stderr.buffer.write(result.stderr)
        sys.exit(result.returncode)
    sys.exit(main())
