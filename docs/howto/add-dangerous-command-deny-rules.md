---
title: "Add Dangerous-Command Deny Rules"
description: "Add your own commands to amplihack's dangerous-command policy, write them into every harness, and check what arrived."
last_updated: 2026-11-15
review_schedule: quarterly
owner: rysweet
doc_type: howto
related:
  - ../reference/dangerous-command-policy.md
  - ../reference/user-config.md
  - ../concepts/dangerous-command-policy.md
---

# Add Dangerous-Command Deny Rules

amplihack ships a default list of commands agents should not run, and writes it
into Claude Code, Codex and Copilot CLI. This guide covers adding your own
entries to that list, applying them, and checking what each harness received.

For the default list and the exact file formats, see
[Dangerous Command Policy](../reference/dangerous-command-policy.md).

## Contents

- [Add an entry](#add-an-entry)
- [Apply it](#apply-it)
- [Check what each harness got](#check-what-each-harness-got)
- [Remove an entry you added](#remove-an-entry-you-added)
- [Troubleshooting](#troubleshooting)
  - [An entry was reported and ignored](#an-entry-was-reported-and-ignored)
  - [Nothing was written at all](#nothing-was-written-at-all)
  - [A profile has no entries](#a-profile-has-no-entries)
  - [A denied command still runs](#a-denied-command-still-runs)

## Add an entry

Entries live under `policy.deny_commands` in your own config file,
`~/.amplihack/config.toml`. `amplihack config set` writes scalar values only,
so open the file:

```sh
amplihack config edit
```

Add the table and the list:

```toml
[policy]
deny_commands = ["shutdown", "systemctl stop", "docker rm -f"]
```

Each entry is a command prefix: ASCII letters, digits and `. _ / @ = + -`,
with single spaces between at most eight tokens, up to 128 bytes. Quotes,
backslashes, parentheses, globs and shell metacharacters are rejected, because
each is syntax in at least one harness's rule format.

Your entries are added to the defaults. They never remove one; there is no
removal mechanism for the shipped entries.

The key is user-only. Putting it in a project `.amplihack/config.toml` has no
effect, and amplihack reports it, so a repository you clone cannot change what
your agents may run.

## Apply it

```sh
amplihack update
```

```text
🛡️  Configuring dangerous-command deny rules:
  ℹ️  Dangerous-command deny rules written from amplihack's own policy list: Claude Code reads them from permissions.deny in settings.json, and Codex from an execpolicy rules file in its home.
  ℹ️  Copilot CLI saves approvals per project and keeps no user-level file for command deny rules, so amplihack supplies the same policy as session arguments on each launch.
```

The Claude writer covers the home settings file and every managed profile that
already has a `settings.json`. `amplihack install` writes the same files, and
prints the same two lines whether or not anything changed. Copilot needs no
write step: the flags are built on each `amplihack copilot` launch, so a new
entry applies to the next launch without running install.

`rusty`, `rustyclawd` and `amplifier` read the same Claude settings file, so
they are covered by the Claude line and have no step of their own.

## Check what each harness got

Read the list amplihack is using:

```console
$ amplihack config get policy.deny_commands --show-origin
shutdown, systemctl stop, docker rm -f  # user (/home/alice/.amplihack/config.toml)
```

`config get` prints the value once, so `--show-origin` adds one trailing
comment for the whole list. The entries are separated by `, `, which is
unambiguous because a comma is not an accepted character in an entry.

Claude Code, home and one profile:

```console
$ jq -r '.permissions.deny[]' ~/.claude/settings.json | tail -3
Bash(shutdown:*)
Bash(systemctl stop:*)
Bash(docker rm -f:*)

$ jq -r '.permissions.deny[]' ~/.amplihack/claude-profiles/work/settings.json | tail -3
Bash(shutdown:*)
Bash(systemctl stop:*)
Bash(docker rm -f:*)
```

Codex, using its own validator:

```console
$ codex execpolicy check --rules ~/.codex/rules/amplihack-dangerous-commands.rules shutdown -h now
{"matchedRules":[{"prefixRuleMatch":{"matchedPrefix":["shutdown"],"decision":"forbidden","justification":"..."}}],"decision":"forbidden"}
```

Copilot CLI, by reading the command line of a running launch (the deny flags
are arguments of the `copilot` process itself):

```console
$ tr '\0' '\n' < /proc/4713/cmdline | grep -A1 -- --deny-tool | tail -2
--deny-tool
shell(docker rm -f)
```

## Remove an entry you added

Delete it from the list in `~/.amplihack/config.toml` and run
`amplihack update`.

Removing it from the config removes it from the Codex rules file, which
amplihack rewrites whole, and from the Copilot flags, which are rebuilt on each
launch. It does **not** remove it from `permissions.deny` in any Claude
settings file: amplihack appends to that array and never prunes it, so your own
entries are safe from amplihack and amplihack's old entries stay until you
delete them:

```sh
jq '.permissions.deny -= ["Bash(docker rm -f:*)"]' ~/.claude/settings.json > /tmp/s.json \
  && mv /tmp/s.json ~/.claude/settings.json
```

## Troubleshooting

### An entry was reported and ignored

```text
warning: /home/alice/.amplihack/config.toml: policy.deny_commands: must be a TOML array of at most 64 command prefixes, each 1 to 8 words of ASCII letters, digits and the characters . _ - / @ = +
```

One bad entry rejects the whole value rather than being dropped from it, so
the key reverts to unset and only the shipped defaults are written. The
message does not quote the entry, because the file is untrusted input. Write
each entry as a bare command prefix, without quotes, globs or shell
operators:

| Instead of | Write |
|------------|-------|
| `"rm -rf /*"` | `rm -rf` |
| `"sudo systemctl stop *"` | `sudo systemctl stop` |
| `"kill -9 $(pgrep x)"` | `kill -9` |

The defaults are always written: they live in code, not in this file, so a
bad line here cannot switch the policy off.

### Nothing was written at all

If the whole key was ignored, check the config file's ownership and mode.
`policy.deny_commands` is user-only, and user-only keys are ignored when the
file is group-writable, world-writable or owned by somebody else:

```console
$ ls -l ~/.amplihack/config.toml
-rw-rw-r-- 1 alice alice 184 Nov 15 09:12 /home/alice/.amplihack/config.toml

$ chmod 600 ~/.amplihack/config.toml
```

If a write itself did not succeed, install prints one line on stderr and still
exits 0:

```text
  ⚠️  Codex: the dangerous-command deny rules were not written (permission denied); fix what that names, then run `amplihack install` again
```

Fix the cause the line names, then run `amplihack update` again.

### A profile has no entries

amplihack writes to managed profiles that already have a `settings.json`. It
does not create one for a profile that has none. Launch the profile once, or
seed it, and the entries arrive:

```sh
amplihack claude --profile work
jq -r '.permissions.deny[]' ~/.amplihack/claude-profiles/work/settings.json | head -3
```

### A denied command still runs

The harnesses match a command prefix, so these forms are not matched:

```sh
sh -c 'pkill -f vitest'        # nested shell
env pkill -f vitest            # interposition
git push origin --force        # flags after the positional argument
git push origin :topic         # refspec deletion
tmux -S /tmp/s kill-server     # socket-qualified, deliberately allowed
copilot                        # started by hand, outside amplihack
```

This is a known limit, not a defect: the policy prevents mistakes and is not a
boundary. The full list is in
[What the policy does not stop](../reference/dangerous-command-policy.md#what-the-policy-does-not-stop).
For stopping processes safely, see
[Wait Without Leaving Orphan Processes](./wait-without-orphan-processes.md).

## Related

- [Dangerous Command Policy](../reference/dangerous-command-policy.md): the default list, the file formats and the limits
- [Why Dangerous Commands Are Denied in the Harness](../concepts/dangerous-command-policy.md): why this is harness configuration rather than a hook
- [User Configuration](../reference/user-config.md#policydeny_commands): the `policy.deny_commands` key, its scope and its trust rules
- [Manage Claude Login Profiles](./claude-profiles.md): the managed profiles the Claude writer covers
