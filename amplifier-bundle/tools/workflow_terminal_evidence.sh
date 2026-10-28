#!/usr/bin/env bash
# Terminal-state evidence collector and adjudicator (issue #1268).
#
# `workflow_pr_scope.sh` answers a genuinely semantic question — "is this the PR
# this workflow was supposed to produce?" — with string matching over head/base
# refs. When it misses, `workflow_final_status.sh` used to exit non-zero and the
# whole run was reported as a failure. That happened AFTER a PR had been
# created, reviewed, quality-audited and merged: the run's real deliverable was
# in `main`, and two follow-up PRs it had opened were abandoned, one rotting
# into a conflicted state with nobody to rebase it. The gate meant to prove
# success destroyed it instead.
#
# The scope match stays as a cheap SIGNAL. It is no longer the arbiter. When it
# misses, this helper looks at what actually happened — is there a PR, did it
# merge, is the work in the base branch — and renders one of three verdicts:
#
#   SUCCESS    the work demonstrably landed or a live PR carries it  -> exit 0
#   UNCERTAIN  the evidence could not be read; nothing is claimed and
#              every outstanding artifact is enumerated for the caller -> exit 0
#   FAILED     the evidence WAS readable and shows the work did not
#              land and no PR carries it                              -> exit 1
#
# It is deliberately not a rubber stamp. `hard_negative` marks the case where
# the evidence positively shows failure; in that state no agent judgement, and
# no caller-supplied verdict, can lift the verdict to SUCCESS. And a SUCCESS
# asserted from outside is admitted ONLY when the collected evidence carries at
# least one positive artifact fact (a merged PR, an open PR, or work already in
# base). Judgement can always DOWNGRADE — that direction is never dangerous.
#
# Usage:
#   workflow_terminal_evidence.sh {observe-implementation|checkpoint-implementation|plan|capture|verify|restore}
#       [--request-file FILE|-]
#   workflow_terminal_evidence.sh finalize --action {final-status|collect|validate|complete}
#       [--request-file FILE|-]
#   Local requests use integer schema_version=1, workflow{name,started_at},
#   repository_root, and operation-specific fields. Logs stay private in the
#   per-worktree Git evidence store. Python 3.11+ is required for local modes.
#   workflow_terminal_evidence.sh collect [--repo O/R] [--head REF] [--base REF]
#       [--head-sha SHA] [--pr-url URL] [--pr-number N] [--created-after TIME]
#   workflow_terminal_evidence.sh collect-for-step        (recipe wrapper for collect)
#   workflow_terminal_evidence.sh verdict-token          (recipe wrapper: prose -> token)
#   workflow_terminal_evidence.sh adjudicate [--evidence JSON | --evidence-file F | -]
#       [--agent-verdict TOKEN] [--scope-reason REASON] [--report-only]
#
# Exit codes:
#   0  SUCCESS or UNCERTAIN (adjudicate); evidence emitted (collect)
#   1  FAILED (adjudicate)
#   2  usage error or missing required tooling

# Deliberately no `-e`: this helper must always terminate with a structured
# answer on stdout rather than dying mid-probe and leaving the caller guessing.
set -uo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  sed -n '/^# Usage:/,/^#   2  usage/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
}

# Local execution evidence is deliberately dispatched before platform plumbing.
# Python 3.11's standard library supplies strict JSON, direct argv execution, file
# descriptor checks, locking and atomic publication; no receipt is shell code.
case "${1:-}" in
  observe-implementation|checkpoint-implementation|plan|capture|verify|restore|finalize)
    command -v python3 >/dev/null 2>&1 || { echo '{"ok":false,"error":{"code":"MISSING_TOOLING","message":"python3 is required","required_next_action":"Provide Python 3."}}'; exit 2; }
    python3 -c 'import tomllib' >/dev/null 2>&1 || { echo '{"ok":false,"error":{"code":"MISSING_TOOLING","message":"Python 3.11 or newer is required","required_next_action":"Provide Python 3.11+."}}'; exit 2; }
    exec python3 /dev/fd/3 "$SCRIPT_DIR" "$@" 3<<'PY_LOCAL_EVIDENCE'
"""Version 1 local workflow evidence. The OS user and workflow store are trusted.

Plans are immutable observations, not receipts. Only capture creates execution
bindings. Verification maps every obligation to a sealed capture. Generations
preserve original typed outputs and support, never reconstructed completion.
"""
import copy
import ast
from contextlib import contextmanager
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import stat
import subprocess
import sys
import uuid
from urllib.parse import urlsplit, urlunsplit
import tomllib
import xml.etree.ElementTree as ET

os.umask(0o077)
HERE = Path(sys.argv[1])
MODE = sys.argv[2]
LIMIT = 8 * 1024 * 1024
SUPPORT = ('implementation_terminal_evidence', 'implementation', 'verdict_json',
           'verification_terminal_evidence', 'precommit_results', 'local_testing_gate')
ENV_KEYS = ('PATH', 'NODE_OPTIONS', 'NODE_ENV', 'npm_config_ignore_scripts',
            'npm_config_script_shell', 'PYTHONPATH', 'PYTHONHOME', 'PYTHONOPTIMIZE',
            'VIRTUAL_ENV', 'UV_PROJECT', 'UV_NO_SYNC', 'GOFLAGS', 'GOTOOLCHAIN',
            'GOWORK', 'GOOS', 'GOARCH', 'DOTNET_ROOT', 'DOTNET_ROLL_FORWARD',
            'RUST_TEST_THREADS', 'RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
            'RUSTC', 'RUSTDOC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
            'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'CARGO_BUILD_TARGET',
            'PYTEST_ADDOPTS', 'PYTEST_PLUGINS', 'GOENV', 'GOROOT',
            'MSBUILD_EXE_PATH', 'MSBuildSDKsPath', 'DOTNET_STARTUP_HOOKS',
            'npm_config_userconfig', 'NPM_CONFIG_USERCONFIG', 'SKIP', 'PRE_COMMIT_ALLOW_NO_CONFIG',
            'PRE_COMMIT_HOME', 'HUSKY', 'HUSKY_SKIP_HOOKS', 'GIT_CONFIG_COUNT',
            'GIT_CONFIG_PARAMETERS', 'UV_PYTHON', 'UV_PYTHON_PREFERENCE', 'UV_CONFIG_FILE',
            'UV_WORKING_DIRECTORY', 'PYTHONEXECUTABLE', 'DOTNET_CLI_HOME')

class EvidenceError(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message
        super().__init__(message)

def require(condition, code, message):
    if not condition:
        raise EvidenceError(code, message)

def encode(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()

def digest(data):
    return hashlib.sha256(data).hexdigest()

def pairs(items):
    obj = {}
    for key, value in items:
        require(key not in obj, 'INVALID_REQUEST', 'Duplicate JSON key: ' + key)
        obj[key] = value
    return obj

def parse(data):
    require(len(data) <= LIMIT and b'\x00' not in data, 'INVALID_REQUEST', 'Oversized document or NUL')
    try:
        obj = json.loads(data, object_pairs_hook=pairs,
                         parse_constant=lambda x: (_ for _ in ()).throw(ValueError(x)))
        def bounded(v, depth=0):
            require(depth < 48, 'INVALID_REQUEST', 'Document nesting limit exceeded')
            if isinstance(v, dict):
                require(len(v) <= 4096, 'INVALID_REQUEST', 'Object limit exceeded')
                for k, val in v.items():
                    bounded(k, depth + 1); bounded(val, depth + 1)
            elif isinstance(v, list):
                require(len(v) <= 4096, 'INVALID_REQUEST', 'Array limit exceeded')
                for val in v: bounded(val, depth + 1)
            elif isinstance(v, str):
                require('\x00' not in v, 'INVALID_REQUEST', 'NUL in string')
        bounded(obj)
        return obj
    except (ValueError, UnicodeError, RecursionError) as exc:
        raise EvidenceError('INVALID_REQUEST', 'Invalid JSON: ' + str(exc)) from exc

def text(v):
    return isinstance(v, str) and bool(v.strip()) and '\x00' not in v

def fields(obj, required, optional=()):
    require(isinstance(obj, dict) and set(required) <= obj.keys() and
            obj.keys() <= set(required) | set(optional), 'INVALID_REQUEST', 'Invalid object fields')

@contextmanager
def regular_fd(path):
    # Inspect and consume the same descriptor, denying symlinked artifact paths.
    path = Path(os.path.abspath(path))
    fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:-1]:
            next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd); fd = next_fd
        file_fd = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        try:
            info = os.fstat(file_fd)
            require(stat.S_ISREG(info.st_mode), 'ARTIFACT_NOT_REGULAR', str(path))
            require(info.st_mode & 0o444 != 0, 'ARTIFACT_UNREADABLE', str(path))
            yield file_fd, info
            after = os.fstat(file_fd)
            require((info.st_size, info.st_mtime_ns, info.st_ctime_ns) ==
                    (after.st_size, after.st_mtime_ns, after.st_ctime_ns),
                    'ARTIFACT_TAMPERED', 'File changed during inspection: ' + str(path))
        finally: os.close(file_fd)
    finally: os.close(fd)

def read_regular(path, limit=LIMIT):
    with regular_fd(path) as (fd, info):
        require(info.st_size <= limit, 'INVALID_REQUEST', 'Oversized structured file: ' + str(path))
        data = bytearray()
        while True:
            chunk = os.read(fd, min(65536, limit + 1 - len(data)))
            if not chunk: break
            data.extend(chunk)
            require(len(data) <= limit, 'INVALID_REQUEST', 'Structured file grew beyond limit')
        return bytes(data)

def inspect_regular(path, patterns=(), prefix_bytes=0):
    # Regexes are internal bounded patterns. Carry exceeds every pattern's
    # maximum match length, including matches split at a read boundary.
    sha = hashlib.sha256(); size = 0; carry = b''; prefix = b''
    hits = [False] * len(patterns)
    with regular_fd(path) as (fd, info):
        while True:
            chunk = os.read(fd, 65536)
            if not chunk: break
            sha.update(chunk); size += len(chunk)
            if len(prefix) < prefix_bytes: prefix += chunk[:prefix_bytes-len(prefix)]
            window = carry + chunk
            for i, pattern in enumerate(patterns):
                if not hits[i] and re.search(pattern, window): hits[i] = True
            carry = window[-4096:]
        require(size == info.st_size, 'ARTIFACT_TAMPERED', 'Incomplete file inspection')
    return {'sha256': sha.hexdigest(), 'bytes': size}, hits, prefix

def file_hash(path):
    return inspect_regular(path)[0]['sha256']


def run(argv, cwd=None, env=None):
    result = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    require(result.returncode == 0, 'MISSING_TOOLING',
            shlex.join(argv) + ': ' + result.stderr.decode(errors='replace')[:1000])
    return result.stdout

