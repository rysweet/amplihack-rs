#!/usr/bin/env bash
# workflow_pr_head_fetch.sh - is a PR head a fast-forward of the local HEAD?
# (issue #1714)
#
# Sourced, never executed. Defines two functions and changes nothing else.
# It is the one definition of "the PR head fast-forwards the local HEAD", used
# by every check that decides whether an open PR is this run's PR:
#   - the open-PR check, workflow_finalization_pr_signal.sh;
#   - workflow_pr_scope.sh --allow-head-ahead;
#   - the terminal-state probe (workflow-terminal-state.yaml) when its caller
#     sets accept_pr_head_ahead=true, which only workflow-finalize does.
# Keeping one definition stops the three from drifting apart.
#
# The PR head commit may be absent from the local object store, as when a
# merge of main was pushed from another checkout or with GitHub's "Update
# branch" button. One `git fetch origin refs/heads/<branch>` is then tried.
# It writes nothing but objects: an empty --refmap updates no
# remote-tracking ref, and --no-write-fetch-head leaves FETCH_HEAD alone. It
# cannot prompt or wait without limit, with or without `timeout(1)` (absent
# on stock macOS):
#   - stdin is /dev/null and GIT_TERMINAL_PROMPT=0, so git never asks for
#     HTTPS credentials;
#   - GIT_SSH_COMMAND is the caller's ssh command (GIT_SSH_COMMAND,
#     core.sshCommand, GIT_SSH or plain ssh) plus `-o BatchMode=yes
#     -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2`,
#     so ssh fails instead of asking for a key passphrase, a password or an
#     unknown host key on /dev/tty, and drops a dead connection after ~30s;
#   - http.lowSpeedLimit/lowSpeedTime abort an HTTPS transfer that stalls
#     below 1 KB/s for 30s;
#   - where `timeout` exists, the whole fetch is also capped at 60s.
# A failed fetch is not an error: if the commit is still missing, or
# merge-base fails for any reason, the answer is "no".
#
# Callers check everything that needs no commit (open, same repo, run
# branch, PR number and URL) before calling, so a PR that will be rejected
# anyway never costs a fetch.

# _finalization_batch_ssh_command <dir> <out_var> -> sets <out_var> to the
# ssh command git would use for <dir> (GIT_SSH_COMMAND, then core.sshCommand,
# then GIT_SSH, then ssh, git's own order) with options that make ssh fail
# rather than prompt, and give up on an unreachable or dead connection.
# Options on the command line take precedence over ~/.ssh/config. A
# non-OpenSSH GIT_SSH program (plink) rejects `-o`, so the fetch fails and
# the answer is "no": closed, never a prompt.
_finalization_batch_ssh_command() {
  local base="${GIT_SSH_COMMAND:-}" q=""
  [ -n "$base" ] || base="$(git -C "$1" config --get core.sshCommand 2>/dev/null || :)"
  if [ -z "$base" ] && [ -n "${GIT_SSH:-}" ]; then
    printf -v q '%q' "$GIT_SSH"; base="$q"
  fi
  [ -n "$base" ] || base="ssh"
  printf -v "$2" '%s' "$base -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2"
}

# _finalization_pr_head_descends <dir> <branch> <local_head> <pr_head>
#   -> 0 only when <pr_head> is a full commit id, present locally (after at
#   most one fetch), and <local_head> is its ancestor. Anything else is 1.
_finalization_pr_head_descends() {
  local dir="$1" branch="$2" local_head="$3" pr_head="$4" bound="" ssh_cmd=""
  [[ "$pr_head" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || return 1
  if ! git -C "$dir" cat-file -e "${pr_head}^{commit}" 2>/dev/null; then
    ! command -v timeout >/dev/null 2>&1 || bound="timeout 60"
    _finalization_batch_ssh_command "$dir" ssh_cmd
    GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" $bound git -C "$dir" \
      -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
      fetch --quiet --no-tags --no-write-fetch-head --refmap= \
      origin "refs/heads/$branch" </dev/null >/dev/null 2>&1 || :
    git -C "$dir" cat-file -e "${pr_head}^{commit}" 2>/dev/null || return 1
  fi
  git -C "$dir" merge-base --is-ancestor "$local_head" "$pr_head" 2>/dev/null
}
