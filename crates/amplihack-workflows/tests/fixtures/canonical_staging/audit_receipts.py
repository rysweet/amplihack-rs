#!/usr/bin/env python3
"""Independently audit retained runner receipts and NUL-separated Git evidence.

This reads evidence, never invokes an agent or implements a staging decision.
It establishes bounded fixture results only, not delivery acceptance.
"""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import os


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def blob_id(contents):
    return hashlib.sha1(
        b"blob " + str(len(contents)).encode("ascii") + b"\0" + contents
    ).hexdigest().encode("ascii")


def entries(path):
    result = {}
    for entry in path.read_bytes().split(b"\0"):
        if not entry:
            continue
        header, name = entry.split(b"\t", 1)
        mode, oid, stage = header.split(b" ")
        assert stage == b"0", "Unmerged entries are outside fixture coverage."
        assert name not in result, "Duplicate staged path."
        result[name] = (mode, oid)
    return result


def saved_index_entries(path):
    with tempfile.TemporaryDirectory(prefix="index-audit-", dir=path.parent) as directory:
        repo = Path(directory)
        env = {"PATH": os.environ["PATH"], "HOME": directory, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}
        subprocess.run(["git", "init", "--bare", "--template=", str(repo)], env=env, capture_output=True, check=True)
        env["GIT_INDEX_FILE"] = str(path.resolve())
        raw = subprocess.run(["git", "--git-dir=" + str(repo), "ls-files", "--stage", "-z"], env=env, capture_output=True, check=True).stdout
        result = repo / "entries"
        result.write_bytes(raw)
        return entries(result)


def yaml_document(raw):
    return json.loads(subprocess.run(["ruby", "-e", 'require "yaml"; require "json"; puts JSON.generate(YAML.load(STDIN.read))'], input=raw, capture_output=True, check=True).stdout)