class Store:
    def __init__(self, root):
        self.root = root
        # Git metadata may be outside the worktree; reject symlinks throughout.
        cursor = Path('/')
        for part in root.parts[1:]:
            cursor /= part
            if not cursor.exists(): cursor.mkdir(mode=0o700)
            require(not cursor.is_symlink() and cursor.is_dir(), 'ARTIFACT_NOT_REGULAR', str(cursor))
        self.lock = os.open(root / 'lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        fcntl.flock(self.lock, fcntl.LOCK_EX)

    def path(self, relative):
        require(isinstance(relative, str) and relative and not Path(relative).is_absolute() and
                '..' not in Path(relative).parts, 'ARTIFACT_NOT_REGULAR', 'Unsafe artifact path')
        return self.root / relative

    def put(self, relative, value, raw=False):
        path = self.path(relative)
        cursor = self.root
        for part in Path(relative).parts[:-1]:
            cursor /= part
            if not cursor.exists(): cursor.mkdir(mode=0o700)
            require(not cursor.is_symlink() and cursor.is_dir(), 'ARTIFACT_NOT_REGULAR', str(cursor))
        data = value if raw else encode(value)
        temp = path.parent / ('.' + uuid.uuid4().hex)
        fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(fd, 'wb') as f:
                f.write(data); f.flush(); os.fsync(f.fileno())
            os.replace(temp, path)
            d = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try: os.fsync(d)
            finally: os.close(d)
        finally:
            if temp.exists(): temp.unlink()
        return {'path': relative, 'sha256': digest(data), 'bytes': len(data)}

    def get(self, relative):
        return parse(read_regular(self.path(relative), LIMIT))

    def inspect(self, ref, patterns=()):
        fields(ref, ('path', 'sha256', 'bytes'))
        info, hits, _ = inspect_regular(self.path(ref['path']), patterns)
        require(type(ref['bytes']) is int and ref['bytes'] == info['bytes'] and
                ref['sha256'] == info['sha256'], 'ARTIFACT_TAMPERED', str(ref['path']))
        return hits

    def artifact(self, ref, limit=LIMIT):
        self.inspect(ref)
        return read_regular(self.path(ref['path']), limit)

class Workflow:
    def __init__(self, request):
        self.r = request
        selected = os.environ.get('AMPLIHACK_CONTEXT_FILE')
        require('AMPLIHACK_CONTEXT_FILE' not in os.environ or text(selected),
                'INVALID_CONTEXT', 'An explicit empty context selector is invalid')
        requested = request.get('context_file')
        require('context_file' not in request or text(requested), 'INVALID_CONTEXT', 'Explicit context selector must be a nonempty file path')
        require(not (selected and requested and os.path.abspath(selected) != os.path.abspath(requested)),
                'INVALID_CONTEXT', 'Context selector conflicts with AMPLIHACK_CONTEXT_FILE')
        self.context_path = selected or requested
        self.context = self.load_context()
        actual_root = Path(run(['git', 'rev-parse', '--show-toplevel']).decode().strip()).resolve()
        root = request.get('repository_root', str(actual_root))
        require(isinstance(root, str) and Path(root).is_absolute() and Path(root) == actual_root,
                'IDENTITY_MISMATCH', 'Repository root must be the invoking canonical worktree root')
        self.root = actual_root
        selectors = [self.context[k] for k in ('repo_path', 'repository_root') if k in self.context]
        selectors += [os.environ[k] for k in ('REPO_PATH', 'RECIPE_VAR_repo_path') if k in os.environ]
        for selector in selectors:
            if selector != '':
                require(text(selector) and Path(selector).is_absolute() and
                        Path(selector).resolve() == self.root, 'IDENTITY_MISMATCH', 'Task repository selector mismatch')
        wt = self.context.get('worktree_setup', {})
        if isinstance(wt, str) and wt.strip(): wt = parse(wt.encode())
        require(isinstance(wt, dict), 'INVALID_CONTEXT', 'Selected worktree setup must be an object')
        if wt.get('worktree_path'):
            require(Path(wt['worktree_path']).resolve() == self.root, 'IDENTITY_MISMATCH', 'Selected worktree mismatch')
        for key in ('workflow_name', 'workflow_started_at'):
            require(key not in self.context or text(self.context[key]), 'INVALID_CONTEXT', 'Invalid original workflow identity: ' + key)
        started = self.context.get('workflow_started_at') or os.environ.get('WORKFLOW_STARTED_AT')
        default_identity = {'name': self.context.get('workflow_name', 'default-workflow'), 'started_at': started}
        run_id = self.context['original_run_id'] if 'original_run_id' in self.context else os.environ.get('AMPLIHACK_ORIGINAL_RUN_ID')
        require('original_run_id' not in self.context or text(run_id), 'INVALID_CONTEXT', 'Invalid original run identity')
        if run_id is not None: default_identity['original_run_id'] = run_id
        self.workflow = request.get('workflow', default_identity)
        fields(self.workflow, ('name', 'started_at'), ('original_run_id',))
        require(all(text(v) for v in self.workflow.values()), 'INVALID_REQUEST', 'Missing workflow identity')
        try:
            instant = datetime.datetime.fromisoformat(self.workflow['started_at'].replace('Z', '+00:00'))
            require(instant.tzinfo is not None and 'T' in self.workflow['started_at'], 'INVALID_REQUEST', 'RFC3339 workflow start required')
        except ValueError as exc: raise EvidenceError('INVALID_REQUEST', 'Invalid workflow start') from exc
        for observed in (started, os.environ.get('WORKFLOW_STARTED_AT')):
            require(not observed or self.workflow['started_at'] == observed,
                    'IDENTITY_MISMATCH', 'Original workflow start changed')
        for field, observed in (('name', self.context.get('workflow_name')), ('original_run_id', run_id)):
            require(observed is None or self.workflow.get(field) == observed,
                    'IDENTITY_MISMATCH', 'Original workflow identity conflicts: ' + field)
        require(not os.environ.get('AMPLIHACK_ORIGINAL_RUN_ID') or
                run_id is None or run_id == os.environ['AMPLIHACK_ORIGINAL_RUN_ID'],
                'IDENTITY_MISMATCH', 'Original run identity selectors conflict')
        self.repository = self.repo_identity()
        self.identity_store = Store(Path(self.repository['git_dir']) / 'amplihack/workflow-identities/v1')
        identity_key = digest(encode({'root': str(self.root), 'started_at': self.workflow['started_at']})) + '.json'
        if (self.identity_store.root / identity_key).exists():
            original_workflow = self.identity_store.get(identity_key)
            if 'workflow' not in request and 'workflow_name' not in self.context:
                self.workflow['name'] = original_workflow['name']
            if 'workflow' not in request and run_id is None and 'original_run_id' in original_workflow:
                self.workflow['original_run_id'] = original_workflow['original_run_id']
            require(self.workflow == original_workflow, 'IDENTITY_MISMATCH', 'Original workflow identity changed')
        # Migration inspects identities only, never earlier success artifacts.
        evidence_root = Path(self.repository['git_dir']) / 'amplihack/workflow-evidence/v1'
        for path in evidence_root.glob('*/identity.json'):
            identity = parse(read_regular(path))
            if identity['workflow']['started_at'] != self.workflow['started_at']: continue
            original_workflow = identity['workflow']
            if 'workflow' not in request and 'workflow_name' not in self.context:
                self.workflow['name'] = original_workflow['name']
            if 'workflow' not in request and run_id is None and 'original_run_id' in original_workflow:
                self.workflow['original_run_id'] = original_workflow['original_run_id']
            require(self.workflow == original_workflow and identity['repository']['root'] == str(self.root),
                    'IDENTITY_MISMATCH', 'Workflow name or original identity conflicts with established cohort')
        self.identity_store.put(identity_key, self.workflow)
        key = digest(encode({'workflow': {k: self.workflow[k] for k in ('name', 'started_at')},
                             'root': str(self.root)}))
        self.store = Store(Path(self.repository['git_dir']) / 'amplihack/workflow-evidence/v1' / key)
        if (self.store.root / 'identity.json').exists():
            original = self.store.get('identity.json')
            if 'workflow' not in request and run_id is None and 'original_run_id' in original['workflow']:
                self.workflow['original_run_id'] = original['workflow']['original_run_id']
            require(original['workflow'] == self.workflow and all(original['repository'][k] == self.repository[k] for k in ('root', 'git_dir')),
                    'IDENTITY_MISMATCH', 'Cohort workflow/worktree identity changed')
        else:
            self.store.put('identity.json', self.identity())

    def load_context(self):
        if self.context_path:
            obj = parse(read_regular(self.context_path, LIMIT))
            require(isinstance(obj, dict), 'INVALID_CONTEXT', 'Selected context must be an object')
            defaults = ('worktree_setup', 'implementation', 'implementation_terminal_evidence', 'verification_terminal_evidence', 'terminal_state', 'finalization_evidence', 'workflow_result', 'finalizer_step_status', 'pr_publish_result', 'publish_terminal_evidence')
            return {k:v for k,v in obj.items() if not (k in defaults and v == '')}
        # Use the established transport reader for canonical spilled values and
        # legacy inputs. Once selected, its cohort authority remains unchanged.
        keys = SUPPORT + ('repo_path', 'worktree_setup', 'allow_no_op',
                         'finalization_evidence', 'workflow_result', 'finalizer_step_status',
                         'pr_url', 'pr_number', 'pr_publish_result', 'remote_host_type',
                         'task_description', 'branch_name', 'base_ref', 'terminal_state',
                         'publish_terminal_evidence', 'issue_number')
        script = '. "$1/workflow_context.sh"; for key in "${@:2}"; do name="RECIPE_VAR_$key"; case "$key" in worktree_setup|implementation|implementation_terminal_evidence|verification_terminal_evidence|terminal_state|finalization_evidence|workflow_result|finalizer_step_status|pr_publish_result|publish_terminal_evidence) if [[ ${!name+x} && -z ${!name} ]]; then continue; fi;; esac; if [[ $key == verdict_json && ${!name+x} ]]; then value=${!name}; else value=$(workflow_context_read "$key"; rc=$?; printf "."; exit "$rc"); rc=$?; value=${value%.}; [ "$rc" != 3 ] || exit 3; fi; printf "%s\\0%s\\0" "$key" "$value"; done'
        process = subprocess.run(['bash', '-c', script, 'context-reader', str(HERE), *keys],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        require(process.returncode == 0, 'INVALID_CONTEXT', 'Invalid authoritative workflow transport: ' + process.stderr.decode(errors='replace')[:1000])
        output = process.stdout.split(b'\0')
        obj = {}
        for k, v in zip(output[::2], output[1::2]):
            if not v: continue
            try: obj[k.decode()] = parse(v)
            except EvidenceError: obj[k.decode()] = v.decode()
        for key in ('workflow_name', 'original_run_id', 'workflow_started_at'):
            name = 'RECIPE_VAR_' + key
            if name in os.environ: obj[key] = os.environ[name]
        return obj

    def repo_identity(self):
        remote = subprocess.run(['git', 'config', '--get', 'remote.origin.url'], cwd=self.root,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        require(remote.returncode in (0, 1), 'IDENTITY_MISMATCH', 'Cannot read origin')
        origin = remote.stdout.decode().strip()
        identity = origin or ('local:' + str(self.root))
        github = re.fullmatch(r'(?:https?://(?:[^/@]+@)?github\.com/|git@github\.com:|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?/?', origin)
        if github:
            identity = github.group(1)
        elif '://' in origin:
            parsed = urlsplit(origin)
            identity = urlunsplit((parsed.scheme, parsed.hostname or '', parsed.path, '', ''))
        return {'root': str(self.root), 'git_dir': str(Path(run(['git', 'rev-parse', '--absolute-git-dir'], self.root).decode().strip()).resolve()),
                'identity': identity, 'origin_sha256': digest(origin.encode())}

    def sequence(self):
        value = self.store.get('sequence.json') if (self.store.root / 'sequence.json').exists() else 0
        require(type(value) is int and value >= 0, 'COHORT_MISMATCH', 'Invalid observation sequence')
        self.store.put('sequence.json', value + 1)
        return value + 1

    def identity(self):
        return {'schema_version': 1, 'workflow': self.workflow, 'repository': self.repository}

    def source(self):
        names = run(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], self.root).split(b'\0')
        values = []
        for name in sorted(set(n for n in names if n)):
            relative = os.fsdecode(name)
            # Existing narrow generated runtime exclusions only. Ignored build
            # products are omitted by Git; tracked files are always observed.
            if relative == '.amplihack/indexing.pid' or relative.startswith('.claude/runtime/'):
                continue
            path = self.root / relative
            if not path.exists() and not path.is_symlink(): value = 'DELETED'
            elif path.is_symlink(): value = self.source_file(path)
            elif path.is_dir():
                value = run(['git', 'rev-parse', 'HEAD'], path).decode().strip()
            else: value = file_hash(path)
            values.append((relative, value, stat.S_IMODE(path.lstat().st_mode) if path.exists() else 0))
        return {'head': run(['git', 'rev-parse', '--verify', 'HEAD'], self.root).decode().strip(),
                'fingerprint': digest(encode(values))}

    def source_file(self, path):
        try: target = path.resolve(strict=True)
        except (OSError, RuntimeError) as exc:
            raise EvidenceError('SOURCE_UNPROVEN', 'Unresolved/cyclic source input: ' + str(path)) from exc
        require(target.is_relative_to(self.root) and target.is_file(), 'SOURCE_UNPROVEN',
                'External or non-file source target needs separate authority: ' + str(path))
        return {'link': os.readlink(path) if path.is_symlink() else None,
                'target': str(target), 'sha256': file_hash(target)}

    def safe_source(self):
        try: return self.source()
        except (EvidenceError, OSError): return None

    def environment(self):
        keys = set(ENV_KEYS) | {k for k in os.environ if k.startswith(('CARGO_TARGET_', 'CARGO_ALIAS_', 'GIT_CONFIG_KEY_', 'GIT_CONFIG_VALUE_', 'HUSKY_'))}
        return {k: os.environ[k] for k in sorted(keys) if k in os.environ}

    def configurations(self, cwd):
        paths = set()
        for parent in (Path(cwd), *Path(cwd).parents):
            for name in ('.cargo/config', '.cargo/config.toml', 'rust-toolchain', 'rust-toolchain.toml',
                         '.npmrc', '.yarnrc', '.yarnrc.yml', 'uv.toml', '.python-version',
                         'pytest.ini', 'tox.ini', 'setup.cfg', 'go.work',
                         'Directory.Build.props', 'Directory.Build.targets', 'Directory.Packages.props',
                         'global.json', 'NuGet.Config', 'nuget.config',
                         '.pre-commit-config.yaml', '.pre-commit-config.yml'):
                paths.add(parent / name)
        cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
        paths.update(cargo_home / n for n in ('config', 'config.toml'))
        paths.update(Path.home() / n for n in ('.npmrc', '.yarnrc', '.yarnrc.yml', '.config/uv/uv.toml', '.config/go/env', '.config/pip/pip.conf'))
        for key in ('GOENV', 'npm_config_userconfig', 'NPM_CONFIG_USERCONFIG', 'SKIP', 'PRE_COMMIT_ALLOW_NO_CONFIG',
            'PRE_COMMIT_HOME', 'HUSKY', 'HUSKY_SKIP_HOOKS', 'GIT_CONFIG_COUNT',
            'GIT_CONFIG_PARAMETERS', 'UV_PYTHON', 'UV_PYTHON_PREFERENCE', 'UV_CONFIG_FILE',
            'UV_WORKING_DIRECTORY', 'PYTHONEXECUTABLE', 'DOTNET_CLI_HOME'):
            if os.environ.get(key) and os.environ[key] != 'off': paths.add(Path(os.environ[key]).absolute())
        git_dir = Path(self.repository['git_dir'])
        common = Path(run(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'], self.root).decode().strip())
        paths.update((common / 'config', git_dir / 'config.worktree', common / 'info/exclude'))
        excludes = subprocess.run(['git', 'config', '--path', '--get', 'core.excludesfile'], cwd=self.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        require(excludes.returncode in (0, 1), 'CONFIGURATION_CHANGED', 'Cannot inspect Git exclude configuration')
        if excludes.returncode == 0: paths.add(Path(excludes.stdout.decode().strip()).absolute())
        hooks = Path(run(['git', 'rev-parse', '--path-format=absolute', '--git-path', 'hooks'], self.root).decode().strip())
        if hooks.is_dir(): paths.update(p for p in hooks.iterdir() if not p.name.endswith('.sample'))
        return {str(p): {'link': os.readlink(p) if p.is_symlink() else None, 'target': str(p.resolve(strict=True)), 'sha256': file_hash(p.resolve(strict=True))} for p in sorted(paths) if p.exists() or p.is_symlink()}

    def executable(self, name, cwd, depth=0):
        require(depth < 4, 'TOOL_OVERRIDE_UNPROVEN', 'Cyclic executable interpreter chain')
        resolved = (str(Path(cwd) / name) if '/' in name and not Path(name).is_absolute()
                    else name if Path(name).is_absolute() else shutil.which(name))
        require(resolved and Path(resolved).exists(), 'MISSING_TOOLING', 'Executable unavailable: ' + name)
        path = Path(resolved).resolve()
        info, _, prefix = inspect_regular(path, prefix_bytes=512)
        result = {'path': str(path), 'sha256': info['sha256']}
        if prefix.startswith(b'#!'):
            require(b'\n' in prefix, 'TOOL_OVERRIDE_UNPROVEN', 'Unbounded executable shebang')
            words = shlex.split(prefix.split(b'\n', 1)[0][2:].decode())
            require(words, 'TOOL_OVERRIDE_UNPROVEN', 'Missing executable interpreter')
            result['interpreter'] = self.executable(words[0], cwd, depth + 1)
            result['interpreter_argv'] = words
            if Path(words[0]).name == 'env':
                delegates = words[1:]
                if delegates[:1] == ['-S']: delegates = delegates[1:]
                require(delegates and not delegates[0].startswith('-') and '=' not in delegates[0],
                        'TOOL_OVERRIDE_UNPROVEN', 'Unresolved env interpreter dispatch')
                result['delegate'] = self.executable(delegates[0], cwd, depth + 1)
        return result

    def tools(self, obligation):
        result = {'entry': self.executable(obligation['argv'][0], obligation['cwd'])}
        result['inputs'] = {}
        for arg in obligation['argv'][1:]:
            path = Path(obligation['cwd']) / arg
            if path.is_file():
                result['inputs'][arg] = self.source_file(path)
        runner = obligation['runner']
        # Bind package-manager delegates as well as the top-level executable.
        delegates = []
        if obligation['toolchain'] in ('javascript', 'typescript'): delegates.append('node')
        if runner in ('vitest', 'tsc', 'jest', 'eslint'):
            candidate = Path(obligation['cwd']) / 'node_modules/.bin' / runner
            require(candidate.exists(), 'MISSING_TOOLING', 'Configured runner unavailable: ' + runner)
            result['runner'] = self.executable(str(candidate), obligation['cwd'])
        if obligation['toolchain'] == 'rust' and Path(result['entry']['path']).name == 'rustup':
            for name in ('cargo', 'rustc', 'rustdoc'):
                delegate = run([result['entry']['path'], 'which', name], obligation['cwd']).decode().strip()
                result[name] = self.executable(delegate, obligation['cwd'])
        if obligation['toolchain'] == 'dotnet':
            version = run([obligation['argv'][0], '--version'], obligation['cwd']).decode().strip().splitlines()[-1]
            installed = run([obligation['argv'][0], '--list-sdks'], obligation['cwd']).decode().splitlines()
            matches = [line[len(version)+2:-1] for line in installed if line.startswith(version+' [') and line.endswith(']')]
            require(len(matches) == 1, 'TOOL_OVERRIDE_UNPROVEN', 'Selected .NET SDK cannot be resolved')
            sdk = Path(matches[0]) / version
            result['sdk'] = {'version': version, 'driver': self.executable(str(sdk / 'dotnet.dll'), obligation['cwd'])}
        if obligation['toolchain'] == 'python':
            argv = obligation['argv']
            if Path(argv[0]).name == 'uv':
                interpreter = run([argv[0], 'python', 'find', '--project', obligation['cwd']], obligation['cwd'],
                                  dict(os.environ, UV_OFFLINE='1')).decode().strip()
                result['python'] = self.executable(interpreter, obligation['cwd'])
            elif Path(argv[0]).name.startswith('python'):
                result['python'] = result['entry']
        for name in delegates:
            # A direct Node command is also its own delegate. Reuse this
            # observation only within this call; later validations rehash it.
            if name == obligation['argv'][0]: result[name] = result['entry']
            elif name == 'node' and result['entry'].get('delegate', {}).get('path') == str(Path(shutil.which('node') or '/missing-node').resolve()):
                result[name] = result['entry']['delegate']
            else: result[name] = self.executable(name, obligation['cwd'])
        return result

    def refs(self, refs):
        require(isinstance(refs, list) and refs, 'INVALID_REQUEST', 'Inspected references required')
        for ref in refs:
            fields(ref, ('path', 'sha256', 'purpose'))
            require(text(ref['purpose']) and text(ref['path']) and not Path(ref['path']).is_absolute() and
                    '..' not in Path(ref['path']).parts, 'INVALID_REQUEST', 'Invalid source reference')
            require(self.source_file(self.root / ref['path'])['sha256'] == ref['sha256'],
                    'SOURCE_STALE', 'Inspected source changed: ' + ref['path'])

    def inventory(self):
        names = run(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], self.root).split(b'\0')
        found = []
        for raw in sorted(set(n for n in names if n)):
            path = Path(os.fsdecode(raw)); name = path.name
            tool = ('rust' if name == 'Cargo.toml' else 'javascript' if name == 'package.json' else
                    'python' if name in ('pyproject.toml', 'setup.py', 'setup.cfg', 'requirements.txt') else
                    'go' if name == 'go.mod' else 'dotnet' if path.suffix in ('.csproj', '.fsproj', '.vbproj') else
                    'unknown' if name in ('pom.xml', 'build.gradle', 'Gemfile', 'mix.exs', 'composer.json', 'CMakeLists.txt') else None)
            if tool == 'javascript':
                package = parse(read_regular(self.root / path))
                deps = {**package.get('dependencies', {}), **package.get('devDependencies', {})}
                if (self.root / path.parent / 'tsconfig.json').exists() or 'typescript' in deps:
                    tool = 'typescript'
            if tool: found.append((str(path.parent), tool, str(path)))
        return found

    def plan(self):
        observed_source = self.source()
        p = copy.deepcopy(self.r)
        components, obligations, exclusions = p['components'], p['obligations'], p['exclusions']
        require(all(isinstance(x, list) for x in (components, obligations, exclusions)) and components,
                'INVALID_REQUEST', 'Plan arrays required')
        ids = set(); unresolved = []
        for c in components:
            fields(c, ('id', 'root', 'toolchain', 'disposition', 'inspected', 'justification'))
            require(text(c['id']) and c['id'] not in ids and text(c['justification']), 'INVALID_REQUEST', 'Invalid component identity')
            ids.add(c['id']); self.refs(c['inspected'])
            require(text(c['root']) and not Path(c['root']).is_absolute() and '..' not in Path(c['root']).parts,
                    'INVALID_REQUEST', 'Component root must be repository-relative')
            require(c['disposition'] in ('required', 'excluded', 'unresolved'), 'INVALID_REQUEST', 'Unknown component disposition')
            if c['toolchain'] not in ('rust', 'javascript', 'typescript', 'python', 'go', 'dotnet') or c['disposition'] == 'unresolved':
                unresolved.append('Unresolved component: ' + c['id'])
        seen = set()
        for o in obligations:
            fields(o, ('id', 'component_ids', 'kind', 'toolchain', 'runner', 'argv', 'cwd', 'scope', 'selected_from'), ('threading', 'scenario'))
            require(text(o['id']) and o['id'] not in seen, 'INVALID_REQUEST', 'Duplicate obligation ID')
            seen.add(o['id'])
            require(isinstance(o['component_ids'], list) and o['component_ids'] and all(text(v) for v in o['component_ids']) and
                    len(o['component_ids']) == len(set(o['component_ids'])) and set(o['component_ids']) <= ids,
                    'INVALID_REQUEST', 'Unknown obligation component')
            require(o['toolchain'] in ('rust', 'javascript', 'typescript', 'python', 'go', 'dotnet'), 'INVALID_REQUEST', 'Unknown obligation toolchain')
            require(o['kind'] in ('test', 'typecheck', 'quality', 'precommit', 'consumer') and text(o['runner']) and text(o['scope']),
                    'INVALID_REQUEST', 'Unknown obligation kind or runner')
            require(isinstance(o['argv'], list) and o['argv'] and all(text(a) for a in o['argv']), 'INVALID_REQUEST', 'Nonempty argv required')
            require(text(o['cwd']) and Path(o['cwd']).is_absolute() and Path(o['cwd']).resolve() == Path(o['cwd']) and
                    Path(o['cwd']).is_relative_to(self.root) and Path(o['cwd']).is_dir(), 'INVALID_REQUEST', 'Invalid execution cwd')
            self.refs(o['selected_from'])
            if o['kind'] == 'consumer':
                scenario = o.get('scenario'); fields(scenario, ('role', 'assertions', 'inspected', 'entry_points', 'cases', 'assertion_refs'))
                require(scenario['role'] in ('basic', 'edge_or_integration') and isinstance(scenario['assertions'], list) and
                        scenario['assertions'] and all(text(a) for a in scenario['assertions']), 'INVALID_REQUEST', 'Consumer assertions required')
                self.refs(scenario['inspected']); self.refs(scenario['assertion_refs'])
            if o['toolchain'] == 'rust' and o['kind'] == 'test' and not self.rust_command(o):
                unresolved.append('Rust full-workspace/default-thread command required: ' + o['id'])
        exclusion_ids = set()
        for e in exclusions:
            fields(e, ('id', 'component_ids', 'subject', 'reason', 'policy_refs'))
            require(text(e['id']) and e['id'] not in exclusion_ids and text(e['reason']) and
                    isinstance(e['component_ids'], list) and e['component_ids'] and all(text(v) for v in e['component_ids']) and set(e['component_ids']) <= ids,
                    'INVALID_REQUEST', 'Invalid exclusion')
            exclusion_ids.add(e['id']); self.refs(e['policy_refs'])
        inventory = self.inventory()
        for root, tool, manifest in inventory:
            matches = [c for c in components if str(Path(c['root'])) == root and
                       (c['toolchain'] == tool or tool == 'javascript' and c['toolchain'] == 'typescript')]
            # Cover only declared members, never a nested independent workspace
            # or an arbitrary package merely sharing a directory prefix.
            if tool == 'rust' and not matches:
                child = tomllib.loads(read_regular(self.root / manifest).decode())
                if 'workspace' not in child:
                    for c in components:
                        if c['toolchain'] != 'rust': continue
                        candidate = self.root / c['root']
                        cargo = candidate / 'Cargo.toml'
                        if not cargo.exists(): continue
                        workspace = tomllib.loads(read_regular(cargo).decode()).get('workspace', {})
                        members = {str(p.relative_to(self.root)) for pattern in workspace.get('members', [])
                                   for p in candidate.glob(pattern) if p.is_dir()}
                        excluded = {str(p.relative_to(self.root)) for pattern in workspace.get('exclude', [])
                                    for p in candidate.glob(pattern) if p.is_dir()}
                        if root in members - excluded: matches.append(c)
            if not matches: unresolved.append('Manifest omitted: ' + manifest)
        roles = {o['scenario']['role'] for o in obligations if o['kind'] == 'consumer'}
        if roles != {'basic', 'edge_or_integration'}: unresolved.append('Both consumer scenario roles required')
        for c in components:
            if c['disposition'] == 'excluded':
                if not any(e['subject'] == 'component' and c['id'] in e['component_ids'] for e in exclusions):
                    unresolved.append('Component exclusion lacks inspected policy: ' + c['id'])
                continue
            selected = [o for o in obligations if c['id'] in o['component_ids']]
            tests = [o for o in selected if o['kind'] == 'test' and o['toolchain'] == c['toolchain']]
            if not tests: unresolved.append('Missing tests: ' + c['id'])
            if c['toolchain'] == 'rust' and not any(self.rust_command(o) and Path(o['cwd']) == self.root / c['root'] for o in tests):
                unresolved.append('Missing workspace execution: ' + c['id'])
            package = self.root / c['root'] / 'package.json'
            if package.exists() and c['toolchain'] in ('javascript', 'typescript'):
                scripts = parse(read_regular(package)).get('scripts', {})
                for kind, names in (('quality', ('quality', 'lint', 'format:check', 'check')), ('typecheck', ('typecheck', 'type-check'))):
                    for name in names:
                        if name in scripts and not any(o['kind'] == kind and
                           self.matches_script(o['argv'], name, scripts[name]) for o in selected):
                            unresolved.append('Configured ' + name + ' omitted: ' + c['id'])
                if c['toolchain'] == 'typescript' and not any(o['kind'] == 'typecheck' for o in selected):
                    unresolved.append('TypeScript typecheck unresolved: ' + c['id'])
            hooks = any((self.root / c['root'] / n).exists() for n in ('.pre-commit-config.yaml', '.pre-commit-config.yml', '.husky/pre-commit'))
            hook_path = Path(run(['git', 'rev-parse', '--git-path', 'hooks/pre-commit'], self.root).decode().strip())
            if not hook_path.is_absolute(): hook_path = self.root / hook_path
            hooks = hooks or hook_path.exists()
            if hooks and not any(o['kind'] == 'precommit' for o in selected): unresolved.append('Configured hooks omitted: ' + c['id'])
            if not hooks and not any(o['kind'] == 'precommit' for o in selected) and not any(
                    e['subject'] == 'precommit' and c['id'] in e['component_ids'] for e in exclusions):
                unresolved.append('Hook applicability unresolved: ' + c['id'])
        require(observed_source == self.source(), 'SOURCE_STALE', 'Source changed while registering plan')
        p.update(self.identity()); p['source'] = observed_source; p['unresolved'] = unresolved
        plan_id = digest(encode(p)); p['plan_id'] = plan_id
        ref = self.store.put('plans/' + plan_id + '/plan.json', p)
        self.store.put('plans/' + plan_id + '/binding.json', {'plan': ref})
        return {'schema_version': 1, 'ok': not unresolved, 'plan_id': plan_id, 'plan': ref,
                'status': 'UNPROVEN' if unresolved else 'READY', 'unresolved': unresolved}, int(bool(unresolved))

    def rust_command(self, o):
        return (o['argv'] == ['cargo', 'test', '--workspace', '--locked'] and
                o['scope'] == 'FULL_WORKSPACE' and o.get('threading') == 'DEFAULT')

    def load_plan(self, plan_id):
        require(isinstance(plan_id, str) and re.fullmatch('[a-f0-9]{64}', plan_id), 'INVALID_REQUEST', 'Invalid plan ID')
        binding = self.store.get('plans/' + plan_id + '/binding.json')
        p = parse(self.store.artifact(binding['plan']))
        require(p['plan_id'] == plan_id and p['repository'] == self.repository and p['workflow'] == self.workflow,
                'IDENTITY_MISMATCH', 'Plan identity mismatch')
        return p

    def plan_references(self, p):
        for component in p['components']: self.refs(component['inspected'])
        for obligation in p['obligations']:
            self.refs(obligation['selected_from'])
            if obligation['kind'] == 'consumer': self.refs(obligation['scenario']['inspected'])
        for exclusion in p['exclusions']: self.refs(exclusion['policy_refs'])

    def capture(self):
        p = self.load_plan(self.r['plan_id'])
        self.plan_references(p)
        require(not p['unresolved'], 'OBLIGATION_UNRESOLVED', 'Cannot capture an unresolved plan')
        matches = [o for o in p['obligations'] if o['id'] == self.r['obligation_id']]
        require(len(matches) == 1, 'INVALID_REQUEST', 'Unknown obligation')
        o = matches[0]
        require(o['argv'] == self.r['argv'], 'COMMAND_MISMATCH', 'Command differs from registered argv')
        require(o['cwd'] == self.r['cwd'], 'CWD_MISMATCH', 'Execution directory differs from plan')
        producer = 'step-13-local-testing' if o['kind'] == 'consumer' else 'step-12-run-precommit'
        require(self.r['producer_step'] == producer, 'IDENTITY_MISMATCH', 'Wrong producing step')
        invoking = os.environ.get('AMPLIHACK_RECIPE_STEP_ID') or os.environ.get('RECIPE_STEP_ID')
        require(not invoking or invoking == producer, 'IDENTITY_MISMATCH', 'Invoking step differs from producer')
        before = self.source()
        require(before == p['source'], 'SOURCE_STALE', 'Source changed after plan registration; register a fresh plan')
        tools = self.tools(o); environment = self.environment(); configs = self.configurations(o['cwd'])
        execution_id = uuid.uuid4().hex; prefix = 'attempts/' + execution_id + '/'
        reserved = dict(self.identity(), sequence=self.sequence(), execution_id=execution_id, plan_id=p['plan_id'], obligation_id=o['id'],
                        producer_step=producer, invocation={'recipe_run_id': os.environ.get('AMPLIHACK_RECIPE_RUN_ID'),
                        'step_id': invoking, 'capture_pid': os.getpid(), 'parent_pid': os.getppid()},
                        source_before=before, command={
                            'argv': o['argv'], 'display': shlex.join(o['argv']), 'cwd': o['cwd'],
                            'toolchain': o['toolchain'], 'runner': o['runner'], 'executable': tools['entry'], 'tools': tools},
                        environment=environment, configurations=configs, state='incomplete')
        self.store.put(prefix + 'attempt.json', reserved)
        self.store.put('latest-executions/' + p['plan_id'] + '/' + digest(o['id'].encode()) + '.json', {'execution_id': execution_id})
        # Reserve before spawn. stdout/stderr stream directly into a private raw
        # log; no buffer limit can turn partial output into a successful receipt.
        logpath = self.store.path(prefix + 'raw.log')
        fd = os.open(logpath, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(fd, 'wb') as output:
                child = subprocess.Popen(o['argv'], cwd=o['cwd'], stdout=output, stderr=subprocess.STDOUT)
                status = child.wait(); output.flush(); os.fsync(output.fileno())
        except OSError as exc:
            raise EvidenceError('MISSING_TOOLING', 'Capture could not start: ' + str(exc)) from exc
        after = self.source()
        normal = status >= 0
        exit_code = status if normal else None
        receipt = self.store.put(prefix + 'exit.txt', (str(status if normal else 128 - status) + '\n').encode(), raw=True)
        log = dict(inspect_regular(logpath)[0], path=prefix + 'raw.log')
        metadata = dict(reserved, state='sealed', source_after=after,
                        process={'termination': 'exited' if normal else 'signaled', 'exit_code': exit_code,
                                 'signal': -status if not normal else None, 'pid': child.pid, 'environment': environment},
                        artifacts={'raw_log': log, 'exit_receipt': receipt})
        metadata_ref = self.store.put(prefix + 'metadata.json', metadata)
        record = dict(metadata, capture_binding={'metadata': metadata_ref})
        record_ref = self.store.put(prefix + 'record.json', record)
        self.store.put('executions/' + execution_id + '.json', {'record': record_ref, 'metadata': metadata_ref})
        return {'schema_version': 1, 'ok': normal and status == 0 and before == after,
                'execution_id': execution_id, 'record': record_ref, 'process_exit_code': exit_code}, int(not(normal and status == 0 and before == after))

    def rust_execution(self, o, e):
        require(self.rust_command(o), 'RUST_SCOPE_INVALID', 'Not a full-workspace Rust execution')
        env = e['process']['environment']
        require(not env.get('RUST_TEST_THREADS'), 'TEST_THREADS_OVERRIDDEN', 'RUST_TEST_THREADS overrides default threading')
        overrides = ('RUSTC', 'RUSTDOC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS')
        require(not any(env.get(k) for k in overrides) and not any(
            (k.startswith('CARGO_TARGET_') and k.endswith('_RUNNER') or k.startswith('CARGO_ALIAS_')) and v for k,v in env.items()),
            'TOOL_OVERRIDE_UNPROVEN', 'Rust execution override needs independent proof')
        for path in e['configurations']:
            cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
            if Path(path).name not in ('config', 'config.toml') or not ('.cargo' in Path(path).parts or Path(path).parent == cargo_home):
                continue
            contents = read_regular(path).decode()
            require(not re.search(r'(?mi)(^\s*(runner|rustc|rustdoc|rustc-wrapper|rustc-workspace-wrapper|rustflags|test)\s*=|RUST_TEST_THREADS|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|CARGO_TARGET_.*_RUNNER|--test-threads|--skip)', contents),
                    'TOOL_OVERRIDE_UNPROVEN', 'Cargo configuration can override execution: ' + path)
        _, _, executable = inspect_regular(e['command']['executable']['path'], prefix_bytes=4)
        require(executable.startswith(b'\x7fELF') or executable[:4] in (b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf'),
                'TOOL_OVERRIDE_UNPROVEN', 'Cargo must resolve to a native Cargo/rustup executable')
        hits = self.store.inspect(e['artifacts']['raw_log'], (rb'test result: ok\.', rb'test result: FAILED'))
        require(hits[0] and not hits[1],
                'RUST_SCOPE_INVALID', 'No completed native Rust test harness found')

    def validate_execution(self, execution_id, p, o):
        require(isinstance(execution_id, str) and re.fullmatch('[a-f0-9]{32}', execution_id), 'CAPTURE_BINDING_MISSING', 'Unknown execution ID')
        index = self.store.get('executions/' + execution_id + '.json')
        e = parse(self.store.artifact(index['record']))
        metadata = parse(self.store.artifact(index['metadata']))
        require(e['capture_binding']['metadata'] == index['metadata'] and
                {k:v for k,v in e.items() if k != 'capture_binding'} == metadata,
                'CAPTURE_BINDING_MISSING', 'Independent capture binding mismatch')
        require(type(e.get('sequence')) is int and e['sequence'] > self.current().get('verification_floor', 0),
                'EXECUTION_SUPERSEDED', 'A failed verification requires genuinely refreshed executions')
        require(e['execution_id'] == execution_id and e['plan_id'] == p['plan_id'] and e['obligation_id'] == o['id'] and
                e['repository'] == self.repo_identity() and e['workflow'] == self.workflow,
                'IDENTITY_MISMATCH', 'Execution identity mismatch')
        require(e['command']['argv'] == o['argv'] and e['command']['cwd'] == o['cwd'] and
                e['command']['toolchain'] == o['toolchain'] and e['command']['runner'] == o['runner'],
                'COMMAND_MISMATCH', 'Captured command mismatch')
        require(e['command']['tools'] == self.tools(o), 'CONFIGURATION_CHANGED', 'Resolved tools changed')
        require(e['process']['environment'] == self.environment() and e['configurations'] == self.configurations(o['cwd']),
                'CONFIGURATION_CHANGED', 'Execution environment or configuration changed')
        require(e['source_before'] == p['source'] == e['source_after'] == self.source(), 'SOURCE_STALE', 'Tested source is stale')
        receipt = self.store.artifact(e['artifacts']['exit_receipt'], 4)
        self.store.inspect(e['artifacts']['raw_log'])
        require(re.fullmatch(rb'(0|[1-9][0-9]{0,2})\n?', receipt) and int(receipt) <= 255,
                'INVALID_EXIT_RECEIPT', 'Expected one ASCII numeric process exit')
        require(e['process']['termination'] == 'exited' and type(e['process']['exit_code']) is int and
                e['process']['exit_code'] == int(receipt) == 0, 'CHECK_FAILED', 'Execution did not normally exit zero')
        if o['toolchain'] == 'rust' and o['kind'] == 'test': self.rust_execution(o, e)
        if o['kind'] == 'consumer': self.scenario_execution(o, e)
        elif o['kind'] == 'test' and o['toolchain'] != 'rust': self.suite_execution(o, e)
        elif o['kind'] == 'precommit': self.hook_execution(o, e)
        return index['record']

    def documented_command(self, o):
        # Inspect complete command lines or explicitly labelled commands. A
        # prefix of a documented suite must not authorize a filtered substitute.
        for ref in o['selected_from']:
            contents = read_regular((self.root / ref['path']).resolve()).decode(errors='strict')
            candidates = contents.splitlines() + re.findall(r'`([^`\n]+)`', contents)
            candidates += re.findall(r'(?im)(?:tests?|quality|typecheck|consumer(?: basic| edge)?):[ \t]*([^\n;]+?)(?=\.[ \t]+[A-Za-z]|;|$)', contents)
            for candidate in candidates:
                for line in (candidate.strip(), candidate.strip().removesuffix('.')):
                    try: argv = shlex.split(line.removeprefix('$ '))
                    except ValueError: continue
                    if argv == o['argv']: return True
        return False

    def hook_command(self, o):
        skip = os.environ.get('SKIP', '')
        require(not any(os.environ.get(k) in ('0', '1', 'true') for k in ('HUSKY_SKIP_HOOKS',)) and
                os.environ.get('HUSKY') != '0' and not os.environ.get('PRE_COMMIT_ALLOW_NO_CONFIG') and
                all(v in ('xpia', 'xpia-defense') for v in skip.split(',') if v),
                'HOOK_UNPROVEN', 'Hooks disabled or skipped beyond the user XPIA exclusion')
        hook = Path(run(['git', 'rev-parse', '--path-format=absolute', '--git-path', 'hooks/pre-commit'], self.root).decode().strip())
        if hook.exists() and os.access(hook, os.X_OK):
            argv = o['argv']
            direct = len(argv) == 1 and '/' in argv[0] and (Path(o['cwd']) / argv[0]).resolve() == hook.resolve()
            require(direct or argv == ['git', 'hook', 'run', 'pre-commit'],
                    'COMMAND_MISMATCH', 'Must execute the actual configured Git pre-commit hook')
            return
        config = any((Path(o['cwd']) / n).is_file() for n in ('.pre-commit-config.yaml', '.pre-commit-config.yml'))
        require(config and o['argv'] == ['pre-commit', 'run', '--all-files'],
                'COMMAND_MISMATCH', 'Must execute configured pre-commit against all files')

    def matches_script(self, argv, name, script):
        try: direct = shlex.split(script)
        except (ValueError, TypeError): return False
        return (argv == direct or argv in (['npm', 'run', name], ['npm', name]) or
                argv in ([tool, name] for tool in ('pnpm', 'yarn', 'bun')))

    def configured_command(self, p, o):
        components = [c for c in p['components'] if c['id'] in o['component_ids']]
        require(all(Path(o['cwd']) == (self.root / c['root']).resolve() for c in components),
                'CWD_MISMATCH', 'Configured check must execute at its inspected component root')
        if o['kind'] == 'precommit':
            self.hook_command(o); return
        if o['kind'] == 'consumer':
            self.scenario_inputs(o); return
        if o['toolchain'] == 'rust' and o['kind'] == 'test':
            require(self.rust_command(o), 'COMMAND_MISMATCH', 'Full workspace Rust command required'); return
        for c in components:
            package = self.root / c['root'] / 'package.json'
            if package.exists() and o['toolchain'] in ('javascript', 'typescript'):
                scripts = parse(read_regular(package)).get('scripts', {})
                names = {'test': ('test',), 'typecheck': ('typecheck', 'type-check'),
                         'quality': ('quality', 'lint', 'format:check', 'check')}[o['kind']]
                require(any(name in scripts and self.matches_script(o['argv'], name, scripts[name]) for name in names), 'COMMAND_MISMATCH',
                        'Command does not execute the configured ' + o['kind'])
            else:
                require(self.documented_command(o), 'COMMAND_MISMATCH',
                        'Command not found in inspected project policy for ' + o['toolchain'])
        argv = o['argv']
        require(not any(a in ('-c', '-e', '--eval', '--listTests', '--list-tests', '--collect-only', '--no-tests',
                             '--passWithNoTests') for a in argv), 'COMMAND_MISMATCH', 'Check does not execute a configured suite')

    def scenario_inputs(self, o):
        scenario = o['scenario']
        entries = scenario.get('entry_points'); cases = scenario.get('cases'); assertions = scenario.get('assertion_refs')
        require(isinstance(entries, list) and entries and all(text(x) for x in entries) and
                isinstance(cases, list) and cases and all(text(x) for x in cases),
                'SCENARIO_UNPROVEN', 'Executable entry points and selected cases required')
        self.refs(assertions)
        require(all(entry in {r['path'] for r in assertions} for entry in entries),
                'SCENARIO_UNPROVEN', 'Every entry point needs inspected assertion references')
        argv = o['argv']
        require(not any(a in ('-e', '-c', '--eval', '--collect-only', '--list-tests', '--listTests', '--passWithNoTests') for a in argv),
                'SCENARIO_UNPROVEN', 'Consumer command does not select executable scenarios')
        for entry in entries:
            target = self.root / entry
            self.source_file(target)
            # Module/program dispatchers are mapped to their native entrypoint.
            selected = str(target.relative_to(Path(o['cwd']))) in argv
            if o['toolchain'] == 'go':
                selected = './' + str(Path(entry).parent) in argv and Path(entry).name == 'main.go'
                require(not re.search(rb'(?m)^//(?:go:build|[ ]+\+build)', read_regular(target.resolve())),
                        'SCENARIO_UNPROVEN', 'Go build-tag applicability requires separate proof')
            if o['toolchain'] == 'rust':
                manifest = tomllib.loads(read_regular(Path(o['cwd']) / 'Cargo.toml').decode())
                package = manifest.get('package', {})
                bins = manifest.get('bin', [])
                # Custom/multiple targets require an explicit inspected adapter.
                expected = bins[0].get('path') if len(bins) == 1 else 'src/main.rs' if not bins else None
                selected = ('run' in argv and bool(package) and expected is not None and
                            target == Path(o['cwd']) / expected and not any(a in ('--bin', '--example', '-p', '--package', '--manifest-path') for a in argv))
            if o['toolchain'] == 'dotnet':
                project = Path(o['cwd']) / argv[argv.index('--project')+1] if '--project' in argv and argv.index('--project')+1 < len(argv) else None
                selected = ('run' in argv and project is not None and target.parent == project.parent and Path(entry).name == 'Program.cs')
                if selected:
                    try: config = ET.fromstring(read_regular(project))
                    except ET.ParseError as exc: raise EvidenceError('SCENARIO_UNPROVEN', 'Invalid .NET project configuration') from exc
                    require(not any(element.tag.rsplit('}',1)[-1] in ('Compile', 'EnableDefaultCompileItems', 'EnableDefaultItems', 'DefaultItemExcludes',
                                  'DefaultExcludesInProjectFolder', 'Import', 'Target') for element in config.iter()),
                            'SCENARIO_UNPROVEN', 'Custom .NET compile inputs require an explicit inspected adapter')
            if o['toolchain'] == 'python':
                require(not os.environ.get('PYTHONOPTIMIZE') and not any(a in ('-O', '-OO') for a in argv),
                        'SCENARIO_UNPROVEN', 'Python assertion optimization disables consumer assertions')
            require(selected, 'SCENARIO_UNPROVEN', 'Captured argv does not select the inspected entry point')
            contents = read_regular(target.resolve()).decode()
            if o['toolchain'] == 'python':
                try: tree = ast.parse(contents)
                except SyntaxError as exc: raise EvidenceError('SCENARIO_UNPROVEN', 'Consumer source is not executable Python') from exc
                executable_assertions = any(isinstance(node, ast.Assert) or isinstance(node, ast.Call) and
                    isinstance(node.func, ast.Attribute) and node.func.attr.startswith('assert') for node in ast.walk(tree))
                case_source = '\n'.join(str(node.value) for node in ast.walk(tree) if isinstance(node, ast.Constant) and isinstance(node.value, str))
            else:
                tokens = re.compile(r"""("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|(/\*.*?\*/|//[^\n]*)""", re.DOTALL)
                case_source = tokens.sub(lambda m: m.group(1) or '', contents)
                code = tokens.sub('', contents)
                executable_assertions = re.search(r'\bassert(?:_eq|_ne)?\s*!|\bassert(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*\(|\bexpect\s*\(|\bpanic\s*\(|\bthrow\s+new|\bt\.(?:Fatal|Error)\s*\(', code)
            require(executable_assertions, 'SCENARIO_UNPROVEN', 'Entry point has no inspectable executable assertions')
            require(all(case in case_source and any(case in arg for arg in argv) for case in cases),
                    'SCENARIO_UNPROVEN', 'Selected cases do not bind source and command')
        return cases

    def suite_execution(self, o, e):
        patterns = (rb'(?:(?:# |\xe2\x84\xb9 )pass [1-9][0-9]{0,8}|[1-9][0-9]{0,8} passed|Ran [1-9][0-9]{0,8} tests?|(?m:^ok[ \t]+[^\n]{1,512})|Passed:[ \t]+[1-9][0-9]{0,8}|check consumer assertions passed)',
                    rb'(?:(?:# |\xe2\x84\xb9 )fail [1-9]|FAILED|no tests (?:ran|found)|No test files found|\[no tests to run\])')
        hits = self.store.inspect(e['artifacts']['raw_log'], patterns)
        require(hits[0] and not hits[1], 'CHECK_UNPROVEN', 'Configured suite has no completed passing test execution')

    def hook_execution(self, o, e):
        patterns = (rb'(?mi)^(?![^\n]{0,512}xpia)[^\n]{0,512}(?:Skipped|SKIPPED)',
                    rb'(?mi)(?:hooks disabled|skipping hooks|SKIP_HOOKS|HUSKY=0)')
        hits = self.store.inspect(e['artifacts']['raw_log'], patterns)
        require(not any(hits), 'HOOK_UNPROVEN', 'Configured hooks were skipped or disabled')

    def scenario_execution(self, o, e):
        cases = self.scenario_inputs(o)
        # Native harness counters prove a selected test ran; plain scenario
        # programs emit a case-specific assertion completion after assertions.
        patterns = [rb'(?:(?:# |\xe2\x84\xb9 )pass [1-9][0-9]{0,8}|[1-9][0-9]{0,8} passed|Ran [1-9][0-9]{0,8} tests?)',
                    rb'(?:(?:# |\xe2\x84\xb9 )fail [1-9]|test result: FAILED|FAILED|no tests (?:ran|found)|No test files found)',
                    rb'consumer assertions passed']
        patterns.extend(re.escape(case.encode()) for case in cases)
        hits = self.store.inspect(e['artifacts']['raw_log'], patterns)
        filtered = len(cases) == 1 and any(arg in ('-t', '--testNamePattern', '--test-name-pattern') or
                   arg.startswith(('--test-name-pattern=', '--testNamePattern=')) for arg in o['argv'])
        require(not hits[1] and (all(hits[3:]) or hits[0] and filtered) and (hits[0] or hits[2]),
                'SCENARIO_UNPROVEN', 'Selected assertions did not run, were skipped, or no completed case was observed')

    def coverage(self, context, plan_id=None):
        pre, local = context.get('precommit_results'), context.get('local_testing_gate')
        require(isinstance(pre, dict) and isinstance(local, dict), 'OBLIGATION_UNCOVERED', 'Typed Step 12 and 13 reports required')
        fields(pre, ('schema_version', 'plan_id', 'status', 'checks', 'exclusions'))
        fields(local, ('schema_version', 'plan_id', 'status', 'scenarios'))
        require(type(pre.get('schema_version')) is int and type(local.get('schema_version')) is int and
                pre.get('schema_version') == 1 and local.get('schema_version') == 1 and pre.get('status') == local.get('status') == 'PASS',
                'CHECK_FAILED', 'Step reports are incomplete or unsuccessful')
        plan_id = plan_id or pre.get('plan_id')
        require(pre.get('plan_id') == local.get('plan_id') == plan_id, 'COHORT_MISMATCH', 'Step reports reference different plans')
        p = self.load_plan(plan_id)
        self.plan_references(p)
        require(not p['unresolved'] and p['source'] == self.source(), 'SOURCE_STALE', 'Plan unresolved or stale')
        require(isinstance(pre.get('checks'), list) and isinstance(local.get('scenarios'), list), 'INVALID_REQUEST', 'Missing check/scenario mappings')
        mapped = {}; used = set(); roles = set(); records = []
        for item in pre['checks']:
            fields(item, ('obligation_id', 'execution_ids'))
            require(isinstance(item['execution_ids'], list) and item['execution_ids'], 'OBLIGATION_UNCOVERED', 'Empty executions')
            require(item['obligation_id'] not in mapped, 'OBLIGATION_UNCOVERED', 'Duplicate check mapping')
            mapped[item['obligation_id']] = (item['execution_ids'], None)
        for item in local['scenarios']:
            fields(item, ('obligation_id', 'execution_id', 'role', 'assertions'))
            require(item['obligation_id'] not in mapped, 'SCENARIO_UNPROVEN', 'Duplicate scenario mapping')
            mapped[item['obligation_id']] = ([item['execution_id']], item)
        require(set(mapped) == {o['id'] for o in p['obligations']}, 'OBLIGATION_UNCOVERED', 'Incomplete obligation coverage')
        require(set(pre.get('exclusions', [])) == {e['id'] for e in p['exclusions']}, 'OBLIGATION_UNCOVERED', 'Missing applicability decisions')
        for o in p['obligations']:
            executions, scenario = mapped[o['id']]
            latest = self.store.get('latest-executions/' + p['plan_id'] + '/' + digest(o['id'].encode()) + '.json')
            require(latest['execution_id'] in executions, 'EXECUTION_SUPERSEDED', 'Latest execution attempt must be represented')
            self.configured_command(p, o)
            if o['kind'] == 'consumer':
                require(scenario is not None and scenario['role'] == o['scenario']['role'] and
                        scenario['assertions'] == o['scenario']['assertions'] and scenario['assertions'],
                        'SCENARIO_UNPROVEN', 'Consumer role/assertions mismatch')
                roles.add(scenario['role'])
            else: require(scenario is None, 'OBLIGATION_UNCOVERED', 'Check reported as consumer')
            for execution_id in executions:
                require(execution_id not in used, 'SCENARIO_UNPROVEN', 'Execution reused across obligations')
                used.add(execution_id); records.append(self.validate_execution(execution_id, p, o))
        require(roles == {'basic', 'edge_or_integration'}, 'SCENARIO_UNPROVEN', 'Two distinct consumer roles required')
        return {'plan_id': plan_id, 'executions': records, 'source': self.source()}

    def terminal(self, completed, reason, noop=False):
        return {'verification_completed': 'true' if completed else 'false', 'terminal_no_op': 'true' if noop else 'false',
                'terminal_state': 'VERIFICATION_COMPLETED' if completed else 'ALLOW_NO_OP' if noop else 'VERIFICATION_UNPROVEN',
                'terminal_reason': reason}

    def terminal_record(self, record):
        if isinstance(record, str):
            try: return parse(record.encode())
            except EvidenceError: pass
        return record

    def implementation_records(self, context):
        records = {k:context.get(k) for k in SUPPORT[:3]}
        records['implementation_terminal_evidence'] = self.terminal_record(records['implementation_terminal_evidence'])
        return records

    def typed(self, record, kind):
        key = kind + '_completed'
        record = self.terminal_record(record)
        return (isinstance(record, dict) and set(record) == {key, 'terminal_no_op', 'terminal_state', 'terminal_reason'} and
                all(isinstance(v, str) for v in record.values()) and record[key] in ('true', 'false') and
                record['terminal_no_op'] in ('true', 'false') and text(record['terminal_reason']) and
                (record[key] != 'true' or record['terminal_no_op'] == 'false' and record['terminal_state'] == kind.upper() + '_COMPLETED'))

    def current(self):
        if not (self.store.root / 'current.json').exists(): return {}
        pointer = self.store.get('current.json')
        cohort = parse(self.store.artifact(pointer['cohort']))
        require(cohort['identity']['workflow'] == self.workflow and
                all(cohort['identity']['repository'][k] == self.repository[k] for k in ('root', 'git_dir')),
                'COHORT_MISMATCH', 'Wrong durable workflow/worktree cohort')
        return cohort

    def publish(self, updates):
        cohort = self.current()
        cohort.update(updates); generation_id = uuid.uuid4().hex
        cohort.update(identity=self.identity(), generation_id=generation_id)
        ref = self.store.put('generations/' + generation_id + '/cohort.json', cohort)
        self.store.put('current.json', {'generation_id': generation_id, 'cohort': ref})
        return generation_id, ref

    def observe_implementation(self):
        """Controller adapter: surround actual producer/verifier subprocesses.

        The controller must place this around step-08-implement and
        step-08c-work-verifier. Passive text import cannot create this record.
        """
        cwd = self.r['cwd']
        require(cwd == str(self.root), 'CWD_MISMATCH', 'Implementation must run in the canonical task worktree')
        self.publish({'implementation': {'records': {}, 'state': 'UNPROVEN'}, 'implementation_floor': self.sequence()})
        contract_path = HERE.parent / 'recipes/workflow-tdd.yaml'
        contract = {'path': str(contract_path), 'sha256': file_hash(contract_path)}
        terminal_inputs = {str(HERE / name): file_hash(HERE / name) for name in ('workflow_implementation_evidence.sh', 'workflow_context.sh')}
        origin_id = uuid.uuid4().hex; prefix = 'origins/' + origin_id + '/'
        captures = []
        for step, key in (('step-08-implement', 'producer_argv'), ('step-08c-work-verifier', 'verifier_argv')):
            argv = self.r[key]
            require(isinstance(argv, list) and argv and all(text(a) for a in argv), 'INVALID_REQUEST', 'Direct producer/verifier argv required')
            before = self.source(); tool = self.executable(argv[0], cwd)
            inputs = {arg:self.source_file(Path(cwd)/arg) for arg in argv[1:] if (Path(cwd)/arg).is_file()}
            environment = self.environment(); configs = self.configurations(cwd)
            observation = dict(self.identity(), step=step, argv=argv, cwd=cwd, source_before=before,
                               executable=tool, inputs=inputs, environment=environment, configurations=configs,
                               observer_pid=os.getpid(), observer_parent=os.getppid(), state='incomplete')
            base = prefix + step + '/'
            self.store.put(base + 'attempt.json', observation)
            outputs = []
            for name in ('stdout', 'stderr'):
                self.store.put(base + name + '.log', b'', raw=True)
                outputs.append(os.fdopen(os.open(self.store.path(base + name + '.log'), os.O_WRONLY | os.O_TRUNC | os.O_NOFOLLOW), 'wb'))
            try:
                child = subprocess.Popen(argv, cwd=cwd, stdout=outputs[0], stderr=outputs[1])
                status = child.wait()
                for output in outputs: output.flush(); os.fsync(output.fileno())
            finally:
                for output in outputs: output.close()
            after = self.source()
            refs = {name: dict(inspect_regular(self.store.path(base + name + '.log'))[0], path=base+name+'.log')
                    for name in ('stdout', 'stderr')}
            observation.update(state='sealed', source_after=after, process_exit_code=status, child_pid=child.pid,
                               outputs=refs, exit_receipt=self.store.put(base+'exit.txt', (str(status)+'\n').encode(), raw=True))
            capture = self.store.put(base + 'record.json', observation); captures.append(capture)
            require(status == 0, 'IMPLEMENTATION_UNPROVEN', 'Actual producer/verifier execution failed; raw evidence retained')
            require(inputs == {arg:self.source_file(Path(cwd)/arg) for arg in argv[1:] if (Path(cwd)/arg).is_file()} and
                    tool == self.executable(argv[0], cwd) and environment == self.environment() and configs == self.configurations(cwd),
                    'CONFIGURATION_CHANGED', 'Producer inputs changed during execution')
            if key == 'verifier_argv':
                require(before == after, 'SOURCE_STALE', 'Verifier changed the source it inspected')
        producer, verifier = [parse(self.store.artifact(ref)) for ref in captures]
        require(producer['source_after'] == verifier['source_before'], 'SOURCE_STALE', 'Producer/verifier sources differ')
        implementation = self.store.artifact(producer['outputs']['stdout']).decode()
        verdict = self.store.artifact(verifier['outputs']['stdout']).decode()
        require(text(implementation) and text(verdict), 'IMPLEMENTATION_UNPROVEN', 'Producer and verifier must retain nonempty original output')
        original = dict(self.context, implementation=implementation, verdict_json=verdict)
        producer_context = dict(original, verdict_json=parse(verdict.strip().splitlines()[-1].encode()))
        context_ref = self.store.put(prefix + 'producer-context.json', producer_context)
        env = dict(os.environ, AMPLIHACK_CONTEXT_FILE=str(self.store.path(context_ref['path'])))
        result = subprocess.run(['bash', str(HERE / 'workflow_implementation_evidence.sh')], cwd=cwd, env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        require(result.returncode == 0, 'IMPLEMENTATION_UNPROVEN', 'Stock terminal producer failed')
        self.store.put(prefix + 'terminal-producer-stdout.json', result.stdout, raw=True)
        self.store.put(prefix + 'terminal-producer-stderr.log', result.stderr, raw=True)
        original['implementation_terminal_evidence'] = parse(result.stdout)
        require(contract['sha256'] == file_hash(contract_path), 'CONFIGURATION_CHANGED', 'Producer recipe contract changed during execution')
        origin = dict(self.identity(), sequence=self.sequence(), captures=captures, source=verifier['source_after'], recipe_contract=contract, terminal_inputs=terminal_inputs,
                      records={k:original[k] for k in SUPPORT[:3]})
        origin_ref = self.store.put(prefix + 'origin.json', origin)
        self.store.put('implementation-origin.json', {'origin': origin_ref})
        self.context = original
        return self.checkpoint()

    def implementation_origin(self, context):
        require((self.store.root / 'implementation-origin.json').exists(), 'IMPLEMENTATION_UNPROVEN',
                'Origin-time execution/source bindings missing. Controller must wrap workflow-tdd step-08-implement and '
                'step-08c-work-verifier with observe-implementation; those producing points are outside the seven-entry scope.')
        origin = parse(self.store.artifact(self.store.get('implementation-origin.json')['origin']))
        require(type(origin.get('sequence')) is int and origin['sequence'] > self.current().get('implementation_floor', 0),
                'IMPLEMENTATION_SUPERSEDED', 'An unsuccessful implementation requires genuinely refreshed producer/verifier execution')
        require(origin['workflow'] == self.workflow and origin['repository'] == self.repo_identity() and
                origin['source'] == self.source() and self.implementation_records(origin['records']) == self.implementation_records(context),
                'SOURCE_STALE', 'Original producer/verifier bindings do not match this source and output')
        require(origin['recipe_contract']['sha256'] == file_hash(origin['recipe_contract']['path']) and
                all(file_hash(path) == sha for path, sha in origin['terminal_inputs'].items()),
                'CONFIGURATION_CHANGED', 'Producer recipe contract or terminal producer inputs changed')
        observations = []
        for ref in origin['captures']:
            e = parse(self.store.artifact(ref)); observations.append(e)
            require(e['workflow'] == self.workflow and e['repository'] == self.repository and e['state'] == 'sealed' and
                    e['process_exit_code'] == 0 and e['cwd'] == str(self.root), 'IMPLEMENTATION_UNPROVEN', 'Producer execution mismatch')
            require(e['inputs'] == {arg:self.source_file(Path(e['cwd'])/arg) for arg in e['argv'][1:] if (Path(e['cwd'])/arg).is_file()} and
                    e['executable'] == self.executable(e['argv'][0], e['cwd']) and
                    e['configurations'] == self.configurations(e['cwd']) and e['environment'] == self.environment(),
                    'CONFIGURATION_CHANGED', 'Original producer/verifier inputs changed')
            require(self.store.artifact(e['exit_receipt'], 4) == b'0\n', 'IMPLEMENTATION_UNPROVEN', 'Producer failed')
            for output in e['outputs'].values(): self.store.inspect(output)
        require(len(observations) == 2 and observations[0]['step'] == 'step-08-implement' and
                observations[1]['step'] == 'step-08c-work-verifier' and
                observations[0]['source_after'] == observations[1]['source_before'] == observations[1]['source_after'] == origin['source'],
                'IMPLEMENTATION_UNPROVEN', 'Independent producing-point observations missing')

    def implementation(self, context):
        record = self.terminal_record(context.get('implementation_terminal_evidence'))
        if record is None and 'implementation_terminal_evidence' not in context: return 'MISSING'
        if not self.typed(record, 'implementation'): return 'INVALID'
        if record['terminal_no_op'] == 'true' and record['implementation_completed'] == 'false':
            explicit = context.get('allow_no_op') is True
            sentinel = 'No files modified — orchestration task' in str(context.get('implementation', ''))
            if self.current().get('implementation_floor'):
                try: self.implementation_origin(context)
                except (EvidenceError, OSError): return 'INVALID'
            return 'NO_OP' if record['terminal_state'] == 'ALLOW_NO_OP' and (explicit or sentinel) else 'INVALID'
        if record['implementation_completed'] != 'true': return 'INVALID'
        impl, verifier = context.get('implementation'), context.get('verdict_json')
        if not text(impl) or not (isinstance(verifier, dict) or text(verifier)): return 'INVALID'
        try:
            verdict = verifier if isinstance(verifier, dict) else parse(verifier.strip().splitlines()[-1].encode())
            require(isinstance(verdict, dict) and verdict.get('verdict') == 'WORK_VERIFIED' and
                    isinstance(verdict.get('evidence'), list) and verdict['evidence'] and all(text(v) for v in verdict['evidence']) and text(verdict.get('rationale')),
                    'IMPLEMENTATION_INVALID', 'Stock verifier verdict/evidence/rationale required on last nonempty line')
            self.implementation_origin(context)
            return 'VALID'
        except (EvidenceError, OSError) as exc:
            self.implementation_error = str(exc)
            return 'INVALID'

    def checkpoint(self):
        original = self.current().get('implementation', {})
        candidate = copy.deepcopy(self.context)
        restored_keys = []
        for key, value in original.get('records', {}).items():
            if key not in candidate:
                candidate[key] = copy.deepcopy(value); restored_keys.append(key)
        snapshot = {k: candidate[k] for k in SUPPORT[:3] if k in candidate}
        generation, cohort = self.publish({'implementation': {'records': snapshot, 'state': 'UNPROVEN'}})
        try:
            source = self.source()
            state = self.implementation(candidate)
        except (EvidenceError, OSError) as exc:
            self.publish({'implementation': {'records': snapshot, 'state': 'UNPROVEN', 'error': str(exc)},
                          'implementation_floor': self.sequence()})
            raise
        if original and original.get('source') != source and (restored_keys or snapshot == original.get('records')):
            state = 'STALE'
        observed = {'records': snapshot, 'source': source, 'state': state,
                    'origin': self.store.get('implementation-origin.json') if (self.store.root / 'implementation-origin.json').exists() else None}
        # Retain every checkpoint, including negatives, but never destroy the
        # original cohort merely because a continuation omitted its transport.
        checkpoint_ref = self.store.put('checkpoints/' + uuid.uuid4().hex + '.json', observed)
        updates = {'implementation': observed}
        if state not in ('VALID', 'NO_OP'): updates['implementation_floor'] = self.sequence()
        generation, cohort = self.publish(updates)
        validation = self.store.put('validations/' + uuid.uuid4().hex + '.json',
                                    {'implementation_state': state, 'checkpoint': checkpoint_ref})
        return {'schema_version': 1, 'ok': state in ('VALID', 'NO_OP'), 'implementation_state': state,
                'generation_id': generation, 'cohort': cohort, 'validation': validation,
                'errors': [] if state in ('VALID', 'NO_OP') else ['Original typed implementation or verifier support missing/invalid/stale']}, int(state not in ('VALID', 'NO_OP'))

    def verify(self):
        self.publish({'verification': {'records': {k:self.context[k] for k in SUPPORT[3:] if k in self.context}, 'state': 'UNPROVEN'}})
        try:
            details = self.coverage(self.context, self.r.get('plan_id'))
            ref = self.store.put('validations/verification-' + digest(encode(details)) + '.json', details)
            terminal = self.terminal(True, 'Validated obligations and captured executions: ' + ref['path'])
            self.publish({'verification': {'records': {**{k: self.context[k] for k in SUPPORT[4:]},
                           'verification_terminal_evidence': terminal}, 'details': details, 'validation': ref}})
            return terminal, 0
        except (EvidenceError, OSError, KeyError, TypeError) as exc:
            details = {'verification_completed': False, 'error': str(exc)}
            ref = self.store.put('validations/' + uuid.uuid4().hex + '.json', details)
            # No-op cannot hide any explicit failed report or changed source.
            noop = self.context.get('allow_no_op') is True and not self.current().get('verification_floor') and not any(self.context.get(k) not in (None, '') for k in SUPPORT[4:]) and not run(['git', 'status', '--porcelain'], self.root).strip()
            terminal = self.terminal(False, 'Verification unproven: ' + str(exc) + '; ' + ref['path'], noop)
            self.publish({'verification': {'records': {**{k:self.context[k] for k in SUPPORT[4:] if k in self.context},
                         'verification_terminal_evidence': terminal}, 'source': self.safe_source(),
                         'state': 'NO_OP' if noop else 'UNPROVEN', 'validation': ref},
                         **({} if noop else {'verification_floor': self.sequence()})})
            return terminal, 0

    def restored(self):
        context = copy.deepcopy(self.context); context.setdefault('repo_path', str(self.root)); cohort = self.current(); errors = []
        impl = cohort.get('implementation', {}); verify = cohort.get('verification', {})
        for group in (impl, verify):
            for key, value in group.get('records', {}).items():
                # Only declared empty recipe defaults count as absent. Null,
                # objects, negatives and malformed strings retain authority.
                if key not in context: context[key] = copy.deepcopy(value)
        implementation_valid = (cohort.get('identity') == self.identity() and impl.get('state') == 'VALID' and impl.get('source') == self.source() and
                                all(context.get(k) == v for k,v in impl.get('records', {}).items()) and
                                self.implementation(context) == 'VALID')
        verification_valid = False
        try:
            record = self.terminal_record(context.get('verification_terminal_evidence'))
            require(self.typed(record, 'verification') and record['verification_completed'] == 'true' and verify,
                    'AUTHORITATIVE_RECORD_INVALID', 'Original typed verification missing/invalid')
            require(all(context.get(k) == v for k,v in verify['records'].items()),
                    'AUTHORITATIVE_RECORD_INVALID', 'Selected verification differs from durable original')
            require(self.coverage(context) == verify['details'], 'COHORT_MISMATCH', 'Verification cohort changed')
            self.store.artifact(verify['validation']); verification_valid = True
        except (EvidenceError, OSError, KeyError, TypeError) as exc: errors.append(str(exc))
        no_op_valid = (cohort.get('identity') == self.identity() and impl.get('state') == 'NO_OP' and
                       impl.get('source') == self.source() and self.implementation(context) == 'NO_OP' and
                       verify.get('state') == 'NO_OP' and verify.get('source') == self.source() and
                       all(context.get(k) == v for group in (impl, verify) for k,v in group.get('records', {}).items()) and
                       not any(context.get(k) not in (None, '') for k in SUPPORT[4:]) and
                       not run(['git', 'status', '--porcelain'], self.root).strip())
        if not implementation_valid and not no_op_valid:
            context['implementation_terminal_evidence'] = {'implementation_completed': 'false', 'terminal_no_op': 'false',
                'terminal_state': 'IMPLEMENTATION_UNPROVEN', 'terminal_reason': 'Original implementation provenance missing, invalid or stale'}
            errors.append('Original implementation provenance missing, invalid or stale: ' + getattr(self, 'implementation_error', 'original checkpoint does not match'))
        if not verification_valid and not no_op_valid: context['verification_terminal_evidence'] = self.terminal(False, '; '.join(errors))
        details = {'schema_version': 1, 'implementation_valid': implementation_valid,
                   'verification_valid': verification_valid, 'no_op_valid': no_op_valid, 'errors': errors, 'source': self.source()}
        validation = self.store.put('validations/' + uuid.uuid4().hex + '.json', details)
        derived = self.store.put('contexts/' + uuid.uuid4().hex + '.json', context)
        response = dict(details, ok=(implementation_valid and verification_valid) or no_op_valid, generation_id=cohort.get('generation_id'),
                        derived_context=derived, validation=validation,
                        implementation_refresh_required='false' if implementation_valid else 'true',
                        verification_refresh_required='false' if verification_valid else 'true')
        return context, response

    def consumer(self, action, context):
        ref = self.store.put('contexts/' + uuid.uuid4().hex + '.json', context)
        env = os.environ.copy(); env['AMPLIHACK_CONTEXT_FILE'] = str(self.store.path(ref['path']))
        selected_tools = Path(os.environ['AMPLIHACK_HOME']) / 'amplifier-bundle/tools' if os.environ.get('AMPLIHACK_HOME') else HERE
        if action == 'final-status':
            for key in ('implementation_terminal_evidence', 'verification_terminal_evidence', 'publish_terminal_evidence',
                        'terminal_state', 'pr_publish_result', 'pr_url', 'pr_number', 'issue_number', 'allow_no_op'):
                value = context.get(key, '')
                serialized = value if isinstance(value, str) else encode(value).decode()
                require(len(serialized.encode()) < 64000, 'INVALID_CONTEXT', 'Final-status field exceeds its legacy transport: ' + key)
                env['RECIPE_VAR_' + key] = serialized
            env['REMOTE_HOST_TYPE'] = str(context.get('remote_host_type', 'other'))
            env['TASK_DESCRIPTION'] = str(context.get('task_description', ''))[:16000]
            argv = ['bash', str(selected_tools / 'workflow_final_status.sh')]
        else: argv = ['bash', str(selected_tools / 'workflow_agentic_finalization.sh'), action]
        process = subprocess.run(argv, cwd=self.root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        sys.stderr.buffer.write(process.stderr)
        return process

    def collected_identity(self, evidence):
        require(isinstance(evidence, dict) and isinstance(evidence.get('git'), dict),
                'IDENTITY_MISMATCH', 'Collected repository identity missing')
        observed = evidence['git']
        for key in ('repo_path', 'worktree_path'):
            require(text(observed.get(key)) and Path(observed[key]).is_absolute() and
                    Path(observed[key]).resolve() == self.root,
                    'IDENTITY_MISMATCH', 'Collected repository does not match invoking worktree')
        head = observed.get('head_sha')
        require(text(head) and re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', head) and head == self.source()['head'],
                'IDENTITY_MISMATCH', 'Collected full HEAD does not match verified source')

    def ci_failed(self, context):
        evidence = context.get('finalization_evidence')
        if not isinstance(evidence, dict): return False
        ci = evidence.get('ci')
        if not isinstance(ci, dict): return False
        return any(str(ci.get(key, '')).upper() in ('FAILURE', 'FAILED', 'CANCELLED', 'CANCELED', 'ERROR')
                   for key in ('state', 'status', 'conclusion'))

    def postconditions(self, value, context, local):
        require(isinstance(value, dict), 'INVALID_CONTEXT', 'Classifier returned no result')
        if value.get('terminal_success') == 'true':
            if 'finalization_evidence' in context: self.collected_identity(context['finalization_evidence'])
            if not (local['implementation_valid'] and local['verification_valid'] or
                    local.get('no_op_valid') and value.get('terminal_state') == 'ALLOW_NO_OP'):
                value.update(terminal_state='FAILED_IMPLEMENTATION', terminal_success='false', terminal_failure='true',
                             finalizer_output_valid='false', terminal_reason='Original implementation and applicable captured verification required',
                             required_next_action='Refresh genuine producer/verifier execution and workflow-precommit-test evidence.')
            elif self.ci_failed(context):
                value.update(terminal_state='BLOCKED_CI', terminal_success='false', terminal_failure='true',
                             finalizer_output_valid='false', terminal_reason='Explicit unsuccessful CI independently prevents success',
                             required_next_action='Resolve failed/cancelled/error CI and collect fresh evidence.')
        # Existing harder unsuccessful classifications are never softened.
        value['local_evidence'] = {k:v for k,v in local.items() if k != 'derived_context'}
        return value, 0 if value.get('terminal_success') == 'true' else 1

    def guarded_validation(self, context, local):
        if 'finalization_evidence' in context: self.collected_identity(context['finalization_evidence'])
        process = self.consumer('validate', context)
        _, local = self.restored()
        value = parse(process.stdout)
        require(isinstance(value, dict), 'INVALID_CONTEXT', 'Classifier returned no result')
        if process.returncode != 0 and value.get('terminal_success') == 'true':
            value.update(terminal_state='FAILED_INVALID_EVIDENCE', terminal_success='false', terminal_failure='true',
                         finalizer_output_valid='false', terminal_reason='Classifier process failed despite a supplied success record',
                         required_next_action='Inspect the original classifier process exit and output.')
        return self.postconditions(value, context, local)

    def finalize(self, action):
        context, local = self.restored()
        if action == 'final-status':
            process = self.consumer(action, context)
            _, fresh = self.restored()
            output = process.stdout.decode(errors='strict')
            original = dict(line.split('=', 1) for line in output.splitlines() if '=' in line)
            guarded, status = self.postconditions(dict(original), context, fresh)
            lines = []
            for line in output.splitlines():
                key = line.split('=', 1)[0]
                if key in original and key in guarded: line = key + '=' + str(guarded[key])
                if status and line == 'All 23 workflow steps completed successfully.': continue
                lines.append(line)
            print('\n'.join(lines))
            return None, process.returncode or status
        if action == 'collect':
            for attempt in range(2):
                before = self.source(); process = self.consumer('collect', context)
                require(process.returncode == 0, 'INVALID_CONTEXT', 'Finalization collection failed')
                value = parse(process.stdout)
                context, local = self.restored()
                if before == self.source(): break
            self.collected_identity(value)
            value['completion']['implementation_completed'] = 'true' if local['implementation_valid'] else 'false'
            value['completion']['verification_completed'] = 'true' if local['verification_valid'] else 'false'
            value['local_evidence'] = {k:v for k,v in local.items() if k != 'derived_context'}
            if 'terminal_success' in value:
                return self.postconditions(value, dict(context, finalization_evidence=value), local)
            return value, 0
        if action == 'validate': return self.guarded_validation(context, local)
        if action == 'complete':
            result, status = self.guarded_validation(context, local)
            prior = context.get('workflow_result')
            if isinstance(prior, dict) and prior.get('terminal_success') != 'true': result, status = prior, 1
            context['workflow_result'] = result
            process = self.consumer('complete', context)
            require(process.returncode == 0, 'INVALID_CONTEXT', 'Completion reporting failed')
            _, fresh = self.restored()
            value, final_status = self.postconditions(parse(process.stdout), context, fresh)
            return value, status or final_status
        raise EvidenceError('INVALID_REQUEST', 'Unknown finalization action')


def main():
    args = sys.argv[3:]; request_file = None; action = None
    while args:
        key = args.pop(0)
        require(key in ('--request-file', '--action') and args, 'INVALID_REQUEST', 'Expected --request-file or --action')
        value = args.pop(0)
        if key == '--request-file':
            require(request_file is None, 'INVALID_REQUEST', 'One request source required'); request_file = value
        else:
            require(action is None, 'INVALID_REQUEST', 'One finalization action required'); action = value
    request = parse(sys.stdin.buffer.read(LIMIT + 1) if request_file == '-' else read_regular(request_file, LIMIT)) if request_file else {'schema_version': 1}
    require(isinstance(request, dict) and type(request.get('schema_version')) is int, 'INVALID_REQUEST', 'Integer schema_version required')
    require(request['schema_version'] == 1, 'UNSUPPORTED_SCHEMA', 'Only schema version 1 is supported')
    if 'extensions' in request: require(isinstance(request['extensions'], dict), 'INVALID_REQUEST', 'extensions must be an object')
    base = ('schema_version', 'workflow', 'repository_root')
    extra = {'observe-implementation': ('producer_argv', 'verifier_argv', 'cwd'), 'plan': ('components', 'obligations', 'exclusions'), 'capture': ('plan_id', 'obligation_id', 'producer_step', 'argv', 'cwd')}
    optional = ('context_file', 'extensions') + (('plan_id',) if MODE == 'verify' else ())
    if request_file: fields(request, base + extra.get(MODE, ()), optional)
    else: require(MODE not in ('plan', 'capture', 'observe-implementation'), 'INVALID_REQUEST', 'This operation needs a request file')
    workflow = Workflow(request)
    if MODE == 'observe-implementation': result, status = workflow.observe_implementation()
    elif MODE == 'plan': result, status = workflow.plan()
    elif MODE == 'capture': result, status = workflow.capture()
    elif MODE == 'checkpoint-implementation': result, status = workflow.checkpoint()
    elif MODE == 'verify': result, status = workflow.verify()
    elif MODE == 'restore':
        _, result = workflow.restored(); status = int(not result['ok'])
    else: result, status = workflow.finalize(action)
    if result is not None: print(json.dumps(result, separators=(',', ':')))
    return status

try:
    sys.exit(main())
except (EvidenceError, OSError, KeyError, TypeError, ValueError, tomllib.TOMLDecodeError) as exc:
    code = exc.code if isinstance(exc, EvidenceError) else 'INVALID_CONTEXT' if isinstance(exc, (KeyError, TypeError, ValueError)) else 'ARTIFACT_UNREADABLE'
    print(json.dumps({'schema_version': 1, 'ok': False, 'error': {'code': code, 'message': str(exc),
                      'details': {}, 'required_next_action': 'Inspect the selected request and retained evidence; rerun its genuine producing stage.'}}))
    sys.exit(2)
PY_LOCAL_EVIDENCE
    ;;
esac

if ! command -v jq >/dev/null 2>&1; then
  echo '{"ok":false,"reason":"missing_jq"}'
  echo "ERROR: jq is required by workflow_terminal_evidence.sh" >&2
  exit 2
fi

MODE="${1:-}"
[ "$#" -gt 0 ] && shift

REPO=""
HEAD_REF=""
BASE_REF=""
HEAD_SHA=""
PR_URL=""
PR_NUMBER=""
CREATED_AFTER=""
EVIDENCE=""
EVIDENCE_FILE=""
AGENT_VERDICT=""
SCOPE_REASON=""
REPORT_ONLY="false"
READ_STDIN="false"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --head) HEAD_REF="${2:-}"; shift 2 ;;
    --base) BASE_REF="${2:-}"; shift 2 ;;
    --head-sha) HEAD_SHA="${2:-}"; shift 2 ;;
    --pr-url) PR_URL="${2:-}"; shift 2 ;;
    --pr-number) PR_NUMBER="${2:-}"; shift 2 ;;
    --created-after) CREATED_AFTER="${2:-}"; shift 2 ;;
    --evidence) EVIDENCE="${2:-}"; shift 2 ;;
    --evidence-file) EVIDENCE_FILE="${2:-}"; shift 2 ;;
    --agent-verdict) AGENT_VERDICT="${2:-}"; shift 2 ;;
    --scope-reason) SCOPE_REASON="${2:-}"; shift 2 ;;
    --report-only) REPORT_ONLY="true"; shift ;;
    -) READ_STDIN="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unrecognised argument: $1" >&2; usage; exit 2 ;;
  esac
