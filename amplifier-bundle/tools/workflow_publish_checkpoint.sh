#!/usr/bin/env bash
# Private bounded checkpoint support. The three phase scripts alone use this
# interface. Authority never changes when repair changes reviewed contents.
workflow_checkpoint_python() {
  python3 -I - "$@" <<'CHECKPOINT_PY'
import base64, hashlib, json, math, os, pathlib, re, stat, subprocess, sys, tempfile, uuid

class Refusal(Exception):
    pass

def require(ok, reason):
    if not ok:
        raise Refusal(reason)

def pairs(items):
    result = {}
    for key, value in items:
        require(key not in result, 'duplicate JSON field')
        result[key] = value
    return result

def finite_number(raw):
    value = float(raw)
    require(math.isfinite(value), 'non-finite JSON number')
    return value

def integer_number(raw):
    value = int(raw)
    return value if -(2 ** 63) <= value < 2 ** 64 else finite_number(raw)

def invalid_constant(_raw):
    raise Refusal('non-JSON numeric constant')

def json_strings(value):
    if isinstance(value, str):
        value.encode('utf-8', errors='strict')
    elif isinstance(value, dict):
        for key, item in value.items():
            json_strings(key)
            json_strings(item)
    elif isinstance(value, list):
        for item in value:
            json_strings(item)

def parse(raw):
    value = json.loads(raw, object_pairs_hook=pairs, parse_constant=invalid_constant,
                       parse_float=finite_number, parse_int=integer_number)
    json_strings(value)
    return value

def serial(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()

def digest(raw):
    return hashlib.sha256(raw).hexdigest()

def b64(raw):
    return base64.b64encode(raw).decode('ascii')

def unb64(value):
    require(isinstance(value, str), 'path encoding must be a string')
    raw = base64.b64decode(value, validate=True)
    require(b64(raw) == value, 'non-canonical path encoding')
    literal(raw)
    return raw

def literal(path):
    require(path and not path.startswith(b'/') and b'\0' not in path and
            all(part not in (b'', b'.', b'..') for part in path.split(b'/')), 'invalid literal path')

# These selectors can change the effective repository, object store, index,
# configuration or literal path interpretation. Refuse them rather than unset
# them and accidentally inspect a different repository from the operation.
def selectors():
    forbidden = ('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR',
                 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES',
                 'GIT_CONFIG', 'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS', 'GIT_NAMESPACE',
                 'GIT_SHALLOW_FILE', 'GIT_GRAFT_FILE', 'GIT_REPLACE_REF_BASE', 'GIT_CEILING_DIRECTORIES',
                 'GIT_GLOB_PATHSPECS', 'GIT_NOGLOB_PATHSPECS', 'GIT_ICASE_PATHSPECS')
    require(not any(k in os.environ for k in forbidden), 'unsupported inherited Git selector')

root = None
remote_advertisements = {}
checked_publication = None

def git(*args, optional=False, data=None):
    env = dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1', GIT_NO_LAZY_FETCH='1')
    result = subprocess.run(['git', '--literal-pathspecs', '-C', root, *args],
                            env=env, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 79:
        raise SystemExit(79)
    if result.returncode:
        if optional:
            return b''
        # Git errors may repeat a credential-bearing remote URL. Retain the
        # status without exporting transport credentials to a role or terminal.
        raise Refusal('Git inspection failed (exit ' + str(result.returncode) + ')')
    return result.stdout

def textgit(*args, **kw):
    return os.fsdecode(git(*args, **kw).rstrip(b'\n'))

def canonical(path):
    return os.path.realpath(path)

def context(phase):
    global root
    selectors()
    owner = os.environ.get('RECIPE_VAR_task_description', '')
    require(owner and owner.strip(), 'missing parent owner task')
    if 'TASK_DESCRIPTION' in os.environ:
        require(os.environ['TASK_DESCRIPTION'] == owner, 'inconsistent parent task identity')
    setup = parse(os.environ.get('RECIPE_VAR_worktree_setup', ''))
    require(isinstance(setup, dict), 'missing step-04 worktree ownership')
    path, branch = setup.get('worktree_path'), setup.get('branch_name')
    require(isinstance(path, str) and os.path.isabs(path) and isinstance(branch, str) and branch, 'invalid step-04 ownership')
    # Step 04 may use a local alias while branch_name remains the publication
    # branch (#1762). Both names come from the owner context, never from HEAD.
    local_branch = setup.get('local_branch', branch)
    require(isinstance(local_branch, str) and local_branch, 'invalid step-04 local branch')
    root = canonical(path)
    require(path == root and '\n' not in path and '\r' not in path, 'step-04 path is not canonical and single-line')
    require(os.path.isdir(root) and canonical(os.getcwd()) == root, 'checkpoint must start at the owned worktree top')
    require(os.path.isfile(os.path.join(root, '.git')) and not os.path.islink(os.path.join(root, '.git')), 'not a linked worktree')
    require(textgit('rev-parse', '--show-toplevel') == root, 'worktree toplevel mismatch')
    gd = canonical(textgit('rev-parse', '--absolute-git-dir'))
    common = canonical(os.path.join(root, textgit('rev-parse', '--git-common-dir')))
    require(gd != common, 'main worktree is unsupported')
    require(not os.path.lexists(os.path.join(common, 'info', 'grafts')), 'legacy grafted history is unsupported')
    require(not git('for-each-ref', '--format=%(refname)', 'refs/replace/'),
            'replacement history is unsupported')
    require(textgit('rev-parse', '--is-shallow-repository') == 'false', 'incomplete shallow history is unsupported')
    require(canonical(textgit('rev-parse', '--git-path', 'index')) == os.path.join(gd, 'index'), 'unsupported index identity')
    index = os.path.join(gd, 'index')
    require(os.path.isfile(index) and not os.path.islink(index), 'unsupported index file')
    raw = pathlib.Path(index).read_bytes()
    require(raw[:4] == b'DIRC' and int.from_bytes(raw[4:8], 'big') in (2, 3, 4), 'unsupported index format')
    require(not textgit('rev-parse', '--shared-index-path'), 'split index is unsupported')
    require(textgit('config', '--bool', 'core.sparseCheckout', optional=True) != 'true', 'sparse index is unsupported')
    require(textgit('config', '--get', 'core.fsmonitor', optional=True) in ('', 'false'), 'filesystem-monitor index inspection is unsupported')
    tags = git('ls-files', '-v', '-z').split(b'\0')
    require(all(not row or row.startswith(b'H ') for row in tags), 'unsupported index flags')
    require(not git('ls-files', '--unmerged', '-z'), 'unmerged index is unsupported')
    actual = textgit('symbolic-ref', '--quiet', '--short', 'HEAD')
    require(actual == local_branch, 'branch differs from step-04 ownership')
    default = textgit('symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD', optional=True)
    protected = {'main', 'master', 'develop', 'trunk', 'HEAD', default.removeprefix('origin/')}
    require(actual not in protected and branch not in protected, 'protected branch')
    if local_branch != branch:
        prefix = branch + '--wt-'
        suffix = local_branch[len(prefix):] if local_branch.startswith(prefix) else ''
        require(suffix and suffix[0] in '123456789' and all(c in '0123456789' for c in suffix),
                'invalid step-04 branch alias')
        require(textgit('config', '--get', 'branch.' + actual + '.remote', optional=True) == 'origin' and
                textgit('config', '--get', 'branch.' + actual + '.merge', optional=True) == 'refs/heads/' + branch,
                'branch alias does not track the step-04 publication branch')
    repo = os.environ.get('RECIPE_VAR_repo_path', '')
    require(repo and os.path.isabs(repo), 'missing parent repository')
    parent = subprocess.run(['git', '-C', repo, 'rev-parse', '--git-common-dir'], env=dict(os.environ, GIT_OPTIONAL_LOCKS='0'), capture_output=True)
    require(parent.returncode == 0 and canonical(os.path.join(repo, os.fsdecode(parent.stdout.rstrip(b'\n')))) == common, 'parent repository mismatch')
    for state in ('MERGE_HEAD', 'CHERRY_PICK_HEAD', 'REVERT_HEAD', 'rebase-merge', 'rebase-apply'):
        require(not os.path.exists(os.path.join(gd, state)), 'unfinished Git operation')
    return dict(owner_task=owner, owner_requirements=os.environ.get('RECIPE_VAR_final_requirements', ''),
                phase=phase, worktree=root, git_dir=gd,
                common_dir=common, index_path=index, branch=actual, publication_branch=branch,
                initial_head=textgit('rev-parse', '--verify', 'HEAD'))

def inspection(*args):
    # Read only configuration names through Git's NUL-delimited interface.
    # Operator command values never enter this helper's output or metadata.
    env = dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1')
    configured = subprocess.run(['git', '--literal-pathspecs', '-C', root, 'config',
                                 '--null', '--name-only', '--get-regexp',
                                 r'^filter\..*\.(clean|process|required)$'],
                                env=env, capture_output=True)
    require(configured.returncode in (0, 1), 'filter inspection configuration is unavailable')
    prefixes = set()
    for key in filter(None, configured.stdout.split(b'\0')):
        prefix, field = key.rsplit(b'.', 1)
        require(prefix.startswith(b'filter.') and field in (b'clean', b'process', b'required') and
                not any(c in prefix for c in (b'=', b'\n', b'\r')),
                'unsupported literal filter inspection key')
        prefixes.add(os.fsdecode(prefix))
    overrides = []
    if prefixes:
        identity = '/bin/cat'
        tool = os.stat(identity)
        require(stat.S_ISREG(tool.st_mode) and tool.st_uid == 0 and
                tool.st_mode & 0o111 and not tool.st_mode & 0o022,
                'inspection identity executable is not qualified')
        for prefix in sorted(prefixes):
            for field, value in (('clean', identity), ('process', ''), ('required', 'false')):
                overrides.extend(('-c', prefix + '.' + field + '=' + value))
    # These overrides belong only to this read-only query. Normal authorised
    # staging and hooks keep their real filter configuration. Identity
    # normalisation can conservatively list filter-normalised files; listing a
    # path never grants permission to stage it.
    return git(*overrides, *args)


def clean_index():
    require(not inspection('diff-index', '--cached', '--raw', '--no-ext-diff', '--no-renames', '-z', 'HEAD'),
            'pre-existing staging has no invocation authority')

def pending():
    tracked = set(filter(None, inspection('diff-files', '--name-only', '--no-ext-diff', '--no-renames', '-z').split(b'\0')))
    staged = set(filter(None, inspection('diff-index', '--cached', '--name-only', '--no-ext-diff', '--no-renames', '-z', 'HEAD').split(b'\0')))
    others = set(filter(None, git('ls-files', '--others', '--exclude-standard', '-z').split(b'\0')))
    # Match the existing untracked-only artefact exclusions. Tracked source
    # always remains visible, including source beneath a runtime directory.
    def excluded(path):
        parts = path.split(b'/')
        if path.startswith((b'.amplihack/', b'.claude/runtime/', b'target/')):
            return True
        if any(p in (b'node_modules', b'__pycache__') or p.endswith(b'.egg-info') for p in parts):
            return True
        for i, part in enumerate(parts[:-1]):
            if part == b'target' and os.path.isfile(os.path.join(os.fsencode(root), *parts[:i+1], b'CACHEDIR.TAG')):
                return True
        return False
    return tracked | staged | {p for p in others if not excluded(p)}

def identity(path):
    literal(path)
    full = os.path.join(os.fsencode(root), path)
    parent = os.fsencode(root)
    for part in path.split(b'/')[:-1]:
        parent = os.path.join(parent, part)
        require(not os.path.islink(parent), 'symlink parent is unsupported')
    if not os.path.lexists(full):
        return dict(path_b64=b64(path), mode='000000', type='deletion', sha256=None)
    info = os.lstat(full)
    if stat.S_ISLNK(info.st_mode):
        payload, mode, kind = os.readlink(full), '120000', 'symlink'
    else:
        require(stat.S_ISREG(info.st_mode), 'candidate is a directory, submodule or special file')
        payload = pathlib.Path(os.fsdecode(full)).read_bytes()
        mode, kind = ('100755' if info.st_mode & 0o111 else '100644'), 'file'
    return dict(path_b64=b64(path), mode=mode, type=kind, sha256=digest(payload))

def commits():
    return {row.split()[0].decode() for row in git('cat-file', '--batch-all-objects', '--batch-check=%(objectname) %(objecttype)').splitlines()
            if row.split()[1] == b'commit'}

def commit_info(oid):
    row = textgit('rev-list', '--parents', '-n', '1', oid).split()
    require(row and row[0] == oid, 'missing commit identity')
    parents = row[1:]
    paths = set()
    if parents:
        for parent in parents:
            paths.update(filter(None, git('diff-tree', '--no-commit-id', '--name-only', '--no-ext-diff', '--no-renames', '-r', '-z', parent, oid).split(b'\0')))
    else:
        paths.update(filter(None, git('diff-tree', '--root', '--no-commit-id', '--name-only', '--no-ext-diff', '--no-renames', '-r', '-z', oid).split(b'\0')))
    return dict(commit=oid, parents=parents, paths=[b64(p) for p in sorted(paths)])

def publication(binding, captured_publication=None):
    upstream_ref = textgit('rev-parse', '--symbolic-full-name', '@{u}', optional=True)
    setup = parse(os.environ['RECIPE_VAR_worktree_setup'])
    owner_ref = 'refs/remotes/origin/' + binding['publication_branch']
    known_owner = textgit('rev-parse', '--verify', owner_ref + '^{commit}', optional=True)
    base = upstream_ref or (owner_ref if known_owner else setup.get('base_ref', ''))
    require(isinstance(base, str) and base and not base.startswith('-'), 'missing publication ancestry')
    upstream = textgit('rev-parse', '--verify', base + '^{commit}')
    remote = textgit('config', '--get', 'branch.' + binding['branch'] + '.remote', optional=True)
    merge = textgit('config', '--get', 'branch.' + binding['branch'] + '.merge', optional=True)
    require(remote in ('', 'origin'), 'unsupported publication remote')
    target = 'refs/heads/' + binding['publication_branch']
    if remote == 'origin':
        require(merge == target and upstream_ref in ('', owner_ref),
                'upstream differs from the owner publication branch')
    else:
        require(not merge and not upstream_ref, 'incomplete publication tracking configuration')
    default = textgit('symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD', optional=True)
    require(target.removeprefix('refs/heads/') not in {'main', 'master', 'develop', 'trunk', default.removeprefix('origin/')},
            'protected publication reference')
    require(textgit('config', '--get', 'push.default', optional=True) in ('', 'simple', 'current', 'upstream'),
            'unsupported implicit push selection')
    for key in ('remote.pushDefault', 'branch.' + binding['branch'] + '.pushRemote', 'remote.origin.push'):
        require(not textgit('config', '--get-all', key, optional=True), 'unsupported push routing: ' + key)
    for key in ('remote.origin.mirror', 'push.followTags'):
        require(textgit('config', '--bool', key, optional=True) != 'true', 'unsupported publication expansion: ' + key)
    require(textgit('config', '--get', 'push.recurseSubmodules', optional=True) in ('', 'no', 'false', 'check'),
            'unsupported recursive publication expansion')
    url = textgit('remote', 'get-url', '--push', '--all', 'origin')
    require(url and '\n' not in url and '\r' not in url, 'multiple or ambiguous publication destinations')
    # Bind the exact destination bytes without including userinfo in capture,
    # authority, observations or any model input.
    pub = dict(ref=target, remote='origin', url_sha256=digest(os.fsencode(url)),
               upstream_ref=upstream_ref, base_ref=base, upstream=upstream)
    tip = destination_tip(pub)
    prepared_base = setup.get('base_ref', base)
    require(isinstance(prepared_base, str) and prepared_base and not prepared_base.startswith('-'),
            'missing prepared publication base')
    # Resolve this input during capture even if the destination exists. A
    # later fetch may move the tracking ref, but cannot renew this authority.
    if captured_publication is None:
        prepared_oid = textgit('rev-parse', '--verify', prepared_base + '^{commit}', optional=True) or None
    else:
        require(prepared_base == captured_publication.get('prepared_base_ref'),
                'prepared publication base reference changed')
        prepared_oid = captured_publication.get('prepared_base_oid')
    # Fetch tracking remains the input to the existing rebase checks. The
    # absent-destination fallback uses only the immutably captured commit.
    publication_base = tip or prepared_oid
    require(publication_base, 'captured prepared publication base is unavailable')
    pub.update(destination_tip=tip, publication_base=publication_base,
               prepared_base_ref=prepared_base, prepared_base_oid=prepared_oid)
    if captured_publication is None:
        pub['history'] = prepare_history(pub, [binding['initial_head'], upstream, prepared_oid])
    else:
        pub['history'] = captured_publication['history']
        require(pub['history']['captured_tip'] == captured_publication['destination_tip'],
                'private captured destination identity changed')
        validate_history(pub['history'], pub)
    return pub

def publication_transport(pub):
    # A command-local alias inspects the exact push destination, even when
    # origin fetches elsewhere. Its URL is neither an argv value nor retained
    # in a public capture. No repository or global configuration is changed.
    alias = 'workflow-checkpoint-' + uuid.uuid4().hex
    push_url = textgit('remote', 'get-url', '--push', '--all', 'origin')
    env = dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1',
               GIT_CONFIG_COUNT='1', GIT_CONFIG_KEY_0='remote.' + alias + '.url',
               GIT_CONFIG_VALUE_0=push_url)
    resolved = subprocess.run(['git', '--literal-pathspecs', '-C', root, 'ls-remote',
                               '--get-url', '--', alias], env=env, capture_output=True)
    if resolved.returncode == 79:
        raise SystemExit(79)
    require(resolved.returncode == 0 and digest(resolved.stdout.rstrip(b'\n')) == pub['url_sha256'],
            'publication lookup destination differs from the bound push destination')
    return env, alias


def destination_tip(pub):
    env, alias = publication_transport(pub)
    result = subprocess.run(['git', '--literal-pathspecs', '-C', root, 'ls-remote',
                             '--exit-code', '--', alias], env=env, capture_output=True)
    if result.returncode == 79:
        print('publication destination inspection failed (exit 79)', file=sys.stderr)
        raise SystemExit(79)
    require(result.returncode in (0, 2),
            'publication destination inspection failed (exit ' + str(result.returncode) + ')')
    refs = {}
    for line in result.stdout.splitlines():
        parts = line.split(b'\t')
        require(len(parts) == 2 and (parts[1] == b'HEAD' or parts[1].startswith(b'refs/')),
                'malformed publication advertisement')
        oid = parts[0].decode('ascii')
        require(len(oid) in (40, 64) and all(c in '0123456789abcdef' for c in oid),
                'invalid publication destination identity')
        require(parts[1] not in refs, 'duplicate publication advertisement')
        refs[parts[1]] = oid
    require(result.returncode != 2 or not refs, 'ambiguous absent publication destination')
    # This process-local view is never permission, a receipt or a fifth field.
    # A push's before check reuses this one actual remote advertisement.
    remote_advertisements[pub['url_sha256']] = refs
    return refs.get(os.fsencode(pub['ref']))


def push_selection(record, argv):
    require(argv[:2] == ['command', 'git'], 'unsupported push executable')
    args = list(argv[2:])
    if args[:1] == ['--no-pager']:
        args.pop(0)
    require(args[:1] == ['push'], 'missing explicit push operation')
    args.pop(0)
    while args and args[0].startswith('-'):
        require(args[0] in ('-u', '--set-upstream', '--quiet', '--verbose', '--'),
                'unsupported task push option')
        if args.pop(0) == '--':
            break
    require(len(args) <= 2 and (not args or args[0] == 'origin'),
            'push must use only the bound task remote and ref')
    if len(args) == 2:
        refspec = args[1]
        require(refspec.count(':') <= 1, 'unsupported task push refspec')
        source, separator, target = refspec.partition(':')
        local = record['binding']['branch']
        expected = record['binding']['publication_branch']
        require(source in ('HEAD', local, 'refs/heads/' + local),
                'push source differs from the task branch')
        if not separator:
            target = source.removeprefix('refs/heads/')
        require(target in (expected, 'refs/heads/' + expected),
                'push destination differs from the task reference')


def verify_publication_destination(record, pub):
    original = record['capture']['publication']
    require(all(pub[key] == original[key] for key in ('ref', 'remote', 'url_sha256')),
            'publication destination binding changed')
    allowed = {unb64(p) for p in record['approved_paths']}
    captured = {item['commit'] for item in record['capture']['outgoing_commits']}
    for row in outgoing(pub):
        require({unb64(p) for p in row['paths']} <= allowed,
                'actual publication destination exposes unreviewed outgoing history')
        require(row['commit'] in captured or row['commit'] not in record['capture']['initial_commits'],
                'actual publication destination exposes an uncaptured existing commit')


def history_environment(directory):
    # History queries never use operator filters, templates or lazy fetching.
    return dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1',
                GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_SYSTEM=os.devnull,
                GIT_CONFIG_GLOBAL=os.devnull, GIT_ATTR_NOSYSTEM='1', GIT_NO_LAZY_FETCH='1',
                HOME=str(directory.parent / 'home'), XDG_CONFIG_HOME=str(directory.parent / 'xdg'))

def history_command(directory, *args, data=None, env=None):
    result = subprocess.run(['git', '--literal-pathspecs', '--git-dir=' + str(directory), *args],
                            env=env or history_environment(directory), input=data, capture_output=True)
    if result.returncode == 79:
        raise SystemExit(79)
    return result

def history_files(directory):
    result = {}
    for folder, dirs, names in os.walk(directory, followlinks=False):
        for name in sorted(dirs + names):
            path = pathlib.Path(folder) / name
            info = os.lstat(path)
            require(info.st_uid == os.getuid() and not info.st_mode & 0o022 and
                    (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)),
                    'unsupported private history entry')
            row = dict(identity=str(info.st_dev) + ':' + str(info.st_ino),
                       mode=stat.S_IMODE(info.st_mode), directory=stat.S_ISDIR(info.st_mode))
            if stat.S_ISREG(info.st_mode):
                checksum = hashlib.sha256()
                with open(path, 'rb') as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                        checksum.update(chunk)
                row['sha256'] = checksum.hexdigest()
            result[str(path.relative_to(directory))] = row
    return result

