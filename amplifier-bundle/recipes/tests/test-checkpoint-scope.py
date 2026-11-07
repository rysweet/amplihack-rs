#!/usr/bin/env python3
"""Bounded checkpoint contract tests for #1554/#1914 (standard library only).

Run this file only in the declared deterministic recipe test step, under the
existing sampled four-GiB observer. The tester does not execute it. The input
must be RED: strict staging and the three extracted checkpoints are absent.
There are no expected failures, unconditional failures or production substitutes.

The private fixture wire convention is deliberately small. New phase capture
steps emit an object with ``binding``, ``paths`` and ``outgoing_commits``. Each
path entry has ``path_b64``, ``mode``, ``type`` and ``sha256`` (null for a
deletion). The binding has ``owner_task``, ``phase``, ``invocation``, ``worktree``,
``git_dir``, ``common_dir``, ``index_path``, ``branch``, ``initial_head`` and
``candidate_digest``. The fixture reviewer returns ``approved`` (a boolean),
the complete ``binding``, ``approved_paths`` (base64 strings) and ``reason``.
The recipe selects output names and passes them to its real shell helpers.
This is a private test interface, not an extra service or generic scope engine.

Only new deterministic phase steps and the existing checkpoint are projected.
Earlier agents are replaced by explicit synthetic owner context, not executed.
The read-only review fixture approves only paths named independently by that
owner before capture. It never derives permission from expected test results.
Actual checkpoint scripts, staging, repair, Git commits, hooks and local bare
remotes run. A private amplihack executable simulates ONLY the fixing actor,
loop-health responses and hygiene dispatch. No installed orchestration, model,
provider, build, Cargo command or network transport is used. Hygiene integration
and model judgement are therefore outside these tests' evidence.

The projection is diagnostic shell integration, never native runner acceptance.
The original 40 removal controls retain their meaning; issue 201 leaves 25
native controls unverified. Four additional opt-out controls are counted
separately. Review, normal quality assurance, CI, runner-owner disposition and
owner integration remain external acceptance requirements. Design consensus,
status SUCCESS and a passing process cannot grant implementation acceptance.
The earlier false consensus and native compile 124 evidence remain untouched.
Terminal 79 is terminal. is_critical_code remains the string "true".
"""

import base64
import copy
import errno
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[3]
RECIPES = ROOT / "amplifier-bundle/recipes"
TOOLS = ROOT / "amplifier-bundle/tools"
SCOPE_AUDIT_BASELINE = "606f3c746e51b8ef66d75380b41791b9f3a9ed8e"
APPROVED_CARGO_SHA256 = {'Cargo.lock': 'bce4b9a564985ec644a5e9c03b079ad5702d5f78e4b290987674a3b92a277c49', 'crates/amplihack-workflows/Cargo.toml': 'd8fa2ee0ca9c8d532c6c27795ec9fa6968d6b046c6c7adfca801dfc15f1e825d'}
MAINTENANCE_PATHS_FILE = ROOT / "crates/amplihack-workflows/tests/fixtures/canonical_staging/maintenance_paths.json"


def maintenance_inventory():
    paths = json.loads(MAINTENANCE_PATHS_FILE.read_text())
    if not isinstance(paths, list) or not all(isinstance(path, str) for path in paths):
        raise ValueError("Maintenance inventory must be a typed string array.")
    if len(paths) != len(set(paths)):
        raise ValueError("Duplicate maintenance entries are not authority.")
    return frozenset(paths)


INTEGRATION_MAINTENANCE_PATHS = maintenance_inventory()
EXISTING_TASK_TEST_PATHS = frozenset((
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/maintenance_paths.json',
    'amplifier-bundle/recipes/tests/canonical-staging.md',
    'amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py',
    'amplifier-bundle/recipes/tests/canonical_staging_conservative_delta.md',
    'amplifier-bundle/recipes/tests/canonical_staging_publication.py',
    'amplifier-bundle/recipes/tests/canonical_staging_synthesis.md',
    'amplifier-bundle/recipes/tests/checkpoint-pre-delta-assets.json',
    'amplifier-bundle/recipes/tests/minimalist-auto-stage.md',
    'amplifier-bundle/recipes/tests/test-canonical-staging-harness.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-publication-boundaries.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-rebase-identity.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-refusal-evidence.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-review-boundaries.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-scope-history.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-scope.py',
    'amplifier-bundle/recipes/tests/test-checkpoint-scoped-format.sh',
    'amplifier-bundle/recipes/tests/test-checkpoint-synthesis-alias.py',
    'amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py',
    'amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py',
    'amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py',
    'amplifier-bundle/recipes/tests/test-pragmatic-crusty-directories.py',
    'amplifier-bundle/recipes/tests/test-pragmatic-evidence.py',
    'crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs',
    'crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs',
    'crates/amplihack-workflows/tests/canonical_staging_policy_test.rs',
    'crates/amplihack-workflows/tests/canonical_staging_runner_test.rs',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/README.md',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/baseline_asset_sha256.json',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/coverage.json',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/inventory.json',
    'crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py',
    'crates/amplihack-workflows/tests/support/canonical_staging.rs',
))
SPECIFICATION_SHA256 = "4201c83f262f0a85ff858957f80649e04493618442acda551aa3cf63e631fe66"
PHASES = (
    ("workflow-publish", "step-15-commit-push", "workflow_publish_checkpoint.sh", "commit_result"),
    ("workflow-pr-review", "step-18c-push-feedback-changes", "workflow_review_checkpoint.sh", "feedback_push_result"),
    ("workflow-finalize", "step-20b-push-cleanup", "workflow_cleanup_checkpoint.sh", "cleanup_push_result"),
)
ASSETS = (
    "autodrive-crusty-round", "workflow-prep", "workflow-design", "workflow-tdd",
    "workflow-refactor-review", "workflow-precommit-test", "workflow-publish",
    "workflow-pr-review", "workflow-finalize",
)
BINDING_FIELDS = (
    "owner_task", "phase", "invocation", "worktree", "git_dir", "common_dir",
    "index_path", "branch", "initial_head", "candidate_digest",
)


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(path):
    return base64.b64encode(os.fsencode(path)).decode("ascii")


