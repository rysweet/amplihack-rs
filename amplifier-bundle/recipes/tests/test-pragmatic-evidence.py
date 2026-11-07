#!/usr/bin/env python3
"""Independently check retained snapshots and write a candidate-bound ledger.

The ledger separates projected regressions, native-template controls, unchanged
checkpoints and delivery. A successful mock regression cannot authorise a merge.
Missing terminal receipts are incomplete evidence, never an implicit pass.
"""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from collections import Counter
import re
import textwrap
import stat
from canonical_staging_publication import publication_history

GIT = "git"

def sha(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(path.read_text())


def git(repo, *args, index=None):
    env = {"PATH": "/usr/bin:/bin", "HOME": str(repo), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0", "GIT_NO_REPLACE_OBJECTS": "1"}
    if index is not None:
        env["GIT_INDEX_FILE"] = str(index)
    result = subprocess.run([GIT, "-C", str(repo), "--literal-pathspecs", *args], env=env, capture_output=True)
    if result.returncode:
        raise RuntimeError((args, result.returncode, result.stderr.decode(errors="replace")))
    return result.stdout


def yaml_document(raw):
    proc = subprocess.run(["ruby", "-e", 'require "yaml"; require "json"; puts JSON.generate(YAML.load(STDIN.read))'], input=raw, capture_output=True, check=True)
    return json.loads(proc.stdout)


def authority_checks(authority):
    """The caller pins this manifest's digest separately from the receipts."""
    source = Path(authority["source_root"])
    return all(sha((source / path).read_bytes()) == expected for path, expected in authority["files"].items()) and \
        sha(requirements_bytes(authority)) == authority["requirements_sha256"]


def requirements_bytes(authority):
    if "_requirements_bytes" not in authority:
        authority["_requirements_bytes"] = Path(authority["requirements_file"]).read_bytes()
    return authority["_requirements_bytes"]


def verify_derivation(root, row, authority):
    spec = authority["cases"][row["case_id"]]
    assert row["planned_variant"] == spec["variant"]
    candidate = Path(authority["source_root"]) / "amplifier-bundle/recipes" / (spec["asset"] + ".yaml")
    baseline = Path(authority["baseline_root"]) / (spec["asset"] + ".yaml")
    candidate_bytes, baseline_bytes = candidate.read_bytes(), baseline.read_bytes()
    assert sha(candidate_bytes) == authority["files"][str(candidate.relative_to(authority["source_root"]))]
    assert sha(baseline_bytes) == authority["baseline_assets"][spec["asset"]]
    assert row["candidate_hash"] == sha(candidate_bytes) and row["baseline_hash"] == sha(baseline_bytes)
    raw = baseline_bytes if spec["variant"] == "red" else candidate_bytes
    assert row["source_hash"] == sha(raw)
    expected = yaml_document(raw)
    steps = {s["id"]: s for s in expected["steps"]}
    expected["steps"] = [steps[sid] for sid in spec["steps"]]
    expected.pop("output", None)
    req = requirements_bytes(authority)
    assert sha(req) == authority["requirements_sha256"]
    assert (root / "requirements-unchanged.json").read_bytes() == req
    context = row["projection_substitutions"]["context"]
    case = root / row["case_id"]
    assert context["repo_path"] == str(case / "task linked")
    assert context["review_dir"] == str(case / ("caller" if spec["shared"] else "review"))
    assert context["worktree_setup"] == {"worktree_path": context["repo_path"]}
    expected_dirs = []
    for step in expected["steps"]:
        wd = step.get("working_dir")
        expected_dirs.append(context["review_dir"] if wd == "{{review_dir}}" else
                             context["repo_path"] if wd else str(case / "caller"))
        if spec["variant"] == "removal":
            assert step.pop("auto_stage") is False
        directory = step.get("working_dir")
        if authority["materialise_working_dir"] and directory:
            substitutions = {"{{repo_path}}": context["repo_path"], "{{review_dir}}": context["review_dir"],
                             "{{worktree_setup.worktree_path}}": context["repo_path"]}
            step["working_dir"] = substitutions[directory]
        step["prompt"] += "\nComplete supplied requirements, unchanged:\n" + req.decode("utf-8")
    assert yaml_document((case / "recipe.yaml").read_bytes()) == expected, "Source-to-projection derivation differs"
    identity = load(root / "identity.json")
    for key in ("runner_path", "runner_sha256", "runner_version"):
        assert identity[key] == authority["runner"][key], "Runner identity differs"
    assert identity["requirements_sha256"] == authority["requirements_sha256"]
    assert identity["test_sha256"] == authority["files"]["amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py"]
    assert row["command"][0] == authority["runner"]["runner_path"]
    assert "--no-auto-stage" not in row["command"]
    assert [r["cwd"] for r in row["adapter"]] == expected_dirs
    return spec


def verify_case(root, row, authority):
    spec = verify_derivation(root, row, authority)
    case = root / row["case_id"]
    checks = []
    expected_repos = list(dict.fromkeys(str(case / name) for name in
        ("caller", "caller" if spec["shared"] else "review", "task linked")))
    checks.append(list(row["resolved_index_paths"]) == expected_repos)
    byte_equal, semantic_equal = True, True
    for n, (repo_name, index_path) in enumerate(row["resolved_index_paths"].items()):
        repo = Path(repo_name)
        before, after = (case / (str(n) + ".index." + phase) for phase in ("before", "after"))
        byte_equal &= before.read_bytes() == after.read_bytes()
        a = git(repo, "ls-files", "--stage", "-z", index=before)
        b = git(repo, "ls-files", "--stage", "-z", index=after)
        semantic_equal &= a == b
        checks.append(a == base64.b64decode(row["staged_entries_before_base64"][repo_name]))
        user = b"user partially staged\tfile.txt"
        entries = {entry.split(b"\t", 1)[1]: entry.split(b"\t", 1)[0].split() for entry in a.split(b"\0") if entry}
        checks.append(entries[user][0] == b"100755")
        checks.append(git(repo, "cat-file", "blob", entries[user][1].decode()) == b"user staged content\n")
        checks.append((repo / os.fsdecode(user)).read_bytes() == b"user unstaged content\n")
        checks.append((repo / os.fsdecode(user)).stat().st_mode & 0o777 == 0o644)
        checks.append(os.fsdecode(git(repo, "rev-parse", "--path-format=absolute", "--git-path", "index")).strip() == index_path)
    adapters = [json.loads(line) for line in (case / "adapter.jsonl").read_text().splitlines()]
    checks.append(adapters == row["adapter"] and bool(adapters) and all(x["success"] for x in adapters))
    checks.append(len(adapters) == len(spec["steps"]) and all(x["mode"] == spec["mode"] for x in adapters))
    terminal = load(case / "stdout")
    checks.append(row["exit_status"] == 0 and terminal["success"] and all(s["status"] == "completed" for s in terminal["step_results"]))
    checks.append(len(terminal["step_results"]) == len(spec["steps"]))
    checks.append(sha((case / "recipe.yaml").read_bytes()) == row["projection_hash"])
    checks.append(byte_equal == row["assertions"]["index_bytes"])
    checks.append(semantic_equal == row["assertions"]["staged_entries_and_modes"])
    controls = row["planned_variant"] in ("red", "removal")
    checks.append(not byte_equal if controls else byte_equal and semantic_equal)
    checks.append(row["assertions"]["metadata_visible"] and row["assertions"]["mutation_source_visible"])
    for adapter in adapters:
        repo = Path(adapter["cwd"])
        if spec["mode"] != "noop":
            metadata = (".amplihack/new\nnote.json", ".claude/runtime/new.json", "indexing.pid")
            checks.append(all((repo / p).read_bytes() == b"visible fixture metadata\n" for p in metadata))
            untracked = set(git(repo, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"))
            controls = spec["variant"] in ("red", "removal")
            checks.append((not all(p.encode() in untracked for p in metadata)) if controls else all(p.encode() in untracked for p in metadata))
            if spec["mode"] == "mutation":
                checks.append((repo / "source.txt").read_bytes() == b"deferred source change\n")
            if not controls:
                changed = set(git(repo, "diff", "--name-only", "-z").split(b"\0"))
                checks.append(b".amplihack/tracked.json" in changed and (spec["mode"] != "mutation" or b"source.txt" in changed))
    return all(checks)


def verify_hook_population(rows, checkpoints):
    required = Counter((asset, sid, variant) for asset, sid, _ in checkpoints
                       for variant in ("normal", "adversarial", "tracked-runtime", "reject"))
    observed = Counter((r.get("asset"), r.get("step_id"), r.get("variant")) for r in rows)
    if observed != required:
        return False
    return all(r.get("hook_executed") and r["assertions"]["rejecting_hook_prevents_commit"]
               and r["assertions"]["rejecting_hook_propagates_failure"]
               for r in rows if r["variant"] in ("normal", "reject"))


def verify_report_identity(root, authority):
    identity = load(root / "identity.json")
    return all(identity[key] == authority["runner"][key] for key in ("runner_path", "runner_version", "runner_sha256")) and \
        sha((root / "requirements-unchanged.json").read_bytes()) == authority["requirements_sha256"]


def verify_hook_evidence(case, row):
    execution = load(case / "execution.json")
    assert execution["command"] == row["command"]
    assert execution["exit_status"] == row["exit_status"]
    assert execution["stdout_sha256"] == sha((case / "stdout").read_bytes())
    assert execution["stderr_sha256"] == sha((case / "stderr").read_bytes())
    lines = (case / "hook.log").read_text().splitlines() if (case / "hook.log").exists() else []
    invoked = bool(lines) and all(line == "hook executed" for line in lines)
    assert row["hook_executed"] == invoked
    observed = {}
    if invoked:
        for entry in (case / "hook.index-entries").read_bytes().split(b"\0"):
            if entry:
                header, path = entry.split(b"\t", 1)
                observed[os.fsdecode(path)] = header.decode()
    assert row["hook_staged_entries"] == observed
    if row["variant"] in ("normal", "reject"):
        assert invoked
        if row["variant"] == "reject":
            assert not row["publication_history"]["created_commits"] and execution["exit_status"] != 0
            assert row["head_before"] == row["head_after"]
        else:
            assert row["publication_history"]["created_commits"] and execution["exit_status"] == 0
    return True


def audited_worktree_snapshot(root):
    """Reconstruct retained fixture observations independently of the producer."""
    rows = []
    for folder, directories, files in os.walk(os.fsencode(root), followlinks=False):
        if Path(os.fsdecode(folder)) == root and b".git" in directories:
            directories.remove(b".git")
        for name in directories[:] + files:
            full = os.path.join(folder, name)
            info = os.lstat(full)
            payload = None
            if stat.S_ISLNK(info.st_mode):
                kind, payload = "symlink", os.readlink(full)
                if name in directories:
                    directories.remove(name)
            elif stat.S_ISDIR(info.st_mode):
                kind = "directory"
            else:
                assert stat.S_ISREG(info.st_mode), "Unsupported fixture object"
                kind, payload = "file", Path(os.fsdecode(full)).read_bytes()
            rows.append({"path_b64":base64.b64encode(os.path.relpath(full, os.fsencode(root))).decode("ascii"),
                         "mode":stat.S_IMODE(info.st_mode), "kind":kind,
                         "contents_b64":base64.b64encode(payload).decode("ascii") if payload is not None else None})
    return sorted(rows, key=lambda row:base64.b64decode(row["path_b64"], validate=True))


def verify_preserved_refusal(case, row):
    required = {"destination_contract_satisfied", "publication_authorised", "unauthorised_entry_refused", "normal_hook_not_reached",
                "all_index_bytes_preserved", "all_worktrees_preserved", "index_entries_preserved",
                "head_preserved", "no_commit_created"}
    assert set(row["assertions"]) == required
    assert all(value is True for value in row["assertions"].values())
    assert row["exit_status"] != 0 and not row["hook_executed"]
    assert row["head_before"] == row["head_after"]
    assert not row["publication_history"]["created_commits"]
    assert row["before_staged_entries"] == row["after_staged_entries"]
    snapshots = [load(case / name) for name in ("worktrees-before.json", "worktrees-after.json")]
    assert snapshots[0] == snapshots[1]
    assert set(snapshots[0]) == {"caller", "task"}
    for number, name in enumerate(("caller", "task")):
        repo = case / name
        assert snapshots[1][name] == audited_worktree_snapshot(repo)
        before = (case / (str(number) + ".index.before")).read_bytes()
        after = (case / (str(number) + ".index.after")).read_bytes()
        assert before == after
        index = Path(os.fsdecode(git(repo, "rev-parse", "--git-path", "index").strip()))
        if not index.is_absolute():
            index = repo / index
        assert after == index.read_bytes()
    entries = {}
    for item in git(case / "task", "ls-files", "--stage", "-z").split(b"\0"):
        if item:
            header, path = item.split(b"\t", 1)
            entries[os.fsdecode(path)] = header.decode()
    assert entries == row["after_staged_entries"]
    return True


def verify_checkpoint_destination(case, row):
    """Independently inspect the actual bare fixture, not a producer boolean."""
    reference = "refs/heads/task"
    observed = row["destination_observation"]
    assert set(observed) == {"reference", "tip", "expected_tip", "entries", "matches_local_head",
                             "contents_authorised", "publication_observed"}
    assert observed["reference"] == reference
    tip = git(case / "remote.git", "rev-parse", "--verify", reference).decode().strip()
    assert observed["tip"] == tip
    assert observed["expected_tip"] == row["head_after"]
    assert row["destination_tip_before"] == row["head_before"]
    assert git(case / "caller", "rev-parse", "HEAD").decode().strip() == row["head_before"]
    raw = git(case / "remote.git", "ls-tree", "-z", tip, "--", "source.txt")
    entry = None
    if raw:
        header, path = raw.rstrip(b"\0").split(b"\t", 1)
        assert path == b"source.txt"
        mode, kind, oid = header.split()
        entry = {"mode":mode.decode(), "type":kind.decode(), "contents_base64":
                 base64.b64encode(git(case / "remote.git", "cat-file", kind.decode(), oid.decode())).decode()}
    assert observed["entries"] == {"source.txt":entry}
    contents_correct = entry == {"mode":"100644", "type":"blob", "contents_base64":
                                base64.b64encode(b"authorised checkpoint source\n").decode()}
    head_matches = tip == row["head_after"]
    assert observed["matches_local_head"] is head_matches
    assert observed["contents_authorised"] is contents_correct
    assert observed["publication_observed"] is (head_matches and contents_correct)
    local_only = {("workflow-tdd", "checkpoint-after-implementation"),
                  ("workflow-refactor-review", "checkpoint-after-review-feedback")}
    required = (row["asset"], row["step_id"]) not in local_only
    assert row["push_required"] is required
    if row["variant"] == "normal" and required:
        assert head_matches and contents_correct and tip != row["destination_tip_before"]
    else:
        assert tip == row["destination_tip_before"]
    assert row["assertions"]["destination_contract_satisfied"] is True
    return True


def verify_checkpoint(root, row, authority):
    assert verify_report_identity(root, authority)
    # Validate identities against the externally supplied inventory before
    # using them to construct source or report paths.
    expected = {(asset, sid) for asset, sid, _ in authority["checkpoints"]}
    assert (row["asset"], row["step_id"]) in expected
    assert row["variant"] in ("normal", "adversarial", "tracked-runtime", "reject")
    raw = (Path(authority["source_root"]) / "amplifier-bundle/recipes" / (row["asset"] + ".yaml")).read_bytes()
    assert row["source_asset_hash"] == sha(raw)
    selected = next(s for s in yaml_document(raw)["steps"] if s["id"] == row["step_id"])
    command = selected.get("command")
    if command is None:
        matches = re.findall(r"```bash\n(.*?)```", selected["prompt"], re.S)
        assert len(matches) == 1
        command = textwrap.dedent(matches[0])
    case = root / (row["step_id"] + "--" + row["variant"])
    assert (case / "extracted.sh").read_text() == command
    assert row["extracted_block_hash"] == sha(command.encode())
    authority_entries = {"source.txt": {"mode": "100644", "contents_base64": base64.b64encode(b"authorised checkpoint source\n").decode()}}
    history = publication_history(lambda *args: git(case / "task", *args), {row["head_before"]}, authority_entries)
    assert git(case / "task", "rev-parse", "HEAD").decode().strip() == row["head_after"]
    assert history == row["publication_history"]
    assert history["publication_authorised"] is True
    assert history["publication_authorised"] == row["assertions"]["publication_authorised"]
    assert verify_hook_evidence(case, row)
    assert verify_checkpoint_destination(case, row)
    if row["variant"] in ("adversarial", "tracked-runtime"):
        assert verify_preserved_refusal(case, row)
    return True


def verify_distinct(root, rows, authority):
    if not verify_report_identity(root, authority):
        return False
    expected = Counter(sid + "--" + variant for sid in ("step-02-crusty-review", "step-04-address-concerns")
                       for variant in ("red", "green", "removal"))
    if Counter(r["case_id"] for r in rows) != expected:
        return False
    for row in rows:
        sid, variant = row["case_id"].split("--")
        case = root / row["case_id"]
        raw = ((Path(authority["baseline_root"]) / "autodrive-crusty-round.yaml") if variant == "red" else
               Path(authority["source_root"]) / "amplifier-bundle/recipes/autodrive-crusty-round.yaml").read_bytes()
        assert sha(raw) == row["source_asset_sha256"]
        source = yaml_document(raw)
        selected = next(s for s in source["steps"] if s["id"] == sid)
        target = case / ("target main" if sid == "step-02-crusty-review" else "target linked")
        if variant == "removal":
            assert selected.pop("auto_stage") is False
        selected["working_dir"] = str(target)
        selected["prompt"] += "\nComplete supplied requirements, unchanged:\n" + requirements_bytes(authority).decode("utf-8")
        source["steps"] = [selected]
        source.pop("output", None)
        assert yaml_document((case / "recipe.yaml").read_bytes()) == source
        assert sha((case / "recipe.yaml").read_bytes()) == row["projected_recipe_sha256"]
        assert row["command"][0] == authority["runner"]["runner_path"]
        assert row["exit_status"] == 0 and load(case / "stdout")["success"]
        adapter = [json.loads(line) for line in (case / "adapter.jsonl").read_text().splitlines()]
        assert len(adapter) == 1 and adapter[0]["success"] and adapter[0]["cwd"] == str(target)
        metadata = (".amplihack/new\nnote.json", ".claude/runtime/new.json", "indexing.pid")
        assert all((target / p).read_bytes() == b"visible fixture metadata\n" for p in metadata)
        if sid == "step-04-address-concerns":
            assert (target / "source.txt").read_bytes() == b"deferred source change\n"
        untracked = set(git(target, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"))
        assert (all(p.encode() in untracked for p in metadata)) if variant == "green" else (not all(p.encode() in untracked for p in metadata))
        byte_equal, semantic_equal = True, True
        for n, name in enumerate(("unrelated caller", "target main", "target linked")):
            repo = case / name
            before, after = case / (str(n) + ".before.index"), case / (str(n) + ".after.index")
            byte_equal &= before.read_bytes() == after.read_bytes()
            semantic_equal &= git(repo, "ls-files", "--stage", "-z", index=before) == git(repo, "ls-files", "--stage", "-z", index=after)
        assert (byte_equal and semantic_equal) if variant == "green" else (not byte_equal and not semantic_equal)
        assert row["accepted"] is True
    return True


def main():
    global GIT
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--git", required=True, type=Path)
    parser.add_argument("--regressions-report", default="projected-regressions")
    parser.add_argument("--checkpoints-report", default="final-checkpoints")
    parser.add_argument("--distinct-report", default="distinct-repositories")
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--rust-audit-receipt", default="rust-audit-renewed.exit.json")
    args = parser.parse_args()
    GIT = str(args.git.resolve())
    root = args.report.resolve()
    assert sha(args.manifest.read_bytes()) == args.manifest_sha256, "Authoritative manifest digest differs"
    authority = load(args.manifest)
    assert authority_checks(authority), "Authoritative candidate or requirements bytes differ"
    identity = load(root / "candidate-identity.json")
    asset_checks = {p: sha((Path(authority["source_root"]) / p).read_bytes()) == expected for p, expected in authority["files"].items()}
    asset_checks["manifest_matches_candidate"] = identity["relevant_asset_hashes"] == authority["files"]
    policy = load(root / "step-policy.json")
    inventory = load(root / "independent-inventory.json")
    population = {(r["asset_path"], r["step_id"]) for r in policy if r["authorised_edit_scope"]}
    structure_ok = len(population) == 40 and len(policy) == 41 and len(inventory["agent_steps"]) == 41
    regressions = root / args.regressions_report
    rows = load(regressions / "results.json") if (regressions / "results.json").exists() else []
    expected = {(Path(p).stem + "--" + sid + "--" + variant) for p, sid in population for variant in ("red", "green", "removal")}
    observed = {r["case_id"] for r in rows}
    validation = {}
    for row in rows:
        try:
            validation[row["case_id"]] = verify_case(regressions, row, authority)
        except Exception as error:
            validation[row["case_id"]] = {"failed": repr(error)}
    terminal_path = root / (args.regressions_report + ".exit.json")
    projected_ok = Counter(r["case_id"] for r in rows) == Counter(authority["cases"].keys()) and expected <= observed and all(v is True for v in validation.values()) and terminal_path.exists() and load(terminal_path)["exit_status"] == 0
    checkpoints_path = root / args.checkpoints_report / "results.json"
    checkpoints = load(checkpoints_path) if checkpoints_path.exists() else []
    checkpoint_failed = [r["step_id"] + "--" + r["variant"] for r in checkpoints if r["acceptance_result"] != "passed"]
    try:
        hooks_ok = verify_hook_population(checkpoints, authority["checkpoints"]) and all(verify_checkpoint(root / args.checkpoints_report, row, authority) for row in checkpoints)
    except Exception:
        hooks_ok = False
    distinct_path = root / args.distinct_report / "results.json"
    distinct = load(distinct_path) if distinct_path.exists() else []
    distinct_exit = root / (args.distinct_report + ".exit.json")
    try:
        distinct_ok = verify_distinct(root / args.distinct_report, distinct, authority) and distinct_exit.exists() and load(distinct_exit)["exit_status"] == 0
    except Exception:
        distinct_ok = False
    rust_path = root / args.rust_audit_receipt
    rust_receipt = load(rust_path) if rust_path.exists() else None
    rust_status = "unverified" if rust_receipt is None else "passed" if rust_receipt.get("exit_status") == 0 and args.manifest_sha256 in rust_receipt.get("command", []) else "failed or stale"
    gates = [
        ("provenance", "passed", "provenance.json"),
        ("structural_inventory", "passed" if structure_ok else "failed", "step-policy.json; independent-inventory.json"),
        ("candidate_bytes", "passed" if all(asset_checks.values()) else "failed", "candidate-identity.json"),
        ("projected_actual_runner_integrity", "passed" if projected_ok else "incomplete or failed", args.regressions_report + "/; independent-snapshot-audit.json"),
        ("distinct_caller_and_target", "passed" if distinct_ok else "incomplete or failed", args.distinct_report + "/"),
        ("native_template_removal_controls", "failed; historical evidence retained", "../hybrid-final/native/summary.json; historical native controls: unrendered staging directory prevents intended failures"),
        ("unchanged_checkpoint_safety", "failed" if checkpoint_failed else "incomplete", args.checkpoints_report + "/"),
        ("rust_receipt_audit", rust_status, args.rust_audit_receipt + "; prior rust-audit.log retains the failed duplicate-population audit"),
        ("focused_normal_and_rejecting_hooks", "passed" if hooks_ok else "incomplete or failed", args.checkpoints_report + "/; repair/evaluator dispatch excluded"),
        ("nested_terminal_adjudicator", "incomplete", "Out of edit scope; omitted opt-out remains true."),
        ("step_22_publication_owner", "unverified", "No later recipe checkpoint exists. The CI agent has an illustrated git add/commit/push protocol; guarded directory selection and path/content authority remain unverified. See ../pragmatic-v2/agent-publication-instruction-audit.json."),
        ("complete_requirement_carryforward_and_independent_approval", "unverified", "The complete original requirements remain in the initiating prompt; no independent final approval receipt was supplied."),
        ("critical_consensus_execution", "incomplete", "All required critical branches and reviews need separate execution evidence despite Refs #1966."),
        ("native_proof_qualification", "blocked", "Authorised native/proof command, 4 GiB mechanism and 595+5-second procedure are unresolved."),
        ("full_QA_and_project_hooks", "unexecuted", "Focused fixture hooks are separate from naturally completed project hooks and full QA."),
        ("resource_conditions", "incomplete", "Inspect this run's resource admission and nice receipt; sampled 4 GiB enforcement remains unestablished."),
        ("final_candidate_review_and_required_CI", "unexecuted", "No committed candidate HEAD, required CI or independent review receipt."),
        ("supported_platform_actual_auto_drive", "blocked", "Darwin has no /proc; authorised Linux execution is required. No guard bypass."),
        ("owner_integration", "unexecuted", "Separate owner authority and platform-confirmed merge evidence are required."),
    ]
    ledger = {"schema_version": 1, "candidate_identity": identity, "requirements": [{"requirement_id": key, "status": status, "evidence_or_blocker": evidence} for key, status, evidence in gates], "expected_projected_cases": len(authority["cases"]), "observed_projected_cases": len(rows), "missing_required_controls": sorted(expected - observed), "checkpoint_failures": checkpoint_failed, "delivery_acceptance": "failed or incomplete; not landed", "issue_audit": {"Refs #1554": "Automatic staging candidate is bounded; checkpoint runtime publication remains unsafe.", "Refs #1914": "Source-only staging is not established. Linked worktree, ancestry, base and identity requirements are separate and remain open."}}
    (root / "independent-snapshot-audit.json").write_text(json.dumps(validation, indent=2) + "\n")
    (root / "acceptance-ledger.json").write_text(json.dumps(ledger, indent=2) + "\n")
    print(json.dumps({"projected_integrity": projected_ok, "distinct_repositories": distinct_ok, "focused_hooks": hooks_ok, "checkpoint_failures": len(checkpoint_failed), "delivery": ledger["delivery_acceptance"]}))
    return 0 if projected_ok and distinct_ok and hooks_ok and all(asset_checks.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