def prepare_history(pub, seeds):
    # All preparation writes belong to one private view. In particular, a
    # missing remote tip is never fetched into the implementation repository.
    parent = private_dir()
    directory = parent / 'repository'
    for name in ('home', 'xdg'):
        (parent / name).mkdir(mode=0o700)
    object_format = textgit('rev-parse', '--show-object-format')
    require(object_format in ('sha1', 'sha256'), 'unsupported history object format')
    journal = dict(directory=str(directory), worktree=root, object_format=object_format,
                   captured_tip=pub['destination_tip'], destination_sha256=pub['url_sha256'],
                   ref_sha256=digest(os.fsencode(pub['ref'])), writes='Private Git metadata and objects only.',
                   operations=[], complete=False)
    previous_umask = os.umask(0o077)
    try:
        initialized = subprocess.run(['git', 'init', '--bare', '--quiet', '--template=',
                                      '--initial-branch=checkpoint-history',
                                      '--object-format=' + object_format, str(directory)],
                                     env=history_environment(directory), capture_output=True)
        journal['operations'].append(dict(operation='private-init', exit_code=initialized.returncode))
        if initialized.returncode == 79:
            raise SystemExit(79)
        require(initialized.returncode == 0, 'private history preparation failed')
        seeds = sorted({oid for oid in seeds if oid})
        seed_file = parent / 'seed.pack'
        with open(seed_file, 'xb') as stream:
            packed = subprocess.run(['git', '--literal-pathspecs', '-C', root,
                                     'pack-objects', '--stdout', '--revs'],
                                    env=dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1', GIT_NO_LAZY_FETCH='1'),
                                    input=('\n'.join(seeds) + '\n').encode('ascii'), stdout=stream, stderr=subprocess.PIPE)
        journal['operations'].append(dict(operation='read-source-pack-to-private-file', exit_code=packed.returncode))
        if packed.returncode == 79:
            raise SystemExit(79)
        require(packed.returncode == 0, 'private history seed preparation failed')
        with open(seed_file, 'rb') as stream:
            imported = subprocess.run(['git', '--git-dir=' + str(directory), 'index-pack', '--stdin'],
                                      env=history_environment(directory), stdin=stream, capture_output=True)
        journal['operations'].append(dict(operation='private-index-pack', exit_code=imported.returncode))
        if imported.returncode == 79:
            raise SystemExit(79)
        require(imported.returncode == 0, 'private history seed import failed')
        seed_file.unlink()
        if pub['destination_tip']:
            transport, alias = publication_transport(pub)
            resolved = history_command(directory, 'ls-remote', '--get-url', '--', alias, env=transport)
            require(resolved.returncode == 0 and digest(resolved.stdout.rstrip(b'\n')) == pub['url_sha256'],
                    'private history destination differs from the captured destination')
            fetched = history_command(directory, 'fetch', '--no-write-fetch-head', '--no-tags',
                                      '--no-recurse-submodules', '--no-auto-maintenance', '--',
                                      alias, pub['destination_tip'], env=transport)
            journal['operations'].append(dict(operation='private-exact-tip-fetch', exit_code=fetched.returncode))
            require(fetched.returncode == 0, 'private destination history preparation failed')
        require(not list((directory / 'objects/pack').glob('*.promisor')),
                'partial private history is unsupported')
        if pub['destination_tip']:
            present = history_command(directory, 'cat-file', '-e', pub['destination_tip'] + '^{commit}')
            require(present.returncode == 0, 'captured destination is absent from private history')
        checked = history_command(directory, 'fsck', '--strict', '--full', '--no-reflogs')
        require(checked.returncode == 0, 'private history objects failed validation')
        require(destination_tip(pub) == pub['destination_tip'], 'destination changed during history preparation')
        require(not any((directory / name).exists() for name in
                        ('FETCH_HEAD', 'shallow', 'objects/info/alternates')),
                'private history introduced unsupported metadata')
        objects = history_command(directory, 'cat-file', '--batch-all-objects',
                                  '--batch-check=%(objectname) %(objecttype) %(objectsize)')
        require(objects.returncode == 0, 'private history inventory is unavailable')
        inventory = dict(files=history_files(directory), object_inventory_sha256=digest(objects.stdout))
        inventory_path = parent / 'history-inventory.json'
        inventory_hash = retain(inventory_path, inventory)
        info = os.lstat(directory)
        parent_info = os.lstat(parent)
        result = dict(directory=str(directory), identity=str(info.st_dev) + ':' + str(info.st_ino),
                      parent_identity=str(parent_info.st_dev) + ':' + str(parent_info.st_ino),
                      inventory_file=str(inventory_path), inventory_identity=file_identity(inventory_path),
                      inventory_sha256=inventory_hash, object_format=object_format, worktree=root,
                      captured_tip=pub['destination_tip'], destination_sha256=pub['url_sha256'],
                      ref_sha256=digest(os.fsencode(pub['ref'])))
        journal['complete'] = True
        return result
    except SystemExit as error:
        journal['terminal_exit'] = error.code
        raise
    finally:
        try:
            path = parent / 'history-preparation.json'
            path.write_bytes(serial(journal)); os.chmod(path, 0o600)
        finally:
            os.umask(previous_umask)