def json_bytes(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def blocks(text):
    """Read the repository's canonical block layout, without a YAML dependency."""
    matches = list(re.finditer(r'^  - id: "([^"]+)"\s*$', text, re.M))
    return {match[1]: text[match.start():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
            for i, match in enumerate(matches)}


def scalar(block, key, default=None):
    match = re.search(r"^    " + re.escape(key) + r": (.*?)\s*$", block, re.M)
    if match is None:
        return default
    raw = match[1]
    if raw in ("true", "false"):
        return raw == "true"
    if raw.startswith('"'):
        return json.loads(raw)
    return raw.strip("'")


def literal(block, key):
    match = re.search(r"^    " + re.escape(key) + r": \|\n((?:(?: {6,}.*)?\n)*)", block, re.M)
    if match is None:
        raise AssertionError("A canonical literal " + key + " block is required.")
    return textwrap.dedent(match[1])


def index_source(path):
    """Read the hash-bound retained input, independent of the current Git index."""
    raw = (ROOT / "crates/amplihack-workflows/tests/fixtures/canonical_staging/checkpoint-pre-delta-assets.json").read_bytes()
    if digest(raw) != "112ea7497a96d7d0257c409c74bb302df6693e6503770d01e63613783191d706":
        raise AssertionError("The retained pre-delta asset fixture changed.")
    entry = json.loads(raw)["assets"][path]
    if digest(entry["text"].encode()) != entry["sha256"]:
        raise AssertionError("The retained pre-delta asset bytes changed: " + path)
    return entry["text"]


def isolated_environment():
    # The tests never consult personal Git configuration, profiles or credentials.
    identity_name = os.environ.get("GIT_AUTHOR_NAME", "Amplihack Fixture")
    identity_email = os.environ.get("GIT_AUTHOR_EMAIL", "123456+amplihack-fixture@users.noreply.github.com")
    return {
        "PATH": os.environ.get("PATH", os.defpath), "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0",
        "GIT_ALLOW_PROTOCOL": "file", "GIT_PAGER": "cat", "PAGER": "cat",
        **{prefix + suffix: os.environ.get(prefix + suffix, identity_name if suffix == "_NAME" else identity_email)
           for prefix in ("GIT_AUTHOR", "GIT_COMMITTER", "AMPLIHACK_GIT_AUTHOR", "AMPLIHACK_GIT_COMMITTER")
           for suffix in ("_NAME", "_EMAIL")},
    }


class GitFixture:
    """Each case owns a new caller, linked task, hooks, private home and remote."""

    def __init__(self, case, branch="task/checkpoint", object_format=None):
        self.case = case
        temporary_root = Path(tempfile.gettempdir()).resolve()
        self.temp = tempfile.TemporaryDirectory(prefix="checkpoint-scope-", dir=temporary_root)
        case.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.caller, self.task, self.remote = (self.root / p for p in ("caller", "linked task", "remote.git"))
        self.private, self.bin, self.home, self.hooks = (self.root / p for p in ("private", "bin", "home", "hooks"))
        for directory in (self.private, self.bin, self.home, self.hooks):
            directory.mkdir()
        self.env = dict(isolated_environment(), HOME=str(self.home), TMPDIR=str(self.private))
        if object_format is not None:
            case.assertIn(object_format, ("sha1", "sha256"))
            self.env["GIT_DEFAULT_HASH"] = object_format
        self.git_binary = shutil.which("git", path=self.env["PATH"])
        self.bash = shutil.which("bash", path=self.env["PATH"])
        case.assertIsNotNone(self.git_binary, "Git is a required fixture prerequisite.")
        case.assertIsNotNone(self.bash, "Bash is a required fixture prerequisite.")
        self.run([self.git_binary, "init", "-b", "fixture-base", str(self.caller)])
        self.git(self.caller, "config", "user.name", self.env["GIT_AUTHOR_NAME"])
        self.git(self.caller, "config", "user.email", self.env["GIT_AUTHOR_EMAIL"])
        self.git(self.caller, "config", "commit.gpgsign", "false")
        self.git(self.caller, "config", "core.filemode", "true")
        for name in (b"source.txt", b"unrelated.txt", b"delete.txt", b"rename-old.txt"):
            self.write(name, b"initial\n", self.caller)
        self.git(self.caller, "add", "--all")
        self.git(self.caller, "commit", "-m", "independent fixture base")
        self.run([self.git_binary, "init", "--bare", str(self.remote)])
        self.git(self.caller, "remote", "add", "origin", str(self.remote))
        self.git(self.caller, "push", "-u", "origin", "fixture-base")
        self.git(self.caller, "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/fixture-base")
        self.git(self.caller, "worktree", "add", "-b", branch, str(self.task))
        self.git(self.task, "push", "-u", "origin", branch)
        self.branch = branch
        self.base = self.git(self.task, "rev-parse", "HEAD").strip().decode()
        self.git(self.task, "config", "core.hooksPath", str(self.hooks))
        self.objects_before = self.commits()
        self.allowed = {b"source.txt"}
        self.owner = "fixture owner #1554: change only literal paths " + encoded(b"source.txt")
        self.context = {
            "repo_path": str(self.caller), "task_description": self.owner,
            "final_requirements": self.owner, "issue_number": "1554", "pr_number": "1",
            "is_critical_code": "true", "autodrive_launch_tools": str(TOOLS),
            "version_bump": {"change_classification": "PATCH"},
            "worktree_setup": {"worktree_path": str(self.task), "branch_name": branch,
                               "base_ref": "origin/fixture-base", "base_branch": "fixture-base", "created": True},
        }
        self.env.update(AMPLIHACK_HOME=str(ROOT), REPO_PATH=str(self.caller),
                        TASK_DESCRIPTION=self.owner, ISSUE_NUMBER="1554", REMOTE_HOST_TYPE="other",
                        SCOPE_FIXTURE_ROOT=str(self.root), SCOPE_FIXTURE_TASK=str(self.task))
        self.install_actor()
        self.sync_context()

    def run(self, argv, cwd=None, env=None, check=True, data=None):
        result = subprocess.run([os.fsdecode(x) for x in argv], cwd=cwd or self.root,
                                env=env or self.env, input=data, capture_output=True, timeout=25)
        if check:
            self.case.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))
        return result

    def git(self, repo, *args, check=True, data=None):
        result = self.run([self.git_binary, "-C", str(repo), *args], check=check, data=data)
        return result.stdout if check else result

    def write(self, name, contents, repo=None):
        path = os.fsencode(repo or self.task) + b"/" + os.fsencode(name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        try:
            stream = open(path, "wb")
        except OSError as error:
            if error.errno == errno.EILSEQ:
                try:
                    os.fsencode(name).decode("utf-8", errors="strict")
                except UnicodeDecodeError:
                    self.case.skipTest("Filesystem cannot create an invalid UTF-8 byte filename (EILSEQ); this case is unexecuted.")
            raise
        with stream:
            stream.write(contents)

    def unlink(self, name):
        os.unlink(os.fsencode(self.task) + b"/" + os.fsencode(name))

    def executable(self, path, body):
        path.write_text("#!/bin/sh\nset -eu\n" + body)
        path.chmod(0o755)

    def install_actor(self):
        # No external actor is invoked. This program changes only test-owned files.
        actor = self.bin / "amplihack"
        self.executable(actor, r'''
case "$1 $2" in
  'hygiene artifact-guard') exit 0 ;;
  'recipe show') exit 0 ;;
  'recipe run')
    case "$3" in
      *loop-health-evaluator*)
        printf 'health\n' >> "$SCOPE_FIXTURE_ROOT/actor.log"
        if [ "${SCOPE_FIXTURE_HEALTH:-STUCK}" = TERMINAL ]; then exit 79; fi
        printf 'LOOP_HEALTH: %s fixture decision\n' "${SCOPE_FIXTURE_HEALTH:-STUCK}"
        exit 0 ;;
      *hook-rejection-repair*)
        printf 'repair\n' >> "$SCOPE_FIXTURE_ROOT/actor.log"
        if [ "${SCOPE_FIXTURE_REPAIR:-none}" = terminal ]; then exit 79; fi
        if [ "${SCOPE_FIXTURE_REPAIR:-none}" = repair-terminal ]; then
          printf 'still-rejected\n' > "$SCOPE_FIXTURE_TASK/source.txt"
        elif [ "${SCOPE_FIXTURE_REPAIR:-none}" != none ]; then
          printf 'fixed\n' > "$SCOPE_FIXTURE_TASK/source.txt"
        fi
        case "${SCOPE_FIXTURE_REPAIR:-none}" in
          unknown|unknown-staged)
            printf 'unknown repair source\n' > "$SCOPE_FIXTURE_TASK/new-path.txt"
            if [ "$SCOPE_FIXTURE_REPAIR" = unknown-staged ]; then
              git -C "$SCOPE_FIXTURE_TASK" add -- new-path.txt
            fi ;;
        esac
        printf '{"hook_repair":{"status":"fixed","class":"code","summary":"Fixture changed code."}}\n'
        exit 0 ;;
    esac ;;
esac
printf 'Fixture forbids this dispatch.\n' >&2
exit 97
''')
        self.executable(self.bin / "gh", r'''
if [ "$*" = 'pr view 1 --json headRefName --jq .headRefName' ]; then
  git -C "$SCOPE_FIXTURE_TASK" branch --show-current
else
  printf 'Fixture forbids provider access.\n' >&2
  exit 97
fi
''')
        for forbidden in ("cargo", "ssh", "curl", "docker", "kubectl", "claude", "copilot"):
            self.executable(self.bin / forbidden, "printf 'Forbidden fixture dispatch.\\n' >&2\nexit 97\n")
        self.env["PATH"] = str(self.bin) + os.pathsep + self.env["PATH"]
        self.env["AMPLIHACK_BIN"] = str(actor)

    def sync_context(self):
        for key, value in self.context.items():
            self.env["RECIPE_VAR_" + key] = json.dumps(value) if isinstance(value, (dict, list, bool)) else str(value)

    def own(self, paths):
        self.allowed = set(map(os.fsencode, paths))
        self.owner = "fixture owner #1554: change only literal paths " + ",".join(encoded(p) for p in sorted(self.allowed))
        for key in ("task_description", "final_requirements"):
            self.context[key] = self.owner
        self.env["TASK_DESCRIPTION"] = self.owner
        self.sync_context()

    def shell(self, body, *args, env=None):
        return self.run([self.bash, "-c", body, "checkpoint-fixture", *args], cwd=self.task,
                        env=env or self.env, check=False)

    def stage(self, paths=None, raw=None, arguments=None):
        path_list = self.private / "literal-paths.nul"
        path_list.write_bytes(raw if raw is not None else b"".join(os.fsencode(p) + b"\0" for p in (paths or [])))
        argv = arguments if arguments is not None else ["--strict", "--path-list", str(path_list)]
        return self.shell('set -euo pipefail\n. "$1"\nshift\nstage_workflow_changes "$@"',
                          str(TOOLS / "workflow_stage_changes.sh"), *argv)

    def staged_paths(self):
        return set(filter(None, self.git(self.task, "diff", "--cached", "--name-only", "--no-renames", "-z").split(b"\0")))

    def commits(self):
        rows = self.git(self.task, "cat-file", "--batch-all-objects", "--batch-check=%(objectname) %(objecttype)")
        return {row.split()[0].decode() for row in rows.splitlines() if row.split()[1] == b"commit"}

    def snapshot(self):
        """Take raw index, staged blob/mode, worktree and HEAD evidence without refresh."""
        entries = self.git(self.task, "ls-files", "--stage", "-z")
        blobs = {}
        for row in filter(None, entries.split(b"\0")):
            mode, oid, stage_number = row.split(b"\t", 1)[0].split()
            if oid.strip(b"0"):
                blobs[oid] = self.git(self.task, "cat-file", "blob", oid.decode())
        index = Path(os.fsdecode(self.git(self.task, "rev-parse", "--git-path", "index").strip()))
        if not index.is_absolute():
            index = self.task / index
        tree = {}
        for parent, directories, files in os.walk(os.fsencode(self.task), followlinks=False):
            for name in directories[:] + files:
                full = os.path.join(parent, name)
                relative = os.path.relpath(full, os.fsencode(self.task))
                if relative == b".git":
                    continue
                info = os.lstat(full)
                if stat.S_ISLNK(info.st_mode):
                    tree[relative] = (stat.S_IMODE(info.st_mode), "link", os.readlink(full))
                    if name in directories:
                        directories.remove(name)
                elif stat.S_ISREG(info.st_mode):
                    with open(full, "rb") as stream:
                        tree[relative] = (stat.S_IMODE(info.st_mode), "file", stream.read())
                else:
                    tree[relative] = (stat.S_IMODE(info.st_mode), "directory", b"")
        return {"raw_index": index.read_bytes(), "entries": entries, "blobs": blobs, "tree": tree,
                "head": self.git(self.task, "rev-parse", "HEAD"), "commits": self.commits(),
                "shared_indexes": {str(p): p.read_bytes() for p in index.parent.glob("sharedindex.*")},
                "remote": self.git(self.remote, "show-ref")}

    def hook(self, name="pre-commit", reject=False, repairable=False, extra=""):
        log = shlex.quote(str(self.root / (name + ".log")))
        body = 'printf "hook\\n" >> ' + log + "\n" + extra
        if repairable:
            body += "if ! grep -qx fixed source.txt; then printf '%s\\n' '- hook id: fixture' >&2; exit 1; fi\n"
        elif reject:
            body += "printf '%s\\n' '- hook id: fixture' 'fixture hook rejected' >&2\nexit 1\n"
        self.executable(self.hooks / name, body)

    def remote_reject(self):
        self.executable(self.remote / "hooks/pre-receive", "printf 'fixture transport rejection\\n' >&2\nexit 1\n")

    def history(self, start=None, end="HEAD"):
        """Inspect every parent separately; a final-tree comparison is insufficient."""
        rows = self.git(self.task, "rev-list", "--parents", (start or self.base) + ".." + end).splitlines()
        result = []
        for row in rows:
            commit, *parents = row.decode().split()
            for parent in parents:
                paths = self.git(self.task, "diff-tree", "--no-commit-id", "--name-only", "--no-renames", "-r", "-z", parent, commit)
                result.append((commit, parent, set(filter(None, paths.split(b"\0")))))
        return result


def projected_json_output(text):
    """Match the pinned runner's direct, fenced and first-balanced JSON rules.

    Checkpoint output may include the original hygiene information before its
    result. The real 4d5187e runner accepts that framing; a strict whole-stream
    decoder would falsely reject an otherwise valid projected checkpoint.
    """
    def finite_number(raw):
        number = float(raw)
        if not math.isfinite(number):
            raise ValueError("Non-finite JSON number")
        return number

    def integer_number(raw):
        number = int(raw)
        return number if -(2 ** 63) <= number < 2 ** 64 else finite_number(raw)

    def invalid_constant(_raw):
        raise ValueError("Non-JSON numeric constant")

    def unicode_strings(value):
        if isinstance(value, str):
            value.encode("utf-8", errors="strict")
        elif isinstance(value, dict):
            for key, item in value.items():
                unicode_strings(key)
                unicode_strings(item)
        elif isinstance(value, list):
            for item in value:
                unicode_strings(item)

    def object_pairs(pairs):
        # Validate every parsed pair before normal last-key-wins construction.
        # Otherwise a later duplicate can hide an invalid earlier string.
        for key, value in pairs:
            unicode_strings(key)
            unicode_strings(value)
        return dict(pairs)

    def decode(raw):
        value = json.loads(raw, parse_float=finite_number,
                           parse_int=integer_number, parse_constant=invalid_constant,
                           object_pairs_hook=object_pairs)
        # Python accepts unpaired surrogate escapes; the pinned Rust parser
        # rejects them. Validate keys and nested strings before using a result.
        unicode_strings(value)
        return value

    text = text.strip()
    try:
        return decode(text)
    except ValueError:
        pass
    fence = re.search(r"```(?:json)?\s*\n?(.*?)\n?\s*```", text, re.S)
    if fence:
        try:
            return decode(fence[1].strip())
        except ValueError:
            pass
    for opening, closing in (("{", "}"), ("[", "]")):
        start = text.find(opening)
        if start < 0:
            continue
        depth, quoted, escaped = 0, False, False
        for offset, char in enumerate(text[start:]):
            if escaped:
                escaped = False
                continue
            if char == "\\":
                escaped = True
                continue
            if char == '"':
                quoted = not quoted
                continue
            if quoted:
                continue
            if char == opening:
                depth += 1
            elif char == closing:
                depth -= 1
                if depth == 0:
                    try:
                        return decode(text[start:start + offset + 1])
                    except ValueError:
                        break
    raise ValueError("No valid direct, fenced or first-balanced JSON output")


class ProjectionJSONTests(unittest.TestCase):
    def test_direct_and_original_hygiene_prefix_preserve_exact_result(self):
        result = {"pushed": "true", "sha": "a" * 40, "branch": "feature", "reason": "already-pushed"}
        for prefix in ("", "Artifact check clean.\nArtifact check clean.\n"):
            self.assertEqual(projected_json_output(prefix + json.dumps(result)), result)

    def test_fenced_and_balanced_outputs_preserve_nested_and_quoted_data(self):
        result = {"nested": [1, {"message": 'a quoted "}" remains data'}]}
        for text in ("```json\n" + json.dumps(result) + "\n```", "Info " + json.dumps(result) + " done"):
            self.assertEqual(projected_json_output(text), result)

    def test_empty_truncated_and_malformed_outputs_remain_refused(self):
        for text in ("", "Artifact check clean.\n", '{"pushed":', "prefix {broken} then {}",
                     "NaN", "Infinity", '{"value": 1e400}'):
            with self.assertRaises(ValueError):
                projected_json_output(text)

    def test_unicode_pairs_pass_and_lone_surrogates_remain_refused(self):
        paired = r'{"value":"\ud83d\ude00"}'
        self.assertEqual(projected_json_output(paired), {"value": "\N{GRINNING FACE}"})
        for invalid in (r'{"value":"\ud800"}', r'{"value":"\udc00"}',
                        r'{"\ud800":"value"}', r'{"nested":["\udc00"]}'):
            for text in (invalid, "Information.\n" + invalid, "```json\n" + invalid + "\n```"):
                with self.assertRaises(ValueError):
                    projected_json_output(text)

    def test_duplicate_values_are_validated_before_last_key_wins(self):
        for invalid in (r'{"value":"\ud800","value":"valid"}',
                        r'{"value":{"nested":"\ud800"},"value":"valid"}'):
            with self.assertRaises(ValueError):
                projected_json_output(invalid)
        self.assertEqual(projected_json_output('{"value":"first","value":"valid"}'),
                         {"value": "valid"})
        self.assertEqual(projected_json_output(r'{"value":"\\ud800"}'),
                         {"value": r"\ud800"})


class PhaseProjection:
    """Run only the bounded delta, never the native runner or the full phase."""

    def __init__(self, fixture, phase):
        self.f = fixture
        self.asset, self.checkpoint, self.helper, self.output = phase
        self.current = blocks((RECIPES / (self.asset + ".yaml")).read_text())
        self.original = blocks(index_source("amplifier-bundle/recipes/" + self.asset + ".yaml"))
        self.new = [sid for sid in self.current if sid not in self.original]
        self.reviewers = [sid for sid in self.new if scalar(self.current[sid], "agent") is not None]
        fixture.case.assertEqual(len(self.reviewers), 1, "Each phase needs exactly one new read-only scope reviewer.")
        self.reviewer = self.reviewers[0]
        ordered = list(self.current)
        fixture.case.assertLess(ordered.index(self.reviewer), ordered.index(self.checkpoint))
        self.before = [sid for sid in self.new if ordered.index(sid) < ordered.index(self.reviewer)]
        self.after = [sid for sid in self.new if ordered.index(self.reviewer) < ordered.index(sid) < ordered.index(self.checkpoint)]
        fixture.case.assertTrue(self.before, "A deterministic capture must precede the scope reviewer.")
        self.capture = None
        self.skipped = None

    def render(self, command):
        def replace(match):
            value = self.f.context
            for part in match[1].strip().split("."):
                if not isinstance(value, dict) or part not in value:
                    raise AssertionError("Unbound fixture template: " + match[1])
                value = value[part]
            return json.dumps(value) if isinstance(value, (dict, list, bool)) else str(value)
        return re.sub(r"\{\{([^{}]+)\}\}", replace, command)

    def execute(self, sid):
        block = self.current[sid]
        self.f.case.assertEqual(scalar(block, "type"), "bash")
        result = self.f.shell(self.render(literal(block, "command")))
        output = scalar(block, "output")
        if output and result.returncode == 0:
            value = result.stdout.decode(errors="surrogateescape").strip()
            if scalar(block, "parse_json") is True:
                try:
                    value = projected_json_output(value)
                except ValueError as error:
                    raise AssertionError("Malformed deterministic output for " + sid + ": " + str(error)) from error
            self.f.context[output] = value
            self.f.sync_context()
        return result

    def capture_scope(self):
        for sid in self.before:
            result = self.execute(sid)
            if result.returncode:
                return result
            value = self.f.context.get(scalar(self.current[sid], "output"))
            if isinstance(value, dict) and value.get("skipped") is True:
                self.f.case.assertIs(value.get("admitted"), False)
                self.f.case.assertEqual(set(value), {"skipped", "admitted", "reason"})
                self.skipped = value
                return result
            if isinstance(value, dict) and {"binding", "paths", "outgoing_commits"} <= value.keys():
                self.capture = value
        self.f.case.assertIsNotNone(self.capture, "The deterministic capture must expose typed, lossless identities.")
        return result

    def verdict(self):
        f = self.f
        f.case.assertIsNotNone(self.capture)
        paths = [base64.b64decode(row["path_b64"], validate=True) for row in self.capture["paths"]]
        f.case.assertEqual(paths, sorted(set(paths)), "Capture ordering must preserve unique filename bytes.")
        binding = self.capture["binding"]
        f.case.assertTrue(set(BINDING_FIELDS) <= binding.keys())
        f.case.assertEqual(binding["owner_task"], f.owner)
        f.case.assertEqual(binding["worktree"], str(f.task))
        f.case.assertEqual(binding["branch"], f.branch)
        f.case.assertEqual(binding["initial_head"], f.git(f.task, "rev-parse", "HEAD").strip().decode())
        f.case.assertEqual(binding["git_dir"], f.git(f.task, "rev-parse", "--absolute-git-dir").strip().decode())
        f.case.assertRegex(binding["candidate_digest"], r"^[0-9a-f]{64}$")
        # This permission set predates capture and comes from the synthetic owner.
        return {"approved": set(paths) <= f.allowed, "binding": copy.deepcopy(binding),
                "approved_paths": [encoded(p) for p in paths if p in f.allowed],
                "reason": "The fixture owner grants these finite task paths; this is not code approval."}

    def finish(self, verdict):
        output = scalar(self.current[self.reviewer], "output")
        self.f.case.assertIsNotNone(output)
        self.f.context[output] = verdict
        self.f.sync_context()
        for sid in self.after + [self.checkpoint]:
            result = self.execute(sid)
            if result.returncode:
                return result
        return result

    def run(self, change=None):
        result = self.capture_scope()
        if result.returncode:
            return result
        if self.skipped is not None:
            # An existing no-operation cleanup skip never invents a reviewer
            # vote, path permission or admitted checkpoint.
            return self.execute(self.checkpoint)
        verdict = self.verdict()
        if change:
            verdict = change(verdict)
        return self.finish(verdict)


class FilenameCapabilityTests(unittest.TestCase):
    def test_unconstructible_invalid_utf8_is_explicitly_unexecuted(self):
        fixture = GitFixture(self)
        with patch("builtins.open", side_effect=OSError(errno.EILSEQ, "controlled creation refusal")):
            with self.assertRaises(unittest.SkipTest) as unavailable:
                fixture.write(b"invalid-\xff", b"fixture bytes\n")
        self.assertIn("unexecuted", str(unavailable.exception))

    def test_other_creation_and_write_errors_remain_failures(self):
        fixture = GitFixture(self)
        cases = [(b"valid-name", errno.EILSEQ), (b"invalid-\xff", errno.EACCES),
                 (b"invalid-\xff", errno.ENOSPC), (b"invalid-\xff", errno.EROFS)]
        for name, code in cases:
            with self.subTest(name=name, errno=code):
                with patch("builtins.open", side_effect=OSError(code, "controlled creation failure")):
                    with self.assertRaises(OSError) as failure:
                        fixture.write(name, b"fixture bytes\n")
                self.assertEqual(failure.exception.errno, code)
        # An error after successful creation is not an unavailable filename.
        with patch("builtins.open") as opened:
            # Real file context managers return the same stream from __enter__.
            opened.return_value.__enter__.return_value = opened.return_value
            opened.return_value.write.side_effect = OSError(errno.EILSEQ, "controlled write failure")
            with self.assertRaises(OSError) as failure:
                fixture.write(b"invalid-\xff", b"fixture bytes\n")
        self.assertEqual(failure.exception.errno, errno.EILSEQ)


class StrictStagingTests(unittest.TestCase):
    def setUp(self):
        self.f = GitFixture(self)
        self.f.write(b"source.txt", b"task change\n")
        self.f.write(b"unrelated.txt", b"unrelated unstaged\n")
        self.f.write(b"untracked.txt", b"unrelated new\n")

    def assert_refusal(self, result, before):
        self.assertNotEqual(result.returncode, 0, "Invalid strict input must visibly refuse.")
        self.assertEqual(self.f.snapshot(), before, "Validation must finish before the first index write.")

    def test_only_exact_reviewed_files_are_staged(self):
        result = self.f.stage([b"source.txt"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.staged_paths(), {b"source.txt"})

    def test_explicit_empty_list_stages_nothing(self):
        before = self.f.snapshot()
        result = self.f.stage([])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.snapshot(), before)

    def test_missing_strict_arguments_never_fall_back(self):
        for argv in (["--strict"], ["--strict", "--path-list"], ["--path-list", "missing"],
                     ["--strict", "--path-list", str(self.f.private / "absent")], ["--strict", "--unknown"]):
            with self.subTest(arguments=argv):
                before = self.f.snapshot()
                self.assert_refusal(self.f.stage(arguments=argv), before)

    def test_unterminated_or_empty_path_records_refuse_before_writing(self):
        for raw in (b"source.txt", b"source.txt\0untracked.txt", b"source.txt\0\0"):
            with self.subTest(raw=raw):
                before = self.f.snapshot()
                self.assert_refusal(self.f.stage(raw=raw), before)

    def test_complete_list_is_validated_before_any_write(self):
        before = self.f.snapshot()
        self.assert_refusal(self.f.stage([b"source.txt", b"../outside"]), before)

    def test_directory_absolute_and_parent_selectors_refuse(self):
        (self.f.task / "directory").mkdir()
        self.f.write(b"directory/file", b"source\n")
        for path in (b"directory", b"directory/", b".", b"..", b"../source.txt", os.fsencode(self.f.task / "source.txt")):
            with self.subTest(path=path):
                before = self.f.snapshot()
                self.assert_refusal(self.f.stage([path]), before)

    def test_nonexistent_untracked_path_refuses(self):
        before = self.f.snapshot()
        self.assert_refusal(self.f.stage([b"does-not-exist"]), before)

    def test_escaping_symlink_parent_refuses(self):
        outside = self.f.root / "outside"
        outside.mkdir()
        (outside / "source").write_bytes(b"private outside\n")
        (self.f.task / "escape").symlink_to(outside, target_is_directory=True)
        before = self.f.snapshot()
        self.assert_refusal(self.f.stage([b"source.txt", b"escape/source"]), before)
        self.assertEqual((outside / "source").read_bytes(), b"private outside\n")

    def test_deletion_and_symlink_are_literal_entries(self):
        self.f.unlink(b"delete.txt")
        (self.f.task / "link").symlink_to("../../not-readable-or-required")
        result = self.f.stage([b"delete.txt", b"link"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.staged_paths(), {b"delete.txt", b"link"})
        entry = self.f.git(self.f.task, "ls-files", "--stage", "--", "link")
        self.assertTrue(entry.startswith(b"120000 "), entry)

    def test_byte_paths_are_not_patterns_or_lines(self):
        paths = [b"with space", b"with\nnewline", b"with\ttab", b"-leading", b"*", b"[ab]", b":(glob)*", b"nonutf8-\xff"]
        for path in paths:
            self.f.write(path, b"literal bytes\n")
        result = self.f.stage(paths)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.staged_paths(), set(paths))

    def test_rename_requires_both_literal_endpoints(self):
        os.rename(self.f.task / "rename-old.txt", self.f.task / "rename-new.txt")
        result = self.f.stage([b"rename-old.txt", b"rename-new.txt"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.staged_paths(), {b"rename-old.txt", b"rename-new.txt"})

    def test_no_argument_legacy_behaviour_remains(self):
        self.f.write(b".amplihack/runtime.json", b"runtime\n")
        result = self.f.stage(arguments=[])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.f.staged_paths(), {b"source.txt", b"unrelated.txt", b"untracked.txt"})

    def test_legacy_runtime_exclusion_does_not_hide_tracked_source(self):
        f = GitFixture(self)
        f.write(b".amplihack/tracked-source", b"baseline tracked source\n")
        f.git(f.task, "add", "-f", "--", ".amplihack/tracked-source")
        f.git(f.task, "commit", "-m", "tracked runtime fixture")
        f.write(b".amplihack/tracked-source", b"changed tracked source\n")
        f.write(b".amplihack/untracked-runtime", b"new runtime\n")
        result = f.stage(arguments=[])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(f.staged_paths(), {b".amplihack/tracked-source"})


class CheckpointIntegrationTests(unittest.TestCase):
    def fixture(self, phase, branch="task/checkpoint"):
        f = GitFixture(self, branch)
        f.write(b"source.txt", b"task source\n")
        f.hook()
        return f, PhaseProjection(f, phase)

    def untouched_refusal(self, f, projection, change=None, allow_existing_skip=False):
        before = f.snapshot()
        result = projection.run(change)
        after = f.snapshot()
        self.assertEqual(after, before, "Unauthorised admission must preserve raw index, blobs, modes, worktree, HEAD and remote.")
        if allow_existing_skip and result.returncode == 0:
            self.assertIn(b"skip", (result.stdout + result.stderr).lower(), "An existing cleanup skip must remain visible.")
            self.assertIsNone(projection.capture)
            self.assertIs(projection.skipped["admitted"], False)
            self.assertNotIn(scalar(projection.current[projection.reviewer], "output"), f.context,
                             "An existing skip must not manufacture an approved scope decision.")
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertFalse((f.root / "pre-commit.log").exists(), "Admission must precede hooks and cleanup.")
        return result

    def assert_published(self, f, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        head = f.git(f.task, "rev-parse", "HEAD").strip()
        self.assertNotEqual(head.decode(), f.base, "A positive checkpoint must create useful source history.")
        self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip(), head)
        self.assertTrue((f.root / "pre-commit.log").exists(), "The actual normal hook must execute.")
        history = f.history()
        self.assertTrue(history)
        for commit, parent, paths in history:
            self.assertLessEqual(paths, f.allowed, (commit, parent, paths))
        # Decision 17.116 checks published history, not unreachable local objects.

    def test_all_three_clean_source_only_checkpoints_publish(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                self.assertEqual(f.staged_paths(), set())
                self.assert_published(f, projection.run())

    def test_preexisting_staged_and_partial_entries_are_untouched(self):
        for phase in PHASES:
            for partial in (False, True):
                with self.subTest(phase=phase[0], partial=partial):
                    f, projection = self.fixture(phase)
                    f.write(b"unrelated.txt", b"user staged\n")
                    f.git(f.task, "add", "--", "unrelated.txt")
                    if partial:
                        f.write(b"unrelated.txt", b"user worktree differs\n")
                    self.untouched_refusal(f, projection)

    def test_staged_mode_deletion_and_intent_to_add_are_untouched(self):
        for phase in PHASES:
            for state in ("mode", "deletion", "intent-to-add"):
                with self.subTest(phase=phase[0], state=state):
                    f, projection = self.fixture(phase)
                    if state == "mode":
                        (f.task / "unrelated.txt").chmod(0o755)
                        f.git(f.task, "add", "--", "unrelated.txt")
                    elif state == "deletion":
                        f.git(f.task, "rm", "--", "unrelated.txt")
                    else:
                        f.write(b"intent.txt", b"not authorised\n")
                        f.git(f.task, "add", "-N", "--", "intent.txt")
                    self.untouched_refusal(f, projection)

    def test_unmerged_entries_are_untouched(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                oid = f.git(f.task, "hash-object", "-w", "--stdin", data=b"conflict\n").strip()
                f.git(f.task, "update-index", "--index-info", data=b"0 " + b"0" * 40 + b"\tunrelated.txt\n" +
                      b"100644 " + oid + b" 1\tunrelated.txt\n100644 " + oid + b" 2\tunrelated.txt\n100644 " + oid + b" 3\tunrelated.txt\n")
                self.untouched_refusal(f, projection)

    def test_split_index_is_refused_without_refresh(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.git(f.task, "update-index", "--split-index")
                self.untouched_refusal(f, projection)

    def test_hostile_git_selectors_cannot_redirect_admission(self):
        for selector in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY", "GIT_CONFIG_COUNT"):
            with self.subTest(selector=selector):
                f, projection = self.fixture(PHASES[0])
                alt = f.private / "alternate-index"
                alt.write_bytes(f.snapshot()["raw_index"])
                f.env[selector] = "1" if selector == "GIT_CONFIG_COUNT" else str(alt if selector == "GIT_INDEX_FILE" else f.caller / ".git")
                if selector == "GIT_CONFIG_COUNT":
                    f.env.update(GIT_CONFIG_KEY_0="core.hooksPath", GIT_CONFIG_VALUE_0=str(f.private))
                # Snapshot through the unredirected oracle, never the hostile selector.
                clean_env = dict(f.env)
                clean_env.pop(selector)
                clean_env.pop("GIT_CONFIG_KEY_0", None)
                clean_env.pop("GIT_CONFIG_VALUE_0", None)
                poisoned_env = f.env
                f.env = clean_env
                before = f.snapshot()
                f.env = poisoned_env
                result = projection.run()
                f.env = clean_env
                self.assertEqual(f.snapshot(), before)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(alt.read_bytes(), before["raw_index"])

    def test_main_master_and_detached_head_cannot_mutate(self):
        for phase in PHASES:
            for branch in ("main", "master", "detached"):
                with self.subTest(phase=phase[0], branch=branch):
                    f, projection = self.fixture(phase, branch if branch != "detached" else "task/checkpoint")
                    if branch == "detached":
                        f.git(f.task, "checkout", "--detach", "HEAD")
                    self.untouched_refusal(f, projection)

    def test_main_checkout_cannot_impersonate_linked_task(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.task = f.caller
                f.context["worktree_setup"].update(worktree_path=str(f.caller), branch_name="fixture-base")
                f.sync_context()
                self.untouched_refusal(f, projection, allow_existing_skip=phase[0] == "workflow-finalize")

    def test_admission_does_not_execute_external_diff(self):
        f, projection = self.fixture(PHASES[0])
        external = f.bin / "external-diff"
        f.executable(external, "printf 'executed\\n' > " + shlex.quote(str(f.root / "external-diff.log")) + "\n")
        f.git(f.task, "config", "diff.external", str(external))
        f.env["GIT_EXTERNAL_DIFF"] = str(external)
        f.write(b"unrelated.txt", b"unrelated user entry\n")
        f.git(f.task, "add", "--", "unrelated.txt")
        self.untouched_refusal(f, projection)
        self.assertFalse((f.root / "external-diff.log").exists())

    def test_missing_task_or_worktree_context_cannot_be_supplied_by_reviewer(self):
        for key in ("task_description", "worktree_setup"):
            with self.subTest(key=key):
                f, projection = self.fixture(PHASES[0])
                f.context[key] = ""
                if key == "task_description":
                    f.env["TASK_DESCRIPTION"] = ""
                    f.context["final_requirements"] = ""
                f.sync_context()
                self.untouched_refusal(f, projection)

    def test_unknown_unstaged_and_tracked_runtime_source_is_not_permission(self):
        for name in (b"unrelated.txt", b"untracked-extra", b".amplihack/tracked-source"):
            with self.subTest(name=name):
                f, projection = self.fixture(PHASES[0])
                if name.startswith(b".amplihack/"):
                    f.write(name, b"tracked runtime baseline\n")
                    f.git(f.task, "add", "-f", "--", os.fsdecode(name))
                    f.git(f.task, "commit", "-m", "pre-existing runtime source")
                    f.git(f.task, "push")
                    (f.root / "pre-commit.log").unlink()
                f.write(name, b"unknown task source\n")
                self.untouched_refusal(f, projection)

    def test_capture_contains_literal_identities_modes_deletions_and_links(self):
        f, projection = self.fixture(PHASES[0])
        paths = {b"source.txt", b"delete.txt", b"rename-old.txt", b"rename-new.txt", b"link", b"line\nname", b"tab\tname", b"*", b":(glob)*", b"-name", b"raw-\xff"}
        f.unlink(b"delete.txt")
        os.rename(f.task / "rename-old.txt", f.task / "rename-new.txt")
        (f.task / "link").symlink_to("../unread-target")
        for path in paths - {b"delete.txt", b"rename-old.txt", b"rename-new.txt", b"link"}:
            f.write(path, b"literal source\n")
        (f.task / "source.txt").chmod(0o755)
        f.own(paths)
        before = f.snapshot()
        result = projection.capture_scope()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(f.snapshot(), before, "Capture is read-only, including the raw index.")
        entries = {base64.b64decode(row["path_b64"], validate=True): row for row in projection.capture["paths"]}
        self.assertEqual(set(entries), paths)
        self.assertEqual([base64.b64decode(row["path_b64"]) for row in projection.capture["paths"]], sorted(paths))
        for path, row in entries.items():
            self.assertTrue({"path_b64", "mode", "type", "sha256"} <= row.keys())
            if path in (b"delete.txt", b"rename-old.txt"):
                self.assertEqual(row["type"], "deletion")
                self.assertIsNone(row["sha256"])
            else:
                payload = b"../unread-target" if path == b"link" else (f.task / os.fsdecode(path)).read_bytes()
                self.assertEqual(row["type"], "symlink" if path == b"link" else "file")
                self.assertEqual(row["sha256"], digest(payload))
                self.assertEqual(row["mode"], "120000" if path == b"link" else "100755" if path == b"source.txt" else "100644")
        self.assert_published(f, projection.finish(projection.verdict()))

    def test_malformed_scope_is_never_permissive_verdict(self):
        variants = [None, "SUCCESS", "approved", {}, {"approved": True},
                    {"approved": "true"}, {"approved": 1}, {"approved": False}]
        for value in variants:
            with self.subTest(value=value):
                f, projection = self.fixture(PHASES[0])
                self.untouched_refusal(f, projection, lambda original, value=value: value)

    def test_missing_truncated_duplicate_and_non_boolean_scope_refuse(self):
        for kind in ("missing", "truncated", "duplicate", "string-boolean"):
            with self.subTest(kind=kind):
                f, projection = self.fixture(PHASES[0])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict()
                output = scalar(projection.current[projection.reviewer], "output")
                raw = json.dumps(verdict)
                if kind == "missing":
                    raw = ""
                elif kind == "truncated":
                    raw = raw[:-3]
                elif kind == "duplicate":
                    raw = '{"approved":false,' + raw[1:]
                else:
                    raw = raw.replace('"approved": true', '"approved": "true"')
                before = f.snapshot()
                # Preserve raw JSON so duplicate fields cannot be hidden by Python.
                f.context[output] = raw
                f.sync_context()
                result = None
                for sid in projection.after + [projection.checkpoint]:
                    result = projection.execute(sid)
                    if result.returncode:
                        break
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)

    def test_scope_review_transport_preserves_duplicate_fields_until_admission(self):
        # The actual pinned runner control proves that parse_json collapses
        # duplicate fields. Exercise that same transport boundary here, rather
        # than injecting raw JSON after the production parser has run.
        for phase in PHASES:
            for decision in ("allow", "refuse", "duplicate"):
                with self.subTest(phase=phase[0], decision=decision):
                    f, projection = self.fixture(phase)
                    self.assertEqual(projection.capture_scope().returncode, 0)
                    verdict = projection.verdict()
                    verdict["approved"] = decision != "refuse"
                    raw = json.dumps(verdict)
                    if decision == "duplicate":
                        raw = '{"approved":false,' + raw[1:]
                    transported = (projected_json_output(raw)
                                   if scalar(projection.current[projection.reviewer], "parse_json") is True
                                   else raw)
                    before = f.snapshot()
                    result = projection.finish(transported)
                    if decision == "allow":
                        self.assert_published(f, result)
                    else:
                        self.assertNotEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(f.snapshot(), before)
                        self.assertFalse((f.root / "pre-commit.log").exists())

    def test_replacement_refs_refuse_before_capture_or_admission(self):
        for phase in PHASES:
            for timing in ("before-capture", "before-admission"):
                with self.subTest(phase=phase[0], timing=timing):
                    f, projection = self.fixture(phase)
                    if timing == "before-admission":
                        self.assertEqual(projection.capture_scope().returncode, 0)
                        verdict = projection.verdict()
                    tree = f.git(f.task, "rev-parse", f.base + "^{tree}").strip().decode()
                    replacement = f.git(f.task, "commit-tree", tree,
                                        data=b"Disposable replacement observation\n").strip().decode()
                    f.git(f.task, "replace", f.base, replacement)
                    before = f.snapshot()
                    result = (projection.capture_scope() if timing == "before-capture"
                              else projection.finish(verdict))
                    self.assertNotEqual(result.returncode, 0, result.stderr)
                    self.assertIn(b"replacement history is unsupported", result.stderr)
                    self.assertEqual(f.snapshot(), before)
                    self.assertFalse((f.root / "pre-commit.log").exists())

    def test_commit_hook_userinfo_is_redacted_without_changing_status(self):
        for phase in PHASES[1:]:
            for rejecting in (False, True):
                with self.subTest(phase=phase[0], rejecting=rejecting):
                    f, projection = self.fixture(phase)
                    userinfo = "fixture-" + os.urandom(12).hex() + ":generated-" + os.urandom(12).hex()
                    schemes = ("http", "https", "ssh", "git", "ftp", "file")
                    lines = "".join(scheme + "://" + userinfo + "@fixture.invalid/path\n"
                                    for scheme in schemes)
                    f.hook(reject=rejecting,
                           extra="printf '%s' " + shlex.quote(lines) + " >&2\n")
                    result = projection.run()
                    self.assertTrue((f.root / "pre-commit.log").exists())
                    self.assertEqual(result.returncode, 1 if rejecting else 0,
                                     "The real hook outcome must retain its original status.")
                    self.assertTrue(userinfo.encode() not in result.stdout + result.stderr,
                                    "Disposable URI userinfo was disclosed; values withheld.")
                    routed = result.stderr if rejecting else result.stdout
                    for scheme in schemes:
                        expected = (scheme + "://REDACTED@fixture.invalid/path").encode()
                        self.assertTrue(expected in routed,
                                        "The redacted hook output must retain its output channel and URI scheme.")

    def test_raw_scope_unicode_rejection_matches_the_pinned_parser_contract(self):
        reasons = (("high-surrogate", "\ud800", False), ("low-surrogate", "\udc00", False),
                   ("valid-pair", "\U0001f642", True), ("escaped-literal", "\\ud800", True))
        for phase in PHASES:
            for label, reason, allowed in reasons:
                with self.subTest(phase=phase[0], reason=label):
                    f, projection = self.fixture(phase)
                    self.assertEqual(projection.capture_scope().returncode, 0)
                    verdict = projection.verdict()
                    verdict["reason"] = reason
                    before = f.snapshot()
                    result = projection.finish(json.dumps(verdict))
                    if allowed:
                        self.assert_published(f, result)
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(f.snapshot(), before)
                        self.assertFalse((f.root / "pre-commit.log").exists())

    def test_raw_duplicate_field_errors_do_not_disclose_uri_userinfo(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                self.assertEqual(projection.capture_scope().returncode, 0)
                userinfo = "fixture-" + os.urandom(12).hex() + ":generated-" + os.urandom(12).hex()
                key = json.dumps("https://" + userinfo + "@fixture.invalid/path")
                raw = json.dumps(projection.verdict())[:-1] + "," + key + ":1," + key + ":2}"
                before = f.snapshot()
                result = projection.finish(raw)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)
                self.assertTrue(userinfo.encode() not in result.stdout + result.stderr,
                                "Malformed scope diagnostics disclosed disposable userinfo; values withheld.")
                self.assertFalse((f.root / "pre-commit.log").exists())

    def test_every_binding_field_is_required_and_exact(self):
        for field in BINDING_FIELDS:
            for mutation in ("missing", "different"):
                with self.subTest(field=field, mutation=mutation):
                    f, projection = self.fixture(PHASES[0])
                    def change(verdict):
                        if mutation == "missing":
                            del verdict["binding"][field]
                        else:
                            verdict["binding"][field] = "different task or repository identity"
                        return verdict
                    self.untouched_refusal(f, projection, change)

    def test_invalid_duplicate_and_uncaptured_path_identities_refuse(self):
        for approved in (["%%%"], [encoded(b"../escape")], [encoded(b"unrelated.txt")],
                         [encoded(b"source.txt"), encoded(b"source.txt")], [encoded(b"source.txt") + "\n"]):
            with self.subTest(approved=approved):
                f, projection = self.fixture(PHASES[0])
                self.untouched_refusal(f, projection, lambda verdict: dict(verdict, approved_paths=approved))

    def test_empty_approval_cannot_conceal_pending_source(self):
        f, projection = self.fixture(PHASES[0])
        self.untouched_refusal(f, projection, lambda verdict: dict(verdict, approved_paths=[]))

    def test_clean_empty_approval_stages_nothing_and_makes_no_commit(self):
        f = GitFixture(self)
        f.own([])
        projection = PhaseProjection(f, PHASES[0])
        before = f.snapshot()
        result = projection.run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(f.snapshot(), before)

    def test_changed_initial_content_mode_or_added_path_is_stale(self):
        for kind in ("content", "mode", "new-path"):
            with self.subTest(kind=kind):
                f, projection = self.fixture(PHASES[0])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict()
                if kind == "content":
                    f.write(b"source.txt", b"changed after capture\n")
                elif kind == "mode":
                    (f.task / "source.txt").chmod(0o755)
                else:
                    f.write(b"extra.txt", b"new after capture\n")
                before = f.snapshot()
                result = projection.finish(verdict)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)

    def test_unsupported_head_change_cannot_renew_authority(self):
        f, projection = self.fixture(PHASES[0])
        self.assertEqual(projection.capture_scope().returncode, 0)
        verdict = projection.verdict()
        f.git(f.task, "commit", "--allow-empty", "-m", "external HEAD change")
        (f.root / "pre-commit.log").unlink()
        before = f.snapshot()
        result = projection.finish(verdict)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(f.snapshot(), before)

    def test_actual_branch_task_and_index_changes_invalidate_capture(self):
        for kind in ("branch", "task", "index"):
            with self.subTest(kind=kind):
                f, projection = self.fixture(PHASES[0])
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict()
                if kind == "branch":
                    f.git(f.task, "branch", "-m", "task/different")
                elif kind == "task":
                    f.context["task_description"] = "A different owner's task."
                    f.context["final_requirements"] = f.context["task_description"]
                    f.env["TASK_DESCRIPTION"] = f.context["task_description"]
                    f.sync_context()
                else:
                    f.write(b"unrelated.txt", b"new user staging after capture\n")
                    f.git(f.task, "add", "--", "unrelated.txt")
                before = f.snapshot()
                result = projection.finish(verdict)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.snapshot(), before)

    def test_a_new_invocation_rejects_the_previous_verdict(self):
        f, first = self.fixture(PHASES[0])
        self.assertEqual(first.capture_scope().returncode, 0)
        old_verdict = first.verdict()
        second = PhaseProjection(f, PHASES[0])
        self.assertEqual(second.capture_scope().returncode, 0)
        self.assertNotEqual(first.capture["binding"]["invocation"], second.capture["binding"]["invocation"])
        before = f.snapshot()
        result = second.finish(old_verdict)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(f.snapshot(), before)

    def test_rejecting_hooks_preserve_legitimate_staging_and_report_failure(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.hook(reject=True)
                f.env["SCOPE_FIXTURE_REPAIR"] = "terminal"
                result = projection.run()
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue((f.root / "pre-commit.log").exists())
                self.assertEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)
                self.assertEqual(f.staged_paths(), {b"source.txt"})
                self.assertEqual((f.task / "source.txt").read_bytes(), b"task source\n")
                self.assertEqual(f.commits(), f.objects_before)

    def test_failed_push_retains_authorised_local_commit_and_error(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.remote_reject()
                result = projection.run()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b"fixture transport rejection", result.stdout + result.stderr)
                self.assertNotEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)
                self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip().decode(), f.base)
                self.assertEqual(f.staged_paths(), set())
                self.assertEqual((f.task / "source.txt").read_bytes(), b"task source\n")
                for _, _, paths in f.history():
                    self.assertEqual(paths, {b"source.txt"})

    def test_hook_added_staged_source_is_checked_in_actual_resulting_commit(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.hook(extra="printf 'hook added unrelated source\\n' > unrelated.txt\ngit add -- unrelated.txt\n")
                result = projection.run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip().decode(), f.base)
                self.assertEqual((f.task / "unrelated.txt").read_bytes(), b"hook added unrelated source\n")
                self.assertNotEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base,
                                    "The actual hook-produced local commit must remain; no destructive recovery is allowed.")
                self.assertTrue(any(b"unrelated.txt" in paths for _, _, paths in f.history()))

    def test_unreachable_local_commit_does_not_block_authorised_publication(self):
        f, projection = self.fixture(PHASES[0])
        # The hook creates an unreachable commit without changing the final HEAD.
        # The path is deleted from its temporary index before the real commit.
        # Only this adversarial, fixture-owned hook performs these Git writes.
        extra = textwrap.dedent('''
            current=$(git rev-parse HEAD)
            alternate="$SCOPE_FIXTURE_ROOT/private/hook-index"
            GIT_INDEX_FILE="$alternate" git read-tree HEAD
            oid=$(printf 'intermediate secret\\n' | git hash-object -w --stdin)
            printf '100644 %s\\thidden-hook-path\\n' "$oid" | GIT_INDEX_FILE="$alternate" git update-index --index-info
            tree=$(GIT_INDEX_FILE="$alternate" git write-tree)
            printf 'intermediate forbidden object\\n' | git commit-tree "$tree" -p "$current" > "$SCOPE_FIXTURE_ROOT/intermediate-commit"
        ''')
        f.hook(extra=extra)
        result = projection.run()
        self.assertTrue((f.root / "intermediate-commit").exists())
        intermediate = (f.root / "intermediate-commit").read_text().strip()
        self.assertIn(intermediate, f.commits(), "The intermediate object must remain observable.")
        # The prior refusal requirement is explicitly withdrawn by 17.116.
        self.assert_published(f, result)
        published = set(f.git(f.remote, "rev-list", "refs/heads/" + f.branch).decode().splitlines())
        self.assertNotIn(intermediate, published, "An unreachable object is not published history.")

    def test_publish_result_reports_actual_publication_or_failed_push(self):
        for failing in (False, True):
            with self.subTest(failing=failing):
                f, projection = self.fixture(PHASES[0])
                if failing:
                    f.remote_reject()
                result = projection.run()
                value = json.loads(result.stdout)
                self.assertEqual(set(value), {"pushed", "sha", "branch", "reason"})
                self.assertEqual(value["pushed"], "false" if failing else "true")
                self.assertEqual(value["branch"], f.branch)
                if failing:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("push", value["reason"])
                else:
                    self.assertEqual(result.returncode, 0)
                    self.assertEqual(value["sha"], f.git(f.task, "rev-parse", "HEAD").strip().decode())

    def test_reinvocation_must_not_reuse_residual_staging_authority(self):
        f, projection = self.fixture(PHASES[0])
        f.hook(reject=True)
        f.env["SCOPE_FIXTURE_REPAIR"] = "terminal"
        self.assertNotEqual(projection.run().returncode, 0)
        before = f.snapshot()
        log = (f.root / "pre-commit.log").read_bytes()
        second = PhaseProjection(f, PHASES[0]).run()
        self.assertNotEqual(second.returncode, 0)
        self.assertEqual(f.snapshot(), before)
        self.assertEqual((f.root / "pre-commit.log").read_bytes(), log)

    def test_preexisting_outgoing_commits_need_scope_even_with_clean_index(self):
        for phase in PHASES:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.write(b"unrelated.txt", b"pre-existing forbidden outgoing\n")
                f.git(f.task, "add", "--", "unrelated.txt")
                f.git(f.task, "commit", "-m", "unreviewed outgoing")
                (f.root / "pre-commit.log").unlink()
                self.untouched_refusal(f, projection)

    def test_reviewed_preexisting_outgoing_range_can_publish(self):
        f, projection = self.fixture(PHASES[0])
        f.git(f.task, "add", "--", "source.txt")
        f.git(f.task, "commit", "-m", "owner source before capture")
        initial = f.git(f.task, "rev-parse", "HEAD").strip().decode()
        self.assertEqual(projection.capture_scope().returncode, 0)
        outgoing = projection.capture["outgoing_commits"]
        self.assertTrue(outgoing)
        self.assertIn(initial, json.dumps(outgoing))
        self.assertIn(f.base, json.dumps(outgoing), "Every outgoing parent must be captured.")
        self.assertIn(encoded(b"source.txt"), json.dumps(outgoing))
        self.assert_published(f, projection.finish(projection.verdict()))

    def test_deleted_unknown_history_cannot_hide_in_final_tree(self):
        f, projection = self.fixture(PHASES[0])
        f.write(b"history-secret", b"must never publish\n")
        f.git(f.task, "add", "--", "history-secret")
        f.git(f.task, "commit", "-m", "unknown intermediate")
        f.git(f.task, "rm", "--", "history-secret")
        f.git(f.task, "commit", "-m", "delete unknown intermediate")
        (f.root / "pre-commit.log").unlink()
        self.untouched_refusal(f, projection)

    def test_merge_second_parent_changes_require_authority(self):
        f, projection = self.fixture(PHASES[0])
        f.git(f.task, "checkout", "-b", "fixture-side")
        f.write(b"unrelated.txt", b"side history\n")
        f.git(f.task, "add", "--", "unrelated.txt")
        f.git(f.task, "commit", "-m", "side source")
        side = f.git(f.task, "rev-parse", "HEAD").strip().decode()
        f.git(f.task, "checkout", f.branch)
        f.git(f.task, "commit", "--allow-empty", "-m", "first parent")
        tree = f.git(f.task, "rev-parse", "HEAD^{tree}").strip().decode()
        head = f.git(f.task, "rev-parse", "HEAD").strip().decode()
        merge = f.git(f.task, "commit-tree", tree, "-p", head, "-p", side, data=b"merge hides side path\n").strip().decode()
        f.git(f.task, "update-ref", "HEAD", merge)
        (f.root / "pre-commit.log").unlink()
        self.untouched_refusal(f, projection)

    def test_supported_review_and_cleanup_rebases_are_tied_to_fetched_upstream(self):
        for phase in PHASES[1:]:
            with self.subTest(phase=phase[0]):
                f, projection = self.fixture(phase)
                f.git(f.caller, "checkout", "-b", "remote-writer", f.base)
                f.write(b"upstream-only.txt", b"independent upstream\n", f.caller)
                f.git(f.caller, "add", "--", "upstream-only.txt")
                f.git(f.caller, "commit", "-m", "known upstream advance")
                upstream = f.git(f.caller, "rev-parse", "HEAD").strip().decode()
                f.git(f.caller, "push", "origin", "HEAD:refs/heads/" + f.branch)
                result = projection.run()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(f.git(f.task, "merge-base", "HEAD", upstream).strip().decode(), upstream)
                self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch), f.git(f.task, "rev-parse", "HEAD"))
                for _, _, paths in f.history(start=upstream):
                    self.assertEqual(paths, {b"source.txt"})


