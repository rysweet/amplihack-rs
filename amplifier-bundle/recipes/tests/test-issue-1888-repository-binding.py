#!/usr/bin/env python3
"""Run the actual merge-loop preflight with a conflicting gh default."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import json

BUNDLE = Path(__file__).resolve().parents[2]

def commands(path):
    return re.findall(r'^    command: \|\n((?:      .*\n|\n)*)', path.read_text(), re.M)

def shell(block):
    return '\n'.join(line[6:] if line.startswith('      ') else line for line in block.splitlines())

with tempfile.TemporaryDirectory(prefix='repository-binding-') as tmp:
    base = Path(tmp)
    repo = base / 'repo'
    repo.mkdir()
    env = {k: v for k, v in os.environ.items() if not k.startswith(('GIT_', 'RECIPE_VAR_'))}
    env.update(TMPDIR=tmp, GH_REPO='decoy/other', AMPLIHACK_HOME=str(BUNDLE.parent),
               RECIPE_VAR_repository='owner/project', REPO_PATH=str(repo),
               RECIPE_VAR_repo_path=str(repo), AUTODRIVE_STATE_DIR=str(base / 'state'),
               PR_NUMBER='', CALLS=str(base / 'calls'))
    subprocess.run(['git', 'init', '-q', str(repo)], env=env, check=True)
    subprocess.run(['git', '-C', str(repo), 'remote', 'add', 'origin',
                    'https://github.com/owner/project.git'], env=env, check=True)
    bin_dir = base / 'bin'
    bin_dir.mkdir()
    subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Fixture',
                    '-c', 'user.email=fixture@example.invalid', '-c', 'core.hooksPath=/dev/null',
                    'commit', '--allow-empty', '-qm', 'fixture'], env=env, check=True)
    env['TARGET_HEAD'] = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], env=env, text=True).strip()
    env['TARGET_BRANCH'] = subprocess.check_output(['git', '-C', str(repo), 'branch', '--show-current'], env=env, text=True).strip()
    gh = bin_dir / 'gh' 
    gh.write_text('''#!/bin/bash
printf '%s %s\\n' "$GH_REPO" "$*" >> "$CALLS"
[ "$GH_REPO" = owner/project ] || { echo MERGED; exit 0; }
case "$*" in
  *--json\ headRefOid*) printf '%s\\t%s\\tOPEN\\n' "$TARGET_HEAD" "$TARGET_BRANCH" ;;
  *--json\ number*) echo 42 ;;
  *--json\ state*) printf '{"state":"%s","mergedAt":null}\\n' "$TARGET_STATE" ;;
  *) exit 99 ;;
esac
''')
    gh.chmod(0o755)
    # macOS chmod rejects GNU's trailing -- used by the existing state helper.
    chmod = bin_dir / 'chmod'
    chmod.write_text('#!/bin/bash\n[ "$1" = go-w ] && [ "$2" = -- ] && exec /bin/chmod go-w "$3"\nexec /bin/chmod "$@"\n')
    chmod.chmod(0o755)
    cli = bin_dir / 'amplihack'
    cli.write_text('''#!/bin/bash
if [ "$1 $2 $3" = 'orch helper extract-json' ]; then cat; exit 0; fi
[ "$1 $2 $3" = 'orch helper extract-field' ] || exit 98
shift 3
field=""; fallback=""
while [ $# -gt 0 ]; do
  case "$1" in --field) field="$2";; --default) fallback="$2";; *) exit 97;; esac
  shift 2
done
jq -r --arg field "$field" --arg fallback "$fallback" '.[$field] // $fallback'
''')
    cli.chmod(0o755)
    env['PATH'] = str(bin_dir) + ':' + env['PATH']
    recipe = BUNDLE / 'recipes/autodrive-merge-loop.yaml'
    preflight = shell(commands(recipe)[0])
    for state, expected in [('OPEN', 'true'), ('MERGED', 'false')]:
        env['TARGET_STATE'] = state
        result = subprocess.run(['bash', '-c', preflight], env=env, text=True, capture_output=True)
        assert result.returncode == 0, result.stderr
        data = json.loads(result.stdout)
        assert data['should_run'] == expected and data['pr_state'] == state, data
        assert all(line.startswith('owner/project ') for line in (base / 'calls').read_text().splitlines())
    # Explicit PR input must resolve the target head, never the decoy's head.
    env.update(PR_NUMBER='42', TARGET_STATE='OPEN')
    result = subprocess.run(['bash', '-c', preflight], env=env, text=True, capture_output=True)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)['repo'] == str(repo.resolve()), result.stdout
    assert 'headRefOid' in (base / 'calls').read_text()
    for target in ['',  'bad target', 'other/repo']:
        (base / 'calls').unlink(missing_ok=True)
        env['RECIPE_VAR_repository'] = target
        result = subprocess.run(['bash', '-c', preflight], env=env, text=True, capture_output=True)
        assert result.returncode != 0, (target, result.stdout)
        assert not (base / 'calls').exists(), target
    # Every recipe command must remain valid Bash after propagation edits.
    for name in ['auto-drive-light', 'auto-drive-to-merge', 'autodrive-build',
                 'autodrive-crusty-loop', 'autodrive-crusty-round',
                 'autodrive-merge-loop', 'autodrive-merge-round', 'autodrive-merge-evidence']:
        for block in commands(BUNDLE / 'recipes' / (name + '.yaml')):
            subprocess.run(['bash', '-n'], input=shell(block), text=True, check=True)
print('PASS: conflicting GH_REPO OPEN/MERGED preflights; empty/invalid/mismatched targets refuse before gh; recipe Bash syntax')