def validate_history(history, pub):
    require(isinstance(history, dict) and set(history) == {
        'directory', 'identity', 'parent_identity', 'inventory_file', 'inventory_identity',
        'inventory_sha256', 'object_format', 'worktree', 'captured_tip',
        'destination_sha256', 'ref_sha256'}, 'malformed private history descriptor')
    directory = pathlib.Path(history['directory'])
    require(str(directory) == canonical(str(directory)) and
            os.path.commonpath((str(directory), root)) != root, 'private history location changed')
    parent_info = os.lstat(directory.parent)
    require(stat.S_ISDIR(parent_info.st_mode) and parent_info.st_uid == os.getuid() and
            stat.S_IMODE(parent_info.st_mode) == 0o700 and
            str(parent_info.st_dev) + ':' + str(parent_info.st_ino) == history['parent_identity'],
            'private history parent identity changed')
    info = os.lstat(directory)
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o022 and
            str(info.st_dev) + ':' + str(info.st_ino) == history['identity'], 'private history identity changed')
    require(history['worktree'] == root and history['destination_sha256'] == pub['url_sha256'] and
            history['ref_sha256'] == digest(os.fsencode(pub['ref'])) and
            history['object_format'] == textgit('rev-parse', '--show-object-format'),
            'private history binding changed')
    path = directory.parent / 'history-inventory.json'
    require(str(path) == history['inventory_file'] and
            file_identity(path) == history['inventory_identity'], 'private history inventory identity changed')
    inventory_info = os.lstat(path)
    require(inventory_info.st_uid == os.getuid() and stat.S_IMODE(inventory_info.st_mode) == 0o600,
            'private history inventory permissions changed')
    raw = path.read_bytes()
    require(digest(raw) == history['inventory_sha256'], 'private history inventory changed')
    saved = parse(raw)
    require(history_files(directory) == saved['files'], 'private history files changed')
    actual_format = history_command(directory, 'rev-parse', '--show-object-format')
    require(actual_format.returncode == 0 and actual_format.stdout.strip().decode() == history['object_format'],
            'private history object format changed')
    objects = history_command(directory, 'cat-file', '--batch-all-objects',
                              '--batch-check=%(objectname) %(objecttype) %(objectsize)')
    require(objects.returncode == 0 and digest(objects.stdout) == saved['object_inventory_sha256'],
            'private history object inventory changed')
    if history['captured_tip']:
        present = history_command(directory, 'cat-file', '-e', history['captured_tip'] + '^{commit}')
        require(present.returncode == 0, 'captured private history tip is unavailable')
    return directory

