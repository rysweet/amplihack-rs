---
title: "Check Agent Processes for Prompt Text"
description: "Confirm that no running amplihack-launched agent has contract, prompt or brief text in its command line, and fix the launch failures an older Claude Code causes."
last_updated: 2026-11-07
review_schedule: quarterly
owner: rysweet
doc_type: howto
related:
  - ../reference/agent-prompt-channels.md
  - ../concepts/prompt-text-out-of-argv.md
  - ../SYSTEM_PROMPT_APPEND.md
---

# Check Agent Processes for Prompt Text

Use this guide to confirm that the agents on a host have no prompt text in
their command lines, and to fix a launch that fails the Claude Code version
check. For the full list of channels, see
[Agent Prompt Channels Reference](../reference/agent-prompt-channels.md).

## Check every running agent

The contract contains the phrase `tail -f /dev/null`. No agent command line
should contain it:

```bash
for pid in $(pgrep -x claude; pgrep -x codex; pgrep -x copilot); do
  printf '%s %s\n' "$pid" "$(grep -c 'tail -f /dev/null' /proc/$pid/cmdline)"
done
```

Expected output: every count is `0`.

```text
412233 0
412871 0
413002 0
```

A count of `1` means that process was started by an amplihack build from
before issue #2030, or by something other than amplihack. Check its parent and
start time:

```bash
ps -o pid,ppid,lstart,args -p 412233 | cut -c1-160
```

## Check one session's contract file

```bash
tr '\0' '\n' < /proc/412233/cmdline | grep -A1 -- '--append-system-prompt-file'
stat -c '%a %U %F' ~/.amplihack/private/system-prompt-append.md ~/.amplihack/private
```

Expected output:

```text
--append-system-prompt-file
/home/alice/.amplihack/private/system-prompt-append.md
600 alice regular file
700 alice directory
```

If `AMPLIHACK_HOME` is set, the file is under that directory instead of
`~/.amplihack`.

## Fix "needs Claude Code 2.1.319 or later"

```text
error: amplihack passes its operating contract with --append-system-prompt-file, which needs Claude Code 2.1.319 or later; found 2.1.318.
```

1. Upgrade Claude Code, then check the version:

   ```bash
   claude --version
   ```

2. If you cannot upgrade yet, launch without the contract:

   ```bash
   AMPLIHACK_NO_SYSTEM_PROMPT_APPEND=1 amplihack claude
   ```

   The session then runs without amplihack's operating contract. amplihack
   does not fall back to passing the contract as text.

If the message says amplihack could not read a version, run the binary the
message names with `--version </dev/null` yourself (for a plain install that is
`claude --version </dev/null`). It must print a version such as
`2.1.319 (Claude Code)` and exit within 10 seconds.

## Fix "is not a directory owned by you"

`<home>/private` is a symlink, a regular file or owned by another user.
amplihack refuses to write the contract there. Look at it before removing it:

```bash
ls -ld ~/.amplihack/private
rm -rf ~/.amplihack/private   # only if it is yours and holds nothing else you need
amplihack claude
```

amplihack creates it again with mode 0700.

## Pass a task prompt without argv from a script

`amplihack claude -- -p "<task>"` keeps the task out of the `claude` child's
argv, but the text is still in the `amplihack` process's argv because your
script put it there. To keep it out of both, use auto mode with a prompt file:

```bash
gh issue view 2030 --json body --jq .body > /var/tmp/issue-2030.md
amplihack claude --auto --max-turns 20 --prompt-file /var/tmp/issue-2030.md
```

Or pipe it on stdin with a bare `-p`:

```bash
gh issue view 2030 --json body --jq .body | amplihack claude -- -p
```

## Related

- [Agent Prompt Channels Reference](../reference/agent-prompt-channels.md) - Every launch path and its channel
- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The incident and the threat model
- [System Prompt Append](../SYSTEM_PROMPT_APPEND.md) - The contract and its opt-out