def audit(path, receipt, expected, phase, authority):
    errors = []

    def require(condition, message):
        if not condition:
            errors.append(message)

    require(receipt["exit_status"] == 0, "Runner did not succeed.")
    adapter = receipt.get("adapter_execution") or {}
    require(adapter.get("completed") is True, "Adapter did not complete.")
    repos = {Path(repo).name: repo for repo, _ in receipt["index_paths"]}
    require(set(repos) == {"caller", "review", "task"}, "Wrong fixture repository population.")
    target = repos["review"] if expected["working_dir"] == "{{review_dir}}" else repos["caller"] if expected["working_dir"] == "<runner working directory>" else repos["task"]
    require(receipt["projection_substitutions"]["working_dir"]["resolved"] == target, "Target differs from source role.")
    require(receipt["projection_substitutions"]["working_dir"]["source_template_preserved"] == authority["rust_preserve_directory_templates"], "Projection mode differs from authority.")
    require(adapter.get("cwd") == target, "Adapter executed in the wrong directory.")
    require(adapter.get("mutation") == expected["mutation"], "Wrong mutation role.")
    require(
        receipt["source_asset_sha256"] == expected["asset_sha256"],
        "Source asset hash differs from the selected candidate.",
    )
    require(digest(path.parent / "projection.yaml") == receipt["projection_sha256"],
            "Projected recipe bytes changed.")
    require(digest(path.parent / "requirements.txt") == receipt["requirements_sha256"],
            "Requirements bytes changed.")
    require(bool(receipt["runner_version"].strip()), "Runner identity is absent.")
    for field in ("runner_path", "runner_sha256", "runner_version"):
        require(receipt[field].strip() == authority["runner"][field].strip() if isinstance(receipt[field], str)
                else receipt[field] == authority["runner"][field], "Authoritative runner identity differs: " + field)
    require(receipt["requirements_sha256"] == authority["requirements_sha256"], "Authoritative requirements differ.")
    source_path = (Path(authority["baseline_root"]) / (receipt["asset"] + ".yaml") if phase == "red" else
                   Path(authority["source_root"]) / "amplifier-bundle/recipes" / (receipt["asset"] + ".yaml"))
    raw = source_path.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == expected["asset_sha256"], "Authoritative source bytes differ.")
    expected_recipe = yaml_document(raw)
    selected = next(s for s in expected_recipe["steps"] if s["id"] == receipt["case_id"])
    if phase == "removal_control":
        require(selected.pop("auto_stage") is False, "Removal source has no opt-out.")
    if not receipt["projection_substitutions"]["working_dir"]["source_template_preserved"]:
        selected["working_dir"] = target
    req = (path.parent / "requirements.txt").read_bytes()
    selected["prompt"] = "Fixture data only. Do not execute this prompt.\n\n" + req.decode("utf-8")
    expected_recipe["steps"] = [selected]
    expected_recipe["name"] = "canonical-staging-projection"
    expected_recipe.pop("output", None)
    require(yaml_document((path.parent / "projection.yaml").read_bytes()) == expected_recipe,
            "Source-to-projection derivation differs.")
    require("--no-auto-stage" not in receipt["command"], "Global staging was disabled.")
    require(receipt["command_argv"][0] == authority["runner"]["runner_path"], "Executed runner differs.")
    require("--no-auto-stage" not in receipt["command_argv"], "Global staging was disabled.")
    require(receipt["command_argv"][1] == str(path.parent / "projection.yaml"), "Executed projection differs.")
    terminal = json.loads((path.parent / "runner.stdout").read_bytes())
    require(terminal.get("success") is True and all(s["status"] == "completed" for s in terminal["step_results"]), "Runner terminal result differs.")
    require(len(receipt["index_paths"]) == 3, "Incomplete index population.")
    require(receipt["edge_entries"] == expected["_edge_entries"], "Edge-case identity differs.")
    require(len({p for _, p in receipt["index_paths"]}) == 3,
            "Caller, review and task indexes were not independent.")
    require(any("/worktrees/" in p for _, p in receipt["index_paths"]),
            "Linked-worktree index was not resolved.")
    green = phase == "green"
    target_changed = False
    for number, (repo, _) in enumerate(receipt["index_paths"]):
        label = Path(repo).name
        before_bytes = (path.parent / f"before-{number}.index").read_bytes()
        after_bytes = (path.parent / f"after-{number}.index").read_bytes()
        before = entries(path.parent / f"{label}-entries.before")
        after = entries(path.parent / f"{label}-entries.after")
        if expected["_edge_entries"]:
            require(all(name in before for name in (b"-leading", b":(glob)*", b"tab\tname", b"space name", b"renamed\nfile.txt"))
                    and b"deleted.txt" not in before and b"old name.txt" not in before, "Supplemental staged edges are absent.")
        require(saved_index_entries(path.parent / f"before-{number}.index") == before, "Raw before index and semantic evidence differ.")
        require(saved_index_entries(path.parent / f"after-{number}.index") == after, "Raw after index and semantic evidence differ.")
        require(before[b"user.txt"] == (b"100755", blob_id(b"user staged content\n")),
                "Initial partial staging or executable mode is absent.")
        require((path.parent / f"user-{number}.before.blob").read_bytes()
                == b"user staged content\n", "Initial staged blob evidence differs.")
        if green or repo != target:
            require(before_bytes == after_bytes, f"Raw index bytes changed at {repo}.")
            require(before == after, f"Staged blobs or modes changed at {repo}.")
            require((path.parent / f"{label}-user.after.blob").read_bytes()
                    == b"user staged content\n", f"User staged blob changed at {repo}.")
        if repo == target:
            target_changed = before_bytes != after_bytes
            stageable_writes = {
                b".amplihack/indexing.pid": b"4242\n",
                b".claude/runtime/sessions.jsonl": b'{"fixture":true}\n',
                b".amplihack/tracked.json": b'{"modified":true}\n',
            }
            if expected["mutation"]:
                stageable_writes.update({
                    b"source.txt": b"deferred source modification\n",
                    b"new source.txt": b"deferred new source\n",
                })
            if green:
                require(receipt.get("fixture_observations_valid") is True,
                        "Working-tree observations were not verified.")
                observed = {x["path"].encode(): bytes(x["working_tree_bytes"] or [])
                            for x in receipt.get("observable_writes", [])}
                require(observed == stageable_writes, "Metadata or source writes are missing.")
            else:
                # The historical baseline predates the working-tree receipt
                # fields. Its staged blobs independently prove the exact writes.
                for name, content in stageable_writes.items():
                    require(after.get(name) == (b"100644", blob_id(content)),
                            f"Expected stageable write was not exercised: {name!r}.")
    if green:
        require(not receipt["violations"], "Runner fixture reported an integrity failure.")
        require(receipt["raw_index_equal"] is True, "Raw equality receipt disagrees.")
        require(receipt["semantic_index_equal"] is True, "Semantic equality receipt disagrees.")
    else:
        require(target_changed, "The intended target index did not change.")
        require(any(x.startswith("index-integrity:") for x in receipt["violations"]),
                "Control did not fail its intended integrity assertion.")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report_root", type=Path)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-sha256", required=True)
    args = parser.parse_args()
    report = args.report_root.resolve(strict=True)
    assert digest(args.manifest) == args.manifest_sha256, "Authoritative manifest digest differs."
    authority = json.loads(args.manifest.read_bytes())
    for name, expected_hash in authority["files"].items():
        assert digest(Path(authority["source_root"]) / name) == expected_hash, "Authoritative candidate differs."
    assert digest(Path(authority["requirements_file"])) == authority["requirements_sha256"]
    fixtures = Path(__file__).resolve().parent
    source_root = fixtures.parents[4]
    inventory = json.loads((fixtures / "inventory.json").read_text())
    required = {(x["asset"], x["step_id"]): x for x in inventory if x["authorised_edit"]}
    assert set(required) == {tuple(pair) for pair in authority["rust_population"]}, "Authoritative population differs."
    cases = []
    failures = []
    superseded = []
    amendment_path = report / "evidence-amendments.json"
    amendments = {}
    if amendment_path.exists():
        for row in json.loads(amendment_path.read_text()):
            key = (row["asset"], row["step_id"])
            assert key in required and key not in amendments, "Invalid evidence amendment."
            assert row["reason"].strip(), "Amendment must disclose its reason."
            amendments[key] = row
    for directory, phase, variant in [
        ("baseline", "red", "Actual"),
        ("corrected", "green", "Actual"),
        ("corrected", "removal_control", "RemovedOptOut"),
    ]:
        population = set()
        supplemental_population = set()
        directory_key = "baseline_directory" if phase == "red" else "corrected_directory"
        directories = [directory] + sorted({row[directory_key] for row in amendments.values()})
        paths = []
        for selected_directory in directories:
            selected = (report / selected_directory).resolve()
            assert selected.is_relative_to(report), "Amendment escapes the report root."
            paths.extend(sorted(selected.glob("*/receipt.json")))
        for path in paths:
            try:
                receipt = json.loads(path.read_text())
                if receipt["variant"] != variant:
                    continue
                key = (receipt["asset"], receipt["case_id"])
                assert key in required, f"Unexpected case: {key}."
                amendment = amendments.get(key)
                if amendment and path.parent.parent.name == directory:
                    superseded.append({"phase":phase, "receipt":str(path.relative_to(report)),
                                       "reason":amendment["reason"]})
                    continue
                if path.parent.parent.name != directory:
                    assert amendment and path.parent.parent.name == amendment[directory_key], \
                        "Amended receipt does not match its declared population."
                edge_entries = receipt["edge_entries"]
                if edge_entries:
                    assert phase != "removal_control" and [key[0], key[1], variant] in authority["rust_supplemental"], "Unplanned supplemental case."
                    assert key not in supplemental_population, "Duplicate supplemental case."
                    supplemental_population.add(key)
                else:
                    assert key not in population, "Duplicate evidence case."
                    population.add(key)
                expected = dict(required[key])
                expected["_edge_entries"] = edge_entries
                if phase != "red":
                    expected["asset_sha256"] = authority["files"]["amplifier-bundle/recipes/" + key[0] + ".yaml"]
                else:
                    expected["asset_sha256"] = authority["baseline_assets"][key[0]]
                errors = audit(path, receipt, expected, phase, authority)
                cases.append({"phase": phase, "asset": key[0], "step_id": key[1], "supplemental":edge_entries,
                              "receipt": str(path.relative_to(report)),
                              "receipt_sha256": digest(path), "errors": errors})
                failures.extend(f"{phase}/{key[1]}: {error}" for error in errors)
            except (AssertionError, KeyError, OSError, ValueError, TypeError, subprocess.CalledProcessError) as error:
                failures.append(f"{path}: invalid evidence: {error}")
        for asset, step_id in sorted(set(required) - population):
            failures.append(f"{phase}: missing {asset}/{step_id}.")
        expected_supplements = {(asset, sid) for asset, sid, _ in authority["rust_supplemental"]} if phase != "removal_control" else set()
        for asset, step_id in sorted(expected_supplements - supplemental_population):
            failures.append(f"{phase}: missing supplemental {asset}/{step_id}.")
    result = {"schema_version": 1, "scope": "Bounded agent-stage integrity only.",
              "planned_population_per_phase": len(required), "cases": cases,
              "superseded_receipts_retained":superseded,
              "failures": failures, "passed": not failures}
    print(json.dumps(result, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