def history_ancestors(pub, oid, directory=None):
    require(isinstance(oid, str) and len(oid) in (40, 64) and
            all(c in '0123456789abcdef' for c in oid), 'invalid explicit history identity')
    directory = validate_history(pub['history'], pub) if directory is None else directory
    available = history_command(directory, 'cat-file', '-e', oid + '^{commit}')
    if available.returncode == 0:
        result = history_command(directory, 'rev-list', oid)
        require(result.returncode == 0, 'private history ancestry is incomplete')
        return set(result.stdout.decode('ascii').splitlines())
    # Only already present implementation objects can describe later normal
    # commits/fetches. This does not import, re-fetch or renew private evidence.
    kind = textgit('cat-file', '-t', oid, optional=True)
    require(kind in ('commit', 'tag', 'tree', 'blob'), 'advertised ancestry is unavailable locally')
    if kind in ('tree', 'blob'):
        return set()
    if kind == 'tag' and not textgit('rev-parse', '--verify', oid + '^{commit}', optional=True):
        peeled = textgit('rev-parse', '--verify', oid + '^{}')
        require(textgit('cat-file', '-t', peeled) in ('tree', 'blob'), 'unsupported advertised tag target')
        return set()
    return set(textgit('rev-list', oid).splitlines())


def outgoing(pub, head='HEAD'):
    require(pub['url_sha256'] in remote_advertisements, 'missing actual remote advertisement')
    tips = ([pub['destination_tip']] if pub['destination_tip'] else
            sorted(set(remote_advertisements[pub['url_sha256']].values())))
    excluded = set()
    directory = validate_history(pub['history'], pub)
    for tip in tips:
        excluded.update(history_ancestors(pub, tip, directory))
    return [commit_info(oid) for oid in textgit('rev-list', '--reverse', head).splitlines()
            if oid not in excluded]


def candidate_digest(snapshot):
    # Decision 17.116 withdraws rejection of unreachable local objects.
    # Keep the captured inventory for existing-identity checks, not permission.
    return digest(serial({key: value for key, value in snapshot.items() if key != 'initial_commits'}))


def snapshot(binding, captured=None):
    pub = publication(binding, captured['publication'] if captured is not None else None)
    history = outgoing(pub)
    paths = pending()
    for row in history:
        paths.update(unb64(p) for p in row['paths'])
    return dict(paths=[identity(p) for p in sorted(paths)], outgoing_commits=history,
                publication=pub, initial_commits=sorted(commits()),
                inspection_policy='External filters use qualified identity normalisation only during inspection. '
                                  'Candidates can include filter-normalised files; this grants no task path permission. '
                                  'Normal authorised staging retains operator filters.')

def private_dir():
    tmp = os.environ.get('TMPDIR', '')
    require(os.path.isabs(tmp) and os.path.isdir(tmp), 'TMPDIR must be an assigned absolute directory')
    tmp = canonical(tmp)
    require(os.path.commonpath((tmp, root)) != root, 'authority must remain outside the worktree')
    directory = tempfile.mkdtemp(prefix='checkpoint-scope-', dir=tmp)
    os.chmod(directory, 0o700)
    return pathlib.Path(directory)

def retain(path, value):
    raw = serial(value)
    with open(path, 'xb') as stream:
        stream.write(raw)
    # Immutability is enforced by the independently held hash and identity.
    # Keep failed tampering observable rather than hiding it behind a sidecar.
    os.chmod(path, 0o600)
    return digest(raw)

def file_identity(path):
    info = os.lstat(path)
    require(stat.S_ISREG(info.st_mode), 'authority is not a regular file')
    return str(info.st_dev) + ':' + str(info.st_ino)

observation_directory = None

def observation(value):
    # Observations are evidence only. They never renew scope or authenticate
    # authority and are never consumed as permission on a later invocation.
    if observation_directory is not None:
        path = str(observation_directory / 'operations.jsonl')
        fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'ab') as stream:
            stream.write(serial(value) + b'\n')

def load_authority():
    global observation_directory
    path = os.environ.get('WF_CHECKPOINT_RECORD', '')
    require(path and file_identity(path) == os.environ.get('WF_CHECKPOINT_RECORD_ID'), 'authority record identity changed')
    raw = pathlib.Path(path).read_bytes()
    require(digest(raw) == os.environ.get('WF_CHECKPOINT_HASH'), 'immutable authority hash mismatch')
    record = parse(raw)
    binding = record['binding']
    actual = context(binding['phase'])
    for key, value in actual.items():
        if key != 'initial_head':
            require(binding[key] == value, 'authority binding changed: ' + key)
    require(binding['invocation'] == os.environ.get('WF_CHECKPOINT_INVOCATION'), 'cross-invocation authority')
    private = pathlib.Path(path).parent
    require(os.path.isabs(path) and canonical(path) == path and pathlib.Path(path).name == 'authority.json',
            'authority is not at its original private location')
    require(canonical(str(private.parent)) == canonical(os.environ.get('TMPDIR', '')) and
            os.path.commonpath((str(private), root)) != root,
            'authority observations are not in the assigned private directory')
    observation_directory = private
    return record, actual

def scope_check(record, observed_upstream=None, observed_tracking=None, publication_view=None):
    global checked_publication
    allowed = {unb64(p) for p in record['approved_paths']}
    require(pending() <= allowed, 'unknown pending or staged source path')
    pub = publication_view if publication_view is not None else publication(record['binding'], record['capture']['publication'])
    checked_publication = pub
    expected = observed_upstream or os.environ.get('WF_CHECKPOINT_UPSTREAM')
    require(pub['upstream'] == expected, 'upstream changed outside a known fetch operation')
    original = record['capture']['publication']
    for key in ('ref', 'remote', 'url_sha256'):
        require(pub[key] == original[key], 'publication binding changed: ' + key)
    tracking = observed_tracking or dict(upstream_ref=os.environ.get('WF_CHECKPOINT_UPSTREAM_REF'),
                                        base_ref=os.environ.get('WF_CHECKPOINT_BASE_REF'))
    for key in ('upstream_ref', 'base_ref'):
        require(pub[key] == tracking[key], 'publication tracking changed outside a known operation: ' + key)
    for row in outgoing(pub):
        require({unb64(p) for p in row['paths']} <= allowed, 'outgoing history exceeds reviewed scope')
    return allowed

def check():
    record, actual = load_authority()
    require(actual['initial_head'] == os.environ.get('WF_CHECKPOINT_HEAD'), 'unexpected HEAD transition')
    scope_check(record)
    return record

def verify_replay(start, old_head, upstream, new_head):
    # Patch identifiers discard whitespace. Replay known changes and compare
    # complete trees instead, including every source byte and file mode.
    original = textgit('rev-list', '--reverse', start + '..' + old_head).splitlines()
    rewritten = textgit('rev-list', '--reverse', upstream + '..' + new_head).splitlines()
    require(len(original) == len(rewritten), 'rewritten history changes the known commit count')
    require(observation_directory is not None, 'replay requires validated private authority')
    directory = pathlib.Path(tempfile.mkdtemp(prefix='replay-', dir=observation_directory))
    original_objects = canonical(os.path.join(root, textgit('rev-parse', '--git-path', 'objects')))
    require('\n' not in original_objects and '\r' not in original_objects, 'unsupported object-store path')
    object_format = textgit('rev-parse', '--show-object-format')
    require(object_format in ('sha1', 'sha256'), 'unsupported repository object format')
    # An object directory alone does not isolate external merge drivers or
    # renormalisation filters. Use private Git metadata, an empty configuration
    # and synthetic home directories. Repository objects are read-only inputs;
    # committed attributes cannot name an executable in this private config.
    for name in ('home', 'xdg'):
        (directory / name).mkdir(mode=0o700)
    env = dict(os.environ, GIT_OPTIONAL_LOCKS='0', GIT_NO_REPLACE_OBJECTS='1',
               GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_SYSTEM=os.devnull,
               GIT_CONFIG_GLOBAL=os.devnull, GIT_ATTR_NOSYSTEM='1', GIT_NO_LAZY_FETCH='1',
               HOME=str(directory / 'home'), XDG_CONFIG_HOME=str(directory / 'xdg'))
    initialized = subprocess.run(['git', 'init', '--bare', '--quiet', '--template=',
                                  '--initial-branch=checkpoint-validation',
                                  '--object-format=' + object_format, str(directory)],
                                 env=env, capture_output=True)
    require(initialized.returncode == 0, 'private replay repository preparation failed')
    objects = directory / 'objects'
    (objects / 'info/alternates').write_text(original_objects + '\n')
    reference = subprocess.run(['git', '--git-dir=' + str(directory), 'update-ref',
                                'refs/heads/checkpoint-validation', new_head],
                               env=env, capture_output=True)
    require(reference.returncode == 0, 'private replay reference preparation failed')
    parent = upstream
    replayed = []
    try:
        for before, after in zip(original, rewritten):
            old = commit_info(before)
            current = commit_info(after)
            require(len(old['parents']) == 1, 'rebase of merge history is unsupported')
            require(current['parents'] == [parent], 'rewritten history changes the expected parent')
            result = subprocess.run(['git', '--literal-pathspecs', '--git-dir=' + str(directory), 'merge-tree',
                                     '--write-tree', '--no-messages', '--merge-base=' + old['parents'][0],
                                     parent, before], env=env, capture_output=True)
            require(result.returncode == 0, 'lossless replay is unsupported or conflicts (exit ' +
                    str(result.returncode) + ')')
            tree = result.stdout.strip().decode('ascii')
            require(tree == textgit('rev-parse', '--verify', after + '^{tree}'),
                    'rewritten source tree differs from the known authorised replay')
            replayed.append(dict(original=before, rewritten=after, parent=parent, tree=tree))
            parent = after
    finally:
        private_files = {}
        for folder, _, names in os.walk(directory, followlinks=False):
            for name in names:
                path = pathlib.Path(folder) / name
                private_files[str(path.relative_to(directory))] = digest(path.read_bytes())
        observation(dict(boundary='lossless-replay-output', directory=str(directory),
                         private_object_files=private_files, validated_commits=replayed,
                         writes='Private Git metadata, refs and replay objects; no user index, worktree, refs or object-store writes.'))

