#!/usr/bin/env bash
# A failed or partial fetch cannot prepare a workspace for publication.
set -euo pipefail
git fetch --atomic origin --no-tags >&2
