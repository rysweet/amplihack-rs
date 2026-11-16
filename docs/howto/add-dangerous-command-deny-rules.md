---
title: "Add Dangerous-Command Deny Rules"
description: "Add your own commands to amplihack's dangerous-command policy, write them into every harness, and check what arrived."
last_updated: 2026-11-16
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
  - [A harness got no line](#a-harness-got-no-line)
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
  ℹ️  Claude Code: deny rules written to permissions.deny in the home settings file and in every managed profile that has one.
  ℹ️  Codex: deny rules written to the execpolicy rules file in its home.
  ℹ️  Copilot CLI: approvals are saved per project and there is no user-level file for command deny rules, so amplihack supplies the same policy as session arguments on each launch.
```

Read the lines: each one is a statement about what this run wrote. A harness
with nothing to write into is named instead (`↩️  Codex: no Codex home on this
machine …`), and a harness whose files were refused gets a warning on stderr
rather than a line saying its rules are in place.

The Claude writer covers the home settings file and every managed profile that
already has a `settings.json`. `amplihack install` writes the same files and
prints the same lines whether or not anything changed. Copilot needs no write
step: the flags are built on each `amplihack copilot` launch, so a new entry
applies to the next launch without running install.

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

If a write itself did not succeed, install prints one line on stderr per file
and still exits 0:

```text
  ⚠️  Codex: the dangerous-command deny rules were not written (permission denied); fix what that names, then run `amplihack install` again
  ⚠️  Claude Code (home settings): the dangerous-command deny rules were not written (permissions is not a JSON object); fix what that names, then run `amplihack install` again
```

A Codex line can also read `(the Codex home is a symlink)` or `(the Codex home
is not a directory)`. amplihack will not write through a link at `$CODEX_HOME`,
nor change its target's mode. Replace the link with a real directory, or point
`CODEX_HOME` at the directory the link was aiming for, then run `amplihack
update` again.

The `rules/` directory inside it is checked the same way, and reports `(the
Codex rules directory is a symlink)` or `(the Codex rules directory is not a
directory)`. So if you keep `rules/` as a link into a dotfiles checkout, no
rules file is written and the link's target is left exactly as it was. Point
`CODEX_HOME` at the real directory instead, or replace the link:

```console
$ readlink ~/.codex/rules     # empty once rules/ is a real directory
$ ls -l ~/.codex/rules/amplihack-dangerous-commands.rules
```

The second form means the settings file holds a `permissions` value amplihack
will not merge into, so that file has no amplihack deny entries. Open it and
make `permissions` an object whose `deny` is an array of strings:

```console
$ jq '.permissions' ~/.claude/settings.json
"bypassPermissions"

$ jq '.permissions = {"defaultMode": "bypassPermissions", "deny": []}' ~/.claude/settings.json > /tmp/s.json \
  && mv /tmp/s.json ~/.claude/settings.json
```

The same warning appears from `amplihack profile add` when the profile being
seeded has such a file, named `Claude Code (new profile <name>)`; the
per-profile refresh during an install names the same file
`Claude Code (profile <name>)`.

Fix the cause the line names, then run `amplihack update` again and check that
the harness's ℹ️ line is back.

### A harness got no line

Each harness gets one line, and the absence of one is information. Two forms
say nothing was written:

```text
  ↩️  Codex: no Codex home on this machine, so there was nothing to write; the rules are written when one appears.
```

amplihack does not create a harness home it did not find. Install or start
Codex once so that `~/.codex` exists (or set `CODEX_HOME` to the directory you
use), then run `amplihack update`.

```text
🛡️  Configuring dangerous-command deny rules:
  ℹ️  Codex: deny rules written to the execpolicy rules file in its home.
  ℹ️  Copilot CLI: approvals are saved per project …
  ⚠️  Claude Code (home settings): the dangerous-command deny rules were not written (permissions is not a JSON object); …
```

No Claude line means no Claude file took the entries. A Claude line reading
"written where they could be" means some did and some did not, and the
warnings name each file that was refused. Either way, treat the harness as
having no amplihack deny rules until the warning is gone.

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
/usr/bin/pkill -f vitest       # path invocation; ./pkill and busybox pkill too
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