try:
    action = sys.argv[1]
    if action == 'selectors':
        selectors()
    elif action == 'capture':
        binding = context(sys.argv[2])
        clean_index()
        snap = snapshot(binding)
        binding.update(invocation=uuid.uuid4().hex, candidate_digest=candidate_digest(snap))
        value = dict(binding=binding, **snap, skipped=False,
                     index_sha256=digest(pathlib.Path(binding['index_path']).read_bytes()))
        directory = private_dir()
        path = directory / 'capture.json'
        sha = retain(path, value)
        # The reviewer needs the finite candidates and outgoing history. Keep
        # the existing-object inventory private, rather than exporting it in a
        # recipe environment variable subject to the platform argv limit.
        public = {k:v for k,v in value.items() if k != 'initial_commits'}
        public.update(capture_file=str(path), capture_hash=sha)
        print(serial(public).decode())
    elif action == 'admit':
        phase = sys.argv[2]
        binding = context(phase)
        clean_index()
        cap = parse(os.environ.get('RECIPE_VAR_checkpoint_scope_capture', ''))
        require(isinstance(cap, dict), 'missing deterministic capture')
        path = pathlib.Path(cap['capture_file'])
        require(os.path.commonpath((canonical(str(path)), root)) != root, 'capture lies inside worktree')
        raw = path.read_bytes()
        require(digest(raw) == cap['capture_hash'], 'capture hash mismatch')
        saved = parse(raw)
        require({k:v for k,v in saved.items() if k != 'initial_commits'} ==
                {k:v for k,v in cap.items() if k not in ('capture_file', 'capture_hash')}, 'capture output mismatch')
        for key, value in binding.items():
            require(saved['binding'][key] == value, 'stale capture binding: ' + key)
        require(saved['index_sha256'] == digest(pathlib.Path(binding['index_path']).read_bytes()), 'initial index changed')
        require(saved['binding']['candidate_digest'] == candidate_digest(snapshot(binding, saved)), 'initial candidates changed')
        verdict = parse(os.environ.get('RECIPE_VAR_checkpoint_scope_review', ''))
        require(isinstance(verdict, dict) and set(verdict) == {'approved', 'binding', 'approved_paths', 'reason'}, 'malformed scope verdict')
        require(verdict['approved'] is True and verdict['binding'] == saved['binding'], 'scope review refused or mismatched')
        require(isinstance(verdict['reason'], str) and verdict['reason'].strip(), 'scope reason is required')
        approved = verdict['approved_paths']
        require(isinstance(approved, list), 'approved paths must be finite')
        decoded = [unb64(p) for p in approved]
        require(len(decoded) == len(set(decoded)), 'duplicate approved identity')
        captured = {unb64(p['path_b64']) for p in saved['paths']}
        require(set(decoded) == captured, 'approval must cover all pending and outgoing candidates')
        record = dict(binding=saved['binding'], approved_paths=approved, capture=saved, verdict=verdict)
        directory = private_dir()
        path = directory / 'authority.json'
        sha = retain(path, record)
        # The shell retains these values independently. No sidecar checksum is
        # read on continuation, and observations are outside this hashed record.
        values = dict(WF_CHECKPOINT_RECORD=str(path), WF_CHECKPOINT_RECORD_ID=file_identity(path), WF_CHECKPOINT_HASH=sha,
                      WF_CHECKPOINT_INVOCATION=saved['binding']['invocation'], WF_CHECKPOINT_HEAD=binding['initial_head'],
                      WF_CHECKPOINT_UPSTREAM=saved['publication']['upstream'],
                      WF_CHECKPOINT_UPSTREAM_REF=saved['publication']['upstream_ref'],
                      WF_CHECKPOINT_BASE_REF=saved['publication']['base_ref'])
        for value in values.values():
            sys.stdout.buffer.write(os.fsencode(value) + b'\0')
    elif action == 'destination-is-head':
        record = check()
        require(record['binding']['phase'] == 'publish', 'invalid destination decision phase')
        pub = publication(record['binding'], record['capture']['publication'])
        require(all(pub[key] == record['capture']['publication'][key]
                    for key in ('ref', 'remote', 'url_sha256')), 'publication destination binding changed')
        print('true' if pub['destination_tip'] == textgit('rev-parse', 'HEAD') else 'false')
    elif action == 'verify-pushed-ref':
        record = check()
        require(len(sys.argv) == 5, 'invalid publication verification input')
        helper, branch, expected = sys.argv[2:]
        require(record['binding']['phase'] == 'publish' and
                branch == record['binding']['publication_branch'] and
                expected == textgit('rev-parse', 'HEAD'), 'publication verification binding differs')
        require(os.path.basename(helper) == 'verify_pushed_ref.sh' and os.path.isfile(helper),
                'publication verification helper is unavailable')
        env, alias = publication_transport(publication(record['binding'], record['capture']['publication']))
        # Run the unchanged verifier against the same privately resolved push
        # destination used for ancestry inspection. Its argv has only an alias.
        result = subprocess.run(['bash', helper, alias, branch, expected], env=env, capture_output=True)
        # The alias is private transport preparation, not a new user-facing
        # remote name. Preserve the original verifier's origin/branch messages.
        sys.stdout.buffer.write(result.stdout.replace(os.fsencode(alias), b'origin'))
        sys.stderr.buffer.write(result.stderr.replace(os.fsencode(alias), b'origin'))
        sys.exit(result.returncode)
    elif action in ('check', 'paths', 'before'):
        record = check()
        if action == 'paths':
            # History-only deleted identities have nothing to stage. Pending
            # files remain a subset of the fixed authority, never an expansion.
            selected = pending()
            for path in sorted(selected):
                identity(path)
            sys.stdout.buffer.write(b''.join(p + b'\0' for p in sorted(selected)))
        elif action == 'before':
            require(len(sys.argv) >= 3 and sys.argv[2] in ('commit', 'push', 'pull', 'fetch', 'sync'),
                    'missing explicit checkpoint operation')
            if sys.argv[2] == 'push':
                push_selection(record, sys.argv[3:])
                verify_publication_destination(record, checked_publication)
            else:
                cleanup_sync = (sys.argv[2] == 'sync' and record['binding']['phase'] == 'cleanup' and
                                len(sys.argv) == 5 and sys.argv[3] == 'workflow_checkpoint_sync_body')
                require(len(sys.argv) == 3 or cleanup_sync, 'unexpected checkpoint operation arguments')
                if cleanup_sync:
                    require('refs/heads/' + sys.argv[4] == checked_publication['ref'] ==
                            'refs/heads/' + record['binding']['publication_branch'],
                            'sync callback reference differs from publication')
            token = dict(head=textgit('rev-parse', 'HEAD'), commits=sorted(commits()),
                         upstream=checked_publication['upstream'], invocation=record['binding']['invocation'])
            if sys.argv[2] == 'sync' and cleanup_sync:
                # Reuse check()'s fresh advertisement. Configuration-only URL
                # resolution honours Git's insteadOf/pushInsteadOf handling.
                fetch_urls = git('remote', 'get-url', '--all', 'origin').splitlines()
                fetch_hash = digest(fetch_urls[0]) if len(fetch_urls) == 1 else None
                no_upstream = (not checked_publication['upstream_ref'] and
                               not textgit('config', '--get', 'branch.' + record['binding']['branch'] + '.remote', optional=True) and
                               not textgit('config', '--get', 'branch.' + record['binding']['branch'] + '.merge', optional=True))
                facts = dict(operation='sync', callback='workflow_checkpoint_sync_body',
                             ref=checked_publication['ref'], fetch_url_sha256=fetch_hash,
                             push_url_sha256=checked_publication['url_sha256'],
                             initial_absent=record['capture']['publication']['destination_tip'] is None,
                             current_absent=checked_publication['destination_tip'] is None,
                             no_upstream=no_upstream,
                             same_destination=fetch_hash == checked_publication['url_sha256'])
                facts['mode'] = ('skip-absent' if all(facts[key] for key in
                                 ('initial_absent', 'current_absent', 'no_upstream', 'same_destination')) else 'fetch')
                token['cleanup_sync'] = facts
                observation(dict(boundary='sync-fetch-policy', mode=facts['mode'], ref=facts['ref'],
                                 same_destination=facts['same_destination'], current_ref_absent=facts['current_absent']))
            observation(dict(boundary='before-operation', head=token['head'], upstream=token['upstream']))
            path = pathlib.Path(os.environ['WF_CHECKPOINT_RECORD']).parent / ('operation-' + uuid.uuid4().hex + '.json')
            sha = retain(path, token)
            # The shell holds this compact descriptor across the operation.
            # The potentially large object inventory never passes through argv.
            print(serial(dict(path=str(path), sha256=sha, identity=file_identity(path))).decode())
    elif action == 'sync-fetch-mode':
        require(len(sys.argv) == 3, 'invalid sync operation descriptor')
        descriptor = parse(sys.argv[2])
        record, actual = load_authority()
        require(record['binding']['phase'] == 'cleanup', 'sync mode is only for cleanup')
        path = descriptor['path']
        require(isinstance(path, str) and os.path.dirname(path) == os.path.dirname(os.environ['WF_CHECKPOINT_RECORD']) and
                re.fullmatch(r'operation-[0-9a-f]{32}\.json', os.path.basename(path)),
                'sync operation is not in its original private directory')
        require(file_identity(path) == descriptor['identity'], 'sync operation identity changed')
        raw = pathlib.Path(path).read_bytes()
        require(digest(raw) == descriptor['sha256'], 'sync operation hash changed')
        token = parse(raw)
        require(token['invocation'] == record['binding']['invocation'] and
                token['head'] == actual['initial_head'] == os.environ['WF_CHECKPOINT_HEAD'],
                'stale sync operation descriptor')
        facts = token.get('cleanup_sync')
        keys = {'operation', 'callback', 'ref', 'fetch_url_sha256', 'push_url_sha256',
                'initial_absent', 'current_absent', 'no_upstream', 'same_destination', 'mode'}
        require(isinstance(facts, dict) and set(facts) == keys, 'malformed sync fetch policy')
        require(facts['operation'] == 'sync' and facts['callback'] == 'workflow_checkpoint_sync_body' and
                facts['ref'] == 'refs/heads/' + record['binding']['publication_branch'],
                'wrong sync operation, callback or reference')
        flags = ('initial_absent', 'current_absent', 'no_upstream', 'same_destination')
        require(all(type(facts[key]) is bool for key in flags), 'malformed sync policy flags')
        require(facts['push_url_sha256'] == record['capture']['publication']['url_sha256'] and
                (facts['fetch_url_sha256'] is None or
                 isinstance(facts['fetch_url_sha256'], str) and re.fullmatch('[0-9a-f]{64}', facts['fetch_url_sha256'])),
                'malformed sync destination binding')
        expected = 'skip-absent' if all(facts[key] for key in flags) else 'fetch'
        require(facts['mode'] == expected, 'inconsistent sync fetch policy')
        if expected == 'skip-absent':
            fetch_urls = git('remote', 'get-url', '--all', 'origin').splitlines()
            push_urls = git('remote', 'get-url', '--push', '--all', 'origin').splitlines()
            require(len(fetch_urls) == len(push_urls) == 1 and
                    digest(fetch_urls[0]) == facts['fetch_url_sha256'] ==
                    digest(push_urls[0]) == facts['push_url_sha256'], 'sync destination changed')
            require(not textgit('rev-parse', '--symbolic-full-name', '@{u}', optional=True) and
                    not textgit('config', '--get', 'branch.' + record['binding']['branch'] + '.remote', optional=True) and
                    not textgit('config', '--get', 'branch.' + record['binding']['branch'] + '.merge', optional=True),
                    'sync upstream changed')
        print(expected)
    elif action == 'after':
        descriptor = parse(sys.argv[2])
        require(file_identity(descriptor['path']) == descriptor['identity'], 'operation observation identity changed')
        token_bytes = pathlib.Path(descriptor['path']).read_bytes()
        require(digest(token_bytes) == descriptor['sha256'], 'operation observation hash changed')
        token = parse(token_bytes)
        op, rc = sys.argv[3], int(sys.argv[4])
        record, actual = load_authority()
        observation(dict(boundary='operation-returned', operation=op, command_exit=rc, starting_head=token['head']))
        require(token['invocation'] == record['binding']['invocation'], 'cross-invocation operation observation')
        if op == 'sync' and token.get('cleanup_sync', {}).get('mode') == 'skip-absent':
            require(record['binding']['phase'] == 'cleanup' and rc == 0 and
                    actual['initial_head'] == token['head'], 'skipped sync changed state')
            observation(dict(boundary='operation-skipped', operation='sync-fetch',
                             reason='first-publication ref absent on the same destination'))
        head = actual['initial_head']
        pub = publication(record['binding'], record['capture']['publication'])
        upstream = pub['upstream']
        tracking = dict(upstream_ref=os.environ['WF_CHECKPOINT_UPSTREAM_REF'], base_ref=os.environ['WF_CHECKPOINT_BASE_REF'])
        if op in ('sync', 'push') and not tracking['upstream_ref'] and pub['upstream_ref']:
            expected_ref = 'refs/remotes/origin/' + record['capture']['publication']['ref'].removeprefix('refs/heads/')
            require(pub['upstream_ref'] == expected_ref, 'new tracking reference differs from intended publication')
            tracking = dict(upstream_ref=expected_ref, base_ref=expected_ref)
        if op == 'commit':
            require(upstream == token['upstream'], 'commit changed the upstream identity')
        if op == 'push':
            require(upstream in (token['upstream'], head), 'push changed the upstream to an unrelated identity')
        new = commits() - set(token['commits'])
        if op in ('pull', 'fetch', 'sync'):
            if op in ('pull', 'sync'):
                require(record['binding']['phase'] in ('review', 'cleanup'), 'publish may not rebase or synchronise HEAD')
        if op == 'pull' and rc == 0:
            fetched = textgit('rev-parse', '--verify', 'FETCH_HEAD^{commit}')
            require(upstream == fetched, 'rebase upstream differs from fetched identity')
            require(textgit('merge-base', head, upstream) == upstream, 'rebase lacks fetched upstream ancestry')
            base = textgit('merge-base', token['head'], upstream)
            verify_replay(base, token['head'], upstream, head)
        if op == 'sync' and head != token['head']:
            require(head == upstream and textgit('merge-base', token['head'], upstream) == token['head'],
                    'head sync is not a fast-forward to the fetched publication reference')
        if op == 'commit' and rc == 0:
            row = commit_info(head)
            require(row['parents'] == [token['head']] and head in new, 'unexpected commit transition')
        elif op not in ('pull', 'sync') or rc != 0:
            require(head == token['head'], 'unexpected operation HEAD transition')
        scope_check(record, observed_upstream=upstream, observed_tracking=tracking, publication_view=pub)
        observation(dict(boundary='operation-checked', operation=op, command_exit=rc, head=head,
                         upstream=upstream, created_commits=sorted(new)))
        values = dict(WF_CHECKPOINT_HEAD=head, WF_CHECKPOINT_UPSTREAM=upstream,
                      WF_CHECKPOINT_UPSTREAM_REF=tracking['upstream_ref'], WF_CHECKPOINT_BASE_REF=tracking['base_ref'])
        for value in values.values():
            sys.stdout.buffer.write(os.fsencode(value) + b'\0')
    else:
        raise Refusal('unknown private checkpoint operation')
