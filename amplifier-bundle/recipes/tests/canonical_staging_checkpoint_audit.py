"""Narrow source audit for the ratified checkpoint extraction.

The caller retains the original 40-control inventory. These additional source
controls do not qualify a native runner or establish implementation acceptance.
"""
import copy
from pathlib import Path

CHECKPOINTS = {
    'workflow-publish': ('step-15-commit-push', 'workflow_publish_checkpoint.sh', 'publish'),
    'workflow-pr-review': ('step-18c-push-feedback-changes', 'workflow_review_checkpoint.sh', 'review'),
    'workflow-finalize': ('step-20b-push-cleanup', 'workflow_cleanup_checkpoint.sh', 'cleanup'),
}
ADDITIONAL_CONTROLS = [
    {'asset': asset, 'step_id': values[0] + '-scope-review', 'mutation': False,
     'classification': 'read-only task path scope review', 'native_verified': False}
    for asset, values in CHECKPOINTS.items()
] + [{'asset': 'hook-rejection-repair', 'step_id': 'fix-hook-rejection', 'mutation': True,
      'classification': 'repair mutation agent opt-out', 'native_verified': False}]


AUXILIARY_EXTRACTIONS = {
    'workflow-prep': (
        'step-00-workflow-preparation',
        'workflow_prepare_banner.sh',
        """PREP_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_prepare_banner.sh" ]; then PREP_TOOL="$c/amplifier-bundle/tools/workflow_prepare_banner.sh"; break; fi
done
[ -n "$PREP_TOOL" ] || { echo 'ERROR: workflow preparation helper not found' >&2; exit 2; }
bash "$PREP_TOOL"
""",
    ),
    'autodrive-crusty-round': (
        'step-01-round-context',
        'autodrive_round_context.sh',
        """bash "/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_round_context.sh"
""",
    ),
}


def restored_auxiliary_command(name, step, command, original):
    if name not in AUXILIARY_EXTRACTIONS or step != AUXILIARY_EXTRACTIONS[name][0]:
        return command
    _, helper, wrapper = AUXILIARY_EXTRACTIONS[name]
    assert command == wrapper, 'Unexpected auxiliary extraction wrapper'
    root = Path(__file__).resolve().parents[3]
    assert (root / 'amplifier-bundle/tools' / helper).read_bytes() == b'#!/usr/bin/env bash\n' + original.encode(), 'The complete extracted command changed'
    return original


EARLY_SYNC_LEGACY_CALL = 'bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh"\n'
EARLY_SYNC_STRICT_CALL = '# Owner-approved #1554 amendment: every executed fetch failure is terminal.\nsync_output_file=$(mktemp "${TMPDIR:?assigned TMPDIR required}/early-pr-sync.XXXXXX")\nsync_rc=0\nbash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh" --strict-fetch --probe-origin-ref >"$sync_output_file" 2>&1 || sync_rc=$?\nredaction_rc=0\nsed -E \'s|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\\1REDACTED@|g\' "$sync_output_file" >&2 || redaction_rc=$?\nrm -f -- "$sync_output_file" || :\n[ "$sync_rc" -eq 0 ] || exit "$sync_rc"\n[ "$redaction_rc" -eq 0 ] || exit "$redaction_rc"\n'


def restored_early_sync_command(command, original):
    # Restore one exact owner-approved call, then compare every other byte.
    assert command.count(EARLY_SYNC_STRICT_CALL) == 1, 'Missing exact early strict-sync status/redaction sequence'
    assert original.count(EARLY_SYNC_LEGACY_CALL) == 1
    restored = command.replace(EARLY_SYNC_STRICT_CALL, EARLY_SYNC_LEGACY_CALL, 1)
    assert restored == original, 'Early sync changed outside the approved call'
    return restored


def restored_early_sync_asset_text(name, text):
    if name != 'workflow-finalize':
        return text
    strict = ''.join('      ' + line if line.strip() else line
                     for line in EARLY_SYNC_STRICT_CALL.splitlines(keepends=True))
    legacy = '      ' + EARLY_SYNC_LEGACY_CALL
    assert text.count(strict) == 1, 'Missing exact early sync raw call'
    amended = '  # probe accepted as ahead and set a missing upstream. Executed failures stop.\n'
    original = '  # probe accepted as ahead and set a missing upstream. Never fails the run.\n'
    assert text.count(amended) == 1, 'Unexpected early sync comment delta'
    return text.replace(strict, legacy, 1).replace(amended, original, 1)