done

# --- shared gh plumbing ----------------------------------------------------
# Evidence reads are best-effort by design: an unreadable GitHub is UNCERTAIN,
# never a fabricated verdict in either direction. A rate limit must not stall
# the final reporting step for a reset window that can be an hour out, so the
# rate-limit wait is pinned off exactly as the display read in
# workflow_final_status.sh does.
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
if [ -f "$GH_RETRY_HELPER" ]; then
  # shellcheck source=/dev/null
  . "$GH_RETRY_HELPER"
fi
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-${SCRIPT_DIR}/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow_terminal_evidence.sh requires the shared GitHub repo identity parser at $GITHUB_REPO_HELPER" >&2
  exit 2
fi
# Defines parse_github_repo_identity (issue #1642).
# shellcheck source=workflow_github_repo.sh disable=SC1090
. "$GITHUB_REPO_HELPER"

gh_read() {
  local label="$1"; shift
  if declare -F _gh_retry_core >/dev/null 2>&1; then
    GH_RETRY_MAX_RL_WINDOWS=0 _gh_retry_core "$label" "$@"
  else
    gh "$@"
  fi
}

resolve_base_ref() {
  local candidate
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
    printf '%s\n' "${candidate#origin/}"; return 0
  fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
      printf '%s\n' "${candidate#origin/}"; return 0
    fi
  done
  return 1
}