except (Refusal, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
    try:
        observation(dict(boundary='refusal', reason=str(error)))
    except OSError:
        pass
    print('checkpoint scope refusal: ' + str(error), file=sys.stderr)
    sys.exit(2)
CHECKPOINT_PY
}

workflow_checkpoint_check() {
  workflow_checkpoint_python check
}

workflow_checkpoint_stager_identity() {
  declare -f stage_workflow_changes workflow_stage_strict |
    python3 -I -c 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())'
}

workflow_checkpoint_stage() {
  workflow_checkpoint_check || return $?
  [ "$(workflow_checkpoint_stager_identity)" = "${WF_CHECKPOINT_STAGE_HASH:?strict helper identity required}" ] || {
    echo 'ERROR: checkpoint strict staging implementation changed; refusing broad fallback' >&2; return 78;
  }
  local list rc=0
  list="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-paths.XXXXXX")" || return $?
  if workflow_checkpoint_python paths >"$list"; then
    stage_workflow_changes --strict --path-list "$list" || rc=$?
  else rc=$?; fi
  rm -f -- "$list"
  [ "$rc" -eq 0 ] || return "$rc"
  workflow_checkpoint_check
}

# Read only the fixed private result fields. Validate the entire stream before
# assigning checkpoint state; values remain data even when they contain shell
# syntax, newlines or empty strings. These reads also work with Bash 3.2.
workflow_checkpoint_read_values() {
  local mode="$1" file="$2" record record_id record_hash invocation
  local head upstream upstream_ref base_ref trailing
  case "$mode" in admit|after) ;; *) return 78 ;; esac
  {
    if [ "$mode" = admit ]; then
      IFS= read -r -d '' record &&
        IFS= read -r -d '' record_id &&
        IFS= read -r -d '' record_hash &&
        IFS= read -r -d '' invocation || return 78
    fi
    IFS= read -r -d '' head &&
      IFS= read -r -d '' upstream &&
      IFS= read -r -d '' upstream_ref &&
      IFS= read -r -d '' base_ref || return 78
    if IFS= read -r -d '' trailing || [ -n "${trailing-}" ]; then return 78; fi
  } < "$file" || return 78
  if [ "$mode" = admit ]; then
    WF_CHECKPOINT_RECORD="$record"
    WF_CHECKPOINT_RECORD_ID="$record_id"
    WF_CHECKPOINT_HASH="$record_hash"
    WF_CHECKPOINT_INVOCATION="$invocation"
    export WF_CHECKPOINT_RECORD WF_CHECKPOINT_RECORD_ID WF_CHECKPOINT_HASH WF_CHECKPOINT_INVOCATION
  fi
  WF_CHECKPOINT_HEAD="$head"
  WF_CHECKPOINT_UPSTREAM="$upstream"
  WF_CHECKPOINT_UPSTREAM_REF="$upstream_ref"
  WF_CHECKPOINT_BASE_REF="$base_ref"
  export WF_CHECKPOINT_HEAD WF_CHECKPOINT_UPSTREAM WF_CHECKPOINT_UPSTREAM_REF WF_CHECKPOINT_BASE_REF
}

# The wrapper observes each real operation and its actual result. It never
# rolls an index, a worktree or a commit back after an authorised operation.
workflow_checkpoint_operation() {
  local op="$1" token rc=0 values_file python_rc=0 parse_rc=0 sync_mode
  shift
  values_file="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-values.XXXXXX")" || return 78
  if [ "$op" = push ]; then
    token="$(workflow_checkpoint_python before "$op" "$@")" || rc=$?
  elif [ "$op" = sync ] && [ "${1:-}" = workflow_checkpoint_sync_body ]; then
    token="$(workflow_checkpoint_python before "$op" workflow_checkpoint_sync_body "${remote_branch:-${current_branch:-}}")" || rc=$?
  else
    token="$(workflow_checkpoint_python before "$op")" || rc=$?
  fi
  if [ "$rc" -ne 0 ]; then
    rm -f -- "$values_file" || :
    [ "$rc" -ne 79 ] || return 79
    return 78
  fi
  if [ "$op" = sync ] && [ "${1:-}" = workflow_checkpoint_sync_body ]; then
    sync_mode="$(workflow_checkpoint_python sync-fetch-mode "$token")" || rc=$?
    if [ "$rc" -ne 0 ]; then
      rm -f -- "$values_file" || :
      [ "$rc" -ne 79 ] || return 79
      return 78
    fi
    case "$sync_mode" in
      fetch) "$@" || rc=$? ;;
      skip-absent) : ;; # No fetch ran; this is not recovery from a failure.
      *) rm -f -- "$values_file" || :; return 78 ;;
    esac
  else
    "$@" || rc=$?
  fi
  if [ "$rc" -eq 79 ]; then rm -f -- "$values_file" || :; return 79; fi
  workflow_checkpoint_python after "$token" "$op" "$rc" >"$values_file" || python_rc=$?
  if [ "$python_rc" -eq 0 ]; then
    workflow_checkpoint_read_values after "$values_file" || parse_rc=$?
  fi
  rm -f -- "$values_file" || :
  if [ "$rc" -ne 0 ]; then
    case "$op:${1:-}" in
      fetch:*|pull:*|sync:workflow_checkpoint_sync_body)
        # The strict cleanup sync propagates its nested fetch status and stops.
        # No later operation follows this failure. Keep its real status even
        # when missing post-failure metadata produces an additional refusal.
        [ "$python_rc" -ne 79 ] || return 79
        return "$rc" ;;
    esac
  fi
  if [ "$python_rc" -ne 0 ] || [ "$parse_rc" -ne 0 ]; then
    # A scope failure must stop retries even in contexts that disable errexit.
    [ "$python_rc" -ne 79 ] || return 79
    return 78
  fi
  return "$rc"
}