class RepairBoundaryTests(unittest.TestCase):
    fixture = CheckpointIntegrationTests.fixture
    assert_published = CheckpointIntegrationTests.assert_published

    def test_in_scope_repair_runs_actual_driver_and_keeps_authority_immutable(self):
        f, projection = self.fixture(PHASES[0])
        f.hook(repairable=True)
        f.env["SCOPE_FIXTURE_REPAIR"] = "within"
        self.assertEqual(projection.capture_scope().returncode, 0)
        capture = copy.deepcopy(projection.capture)
        result = projection.finish(projection.verdict())
        self.assert_published(f, result)
        self.assertEqual(projection.capture, capture)
        self.assertEqual((f.task / "source.txt").read_bytes(), b"fixed\n")
        self.assertEqual((f.root / "actor.log").read_text().splitlines(), ["repair"])
        self.assertEqual((f.root / "pre-commit.log").read_text().splitlines(), ["hook", "hook"])

    def test_unknown_repair_source_stops_before_internal_restaging_or_retry(self):
        for mode in ("unknown", "unknown-staged"):
            with self.subTest(mode=mode):
                f, projection = self.fixture(PHASES[0])
                f.hook(repairable=True)
                f.env["SCOPE_FIXTURE_REPAIR"] = mode
                result = projection.run()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual((f.root / "actor.log").read_text().splitlines(), ["repair"])
                self.assertEqual((f.root / "pre-commit.log").read_text().splitlines(), ["hook"])
                self.assertEqual((f.task / "source.txt").read_bytes(), b"fixed\n")
                self.assertEqual((f.task / "new-path.txt").read_bytes(), b"unknown repair source\n")
                self.assertEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)
                self.assertEqual(f.staged_paths(), {b"source.txt", b"new-path.txt"} if mode == "unknown-staged" else {b"source.txt"})
                self.assertEqual(f.git(f.task, "show", ":source.txt"), b"task source\n", "Unknown scope must stop before restaging even the known path.")

    def test_pre_push_repair_checks_scope_before_nested_fixing_commit(self):
        f, projection = self.fixture(PHASES[0])
        f.hook(name="pre-push", reject=True)
        f.env["SCOPE_FIXTURE_REPAIR"] = "unknown"
        result = projection.run()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((f.root / "pre-commit.log").read_text().splitlines(), ["hook"])
        self.assertEqual(len(f.history()), 1, "There must be no nested fixing commit for unknown paths.")
        self.assertEqual((f.task / "new-path.txt").read_bytes(), b"unknown repair source\n")
        self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip().decode(), f.base)

    def test_in_scope_pre_push_repair_creates_and_checks_nested_fixing_commit(self):
        f, projection = self.fixture(PHASES[0])
        f.hook(name="pre-push", repairable=True)
        f.env["SCOPE_FIXTURE_REPAIR"] = "within"
        result = projection.run()
        self.assert_published(f, result)
        self.assertEqual(len(f.history()), 2)
        self.assertEqual((f.root / "pre-commit.log").read_text().splitlines(), ["hook", "hook"])
        self.assertEqual((f.root / "pre-push.log").read_text().splitlines(), ["hook", "hook"])
        self.assertEqual((f.root / "actor.log").read_text().splitlines(), ["repair"])

    def test_scope_is_rechecked_after_a_rejected_retry_changes_paths(self):
        f, projection = self.fixture(PHASES[0])
        count = shlex.quote(str(f.root / "pre-commit.log"))
        f.hook(reject=True, extra='if [ "$(wc -l < ' + count + ')" -ge 2 ]; then printf "retry created source\\n" > retry-extra.txt; fi\n')
        f.env.update(SCOPE_FIXTURE_REPAIR="within", SCOPE_FIXTURE_HEALTH="CONTINUE")
        result = projection.run()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((f.root / "pre-commit.log").read_text().splitlines(), ["hook", "hook"])
        self.assertEqual((f.task / "retry-extra.txt").read_bytes(), b"retry created source\n")
        self.assertNotIn(b"retry-extra.txt", f.staged_paths())
        self.assertEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)

    def test_missing_truncated_altered_or_self_rehashed_authority_refuses(self):
        for mutation in ("removed", "truncated", "altered", "altered-and-rehashed"):
            with self.subTest(mutation=mutation):
                f, projection = self.fixture(PHASES[0])
                # Alter only metadata. There is no new source path whose refusal
                # could masquerade as an integrity check on the immutable record.
                program = textwrap.dedent('''
                    import hashlib, json, os
                    from pathlib import Path
                    root = Path(os.environ["SCOPE_FIXTURE_ROOT"])
                    task = Path(os.environ["SCOPE_FIXTURE_TASK"])
                    changed = []
                    def owned_write(path, payload):
                        before = path.stat()
                        if before.st_uid != os.getuid():
                            raise RuntimeError("The fixture may alter only its own metadata")
                        mode = before.st_mode & 0o777
                        path.chmod(mode | 0o200)
                        try:
                            path.write_bytes(payload)
                        finally:
                            path.chmod(mode)
                    for path in sorted(root.rglob("*")):
                        if path.is_symlink() or not path.is_file() or task in path.parents:
                            continue
                        if "objects" in path.parts or path.stat().st_size > 1048576:
                            continue
                        raw = path.read_bytes()
                        try:
                            record = json.loads(raw)
                        except (ValueError, UnicodeError):
                            continue
                        if not isinstance(record, dict):
                            continue
                        serial = json.dumps(record)
                        if '"binding"' not in serial or '"approved_paths"' not in serial:
                            continue
                        old_hash = hashlib.sha256(raw).hexdigest()
                        record["fixture_unauthorised_replacement"] = True
                        replacement = json.dumps(record, sort_keys=True).encode()
                        mutation = os.environ["SCOPE_FIXTURE_RECORD_MUTATION"]
                        if mutation == "removed":
                            path.unlink()
                        elif mutation == "truncated":
                            owned_write(path, replacement[:7])
                        else:
                            owned_write(path, replacement)
                        changed.append(str(path))
                        if mutation == "altered-and-rehashed":
                            new_hash = hashlib.sha256(replacement).hexdigest()
                            for adjacent in path.parent.iterdir():
                                if adjacent == path or adjacent.is_symlink() or not adjacent.is_file():
                                    continue
                                if adjacent.stat().st_size <= 256:
                                    sidecar = adjacent.read_bytes()
                                    if old_hash.encode() in sidecar:
                                        owned_write(adjacent, sidecar.replace(old_hash.encode(), new_hash.encode()))
                            checksum_path = path.with_name(path.name + ".sha256")
                            checksum_bytes = (new_hash + "\\n").encode()
                            if checksum_path.exists():
                                owned_write(checksum_path, checksum_bytes)
                            else:
                                checksum_path.write_bytes(checksum_bytes)
                    (root / "record-tampering.json").write_text(json.dumps(changed))
                ''')
                f.env["SCOPE_FIXTURE_RECORD_MUTATION"] = mutation
                f.hook(extra=shlex.quote(sys.executable) + " -c " + shlex.quote(program) + "\n")
                result = projection.run()
                log = f.root / "record-tampering.json"
                self.assertTrue(log.exists(), "The normal hook must reach the live authority boundary.")
                changed = json.loads(log.read_text())
                self.assertTrue(changed, "The fixture must alter a real retained record, not a fabricated test record.")
                self.assertNotEqual(result.returncode, 0, "A record cannot authenticate its own replacement.")
                self.assertEqual(f.git(f.remote, "rev-parse", "refs/heads/" + f.branch).strip().decode(), f.base)
                self.assertEqual((f.task / "source.txt").read_bytes(), b"task source\n")
                for path in changed:
                    self.assertNotIn(f.task, Path(path).parents)
                    if mutation == "removed":
                        self.assertFalse(Path(path).exists())
                    elif mutation == "truncated":
                        with self.assertRaises(ValueError):
                            json.loads(Path(path).read_bytes())
                    else:
                        self.assertTrue(json.loads(Path(path).read_bytes())["fixture_unauthorised_replacement"])

    def test_terminal_79_is_not_retried_or_reclassified_as_success(self):
        for mode in ("terminal", "repair-terminal"):
            with self.subTest(mode=mode):
                f, projection = self.fixture(PHASES[0])
                f.hook(reject=True)
                f.env.update(SCOPE_FIXTURE_REPAIR=mode, SCOPE_FIXTURE_HEALTH="TERMINAL")
                result = projection.run()
                self.assertEqual(result.returncode, 79, result.stderr)
                calls = (f.root / "actor.log").read_text().splitlines()
                self.assertEqual(calls, ["repair"] if mode == "terminal" else ["repair", "health"])
                self.assertEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)

    def test_legacy_repair_callers_retain_their_existing_staging_behaviour(self):
        f = GitFixture(self)
        f.write(b"source.txt", b"needs repair\n")
        f.git(f.task, "add", "--", "source.txt")
        f.hook(repairable=True)
        f.env["SCOPE_FIXTURE_REPAIR"] = "unknown"
        result = f.shell('set -euo pipefail\n. "$1"\n. "$2"\nhook_repair_run commit unrelated-caller -- git commit -m "legacy repair"',
                         str(TOOLS / "workflow_stage_changes.sh"), str(TOOLS / "workflow_hook_repair.sh"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(b"new-path.txt", f.git(f.task, "ls-tree", "--name-only", "HEAD"))


class PrivateHistoryTests(unittest.TestCase):
    def fixture(self, phase=PHASES[0], object_format="sha1"):
        f = GitFixture(self, object_format=object_format)
        f.write(b"source.txt", b"local task change\n")
        f.hook()
        return f, PhaseProjection(f, phase)

    def metadata(self, f):
        common = Path(os.fsdecode(f.git(f.task, "rev-parse", "--git-common-dir").strip()))
        if not common.is_absolute():
            common = f.task / common
        files = {}
        for directory, dirs, names in os.walk(common, followlinks=False):
            for name in dirs + names:
                path = Path(directory) / name
                info = os.lstat(path)
                row = (info.st_dev, info.st_ino, stat.S_IMODE(info.st_mode))
                if stat.S_ISLNK(info.st_mode):
                    row += ("link", os.readlink(path))
                elif stat.S_ISREG(info.st_mode):
                    row += ("file", path.read_bytes())
                else:
                    row += ("directory",)
                files[str(path.relative_to(common))] = row
        return f.snapshot(), files

    def advance(self, f, unrelated=False):
        other = f.root / "remote writer"
        f.run([f.git_binary, "clone", "--branch", f.branch, str(f.remote), str(other)])
        f.git(other, "config", "user.name", f.env["GIT_AUTHOR_NAME"])
        f.git(other, "config", "user.email", f.env["GIT_AUTHOR_EMAIL"])
        if unrelated:
            f.git(other, "checkout", "--orphan", "unrelated-fixture")
            f.git(other, "rm", "-rf", ".")
        f.write(b"other.txt", b"remote advance\n", other)
        f.git(other, "add", "other.txt")
        f.git(other, "commit", "-m", "independent remote advance")
        tip = f.git(other, "rev-parse", "HEAD").strip().decode()
        if unrelated:
            f.git(other, "push", "origin", "HEAD:refs/heads/unrelated-fixture")
            f.git(f.remote, "update-ref", "refs/heads/" + f.branch, tip)
        else:
            f.git(other, "push", "origin", f.branch)
        return tip

    def test_missing_and_present_destination_history_preserves_all_source_state(self):
        for object_format in ("sha1", "sha256"):
            for prepared in (False, True):
                with self.subTest(object_format=object_format, prepared=prepared):
                    f, projection = self.fixture(object_format=object_format)
                    tip = self.advance(f)
                    self.assertNotEqual(f.git(f.task, "cat-file", "-e", tip, check=False).returncode, 0)
                    tracking = f.git(f.task, "rev-parse", "@{u}")
                    if prepared:
                        f.git(f.task, "fetch", "--no-write-fetch-head", "origin", tip)
                    self.assertEqual(f.git(f.task, "rev-parse", "@{u}"), tracking)
                    before = self.metadata(f)
                    capture = projection.capture_scope()
                    self.assertEqual(capture.returncode, 0, capture.stderr)
                    self.assertEqual(self.metadata(f), before,
                                     "Private evidence preparation must not change any implementation metadata or source.")
                    history = projection.capture["publication"]["history"]
                    self.assertEqual(history["captured_tip"], tip)
                    self.assertEqual(history["object_format"], object_format)
                    view = Path(history["directory"])
                    self.assertTrue(view.resolve().is_relative_to(f.private.resolve()))
                    self.assertFalse((view / "objects/info/alternates").exists())
                    self.assertFalse((view / "FETCH_HEAD").exists())
                    f.run([f.git_binary, "--git-dir=" + str(view), "cat-file", "-e", tip + "^{commit}"])
                    self.assertEqual(f.git(f.task, "cat-file", "-e", tip, check=False).returncode == 0, prepared)
                    result = projection.finish(projection.verdict())
                    self.assertEqual(result.returncode, 1)
                    output = projected_json_output(result.stdout.decode())
                    self.assertTrue(output["reason"].startswith("diverged-upstream: "))
                    self.assertEqual(f.git(f.task, "rev-parse", "@{u}").strip().decode(), tip)
                    self.assertNotEqual(f.git(f.task, "rev-parse", "HEAD").strip().decode(), f.base)
                    self.assertTrue((f.root / "pre-commit.log").exists())

    def test_private_range_matches_native_git_and_all_parent_paths(self):
        for kind in ("ordinary", "divergent", "merge", "unrelated"):
            with self.subTest(history=kind):
                f, projection = self.fixture()
                f.own([b"source.txt", b"unrelated.txt", b"delete.txt", b"rename-old.txt"])
                if kind in ("divergent", "unrelated"):
                    tip = self.advance(f, unrelated=kind == "unrelated")
                    f.git(f.task, "fetch", "--no-write-fetch-head", "origin", tip)
                f.git(f.task, "add", "source.txt")
                f.git(f.task, "commit", "-m", "known local task commit")
                if kind == "merge":
                    f.write(b"unrelated.txt", b"independent side change\n", f.caller)
                    f.git(f.caller, "add", "unrelated.txt")
                    f.git(f.caller, "commit", "-m", "known second parent")
                    side = f.git(f.caller, "rev-parse", "HEAD").strip().decode()
                    f.git(f.task, "merge", "--no-ff", side, "-m", "fixture merge")
                before = self.metadata(f)
                result = projection.capture_scope()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.metadata(f), before)
                publication = projection.capture["publication"]
                native = set(f.git(f.task, "rev-list", publication["publication_base"] + "..HEAD").decode().splitlines())
                actual = {row["commit"]: row for row in projection.capture["outgoing_commits"]}
                self.assertEqual(set(actual), native)
                for oid, row in actual.items():
                    parents = f.git(f.task, "rev-list", "--parents", "-n", "1", oid).decode().split()[1:]
                    self.assertEqual(row["parents"], parents)
                    paths = set()
                    for parent in parents:
                        paths.update(filter(None, f.git(f.task, "diff-tree", "--no-commit-id", "--name-only", "--no-renames", "-r", "-z", parent, oid).split(b"\0")))
                    if not parents:
                        paths.update(filter(None, f.git(f.task, "diff-tree", "--root", "--no-commit-id", "--name-only", "--no-renames", "-r", "-z", oid).split(b"\0")))
                    self.assertEqual({base64.b64decode(value) for value in row["paths"]}, paths)

    def test_private_history_tampering_refuses_before_source_mutation(self):
        for mutation in ("configuration", "inventory-inode", "object-bytes", "directory-inode", "reference"):
            with self.subTest(mutation=mutation):
                f, projection = self.fixture()
                self.assertEqual(projection.capture_scope().returncode, 0)
                verdict = projection.verdict()
                history = projection.capture["publication"]["history"]
                directory = Path(history["directory"])
                self.assertTrue(directory.resolve().is_relative_to(f.private.resolve()))
                if mutation == "configuration":
                    with (directory / "config").open("a") as stream:
                        stream.write("\n# owner tampering\n")
                elif mutation == "inventory-inode":
                    inventory = Path(history["inventory_file"])
                    old = inventory.read_bytes()
                    inventory.rename(inventory.with_name("retained-inventory.json"))
                    inventory.write_bytes(old)
                    inventory.chmod(0o600)
                elif mutation == "object-bytes":
                    path = next((directory / "objects/pack").glob("*.pack"))
                    original = path.stat()
                    raw = path.read_bytes()
                    path.chmod(stat.S_IMODE(original.st_mode) | stat.S_IWUSR)
                    path.write_bytes(bytes([raw[0] ^ 1]) + raw[1:])
                    path.chmod(stat.S_IMODE(original.st_mode))
                    self.assertEqual(path.stat().st_ino, original.st_ino)
                    self.assertEqual(stat.S_IMODE(path.stat().st_mode), stat.S_IMODE(original.st_mode))
                elif mutation == "directory-inode":
                    retained = directory.with_name("retained-repository")
                    directory.rename(retained)
                    shutil.copytree(retained, directory)
                else:
                    f.run([f.git_binary, "--git-dir=" + str(directory), "update-ref", "refs/heads/owner-tamper", f.base])
                before = self.metadata(f)
                result = projection.finish(verdict)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.metadata(f), before)
                self.assertFalse((f.root / "pre-commit.log").exists())

    def test_refused_scope_after_private_preparation_preserves_source(self):
        f, projection = self.fixture()
        tip = self.advance(f)
        before = self.metadata(f)
        self.assertEqual(projection.capture_scope().returncode, 0)
        self.assertEqual(self.metadata(f), before)
        verdict = projection.verdict()
        verdict["approved"] = False
        result = projection.finish(verdict)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.metadata(f), before)
        self.assertNotEqual(f.git(f.task, "cat-file", "-e", tip, check=False).returncode, 0)
        self.assertFalse((f.root / "pre-commit.log").exists())


    def test_missing_owned_blob_cannot_trigger_an_implicit_source_fetch(self):
        f, projection = self.fixture()
        f.write(b"source.txt", b"older fixture blob\n")
        f.git(f.task, "add", "source.txt")
        f.git(f.task, "commit", "-m", "owned older source")
        old_blob = f.git(f.task, "rev-parse", "HEAD:source.txt").strip().decode()
        f.write(b"source.txt", b"current fixture blob\n")
        f.git(f.task, "add", "source.txt")
        f.git(f.task, "commit", "-m", "owned current source")
        f.git(f.task, "push", "origin", f.branch)
        f.git(f.remote, "config", "uploadpack.allowFilter", "true")
        f.git(f.remote, "config", "uploadpack.allowAnySHA1InWant", "true")
        marker = f.root / "source-lazy-fetch-marker"
        upload = f.root / "fixture-upload-pack"
        f.executable(upload, "printf 'fetch\\n' >> " + shlex.quote(str(marker)) +
                     "\nexec " + shlex.quote(f.git_binary) + " upload-pack \"$@\"\n")
        f.git(f.task, "config", "core.repositoryFormatVersion", "1")
        f.git(f.task, "config", "extensions.partialClone", "origin")
        f.git(f.task, "config", "remote.origin.promisor", "true")
        f.git(f.task, "config", "remote.origin.uploadpack", shlex.quote(str(upload)))
        object_path = Path(os.fsdecode(f.git(f.task, "rev-parse", "--git-path", "objects").strip()))
        if not object_path.is_absolute():
            object_path = f.task / object_path
        missing = object_path / old_blob[:2] / old_blob[2:]
        self.assertTrue(missing.is_file())
        missing.unlink()
        f.write(b"source.txt", b"pending owner source\n")
        before = self.metadata(f)
        result = projection.capture_scope()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists(), "Preparation must not ask the source promisor to fetch.")
        self.assertEqual(self.metadata(f), before)
        self.assertFalse(missing.exists())
        # Independently prove that this real promisor dependency can restore
        # the object when the fixture explicitly requests a normal lazy read.
        fetched = f.git(f.task, "cat-file", "-p", old_blob)
        self.assertEqual(fetched, b"older fixture blob\n")
        self.assertTrue(marker.exists())
        self.assertEqual(f.git(f.task, "cat-file", "-e", old_blob, check=False).returncode, 0)
        self.assertNotEqual(self.metadata(f), before)


    def test_failed_unsupported_and_terminal_private_preparation_preserves_source(self):
        for code in (128, 129, 79):
            with self.subTest(private_fetch_exit=code):
                f, projection = self.fixture()
                shim = "private=0\nfor arg in \"$@\"; do\n" \
                       "  case \"$arg\" in\n" \
                       "    --git-dir=*) private=1 ;;\n" \
                       "    fetch) if [ \"$private\" = 1 ]; then exit " + str(code) + "; fi ;;\n" \
                       "  esac\ndone\nexec " + shlex.quote(f.git_binary) + " \"$@\"\n"
                f.executable(f.bin / "git", shim)
                before = self.metadata(f)
                result = projection.capture_scope()
                self.assertEqual(result.returncode, 79 if code == 79 else 2)
                self.assertEqual(self.metadata(f), before)
                self.assertFalse((f.root / "pre-commit.log").exists())
                journals = list(f.private.glob("checkpoint-scope-*/history-preparation.json"))
                self.assertEqual(len(journals), 1)
                self.assertIs(json.loads(journals[0].read_bytes())["complete"], False)