def restored_checkpoint_text(name, extracted_text):
    # Only the owner-approved URI userinfo redaction changes this legacy line.
    corrected = 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\\1REDACTED@|g'
    assert extracted_text.count(corrected) == 1
    extracted_text = extracted_text.replace(corrected, 's#(https?://)[^@[:space:]]+@#\\1REDACTED@#g')
    if name == 'workflow-publish':
        # The owner explicitly withdraws cached-fallback success in 17.116.
        current_failure = '\n'.join((
            '    echo "ERROR: git fetch origin failed before ancestry check (exit $fetch_rc); refusing publication" >&2',
            '    redact_sensitive_file "$fetch_output_file" >&2',
            '    rm -f "$fetch_output_file"',
            '    reason="fetch-failed"',
            '    exit "$fetch_rc"',
        ))
        original_failure = '\n'.join((
            '    echo "WARNING: git fetch origin failed before ancestry check; using cached upstream state" >&2',
            '    redact_sensitive_file "$fetch_output_file" >&2',
        ))
        assert extracted_text.count(current_failure) == 1, 'Missing exact nonzero fetch-failure exit'
        extracted_text = extracted_text.replace(current_failure, original_failure, 1)
        # These two decisions now inspect the same bound private destination as
        # normal push verification, while retaining the original branch logic.
        decision = 'DESTINATION_IS_HEAD=$(workflow_checkpoint_python destination-is-head) || { reason="publication-destination-inspection-failed"; exit 1; }'
        assert extracted_text.count(decision) == 2
        extracted_text = extracted_text.replace(decision + '\n', '')
        extracted_text = extracted_text.replace('if [ "$COMMITS_AHEAD" -ne 0 ] || [ "$DESTINATION_IS_HEAD" != true ]; then', 'if [ "$COMMITS_AHEAD" -ne 0 ]; then')
        extracted_text = extracted_text.replace('if [ "$COMMITS_AHEAD" -eq 0 ] && [ "$DESTINATION_IS_HEAD" = true ]; then', 'if [ "$COMMITS_AHEAD" -eq 0 ]; then')
        corrected = 'workflow_checkpoint_python verify-pushed-ref "$V" "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha"'
        original = 'bash "$V" origin "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha"'
        assert extracted_text.count(corrected) == 1, 'Missing bounded push-destination verification'
        extracted_text = extracted_text.replace(corrected, original)
    if name == 'workflow-pr-review':
        first_failure = ('    # Decision 17.116 permits this first-failure stop. Push retries are unchanged.\n'
                         '    if [ "$#" -eq 3 ] && [ "$1" = git ] && [ "$2" = pull ] && [ "$3" = --rebase ]; then return "$rc"; fi\n')
        assert extracted_text.count(first_failure) == 1, 'Missing bounded pull-failure policy'
        extracted_text = extracted_text.replace(first_failure, '', 1)
        corrected = 'redact_sensitive_file "$push_output_file"'
        assert extracted_text.count(corrected) == 2, 'Missing bounded push diagnostic redaction'
        extracted_text = extracted_text.replace(corrected, 'cat "$push_output_file"')
    if name == 'workflow-finalize':
        strict_sync = 'workflow_checkpoint_sync_body() {\nbash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh" --strict-fetch\n}\nsync_output_file=$(mktemp -t step20b-cleanup-sync-XXXXXX)\nif workflow_checkpoint_operation sync workflow_checkpoint_sync_body >"$sync_output_file" 2>&1; then\n  redact_sensitive_output < "$sync_output_file"\n  rm -f -- "$sync_output_file"\nelse\n  sync_rc=$?\n  redact_sensitive_output < "$sync_output_file" >&2\n  rm -f -- "$sync_output_file"\n  exit "$sync_rc"\nfi'
        prior_sync = 'workflow_checkpoint_sync_body() {\nbash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh"\n}\nworkflow_checkpoint_operation sync workflow_checkpoint_sync_body || exit $?'
        assert extracted_text.count(strict_sync) == 1, 'Missing exact strict sync capture/status/redaction sequence'
        extracted_text = extracted_text.replace(strict_sync, prior_sync, 1)
        assert extracted_text.count('|| workflow_cleanup_skip ') == 3
        extracted_text = extracted_text.replace('|| workflow_cleanup_skip ', '|| skip ')
    # Restore exactly the four separately authorised commit-output sites.
    # The unchanged comparison below still checks channel, order and status.
    commit_redactors = {
        'workflow-pr-review': 'redact_sensitive_file "$commit_output_file"',
        'workflow-finalize': 'redact_sensitive_output < "$commit_output_file"',
    }
    if name in commit_redactors:
        redactor = commit_redactors[name]
        assert extracted_text.count(redactor) == 2, 'Expected exactly two authorised commit-output substitutions'
        assert 'cat "$commit_output_file"' not in extracted_text, 'Raw commit-output emission remains'
        lines = extracted_text.splitlines(keepends=True)
        positions = []
        for suffix in ('', ' >&2'):
            corrected = '    ' + redactor + suffix + '\n'
            original = '    cat "$commit_output_file"' + suffix + '\n'
            assert lines.count(corrected) == 1, 'Missing exact commit-output channel substitution'
            positions.append(lines.index(corrected))
            lines[positions[-1]] = original
        assert positions[0] < positions[1], 'Commit-output success/rejection order changed'
        extracted_text = ''.join(lines)
    return extracted_text