workflow_checkpoint_enter() {
  local values_file python_rc=0 parse_rc=0
  values_file="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-values.XXXXXX")" || return $?
  workflow_checkpoint_python admit "$1" >"$values_file" || python_rc=$?
  if [ "$python_rc" -eq 0 ]; then
    workflow_checkpoint_read_values admit "$values_file" || parse_rc=$?
  fi
  rm -f -- "$values_file" || :
  [ "$python_rc" -eq 0 ] || return "$python_rc"
  [ "$parse_rc" -eq 0 ] || return "$parse_rc"
  readonly WF_CHECKPOINT_RECORD WF_CHECKPOINT_RECORD_ID WF_CHECKPOINT_HASH WF_CHECKPOINT_INVOCATION
  export GIT_OPTIONAL_LOCKS=0 WF_CHECKPOINT_REQUIRED=1
  readonly WF_CHECKPOINT_REQUIRED
  . "${BASH_SOURCE[0]%/*}/workflow_stage_changes.sh"
  WF_CHECKPOINT_STAGE_HASH="$(workflow_checkpoint_stager_identity)" || return $?
  readonly WF_CHECKPOINT_STAGE_HASH
  git() {
    local operation="${1:-}"
    [ "$operation" != --no-pager ] || operation="${2:-}"
    case "$operation" in
      commit|push|pull|fetch)
        workflow_checkpoint_operation "$operation" command git "$@" ;;
      *) command git "$@" ;;
    esac
  }
}


# Sourcing this file provides only the private support functions.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then return 0; fi
if [ "${1:-}" = --scope-capture ]; then
  workflow_checkpoint_python capture "${2:?phase required}"
  exit $?
fi
workflow_checkpoint_python selectors || {
  scope_rc=$?
  python3 -I - <<'ADMISSION_RESULT_PY'
import json, os
try:
    branch = json.loads(os.environ.get('RECIPE_VAR_worktree_setup', '{}')).get('branch_name', '')
except (ValueError, AttributeError):
    branch = ''
print(json.dumps(dict(pushed='false', sha='', branch=branch, reason='scope-admission-refused')))
ADMISSION_RESULT_PY
  exit "$scope_rc"
}
set -euo pipefail
export GIT_PAGER=cat
export PAGER=cat
export LESS=FRX
WT_TOOL=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ] && { WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"; break; }; done
WORKTREE_SETUP_WORKTREE_PATH="$(bash "${WT_TOOL:?worktree_setup_path.sh not found}" step-15-commit-push)" || WORKTREE_SETUP_WORKTREE_PATH=""
# Issue #1528: stage and commit only at the top of the linked task worktree; no REPO_PATH fallback.
# Hard-fail rather than skip: a skipped commit here would lose work silently.
cd -P "${WORKTREE_SETUP_WORKTREE_PATH:?step-15 requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: step-15-commit-push requires a git repo at $(pwd); either \`git init\` or rerun from a checkout" >&2
  exit 1
fi
{ gd=$(git rev-parse --absolute-git-dir) && cd_=$(git rev-parse --path-format=absolute --git-common-dir) && [ "$gd" != "$cd_" ] && [ "$(git rev-parse --show-toplevel)" = "$(pwd -P)" ]; } 2>/dev/null || { echo "ERROR: step-15-commit-push: $(pwd -P) is not the top of a linked task worktree; refusing to stage or commit; never commits in REPO_PATH (#1528)" >&2; exit 1; }
# Security: refuse to commit/push on protected branches, even inside a linked worktree.
_CURRENT_BRANCH=$(git branch --show-current 2>/dev/null || echo "")
if [ "$_CURRENT_BRANCH" = "main" ] || [ "$_CURRENT_BRANCH" = "master" ]; then echo "ERROR: step-15 landed on protected branch '$_CURRENT_BRANCH' — refusing to commit. Worktree setup likely failed." >&2; exit 1; fi
workflow_checkpoint_enter publish || {
  scope_rc=$?
  python3 -I - <<'ADMISSION_RESULT_PY'
import json, os
try:
    branch = json.loads(os.environ.get('RECIPE_VAR_worktree_setup', '{}')).get('branch_name', '')
except (ValueError, AttributeError):
    branch = ''
print(json.dumps(dict(pushed='false', sha='', branch=branch, reason='scope-admission-refused')))
ADMISSION_RESULT_PY
  exit "$scope_rc"
}
echo "=== Step 15: Commit and Push ===" >&2
# FIX (#495): commit_result MUST be emitted on every exit path so step-16
# (PR creation) can be gated on the actual push outcome. Trap installed
# AFTER var init, BEFORE any failing command — reordering produces invalid JSON.
if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq is required by step-15 (commit_result emission)." >&2
  exit 2
fi
redact_sensitive_file() {
  sed -E 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g' "$1"
}
pushed="false"; sha=""; branch=""; reason="aborted"
_emit_commit_result() {
  jq -nc --arg pushed "$pushed" --arg sha "$sha" --arg branch "$branch" --arg reason "$reason" \
    '{pushed:$pushed,sha:$sha,branch:$branch,reason:$reason}'
}
trap _emit_commit_result EXIT
branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
echo "--- Staging Changes ---" >&2
RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_runtime_artifacts.sh}"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="$(pwd)/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found (searched WORKFLOW_RUNTIME_ARTIFACT_HELPER, AMPLIHACK_HOME, REPO_PATH, worktree, ~/.copilot, ~/.amplihack): $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
. "$RUNTIME_ARTIFACT_HELPER"
workflow_checkpoint_check || exit $?
preflight_known_workflow_runtime_artifacts .
# #1671: every step-15 helper comes from the bundle that supplied the runtime
# artifact helper above; the older search order is only the fallback.
GIT_IDENTITY_HELPER="${RUNTIME_ARTIFACT_HELPER:+${RUNTIME_ARTIFACT_HELPER%/*}/git-identity.sh}"; [ -n "$GIT_IDENTITY_HELPER" ] && [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }; . "$GIT_IDENTITY_HELPER"
# #1671: stage_workflow_changes is `git add -A` minus build artefacts, and a
# rejecting git hook goes to a fixing agent before the commit or push is retried.
WF_TOOLS="${RUNTIME_ARTIFACT_HELPER%/*}"; [ -f "$WF_TOOLS/workflow_hook_repair.sh" ] || WF_TOOLS="${GIT_IDENTITY_HELPER%/*}"; REMOTE_BRANCH="$(bash "$WF_TOOLS/workflow_remote_branch.sh" .)" || exit 2; [ "$REMOTE_BRANCH" != "$(git branch --show-current)" ] || REMOTE_BRANCH=""; SETUP_BRANCH="$(printf '%s' "${RECIPE_VAR_worktree_setup:-}" | jq -r '.branch_name // empty' 2>/dev/null)" || SETUP_BRANCH=""; [ -z "$REMOTE_BRANCH" ] || [ "$REMOTE_BRANCH" = "$SETUP_BRANCH" ] || { echo "ERROR: push target '$REMOTE_BRANCH' does not match worktree_setup.branch_name '$SETUP_BRANCH'; refusing to push (issue #1762)." >&2; exit 1; }  # #1762: an alias pushes to <B>
for wf_helper in "${WORKFLOW_STAGE_CHANGES_HELPER:-$WF_TOOLS/workflow_stage_changes.sh}" "${WORKFLOW_HOOK_REPAIR_HELPER:-$WF_TOOLS/workflow_hook_repair.sh}"; do [ -f "$wf_helper" ] || { echo "ERROR: workflow helper not found: $wf_helper" >&2; exit 2; }; . "$wf_helper"; done
amplihack hygiene artifact-guard --repo . --mode pre-publish
workflow_checkpoint_stage; amplihack hygiene artifact-guard --repo . --mode pre-publish
echo "--- Creating Commit ---" >&2
# FIX (#469/#311): env vars are injection-safe; ${VAR:-} nounset reads so an unset value can't abort before the #944 fallback below.
TASK_DESC="${TASK_DESCRIPTION:-}"
ISSUE_NUMBER="${ISSUE_NUMBER:-}"
# FIX (#944): commit prefix from step-14's real change type (PATCH->fix:,
# MINOR->feat:, MAJOR->feat!:), not a hardcoded 'feat:'. Unknown -> bugfix heuristic.
CHANGE_CLASS="$(printf '%s' "${RECIPE_VAR_version_bump:-}" | jq -r 'if type=="object" and has("change_classification") and .change_classification != null then .change_classification | tostring else empty end' 2>/dev/null)" || { echo "WARNING: step-15-commit-push: version_bump is not valid JSON; treated as missing (#1561)" >&2; CHANGE_CLASS=""; }
case "$(printf '%s' "$CHANGE_CLASS" | tr '[:lower:]' '[:upper:]')" in
  PATCH) COMMIT_PREFIX="fix: " ;;
  MINOR) COMMIT_PREFIX="feat: " ;;
  MAJOR) COMMIT_PREFIX="feat!: " ;;
  *)
    case "$(printf '%s' "$TASK_DESC" | tr '[:upper:]' '[:lower:]')" in
      *bug*|*bugfix*|*fix*) COMMIT_PREFIX="fix: " ;;
      *)                    COMMIT_PREFIX="feat: " ;;
    esac ;;