class RecipeContractTests(unittest.TestCase):
    def review_block(self, phase):
        asset = phase[0]
        current = blocks((RECIPES / (asset + ".yaml")).read_text())
        original = blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))
        additions = [block for sid, block in current.items() if sid not in original and scalar(block, "agent")]
        self.assertEqual(len(additions), 1)
        return additions[0]

    def assert_review_opt_out(self, block):
        self.assertIs(scalar(block, "auto_stage"), False)
        self.assertEqual(scalar(block, "working_dir"), "{{worktree_setup.worktree_path}}")
        self.assertIs(scalar(block, "parse_json"), False)

    def test_all_original_40_agent_controls_and_prompts_remain(self):
        count = 0
        for asset in ASSETS:
            original = blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))
            current = blocks((RECIPES / (asset + ".yaml")).read_text())
            for sid, block in original.items():
                if scalar(block, "agent") is not None:
                    with self.subTest(asset=asset, step=sid):
                        self.assertIn(sid, current)
                        self.assertIs(scalar(current[sid], "auto_stage"), False)
                        self.assertEqual(literal(current[sid], "prompt"), literal(block, "prompt"))
                        for field in ("agent", "working_dir", "condition", "output", "parse_json"):
                            self.assertEqual(scalar(current[sid], field), scalar(block, field))
                    count += 1
        self.assertEqual(count, 40, "Do not reduce the original control population.")

    def test_three_complete_extractions_preserve_every_original_comment(self):
        for asset, sid, helper, output in PHASES:
            with self.subTest(phase=asset):
                self.assertTrue((TOOLS / helper).is_file(), "The complete checkpoint must be written to its approved helper.")
                original = literal(blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))[sid], "command")
                extracted = (TOOLS / helper).read_text()
                cursor = 0
                for line in original.splitlines():
                    if line.lstrip().startswith("#"):
                        comment = line.strip()
                        position = extracted.find(comment, cursor)
                        self.assertGreaterEqual(position, 0, "An original comment was removed, reordered or compressed: " + comment)
                        cursor = position + len(comment)
                current = blocks((RECIPES / (asset + ".yaml")).read_text())[sid]
                self.assertIn(helper, literal(current, "command"))
                self.assertEqual(scalar(current, "output"), output)
                self.assertEqual(scalar(current, "condition"), scalar(blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))[sid], "condition"))

    def test_extractions_retain_non_staging_commands_and_resolver_order(self):
        import importlib.util
        audit_path = Path(__file__).with_name("canonical_staging_checkpoint_audit.py")
        spec = importlib.util.spec_from_file_location("checkpoint_extraction_audit", audit_path)
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        for asset, sid, helper, output in PHASES:
            with self.subTest(phase=asset):
                self.assertTrue((TOOLS / helper).is_file())
                original = literal(blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))[sid], "command")
                restored = audit.restored_checkpoint_text(asset, (TOOLS / helper).read_text())
                extracted_lines = [line.strip() for line in restored.splitlines()]
                cursor = 0
                for raw in original.splitlines():
                    line = raw.strip()
                    if not line or line.startswith("#") or "{{" in line:
                        continue
                    # These are the explicitly authorised mutation boundaries.
                    # Their replacements are tested through actual Git state.
                    if re.search(r"\bstage_workflow_changes(?:\s|;|$)|\bgit add\b", line):
                        continue
                    try:
                        cursor = extracted_lines.index(line, cursor) + 1
                    except ValueError:
                        self.fail("The complete extraction changed or reordered non-staging code: " + line)

    def test_publish_scope_review_opt_out_control(self):
        self.assert_review_opt_out(self.review_block(PHASES[0]))

    def test_feedback_scope_review_opt_out_control(self):
        self.assert_review_opt_out(self.review_block(PHASES[1]))

    def test_cleanup_scope_review_opt_out_control(self):
        self.assert_review_opt_out(self.review_block(PHASES[2]))

    def test_repair_opt_out_control_preserves_all_other_bytes(self):
        path = "amplifier-bundle/recipes/hook-rejection-repair.yaml"
        original = index_source(path)
        current = (ROOT / path).read_text()
        block = blocks(current)["fix-hook-rejection"]
        self.assertIs(scalar(block, "auto_stage"), False)
        stripped, count = re.subn(r"^    auto_stage: false\n", "", current, flags=re.M)
        self.assertEqual(count, 1)
        self.assertEqual(stripped, original, "Only the explicit mutation-agent opt-out is permitted.")

    def test_four_new_opt_out_removals_are_separate_controls(self):
        population = [self.review_block(phase) for phase in PHASES]
        repair = blocks((RECIPES / "hook-rejection-repair.yaml").read_text())["fix-hook-rejection"]
        population.append(repair)
        self.assertEqual(len(population), 4)
        for number, block in enumerate(population):
            with self.subTest(additional_control=number):
                self.assertIs(scalar(block, "auto_stage"), False)
                removed, count = re.subn(r"^    auto_stage: false\n", "", block, flags=re.M)
                self.assertEqual(count, 1)
                self.assertIsNot(scalar(removed, "auto_stage"), False)
                if number < 3:
                    with self.assertRaises(AssertionError):
                        self.assert_review_opt_out(removed)

    def test_original_non_checkpoint_bash_steps_are_unchanged(self):
        import importlib.util
        path = Path(__file__).with_name("canonical_staging_checkpoint_audit.py")
        spec = importlib.util.spec_from_file_location("auxiliary_extraction_audit", path)
        audit = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(audit)
        checkpoint_ids = {phase[1] for phase in PHASES}
        for asset in ASSETS:
            original = blocks(index_source("amplifier-bundle/recipes/" + asset + ".yaml"))
            current = blocks((RECIPES / (asset + ".yaml")).read_text())
            for sid, block in original.items():
                if scalar(block, "type") == "bash" and sid not in checkpoint_ids:
                    with self.subTest(asset=asset, step=sid):
                        original = literal(block, "command")
                        restored = audit.restored_auxiliary_command(asset, sid, literal(current[sid], "command"), original)
                        self.assertEqual(restored, original)

    def test_shared_maintenance_inventory_covers_executed_omissions(self):
        required = {
            "tests/integration/auto_drive_to_merge_test.rs",
            "tests/integration/issue_1714_finalization_handoff/fixture_transport.rs",
            "amplifier-bundle/recipes/tests/test-checkpoint-value-transport.py",
        }
        self.assertTrue(required <= maintenance_inventory(),
                        "The shared typed inventory must retain the three previously omitted approved paths.")

    def test_no_additional_cargo_runtime_runner_or_consensus_changes(self):
        # The preparation must hydrate this exact retained base before offline
        # qualification. Missing ancestry fails rather than yielding an empty audit.
        audit_env = dict(isolated_environment(), GIT_NO_REPLACE_OBJECTS="1")
        shallow = subprocess.run(["git", "rev-parse", "--is-shallow-repository"], cwd=ROOT,
                                 env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(shallow.returncode, 0, shallow.stderr)
        self.assertEqual(shallow.stdout.strip(), b"false", "Complete task ancestry is required.")
        graft = subprocess.run(["git", "rev-parse", "--git-path", "info/grafts"], cwd=ROOT,
                               env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(graft.returncode, 0, graft.stderr)
        graft_path = Path(os.fsdecode(graft.stdout.strip()))
        if not graft_path.is_absolute():
            graft_path = ROOT / graft_path
        self.assertFalse(os.path.lexists(graft_path), "Legacy grafts cannot replace actual task ancestry.")
        ancestor = subprocess.run(["git", "merge-base", "--is-ancestor", SCOPE_AUDIT_BASELINE, "HEAD"],
                                  cwd=ROOT, env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(ancestor.returncode, 0, "The retained task base must be an actual ancestor.")
        result = subprocess.run(["git", "diff", "--name-only", "--no-renames", "--no-ext-diff",
                                 "-z", SCOPE_AUDIT_BASELINE], cwd=ROOT,
                                env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        cached = subprocess.run(["git", "diff", "--cached", "--name-only", "--no-renames", "--no-ext-diff",
                                 "-z", SCOPE_AUDIT_BASELINE], cwd=ROOT,
                                env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(cached.returncode, 0, cached.stderr)
        history = subprocess.run(["git", "rev-list", SCOPE_AUDIT_BASELINE + "..HEAD"], cwd=ROOT,
                                 env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(history.returncode, 0, history.stderr)
        history_paths = b""
        for oid in history.stdout.splitlines():
            changed = subprocess.run(["git", "diff-tree", "--no-commit-id", "--name-only",
                                      "--no-renames", "--no-ext-diff", "-r", "-m", "-z", oid.decode("ascii")],
                                     cwd=ROOT, env=audit_env, capture_output=True, timeout=10)
            self.assertEqual(changed.returncode, 0, changed.stderr)
            history_paths += changed.stdout
        allowed = {"amplifier-bundle/recipes/" + asset + ".yaml" for asset in ASSETS}
        allowed.add("amplifier-bundle/recipes/hook-rejection-repair.yaml")
        allowed.update("amplifier-bundle/tools/" + phase[2] for phase in PHASES)
        allowed.update({"amplifier-bundle/tools/workflow_hook_repair.sh", "amplifier-bundle/tools/workflow_stage_changes.sh"})
        allowed.update({"amplifier-bundle/tools/workflow_prepare_banner.sh", "amplifier-bundle/tools/autodrive_round_context.sh"})
        untracked = subprocess.run(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=ROOT,
                                   env=audit_env, capture_output=True, timeout=10)
        self.assertEqual(untracked.returncode, 0, untracked.stderr)
        for raw in filter(None, (result.stdout + cached.stdout + history_paths + untracked.stdout).split(b"\0")):
            path = os.fsdecode(raw)
            if path in APPROVED_CARGO_SHA256:
                candidate = ROOT / path
                self.assertTrue(candidate.is_file() and not candidate.is_symlink(), "Approved Cargo input is missing.")
                self.assertEqual(digest(candidate.read_bytes()), APPROVED_CARGO_SHA256[path],
                                 "Only the exact approved workspace dev-dependency input is permitted.")
                baseline = subprocess.run(["git", "show", SCOPE_AUDIT_BASELINE + ":" + path],
                                          cwd=ROOT, env=audit_env, capture_output=True, timeout=10)
                permitted_hashes = {APPROVED_CARGO_SHA256[path]}
                if baseline.returncode == 0:
                    permitted_hashes.add(digest(baseline.stdout))
                # The index and every historical blob are independent inputs.
                # Restoring approved worktree bytes must not hide a different
                # staged blob or a forbidden intermediate commit.
                for revision in [""] + [os.fsdecode(oid) for oid in history.stdout.splitlines()]:
                    blob = subprocess.run(["git", "show", revision + ":" + path], cwd=ROOT,
                                          env=audit_env, capture_output=True, timeout=10)
                    self.assertEqual(blob.returncode, 0, "Approved Cargo input is absent from the index or history.")
                    self.assertIn(digest(blob.stdout), permitted_hashes,
                                  "An unapproved Cargo blob is staged or committed.")
                continue
            permitted_test = path in EXISTING_TASK_TEST_PATHS or path in INTEGRATION_MAINTENANCE_PATHS
            self.assertTrue(path in allowed or permitted_test, "The input already contains the sole permitted Cargo edge: " + path)


# The mapping is data for later roles. A named test is coverage intent until its
# actual deterministic run passes; no map entry asserts implementation approval.
REQUIREMENTS_COVERAGE = {
    "Literal staging validates all input before writing and preserves legacy callers": ["StrictStagingTests"],
    "Independent linked ownership and uniform protected-branch admission": [
        "test_main_master_and_detached_head_cannot_mutate", "test_main_checkout_cannot_impersonate_linked_task",
        "test_missing_task_or_worktree_context_cannot_be_supplied_by_reviewer"],
    "Unauthorised admission preserves raw index, staged blobs/modes, worktree and HEAD": [
        "test_preexisting_staged_and_partial_entries_are_untouched", "test_staged_mode_deletion_and_intent_to_add_are_untouched",
        "test_unmerged_entries_are_untouched", "test_split_index_is_refused_without_refresh",
        "test_hostile_git_selectors_cannot_redirect_admission"],
    "Typed finite scope binds exact task, worktree, repository, index, HEAD and initial candidates": [
        "test_every_binding_field_is_required_and_exact", "test_capture_contains_literal_identities_modes_deletions_and_links",
        "test_changed_initial_content_mode_or_added_path_is_stale", "test_unsupported_head_change_cannot_renew_authority",
        "test_actual_branch_task_and_index_changes_invalidate_capture", "test_a_new_invocation_rejects_the_previous_verdict"],
    "Immutable authority is separate from changing observations and independently authenticated": [
        "test_missing_truncated_altered_or_self_rehashed_authority_refuses",
        "test_in_scope_repair_runs_actual_driver_and_keeps_authority_immutable"],
    "Malformed scope, ambiguous identities and clean-index-only permission refuse": [
        "test_malformed_scope_is_never_permissive_verdict", "test_missing_truncated_duplicate_and_non_boolean_scope_refuse",
        "test_invalid_duplicate_and_uncaptured_path_identities_refuse", "test_empty_approval_cannot_conceal_pending_source",
        "test_clean_empty_approval_stages_nothing_and_makes_no_commit"],
    "Repair checks scope before restaging, retry and nested commit and retains terminal 79": [
        "test_in_scope_repair_runs_actual_driver_and_keeps_authority_immutable",
        "test_unknown_repair_source_stops_before_internal_restaging_or_retry",
        "test_pre_push_repair_checks_scope_before_nested_fixing_commit",
        "test_in_scope_pre_push_repair_creates_and_checks_nested_fixing_commit",
        "test_scope_is_rechecked_after_a_rejected_retry_changes_paths",
        "test_terminal_79_is_not_retried_or_reclassified_as_success", "test_legacy_repair_callers_retain_their_existing_staging_behaviour"],
    "Actual normal/rejecting hooks, useful publication and post-mutation failures remain distinct": [
        "test_all_three_clean_source_only_checkpoints_publish", "test_rejecting_hooks_preserve_legitimate_staging_and_report_failure",
        "test_failed_push_retains_authorised_local_commit_and_error", "test_reinvocation_must_not_reuse_residual_staging_authority",
        "test_publish_result_reports_actual_publication_or_failed_push"],
    "Every commit, every parent, full outgoing range and supported rebase require scope": [
        "test_preexisting_outgoing_commits_need_scope_even_with_clean_index", "test_reviewed_preexisting_outgoing_range_can_publish",
        "test_deleted_unknown_history_cannot_hide_in_final_tree", "test_merge_second_parent_changes_require_authority",
        "test_hook_added_staged_source_is_checked_in_actual_resulting_commit",
        "test_intermediate_commit_cannot_be_hidden_by_unchanged_final_head",
        "test_supported_review_and_cleanup_rebases_are_tied_to_fetched_upstream"],
    "Complete extraction, original controls and narrow authorised edits remain preserved": ["RecipeContractTests"],
}


if __name__ == "__main__":
    unittest.main(verbosity=2)