def without_checkpoint_delta(name, current, baseline):
    """Remove only approved additions after checking the complete extraction."""
    normalised = copy.deepcopy(current)
    if name in AUXILIARY_EXTRACTIONS:
        sid = AUXILIARY_EXTRACTIONS[name][0]
        old_step = next(step for step in baseline['steps'] if step['id'] == sid)
        selected = next(step for step in normalised['steps'] if step['id'] == sid)
        selected['command'] = restored_auxiliary_command(name, sid, selected['command'], old_step['command'])
    if name == 'workflow-finalize':
        early = next(step for step in normalised['steps'] if step['id'] == 'step-20-sync-pr-head')
        before = next(step for step in baseline['steps'] if step['id'] == 'step-20-sync-pr-head')
        early['command'] = restored_early_sync_command(early['command'], before['command'])
    if name in ('workflow-pr-review', 'workflow-finalize'):
        assert 'version_bump' not in baseline['context']
        assert normalised['context'].pop('version_bump') == ''
    if name not in CHECKPOINTS:
        return normalised
    sid, helper, phase = CHECKPOINTS[name]
    old = {step['id']: step for step in baseline['steps']}
    new = {step['id']: step for step in current['steps']}
    assert len(new) == len(current['steps']), 'Duplicate phase step identity'
    capture_id, review_id = sid + '-scope-capture', sid + '-scope-review'
    assert set(new) - set(old) == {capture_id, review_id}, 'Unexpected new phase step'
    capture, review = new[capture_id], new[review_id]
    assert capture.get('type') == 'bash' and 'agent' not in capture
    assert capture.get('output') == 'checkpoint_scope_capture' and capture.get('parse_json') is True
    assert '--scope-capture ' + phase in capture['command']
    assert review.get('auto_stage') is False
    assert review.get('working_dir') == '{{worktree_setup.worktree_path}}'
    assert review.get('output') == 'checkpoint_scope_review' and review.get('parse_json') is False
    positions = list(new)
    assert positions.index(capture_id) < positions.index(review_id) < positions.index(sid)
    assert helper in new[sid]['command']
    root = Path(__file__).resolve().parents[3]
    extracted_text = (root / 'amplifier-bundle/tools' / helper).read_text()
    extracted_text = restored_checkpoint_text(name, extracted_text)
    extracted = extracted_text.splitlines()
    extracted = [line.strip() for line in extracted]
    cursor = 0
    for raw in old[sid]['command'].splitlines():
        line = raw.strip()
        if not line or line.startswith(('stage_workflow_changes', 'git add ')):
            continue
        cursor = extracted.index(line, cursor) + 1
    normalised['steps'] = [step for step in normalised['steps'] if step['id'] not in (capture_id, review_id)]
    selected = next(step for step in normalised['steps'] if step['id'] == sid)
    selected['command'] = old[sid]['command']
    return normalised