PR_FIELDS="number,state,title,url,headRefName,baseRefName,createdAt,isCrossRepository"

collect() {
  local prs_for_head="[]" explicit_pr="null" open_all="[]"
  local evidence_readable="true" unreadable_reason=""
  local work_in_base="unknown" raw rc

  if [ -z "$REPO" ]; then
    REPO="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" 2>/dev/null || true)"
  fi
  [ -n "$HEAD_REF" ] || HEAD_REF="$(git branch --show-current 2>/dev/null || true)"
  [ -n "$HEAD_SHA" ] || HEAD_SHA="$(git rev-parse --verify HEAD 2>/dev/null || true)"
  [ -n "$BASE_REF" ] || BASE_REF="$(resolve_base_ref 2>/dev/null || true)"

  # --- git-side evidence: did the work already land in the base branch? -----
  # Ancestry catches a merge commit / fast-forward; an empty diff catches a
  # squash-merge, where the base contains the CONTENT but none of the SHAs.
  # Neither is string matching, and both are facts about the repository.
  if [ -n "$BASE_REF" ] && command -v git >/dev/null 2>&1; then
    local base_commit=""
    for candidate in "refs/remotes/origin/${BASE_REF}" "$BASE_REF" "origin/${BASE_REF}"; do
      if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
        base_commit="$candidate"; break
      fi
    done
    if [ -n "$base_commit" ] && [ -n "$HEAD_SHA" ]; then
      if git merge-base --is-ancestor "$HEAD_SHA" "$base_commit" >/dev/null 2>&1; then
        work_in_base="true"
      elif git diff --quiet "${base_commit}..${HEAD_SHA}" >/dev/null 2>&1; then
        work_in_base="true"
      else
        work_in_base="false"
      fi
    fi
  fi

  # --- GitHub-side evidence -------------------------------------------------
  if ! command -v gh >/dev/null 2>&1; then
    evidence_readable="false"; unreadable_reason="gh CLI is not available"
  elif [ -z "$REPO" ]; then
    evidence_readable="false"; unreadable_reason="could not resolve an owner/repo identity"
  elif [ -z "$HEAD_REF" ]; then
    evidence_readable="false"; unreadable_reason="could not resolve the head branch"
  else
    # Every PR ever opened FROM this head branch, in any state. `--state all` is
    # the whole point: the incident's PR was MERGED, so an open-only query could
    # never see it.
    raw="$(gh_read "terminal evidence pr list" pr list --repo "$REPO" --head "$HEAD_REF" --state all --json "$PR_FIELDS" 2>/dev/null)"
    rc="$?"
    if [ "$rc" -ne 0 ]; then
      evidence_readable="false"; unreadable_reason="unable to list pull requests for head branch '$HEAD_REF'"
    else
      [ -n "${raw//[[:space:]]/}" ] || raw="[]"
      if printf '%s' "$raw" | jq -e 'type == "array"' >/dev/null 2>&1; then
        prs_for_head="$(printf '%s' "$raw" | jq -c '.')"
      else
        evidence_readable="false"; unreadable_reason="pull-request metadata was not a JSON array"
      fi
    fi

    # The explicitly published PR, when the run recorded one. It may live on a
    # different head ref than the current branch (a follow-up rebase, a renamed
    # branch), which is precisely why the list above is not sufficient.
    local explicit_target=""
    if [ -n "$PR_URL" ]; then explicit_target="$PR_URL"
    elif [ -n "$PR_NUMBER" ]; then explicit_target="$PR_NUMBER"
    fi
    if [ -n "$explicit_target" ]; then
      if raw="$(gh_read "terminal evidence pr view" pr view "$explicit_target" --repo "$REPO" --json "$PR_FIELDS" 2>/dev/null)" \
        && printf '%s' "$raw" | jq -e 'type == "object"' >/dev/null 2>&1; then
        explicit_pr="$(printf '%s' "$raw" | jq -c '.')"
      fi
    fi

    # Outstanding open PRs. Acceptance criterion: these are enumerated in the
    # final status WHATEVER the verdict, because the incident's real damage was
    # two live PRs nobody was driving.
    if raw="$(gh_read "terminal evidence open pr list" pr list --repo "$REPO" --state open --limit 100 --json "$PR_FIELDS" 2>/dev/null)" \
      && printf '%s' "$raw" | jq -e 'type == "array"' >/dev/null 2>&1; then
      open_all="$(printf '%s' "$raw" | jq -c '.')"
    fi
  fi

  jq -nc \
    --arg schema_version "1" \
    --arg repo "$REPO" \
    --arg head_ref "$HEAD_REF" \
    --arg base_ref "$BASE_REF" \
    --arg head_sha "$HEAD_SHA" \
    --arg pr_url "$PR_URL" \
    --arg pr_number "$PR_NUMBER" \
    --arg created_after "$CREATED_AFTER" \
    --arg evidence_readable "$evidence_readable" \
    --arg unreadable_reason "$unreadable_reason" \
    --arg work_in_base "$work_in_base" \
    --arg scope_reason "$SCOPE_REASON" \
    --argjson prs_for_head "$prs_for_head" \
    --argjson explicit_pr "$explicit_pr" \
    --argjson open_all "$open_all" '
    def merged: [ .[] | select(((.state // "") | ascii_upcase) == "MERGED") ];
    def open_only: [ .[] | select(((.state // "") | ascii_upcase) == "OPEN") ];
    def closed_unmerged: [ .[] | select(((.state // "") | ascii_upcase) == "CLOSED") ];
    def brief: [ .[] | {number, state, title, url, headRefName, baseRefName, createdAt} ];

    ($prs_for_head + (if $explicit_pr == null then [] else [$explicit_pr] end))
      | unique_by(.number) as $all
    | ($all | merged) as $merged
    | ($all | open_only) as $open
    | ($all | closed_unmerged) as $closed
    # Outstanding = every open PR from this head branch, plus (when the run
    # start time is known) every PR opened during the run. Broad on purpose:
    # this is a REPORT, and under-reporting a live PR is the failure that
    # orphaned #1132/#1133.
    | ( ($open)
        + ( if $created_after == "" then []
            else [ $open_all[] | select((.createdAt // "") >= $created_after) ]
            end )
      ) | unique_by(.number) as $outstanding
    | ($merged | length) as $merged_count
    | ($open | length) as $open_count
    | ($closed | length) as $closed_count
    | (($merged_count > 0) or ($open_count > 0) or ($work_in_base == "true")) as $positive
    | (if $evidence_readable != "true" then
         {v: "UNCERTAIN", r: ("terminal evidence could not be read: " + $unreadable_reason)}
       elif $merged_count > 0 then
         {v: "SUCCESS", r: ("a pull request from head branch \"" + $head_ref + "\" is MERGED: #" + (($merged[0].number // 0) | tostring))}
       elif $work_in_base == "true" then
         {v: "SUCCESS", r: ("the head commit is already contained in base branch \"" + $base_ref + "\"; the work landed")}
       elif $open_count > 0 then
         {v: "SUCCESS", r: ("an OPEN pull request carries this work: #" + (($open[0].number // 0) | tostring))}
       elif ($closed_count > 0) and ($work_in_base == "false") then
         {v: "FAILED", r: "every pull request for this head branch is CLOSED and unmerged, and the work is not in the base branch"}
       elif ($closed_count == 0) and ($open_count == 0) and ($merged_count == 0) and ($work_in_base == "false") then
         {v: "FAILED", r: ("no pull request exists for head branch \"" + $head_ref + "\" and its commits are not in the base branch; the work was never published")}
       else
         {v: "UNCERTAIN", r: "no pull request matched and the base-branch state of the work could not be determined"}
       end) as $mech
    | {
        schema_version: ($schema_version | tonumber),
        repo: $repo,
        head_ref: $head_ref,
        base_ref: $base_ref,
        head_sha: $head_sha,
        published_pr_url: $pr_url,
        published_pr_number: $pr_number,
        created_after: $created_after,
        evidence_readable: $evidence_readable,
        unreadable_reason: $unreadable_reason,
        scope_reason: $scope_reason,
        work_in_base: $work_in_base,
        prs_for_head: ($all | brief),
        merged_pr_count: $merged_count,
        open_pr_count: $open_count,
        closed_unmerged_pr_count: $closed_count,
        positive_artifact: (if $positive then "true" else "false" end),
        outstanding_prs: ($outstanding | brief),
        outstanding_pr_count: ($outstanding | length),
        outstanding_pr_basis: "open PRs on this head branch, plus every PR opened since the run started",
        mechanical_verdict: $mech.v,
        mechanical_reason: $mech.r,
        hard_negative: (if $mech.v == "FAILED" then "true" else "false" end)
      }'
}

# Case-insensitive EXACT-TOKEN equality, never substring: `NOT_MERGED` contains
# `MERGED` and `UNSUCCESSFUL` contains `SUCCESS`, and a `contains` match would
# read both as approval. Anything unrecognised — including the empty string —
# is UNCERTAIN, which never lifts a verdict on its own.
normalise_verdict_token() {
  local t
  t="$(printf '%s' "${1:-}" | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')"
  case "$t" in
    SUCCESS|SUCCEEDED|SUCCESSFUL|PASS|PASSED|LANDED|MERGED|COMPLETE|COMPLETED|ACHIEVED) printf 'SUCCESS' ;;
    FAILED|FAILURE|FAIL|NOT_LANDED|NOTLANDED|UNMERGED|ABANDONED|BLOCKED) printf 'FAILED' ;;
    *) printf 'UNCERTAIN' ;;
  esac
}

adjudicate() {
  local ev
  if [ -n "$EVIDENCE_FILE" ]; then
    ev="$(cat "$EVIDENCE_FILE" 2>/dev/null)"
  elif [ -n "$EVIDENCE" ]; then
    ev="$EVIDENCE"
  elif [ "$READ_STDIN" = "true" ] || [ ! -t 0 ]; then
    ev="$(cat)"
  else
    ev=""
  fi

  if [ -z "${ev//[[:space:]]/}" ] || ! printf '%s' "$ev" | jq -e 'type == "object"' >/dev/null 2>&1; then
    # No structured evidence at all. This is not a licence to pass: nothing was
    # proven, so the honest answer is UNCERTAIN with the artifacts we do know.
    jq -nc --arg reason "terminal evidence was absent or unparseable" \
           --arg pr_url "$PR_URL" --arg pr_number "$PR_NUMBER" '
      {terminal_verdict:"UNCERTAIN", verdict_source:"missing_evidence",
       terminal_success_proven:"false", mechanical_verdict:"UNCERTAIN",
       agent_verdict:"", reason:$reason,
       required_next_action:"Re-run terminal evidence collection; do not close the run until the outstanding pull requests below are driven to a terminal state.",
       outstanding_prs: (if $pr_url == "" and $pr_number == "" then [] else [{url:$pr_url, number:$pr_number}] end),
       outstanding_pr_count: (if $pr_url == "" and $pr_number == "" then 0 else 1 end)}'
    echo "WARNING: workflow_terminal_evidence.sh: UNCERTAIN — terminal evidence was absent or unparseable." >&2
    return 0
  fi

  field() { printf '%s' "$ev" | jq -r --arg f "$1" '.[$f] // ""'; }

  local mech positive hard agent final source reason
  mech="$(field mechanical_verdict)"
  positive="$(field positive_artifact)"
  hard="$(field hard_negative)"
  case "$mech" in SUCCESS|UNCERTAIN|FAILED) ;; *) mech="UNCERTAIN" ;; esac

  agent=""
  if [ -n "${AGENT_VERDICT//[[:space:]]/}" ]; then
    agent="$(normalise_verdict_token "$AGENT_VERDICT")"
  fi

  if [ "$hard" = "true" ] || [ "$mech" = "FAILED" ]; then
    # HARD NEGATIVE. The evidence was readable and it shows the work did not
    # land and no PR carries it. Judgement is not consulted and cannot lift it —
    # this is the guard that stops the adjudicator becoming a rubber stamp.
    final="FAILED"; source="deterministic_hard_negative"
    reason="$(field mechanical_reason)"
  elif [ -z "$agent" ]; then
    final="$mech"; source="deterministic_only"
    reason="$(field mechanical_reason)"
  elif [ "$agent" = "FAILED" ]; then
    # Downgrades are always admitted: judgement that says "this merged PR is not
    # ours" costs a false alarm, never a silently orphaned run.
    final="FAILED"; source="agent_downgrade"
    reason="evaluation judged the run failed despite mechanical evidence of ${mech}"
  elif [ "$agent" = "UNCERTAIN" ]; then
    final="UNCERTAIN"; source="agent_uncertain"
    reason="evaluation could not confirm the run's terminal state"
  elif [ "$positive" = "true" ]; then
    final="SUCCESS"; source="agent_confirmed"
    reason="evaluation confirmed the run achieved its goal; $(field mechanical_reason)"
  else
    # An asserted SUCCESS with nothing to stand on is refused. No merged PR, no
    # open PR, no work in base — there is no artifact that could make this true.
    final="UNCERTAIN"; source="agent_success_refused_no_artifact"
    reason="evaluation asserted success but the collected evidence contains no merged PR, no open PR, and no work in the base branch"
  fi

  local proven="false"
  [ "$final" = "SUCCESS" ] && proven="true"

  local next_action
  case "$final" in
    SUCCESS) next_action="None for the primary deliverable. Drive any outstanding pull requests listed below to a terminal state." ;;
    UNCERTAIN) next_action="Terminal success was NOT proven and NOT disproven. Do not discard this run: hand the outstanding pull requests listed below to the goal-seeking loop and re-check." ;;
    *) next_action="Terminal failure is proven from readable evidence. Investigate why the work never reached a pull request or the base branch." ;;
  esac

  printf '%s' "$ev" | jq -c \
    --arg terminal_verdict "$final" \
    --arg verdict_source "$source" \
    --arg terminal_success_proven "$proven" \
    --arg agent_verdict "$agent" \
    --arg reason "$reason" \
    --arg required_next_action "$next_action" '
    {
      terminal_verdict: $terminal_verdict,
      verdict_source: $verdict_source,
      terminal_success_proven: $terminal_success_proven,
      mechanical_verdict: (.mechanical_verdict // "UNCERTAIN"),
      agent_verdict: $agent_verdict,
      reason: $reason,
      required_next_action: $required_next_action,
      scope_reason: (.scope_reason // ""),
      evidence_readable: (.evidence_readable // "false"),
      work_in_base: (.work_in_base // "unknown"),
      merged_pr_count: (.merged_pr_count // 0),
      open_pr_count: (.open_pr_count // 0),
      closed_unmerged_pr_count: (.closed_unmerged_pr_count // 0),
      outstanding_prs: (.outstanding_prs // []),
      outstanding_pr_count: (.outstanding_pr_count // 0)
    }'

  # Human-readable half. The incident's error text — `no_scoped_pr` — told the
  # operator nothing about the PR that HAD merged or the two left dangling.
  {
    echo "=== TERMINAL STATE ADJUDICATION (issue #1268) ==="
    echo "verdict=${final} (source=${source})"
    echo "reason: ${reason}"
    [ -n "$(field scope_reason)" ] && echo "mechanical scope signal: $(field scope_reason) (a signal, not the verdict)"
    echo "merged_pr_count=$(field merged_pr_count) open_pr_count=$(field open_pr_count) closed_unmerged_pr_count=$(field closed_unmerged_pr_count) work_in_base=$(field work_in_base)"
    echo "outstanding_pr_count=$(field outstanding_pr_count)"
    printf '%s' "$ev" | jq -r '(.outstanding_prs // [])[] | "  OUTSTANDING PR #\(.number) [\(.state)] \(.url) — \(.title)"'
    echo "required_next_action: ${next_action}"
  } >&2

  case "$final" in
    SUCCESS) return 0 ;;
    UNCERTAIN) return 0 ;;
    *) [ "$REPORT_ONLY" = "true" ] && return 0; return 1 ;;
  esac
}

# rv_field <key> <field>: this run's own field from the base RECIPE_VAR_<key> (#1561).
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/recipe_var.sh" \
  || { echo "ERROR: workflow_terminal_evidence.sh: recipe_var.sh not found beside it" >&2; exit 2; }

# Recipe-facing wrappers (issue #1268). These keep the recipe steps to a couple
# of lines each: the env-variable precedence and the fail-soft fallbacks live
# here, next to the logic they feed, instead of being duplicated across recipes.
collect_for_step() {
  local pr_url pr_number
  pr_url="${RECIPE_VAR_pr_url:-$(rv_field pr_publish_result pr_url)}"
  pr_number="${RECIPE_VAR_pr_number:-$(rv_field pr_publish_result pr_number)}"
  [ "$pr_url" = "''" ] && pr_url=""
  PR_URL="$pr_url"
  case "$pr_number" in ''|"''"|*[!0-9]*) PR_NUMBER="" ;; *) PR_NUMBER="$pr_number" ;; esac
  CREATED_AFTER="${CREATED_AFTER:-${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}}"
  SCOPE_REASON="${SCOPE_REASON:-pre-collected for the terminal-status gate}"
  collect
}

# Extract the single verdict token from the adjudicator agent's prose. The last
# explicit line wins; anything unrecognised — including no narrative at all,
# when the agent step was skipped — stays empty, which the adjudicator treats
# as "no judgement offered" and which therefore lifts nothing on its own.
verdict_token() {
  local narrative token
  narrative="${RECIPE_VAR_terminal_adjudication_narrative:-}"
  token="$(printf '%s\n' "$narrative" \
    | grep -Eo '^[[:space:]]*TERMINAL_VERDICT:[[:space:]]*[A-Za-z_]+' \
    | tail -1 \
    | sed -E 's/^[[:space:]]*TERMINAL_VERDICT:[[:space:]]*//' \
    | tr '[:lower:]' '[:upper:]' || true)"
  case "$token" in SUCCESS|FAILED|UNCERTAIN) ;; *) token="" ;; esac
  jq -nc --arg agent_verdict "$token" '{agent_verdict: $agent_verdict}'
}

case "$MODE" in
  collect) collect ;;
  collect-for-step) collect_for_step ;;
  verdict-token) verdict_token ;;
  adjudicate) adjudicate ;;
  *)
    echo "ERROR: workflow_terminal_evidence.sh requires mode: collect, collect-for-step, verdict-token, or adjudicate" >&2
    usage
    exit 2
    ;;
esac