esac
COMMIT_TITLE=$(printf '%s%.72s' "$COMMIT_PREFIX" "$(printf '%s' "$TASK_DESC" | tr '\n\r' ' ' | head -1)")
# FIX #684: Host-aware commit references.
# FIX #1361: on GitHub the closing keyword is EARNED BY THE DIFF. PR #1283's
# body said `Closes #1277` while its whole change was one added planning
# document; had it merged, a genuine bug report would have been auto-closed
# by an artifact describing what somebody intended to do about it. A change
# that only writes down a plan gets `Refs`, which still links the PR to the
# issue but leaves it open for the work itself. Unreachable helper -> `Refs`:
# under-linking costs a manual close, over-linking closes a live bug silently.
HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
CLOSE_KW="Refs"; IR=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_issue_reference.sh" ] && { IR="$c/amplifier-bundle/tools/workflow_issue_reference.sh"; break; }; done
if [ -n "$IR" ]; then CLOSE_KW="$(git diff --cached --name-only | bash "$IR" --keyword)" || CLOSE_KW="Refs"; else echo "WARNING: workflow_issue_reference.sh not found (searched AMPLIHACK_HOME, REPO_PATH, cwd, ~/.copilot, ~/.amplihack) — using the non-closing 'Refs' keyword (#1361)" >&2; fi
case "$HOST_TYPE" in
  github) ISSUE_REF="${CLOSE_KW} #${ISSUE_NUMBER}"; ISSUE_IMPL="issue #${ISSUE_NUMBER}" ;;
  azdo)   ISSUE_REF="AB#${ISSUE_NUMBER}"; ISSUE_IMPL="AB#${ISSUE_NUMBER}" ;;
  *)      ISSUE_REF="Ref #${ISSUE_NUMBER}"; ISSUE_IMPL="issue #${ISSUE_NUMBER}" ;;
esac
COMMIT_MSG=$(printf '%s\n\nImplements %s\n\nChanges:\n- Implementation as per design specification\n- Tests added for new functionality\n- Documentation updated\n\n%s' "$COMMIT_TITLE" "$ISSUE_IMPL" "$ISSUE_REF")
# PERF: --quiet short-circuits on the first staged change.
if ! git diff --cached --quiet; then
  amplihack_prepare_git_commit_identity
  commit_output_file=$(mktemp -t step15-commit-XXXXXX)
  # A plain commit, as before #1671: PRE_COMMIT_ALLOW_NO_CONFIG is not set here.
  hook_repair_run commit step-15-commit-push --output "$commit_output_file" -- git commit -m "$COMMIT_MSG" || {
    commit_rc=$?
    # A commit that fails for any other reason reports commit-failed, not aborted (#1671).
    reason="$(hook_repair_step15_reason commit)" || { reason="commit-failed"; redact_sensitive_file "$commit_output_file" >&2; }
    rm -f "$commit_output_file"
    exit "$commit_rc"
  }
  redact_sensitive_file "$commit_output_file" >&2; rm -f "$commit_output_file"
else
  # FIX (#282): avoid hollow-success false-positive — check the current branch
  # for new commits (in any worktree) before declaring nothing was done.
  CURRENT_BRANCH=$(git branch --show-current)
  ALT_WORKTREE_COMMITS=0
  if [ -n "$CURRENT_BRANCH" ] && git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
    ALT_WORKTREE_COMMITS=$(git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0)
  fi
  if [ "$ALT_WORKTREE_COMMITS" -gt 0 ]; then
    echo "INFO: Nothing staged in ${WORKTREE_SETUP_WORKTREE_PATH:-(unset)}, but the" >&2
    echo "current branch '$CURRENT_BRANCH' is $ALT_WORKTREE_COMMITS commit(s) ahead of upstream." >&2
    git --no-pager log --oneline -"$ALT_WORKTREE_COMMITS" >&2
  else
    echo "INFO: Nothing staged to commit; continuing so already-pushed meaningful work can be published or classified." >&2
  fi
fi
echo "--- Pushing to Remote ---" >&2
# REL-002: Detect missing upstream tracking branch before rev-list
if ! git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
  echo "WARNING: No upstream tracking branch configured — skipping push" >&2
  reason="no-upstream"
  exit 0
fi
COMMITS_AHEAD=$(git rev-list --count @{u}..HEAD)
DESTINATION_IS_HEAD=$(workflow_checkpoint_python destination-is-head) || { reason="publication-destination-inspection-failed"; exit 1; }
if [ "$COMMITS_AHEAD" -ne 0 ] || [ "$DESTINATION_IS_HEAD" != true ]; then
  # FIX (#978): ancestry-aware integration — never blind `git pull --rebase`.
  # Refresh upstream, then decide by ancestry: behind==0 -> fast-forward push
  # (preserve tested commit identities); behind>0 -> fail closed with structured
  # merge-base/ahead/behind evidence. Never auto-replay already-integrated history.
  UPSTREAM_REF=$(git rev-parse --abbrev-ref '@{u}' 2>/dev/null || echo "unknown")
  fetch_output_file=$(mktemp -t step15-fetch-XXXXXX)
  if git fetch origin >"$fetch_output_file" 2>&1; then
    redact_sensitive_file "$fetch_output_file" >&2
  else
    fetch_rc=$?
    if [ "$fetch_rc" -eq 78 ] || [ "$fetch_rc" -eq 79 ]; then
      redact_sensitive_file "$fetch_output_file" >&2
      rm -f "$fetch_output_file"
      reason="fetch-policy-refusal"
      exit "$fetch_rc"
    fi
    echo "ERROR: git fetch origin failed before ancestry check (exit $fetch_rc); refusing publication" >&2
    redact_sensitive_file "$fetch_output_file" >&2
    rm -f "$fetch_output_file"
    reason="fetch-failed"
    exit "$fetch_rc"
  fi
  rm -f "$fetch_output_file"
  COMMITS_AHEAD=$(git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0)
  COMMITS_BEHIND=$(git rev-list --count 'HEAD..@{u}' 2>/dev/null || echo 0)
  DESTINATION_IS_HEAD=$(workflow_checkpoint_python destination-is-head) || { reason="publication-destination-inspection-failed"; exit 1; }
  if [ "$COMMITS_AHEAD" -eq 0 ] && [ "$DESTINATION_IS_HEAD" = true ]; then
    echo "INFO: HEAD no longer ahead of upstream '$UPSTREAM_REF' after fetch — nothing to push" >&2
    raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
    if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then sha="$raw_sha"; fi
    pushed="true"; reason="already-pushed"
    echo "=== Commit and Push Complete ===" >&2
    exit 0
  fi
  if [ "$COMMITS_BEHIND" -ne 0 ]; then
    MERGE_BASE=$(git merge-base HEAD '@{u}' 2>/dev/null || echo "none")
    HEAD_SHA=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
    echo "ERROR: step-15 refuses to rebase divergent history onto upstream '$UPSTREAM_REF' (issue #978)." >&2
    echo "  ancestry: ahead=$COMMITS_AHEAD behind=$COMMITS_BEHIND merge_base=$MERGE_BASE head=$HEAD_SHA upstream=$UPSTREAM_REF" >&2
    echo "  Auto-rebasing would replay already-integrated commits and create add/add conflicts." >&2
    echo "  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision." >&2
    reason="diverged-upstream: ahead=$COMMITS_AHEAD behind=$COMMITS_BEHIND merge_base=$MERGE_BASE upstream=$UPSTREAM_REF"
    exit 1
  fi
  echo "INFO: upstream '$UPSTREAM_REF' is an ancestor of HEAD — fast-forward push, no rebase (#978)" >&2
  # FIX (#495): capture push stderr, redact tokens before reason emission.
  push_stderr_file=$(mktemp -t step15-push-XXXXXX)
  # shellcheck disable=SC2064
  trap "rm -f \"$push_stderr_file\"; _emit_commit_result" EXIT
  if hook_repair_run push step-15-commit-push \
    --fix-message "fix: address {hook_ids} pre-push rejection${ISSUE_NUMBER:+ (#$ISSUE_NUMBER)}" \
    --output "$push_stderr_file" -- git push ${REMOTE_BRANCH:+origin "HEAD:$REMOTE_BRANCH"}; then
    redact_sensitive_file "$push_stderr_file" >&2  # kept for the step log; never printed raw (#978)
    # FIX (#1269): confirm the published commit is the tested one. A missing
    # helper warns, never fails -- see verify_pushed_ref.sh and #1268.
    V=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/verify_pushed_ref.sh" ] && { V="$c/amplifier-bundle/tools/verify_pushed_ref.sh"; break; }; done || true
    raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
    if [ -n "$V" ]; then workflow_checkpoint_python verify-pushed-ref "$V" "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha" || { reason="push-verify-failed"; exit 1; }; else echo "WARNING: verify_pushed_ref.sh not reachable; published commit unverified (#1269)" >&2; fi
    if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then sha="$raw_sha"; fi
    pushed="true"; reason="pushed"  # step-16 gate: only after verification
  else
    push_rc=$?
    # #1671: an unresolved hook rejection (hook-rejected-<status>:push:<ids>) or exit 79 (orchestration-unavailable) has its own reason.
    if reason="$(hook_repair_step15_reason push)"; then exit "$push_rc"; fi
    sanitized=$(redact_sensitive_file "$push_stderr_file" | tr '\n' ' ' | head -c 500)
    reason="push-failed: ${sanitized}"
    echo "ERROR: git push failed (exit $push_rc)" >&2
    redact_sensitive_file "$push_stderr_file" >&2
    exit "$push_rc"
  fi
else
  echo "INFO: Nothing to push - branch is already up to date with remote" >&2
  raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
  if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then
    sha="$raw_sha"
  fi
  pushed="true"
  reason="already-pushed"
fi
echo "=== Commit and Push Complete ===" >&2
