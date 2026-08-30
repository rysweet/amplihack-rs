#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
umask 077

delete_key() {
  printf '%s\n' "$1" | docker compose exec -T litellm python -c '
import json
import os
import sys
import urllib.request

key = sys.stdin.read().strip()
request = urllib.request.Request(
    "http://127.0.0.1:4000/key/delete",
    data=json.dumps({"keys": [key]}).encode(),
    headers={
        "Authorization": "Bearer " + os.environ["LITELLM_MASTER_KEY"],
        "Content-Type": "application/json",
    },
)
with urllib.request.urlopen(request, timeout=30):
    pass
'
}

pending_revocation_file=.amplihack-api-key.previous
if [[ -f "$pending_revocation_file" ]]; then
  pending_key="$(cat "$pending_revocation_file")"
  delete_key "$pending_key"
  rm -f "$pending_revocation_file"
fi

key=
temporary_key_file=
backup_key_file=
rotation_committed=0
compose_exec=(docker compose exec -T)
for name in \
  AMPLIHACK_KEY_MAX_BUDGET \
  AMPLIHACK_KEY_BUDGET_DURATION \
  AMPLIHACK_KEY_REQUESTS_PER_MINUTE
do
  if [[ -v "$name" && -n "${!name}" ]]; then
    compose_exec+=(-e "$name")
  fi
done
cleanup_rotation() {
  if [[ -n "$temporary_key_file" ]]; then
    rm -f "$temporary_key_file"
  fi
  if [[ "$rotation_committed" == 0 ]]; then
    if [[ -n "$backup_key_file" ]]; then
      mv -f "$backup_key_file" .amplihack-api-key
    fi
    if [[ -n "$key" ]]; then
      delete_key "$key" ||
        printf 'Warning: failed to revoke uncommitted replacement key\n' >&2
    fi
  fi
}
trap cleanup_rotation EXIT

key="$(
  "${compose_exec[@]}" litellm python -c '
import json
import os
import urllib.request
import uuid

payload = json.dumps({
    "models": ["amplihack-default"],
    "key_alias": "amplihack-local-agent-" + uuid.uuid4().hex,
})
budget = os.environ.get("AMPLIHACK_KEY_MAX_BUDGET")
duration = os.environ.get("AMPLIHACK_KEY_BUDGET_DURATION")
if bool(budget) != bool(duration):
    raise SystemExit("AMPLIHACK_KEY_MAX_BUDGET and AMPLIHACK_KEY_BUDGET_DURATION must be set together")
if budget:
    budget_value = float(budget)
    if budget_value <= 0:
        raise SystemExit("AMPLIHACK_KEY_MAX_BUDGET must be positive")
    payload["max_budget"] = budget_value
    payload["budget_duration"] = duration
rpm = os.environ.get("AMPLIHACK_KEY_REQUESTS_PER_MINUTE")
if rpm:
    rpm_value = int(rpm)
    if rpm_value <= 0:
        raise SystemExit("AMPLIHACK_KEY_REQUESTS_PER_MINUTE must be positive")
    payload["rpm_limit"] = rpm_value
request = urllib.request.Request(
    "http://127.0.0.1:4000/key/generate",
    data=json.dumps(payload).encode(),
    headers={
        "Authorization": "Bearer " + os.environ["LITELLM_MASTER_KEY"],
        "Content-Type": "application/json",
    },
)
with urllib.request.urlopen(request, timeout=30) as response:
    print(json.load(response)["key"])
'
)"

temporary_key_file="$(mktemp .amplihack-api-key.XXXXXX)"
printf '%s\n' "$key" > "$temporary_key_file"

if [[ -f .amplihack-api-key ]]; then
  backup_key_file="$pending_revocation_file"
  mv .amplihack-api-key "$backup_key_file"
fi

mv "$temporary_key_file" .amplihack-api-key
temporary_key_file=

if [[ -n "$backup_key_file" ]]; then
  previous_key="$(cat "$backup_key_file")"
  if ! delete_key "$previous_key"; then
    rotation_committed=1
    trap - EXIT
    printf '%s\n' \
      "New key installed, but the previous key's revocation could not be confirmed." \
      "Re-run this script to retry revocation using $backup_key_file." >&2
    exit 1
  fi
  rotation_committed=1
  obsolete_backup="$backup_key_file"
  backup_key_file=
  rm -f "$obsolete_backup" ||
    printf 'Warning: revoked key remains in %s\n' "$obsolete_backup" >&2
else
  rotation_committed=1
fi

trap - EXIT
printf 'Restricted agent key written to %s/.amplihack-api-key\n' "$PWD"
