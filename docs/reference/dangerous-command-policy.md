---
title: "Dangerous Command Policy"
description: "The single list of dangerous commands amplihack ships, and the deny configuration it writes for Claude Code, Codex and Copilot CLI from that list."
last_updated: 2026-11-16
review_schedule: quarterly
owner: rysweet
doc_type: reference
related:
  - ./user-config.md
  - ./hook-specifications.md
  - ./launch-flag-injection.md
  - ../concepts/dangerous-command-policy.md
  - ../howto/add-dangerous-command-deny-rules.md
---

# Dangerous Command Policy

amplihack ships one list of commands that an agent should not run, and writes
that list into each supported harness's own allow/deny mechanism. `amplihack
install`, `amplihack update` and profile setup all write it. No new hook is
added, and no harness declares its own copy of the list.

The policy sits beside, and does not replace, the `tmux_guard` pre-tool-use
hook described in [Hook Specifications](./hook-specifications.md#tmux-server-guard).
Where the two cover the same command, they are deliberately aligned; see
[Relationship to the tmux guard](#relationship-to-the-tmux-guard).

For why the policy uses each harness's own mechanism, see
[Why Dangerous Commands Are Denied in the Harness](../concepts/dangerous-command-policy.md).
To add your own entries, see
[Add Dangerous-Command Deny Rules](../howto/add-dangerous-command-deny-rules.md).

## Contents

- [The default list](#the-default-list)
- [Justification text](#justification-text)
- [Relationship to the tmux guard](#relationship-to-the-tmux-guard)
- [What each harness gets](#what-each-harness-gets)
  - [Claude Code and the Claude-compatible tools](#claude-code-and-the-claude-compatible-tools)
  - [Codex](#codex)
  - [Copilot CLI](#copilot-cli)
- [Instance additions](#instance-additions)
  - [Accepted prefixes](#accepted-prefixes)
- [When the writers run](#when-the-writers-run)
- [How the files are written](#how-the-files-are-written)
- [Install and update output](#install-and-update-output)
- [Uninstall](#uninstall)
- [Checking what was written](#checking-what-was-written)
- [What the policy does not stop](#what-the-policy-does-not-stop)
- [Verified behaviour and open checks](#verified-behaviour-and-open-checks)
- [Related](#related)

## The default list

Entries are rendered in this order, in every harness. The order is fixed, which
is what makes a second run byte-identical to the first.

Ten rendered prefixes come from six policy entries. The five push forms share
one entry, whose last position holds the five flags as alternatives; that entry
has one identifier and becomes one Codex rule, while Claude and Copilot, which
match text rather than tokens, get one prefix per alternative. The six
identifiers are `pkill`, `killall`, `pgrep`, `tmux-kill-server`,
`tmux-kill-session-all` and `git-push-force-or-delete`; they appear in the
Codex file's comments and in the tests. Count prefixes when reading the tables
below, and entries when reading the code.

| # | Command prefix | Why |
|---|----------------|-----|
| 1 | `pkill` | Pattern kills match processes the run did not start. |
| 2 | `killall` | Same, by process name. |
| 3 | `pgrep` | `pgrep -f` matches the shell that is running the search, so wait loops built on it never end. |
| 4 | `tmux kill-server` | Ends every tmux session on the host, including other people's. |
| 5 | `tmux kill-session -a` | Ends every session but the current one, which the run does not own. |
| 6 | `git push --force` | Discards commits on a shared branch. |
| 7 | `git push -f` | The short form of the same. |
| 8 | `git push --force-with-lease` | Still rewrites a pushed branch. |
| 9 | `git push --delete` | Removes a remote branch. |
| 10 | `git push -d` | The short form of the same. |

Two kinds of command are deliberately **not** denied.

Ordinary pushes run. `git push`, `git push -u origin <branch>` and
`git push origin HEAD:<branch>` share no prefix with prefixes 6 to 10, and the
Codex rules file carries them as `not_match` examples, which Codex checks when
it loads the file. The forced and deleting forms are named one flag at a time
because every harness matches a command prefix and none of them can express
"`git push` unless a flag follows".

Targeted tmux kills run. `tmux kill-session -t <name>` ends one named session
and is how [`amplihack recipe`](./recipe-command.md) runs and the
[profile rotator](../howto/claude-profile-rotation.md) are stopped. Prefix 5
denies only `-a`, which matches the `tmux_guard` hook's own line. Any
socket-scoped invocation (`tmux -S <socket> kill-server`, `tmux -L <name>
kill-server`) is outside the prefix as well, because tests and sandboxes need
it.

## Justification text

Each entry carries text saying what to do instead.

**Only Codex renders it.** The Codex execpolicy `prefix_rule` has a
`justification` field, and Codex prints it when it forbids a command. Claude
Code's `permissions.deny` entries are bare strings with nowhere to put one, and
Copilot CLI's `--deny-tool` is a flag with one argument. On those two the agent
gets the harness's own denial message, which names the rule that matched but
carries none of amplihack's text. The remedy there is the agent guidance, not
the deny entry; see
[Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md).

The pattern-kill entries (`pkill`, `killall`, `pgrep`) share this text:

> List processes with `ps -ef` (or `ps aux`) and read the list before stopping
> anything; stop only a PID this run started and recorded, with
> `cmd & pid=$!; wait $pid`, or a PID read from a pid file.

The text does not recommend `pgrep`, because `pgrep` is denied too. See
[What the policy does not stop](#what-the-policy-does-not-stop) for the cost of
that. Why a command-line pattern is the wrong thing to wait on at all (on a
shared machine it matches other runs, and `pgrep -f` matches the waiting shell
itself) belongs to the agent guidance rather than to a one-line justification;
see
[Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md).

The tmux entries say to name one session (`tmux kill-session -t <name>`) or to
pass a private socket (`tmux -S <socket>`). The push entries say to add a
commit instead of rewriting a pushed branch, and to remove a remote branch
through the review tool.

## Relationship to the tmux guard

amplihack already denies the dangerous tmux forms through the `tmux_guard`
pre-tool-use hook, which runs on Claude Code, Copilot and Codex alike. The
guard is structural: it tokenises the command, sees through `env`, `command`,
`exec`, `sudo`, `timeout` and absolute paths, follows `if-shell`,
`command-alias` and `send-keys`, and reads the global options to decide whether
the invocation is socket-scoped. A command prefix can do none of that. See
[Hook Specifications](./hook-specifications.md#tmux-server-guard) for the full rules.

Prefixes 4 and 5 are therefore a second layer, not the primary one. They are
kept for two reasons:

- The hook fails open. If the harness times out the hook, Claude Code and
  Copilot run the command as though it had been allowed. A deny entry in the
  settings file does not have that failure mode.
- The hook is installed by amplihack. A harness launched with amplihack's
  hooks absent or disabled still reads its own deny configuration.

The two are aligned on purpose: the guard allows `tmux kill-session -t foo`
and socket-scoped kills, and so does the policy. Where they differ, the guard
is wider. `env tmux kill-server`, `/usr/bin/tmux kill-server` and
`FOO=1 tmux kill-server` are outside every policy rendering and denied by the
guard. So is `tmux kill-session -t foo -a`, which puts `-a` after the target
and so is a prefix of nothing. The grouped forms `tmux kill-session -at x` and
`-aC` divide the renderings: Claude and Copilot match a command prefix as
text, so `-at` begins with `-a` and is matched; Codex compares whole tokens,
so `-at` is not `-a` and the Codex rule does not fire. The guard denies all
of them.

For tmux, therefore, the Claude path has `env`, absolute-path and
environment-variable-prefix forms closed by the hook rather than by the deny
entry. For `pkill`, `killall`, `pgrep` and `git push` nothing closes them; see
[What the policy does not stop](#what-the-policy-does-not-stop).

## What each harness gets

| Harness | Mechanism | Where | Written by |
|---------|-----------|-------|------------|
| Claude Code (`claude`) | `permissions.deny` in `settings.json` | `$CLAUDE_CONFIG_DIR/settings.json`, which is `$HOME/.claude` or a managed profile | install, update, profile setup and seeding, `amplihack launch` repair |
| `rusty`, `rustyclawd`, `amplifier` | the same file | the same file | the same writers |
| Codex | execpolicy rules file | `$CODEX_HOME/rules/amplihack-dangerous-commands.rules` | install, update, `amplihack codex` |
| Copilot CLI | `--deny-tool` arguments | the `copilot` command line | `amplihack copilot` and every other launch of the Copilot CLI |

`rusty`, `rustyclawd` and `amplifier` are
[Claude-compatible tools](./launch-flag-injection.md): amplihack launches each
of them with `CLAUDE_CONFIG_DIR` pinned to the home or profile directory it
manages, so all four read the same `settings.json` and get the same
`permissions.deny` entries. There is one writer and one file, not four.

### Claude Code and the Claude-compatible tools

The ten default prefixes are appended to `permissions.deny`, one entry each:

```json
{
  "permissions": {
    "defaultMode": "bypassPermissions",
    "deny": [
      "Bash(pkill:*)",
      "Bash(killall:*)",
      "Bash(pgrep:*)",
      "Bash(tmux kill-server:*)",
      "Bash(tmux kill-session -a:*)",
      "Bash(git push --force:*)",
      "Bash(git push -f:*)",
      "Bash(git push --force-with-lease:*)",
      "Bash(git push --delete:*)",
      "Bash(git push -d:*)"
    ]
  }
}
```

- Entries already present are not duplicated. Entries you added yourself stay,
  and every other key in `permissions` is left as it was.
- If `permissions` is not an object, or `permissions.deny` is not an array, the
  `permissions` value is left exactly as found and gets no entries at all.
  Nothing under `permissions` is overwritten, and the file is named in a
  warning on stderr. The same run still merges the hooks block, the status
  line and the backup, so the file as a whole may well change: a settings file
  that cannot take the entries is a file with no deny rules, and the run that
  met it does not claim Claude coverage. See *Loud on failure* under
  [How the files are written](#how-the-files-are-written), and
  [Install and update output](#install-and-update-output).

Two properties of Claude's own matcher decide how much this covers: whether a
deny entry still applies under `defaultMode: bypassPermissions`, which is what
amplihack writes into the same file, and whether a denied command is matched
when it appears inside a `&&`, `;`, `|` or `$(...)` chain rather than at the
head. The first was checked in a live session while this feature was built,
and the second was checked there for `&&` only; the remaining three separators
rest on the live testing reported in
[#2674](https://github.com/rysweet/amplihack-rs/issues/2674). Each claim, with
what it was checked against, is in
[Verified behaviour and open checks](#verified-behaviour-and-open-checks).
These are third-party behaviours that can change between releases, so they are
recorded with their provenance rather than assumed.

The same entries are written to each managed profile's `settings.json`, under
`~/.amplihack/claude-profiles/<name>/settings.json`. A profile directory with
no `settings.json` is left alone; amplihack does not create one. A profile
seeded from your home settings gets the entries from the seeding path, so a
profile created from a settings file written before this feature existed is
still covered.

### Codex

The Codex home (`$CODEX_HOME` if set and non-empty, otherwise `~/.codex`)
gets one file, `$CODEX_HOME/rules/amplihack-dangerous-commands.rules`. It holds
six rules, one per policy entry: Codex compares whole tokens, so the five push
forms stay one rule whose last position lists them as alternatives rather than
becoming five. Two of the six are shown:

```python
# amplihack: dangerous-command deny rules
# Written by amplihack from one policy list, and replaced on the next
# install or update. Keep your own rules in another file in this
# directory; amplihack never reads or replaces those.

# pkill
prefix_rule(
    pattern = ["pkill"],
    decision = "forbidden",
    justification = "List processes with `ps -ef` (or `ps aux`) and read the list before stopping anything; stop only a PID this run started and recorded, with `cmd & pid=$!; wait $pid`, or a PID read from a pid file.",
    match = ["pkill -f vitest", "pkill node"],
    not_match = ["ps -ef", "kill 1234"],
)

# git-push-force-or-delete
prefix_rule(
    pattern = ["git", "push", ["--force", "-f", "--force-with-lease", "--delete", "-d"]],
    decision = "forbidden",
    justification = "Add a new commit rather than rewriting a branch that has been pushed, and remove a remote branch through the review tool; plain `git push`, `git push -u origin <branch>` and `git push origin HEAD:<branch>` are unaffected.",
    match = ["git push --force", "git push -f origin main"],
    not_match = ["git push", "git push -u origin main", "git push origin HEAD:topic"],
)
```

The first line is the ownership marker. `amplihack uninstall` removes a file
at that path only when it starts with that line, so a symlink or a file
someone else wrote there is left alone.

The file is amplihack's own and is rewritten whole. Any other file in
`rules/`, including one you wrote by hand, is never read, rewritten or
removed. If `rules/` is missing, amplihack creates it at mode 0700 inside a
Codex home that is a real directory.

If `rules/` is there but is a symlink or is not a directory, the write is
refused and nothing is created, chmod-ed or written through it. The link's
target keeps the mode it had and receives no file.

`amplihack install` and `amplihack update` write the file whenever the Codex
home directory exists, whether or not the Codex plugin itself is selected, so
a user who has a `~/.codex` but has never installed amplihack's Codex plugin
still gets it. `amplihack uninstall` removes it again on the same terms.

A launch writes it only when Codex is the client being launched. The implicit
re-stage that runs on every launch deliberately does not: an explicit
`amplihack claude` or `amplihack copilot` must leave a Codex home
byte-identical, which issue #1538 pins. The Claude entries have no such
restriction, because the file they go in is the one the launch repair already
owns.

Check a rule yourself with Codex's own validator, which prints JSON:

```console
$ codex execpolicy check --rules ~/.codex/rules/amplihack-dangerous-commands.rules pkill -f vitest
{"matchedRules":[{"prefixRuleMatch":{"matchedPrefix":["pkill"],"decision":"forbidden","justification":"List processes with `ps -ef` (or `ps aux`) and read the list before stopping anything; ..."}}],"decision":"forbidden"}
```

A command that no rule matches prints `{"matchedRules":[]}` and no
`decision`. Add `--pretty` to read the output on more than one line. The
output above is from codex-cli 0.166.0.

The `not_match` examples are checked when the file is parsed, so a rules file
that denied an ordinary push would not load at all:

```text
Error: failed to parse policy at amplihack-dangerous-commands.rules

Caused by:
    expected example to not match rule `PrefixRuleMatch { matched_prefix: ["git", "push", "--force"], ... }`: git push --force
```

Which command forms Codex applies the rule to, and whether it discovers
`$CODEX_HOME/rules/` without being pointed at it, are recorded in
[Verified behaviour and open checks](#verified-behaviour-and-open-checks).

### Copilot CLI

Copilot CLI saves approvals per project in `permissions-config.json`, and that
file holds allow entries only. GitHub documents no user-level file for command
deny rules; `--allow-tool` and `--deny-tool` apply to one session and are not
written back.

So amplihack applies the policy to the command line of every Copilot launch:

```sh
amplihack copilot

# amplihack spawns (argv, one element per line):
copilot
--deny-tool
shell(pkill:*)
--deny-tool
shell(killall:*)
--deny-tool
shell(pgrep:*)
--deny-tool
shell(tmux kill-server:*)
--deny-tool
shell(tmux kill-session -a:*)
--deny-tool
shell(git push --force:*)
--deny-tool
shell(git push -f:*)
--deny-tool
shell(git push --force-with-lease:*)
--deny-tool
shell(git push --delete:*)
--deny-tool
shell(git push -d:*)
```

amplihack spawns `copilot` directly, with no shell in between, so each value is
one argv element and holds no quotes. The quotes in
`--deny-tool 'shell(pkill:*)'` are shell syntax, needed only when you type the
flag at a prompt yourself; do not copy them into a config file.

The `:*` suffix is the prefix form GitHub documents for shell rules:
`shell(git:*)` matches `git push` but not `gitea`, while a bare
`shell(git status)` is documented as matching that text literally and so
would spare every invocation that carries arguments. Whether a bare stem
nevertheless matches invocations with arguments is
[not settled upstream](#verified-behaviour-and-open-checks); the suffixed
form is the one both GitHub pages agree on, so it is what amplihack sends.
One consequence: if you already pass `--deny-tool=shell(pkill)` yourself,
amplihack does not recognise it as the same rule and adds `shell(pkill:*)`
beside it. Both deny, so the duplicate is harmless.

These are not the only flags amplihack injects into a `copilot` launch:
`--allow-all` is injected as well, unless the caller passed an allow-all flag
of its own or set `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1`. The deny flags are
injected either way, including when the opt-out suppresses `--allow-all`. See
[Launch Flag Injection](./launch-flag-injection.md#-deny-tool).

How Copilot resolves a `--deny-tool` against that `--allow-all`, or against an
approval already saved in the project's `permissions-config.json`, is
Copilot's own behaviour. amplihack passes both sets of flags and reads neither
the outcome nor the saved approvals; nothing in amplihack enforces an order
between them, and no test here asserts one. Treat the Copilot rendering as
flags handed to a harness that documents a deny-over-allow order, not as a
stop amplihack performs.

- A `--deny-tool` you pass yourself for the same prefix is not repeated. Both
  spellings count: `--deny-tool shell(pkill:*)` as two arguments, and the
  joined `--deny-tool=shell(pkill:*)`. The comparison is on the exact
  `shell(<prefix>:*)` text, so `shell(pkill :*)`, `shell(pkill -f:*)` and a
  bare `shell(pkill)` are each a different value and both are passed.
- Because this runs at launch, a `copilot` you start by hand, outside
  `amplihack`, has no deny rules.
- The denied command names are visible in the process table, since they are
  command-line arguments. They are short command names, not task text. See
  [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md)
  for the rule this sits beside.

Whether Copilot resolves a `--deny-tool` over a session approval already
saved in the project's `permissions-config.json`, and whether it accepts a
multi-token prefix such as `shell(git push --force:*)` at all, are recorded in
[Verified behaviour and open checks](#verified-behaviour-and-open-checks).
Until both are settled this is the weakest of the three renderings, and the
only one that is rebuilt rather than stored.

## Instance additions

An instance adds entries through `policy.deny_commands` in
`~/.amplihack/config.toml`:

```toml
[policy]
deny_commands = ["shutdown", "systemctl stop", "docker rm -f"]
```

- Additions are appended after the defaults, in the order written, with
  duplicates removed.
- An addition cannot remove a default. There is no removal mechanism.
- The key is user-only. An entry in a project `.amplihack/config.toml` is
  reported and ignored, so a repository you clone cannot change what your
  agents may run.
- If the user config file is group-writable, world-writable or owned by
  somebody else, its user-only keys are ignored and reported. The defaults are
  still written.
- Additions reach every harness: Claude `permissions.deny`, the Codex rules
  file, and the Copilot launch flags. On Copilot they become process arguments
  visible to other users on the host.

Read the current value. `config get` prints the entries on one line, separated
by `, `, which is unambiguous because a comma is not an accepted character in
an entry:

```console
$ amplihack config get policy.deny_commands --show-origin
shutdown, systemctl stop, docker rm -f  # user (/home/alice/.amplihack/config.toml)
```

`amplihack config set` writes scalars only, so this key is set by editing the
file, or with `amplihack config edit`. See
[policy.deny_commands](./user-config.md#policydeny_commands).

### Accepted prefixes

Each entry is validated before any harness sees it. One entry that fails
validation is reported once and rejects the whole value, which reverts to
unset: a deny list quietly one entry shorter is worse than one refused with a
reason, because the shorter list is still believed. The shipped defaults live
in code and are written regardless.

| Rule | Value |
|------|-------|
| Characters | ASCII letters, digits and `. _ / @ = + -` |
| Separator | a single space between tokens |
| Tokens | 1 to 8 |
| Length | 1 to 128 bytes |

Quotes, backslashes, parentheses, commas, colons, semicolons, glob characters,
control characters and NUL are all rejected, because each of them is syntax in
at least one of the three renderings or in `config get` output.

One warning is printed for the whole value, and it never quotes what the file
held. The rejected entry is attacker-chosen text, so the diagnostic states the
requirement instead of echoing it; the same requirement is printed whether the
value was too long, held a non-string element, or held one entry with a
rejected character:

```text
warning: /home/alice/.amplihack/config.toml: policy.deny_commands: must be a TOML array of at most 64 command prefixes, each 1 to 8 words of ASCII letters, digits and the characters . _ - / @ = +
```

The line is the config diagnostic form, `<path>: <key>: <reason>` behind
`warning: `, and it names no index. Nothing tells you which entry failed, so
check the value against the table above entry by entry. `amplihack config get
policy.deny_commands` printing an empty line after such a warning is the
confirmation that the whole value reverted to unset.

## When the writers run

| Command | Claude home | Managed profiles | Codex | Copilot |
|---------|-------------|------------------|-------|---------|
| `amplihack install` | yes | yes | yes | states the per-launch mechanism |
| `amplihack update` | yes | yes | yes | states the per-launch mechanism |
| `amplihack profile add` / seeding | no | the new profile | no | no |
| `amplihack claude`, `amplihack copilot` | only if launch repair rewrites it | no | no | the deny flags |
| `amplihack codex` | only if launch repair rewrites it | no | yes | n/a |

The entries are written by `amplihack install`, `amplihack update` and
`amplihack profile add`. A launch writes them only if it already had to
repair the hooks block: launch repair returns early when the hooks block is
current, and drift is computed over that block alone, so a launch whose
hooks are up to date never reaches the settings writer. Empty
`permissions.deny` by hand and no launch restores it; run `amplihack
install` or `amplihack update`.

A launch repairs the home settings file, not the launched profile's. The
`--profile` flag sets `CLAUDE_CONFIG_DIR` for the child process only, and
launch repair reads amplihack's own environment; when `CLAUDE_CONFIG_DIR`
*is* exported into amplihack itself, an absent settings file there is not
created. Only `amplihack install` (which refreshes every managed profile)
or `amplihack profile add` (which seeds a new one) writes a profile's file.

The Codex row is the one exception, and it is deliberate. A launch of another
client leaves the Codex home byte-identical, which issue #1538 pins and its
integration tests check; only a Codex launch refreshes the rules file. An
explicit `amplihack install` or `amplihack update` writes it either way.

A Codex home that does not exist is not created; the run says so and writes
nothing. The Claude writer is the other way round: an explicit install
creates `~/.claude/settings.json` if it is absent, so a bare `$HOME` comes
out of `amplihack install` with the entries in place.

The table above describes the default global hook scope. An install that
chose repo-local scope writes `<repo>/.claude/settings.json` instead, which is
outside `$HOME`, and does not refresh the managed profiles at all, even though
the line it prints still mentions them. The output does not change with the
scope either. The writer is handed the same label whichever scope was chosen,
so the Claude line still reads "the home settings file" and a refusal there is
still labelled `Claude Code (home settings)`, while the file behind both is the
repo's. Everything else is confined to `$HOME`, the Codex home and the managed
profile root.

## How the files are written

- **Merged, never replaced.** The Claude writer appends missing entries to the
  array that is there. Your own entries and every unrelated key survive. The
  Codex rules file is amplihack's own and is rewritten whole.
- **Idempotent by content.** A second run leaves every written file
  byte-identical. The report does not distinguish the two: a run that added
  entries and a run that found them all already there print the same line.
- **Atomic, and private.** Each file is written to a temporary file in the same
  directory, created with `O_EXCL` and `O_NOFOLLOW` at mode 0600, synced, then
  renamed over the target. Those two flags are what makes the temporary file
  safe: a name already taken is an error rather than an overwrite, and a
  symlink at that name is never followed. Every file the policy writers touch
  therefore ends up mode 0600, the Claude `settings.json` included; a `rules/`
  directory amplihack creates is mode 0700. A symlinked, non-regular or
  foreign-owned target is left alone and reported.
- **The Codex home is checked before anything is created.** It is examined
  without following a link. A symlink there refuses the write, reported as
  `the Codex home is a symlink`, and a path that exists and is not a
  directory is refused the same way, `the Codex home is not a directory`. Neither case
  creates anything.
- **The `rules/` directory is checked the same way.** A symlink or a
  non-directory at `$CODEX_HOME/rules` refuses the write with `the Codex rules
  directory is a symlink` or `the Codex rules directory is not a directory`,
  and the link's target is neither given mode 0700 nor written into. The check
  is needed one level down as well as at the home, because the directory is
  created with `create_dir_all` and chmod-ed 0700, and both of those follow a
  link.
- **Loud on failure.** A policy write that does not succeed prints one line on
  stderr, and install still exits 0. Every writer uses
  the same sentence, which names the harness, the file in question and the
  reason, and nothing else. The two leading spaces below are the install
  section's own indent, not part of the sentence:

  ```text
    ⚠️  Codex: the dangerous-command deny rules were not written (permission denied); fix what that names, then run `amplihack install` again
    ⚠️  Codex: the dangerous-command deny rules were not written (the Codex home is a symlink); fix what that names, then run `amplihack install` again
    ⚠️  Codex: the dangerous-command deny rules were not written (the Codex rules directory is a symlink); fix what that names, then run `amplihack install` again
    ⚠️  Claude Code (home settings): the dangerous-command deny rules were not written (permissions is not a JSON object); fix what that names, then run `amplihack install` again
    ⚠️  Claude Code (profile work): the dangerous-command deny rules were not written (permissions.deny is not a JSON array); fix what that names, then run `amplihack install` again
  ```

  Four paths report this way: `amplihack install` and `amplihack update`
  (whose policy section carries the line), the per-profile refresh (reported
  through the same section), and profile seeding, which writes the line to
  stderr itself as it seeds, prefixed `amplihack: warning:` rather than
  indented under a marker, because `amplihack profile add` has no policy
  section to carry it. The launch-time repair is the fourth: it writes the
  same home settings file but says nothing about the policy, so a file it
  cannot write there is named by the next `amplihack install` or `amplihack
  update`.

  A file the merge refuses before the policy writer runs, whether for malformed
  JSON, a symlink, a hard link, or a foreign-owned or oversized
  `settings.json`, is reported the same way. It is a file that took no entries, not a file that
  does not exist, and the two must not read alike. The reason names a JSON
  key or an I/O error kind; it never quotes a value read out of the file.

  A deny list that is believed to be present and is not is worse than none, so
  the warning is never suppressed, and the same run does not also print a line
  claiming that harness was written.

## Install and update output

The section reports what this run actually wrote, one line per harness. A
harness whose files were refused does not get a line saying they were written,
and a harness there was nothing to write for is named rather than passed over:
a positive claim never outlives the write behind it.

"This run" is exact. The record each Claude writer leaves is cleared when the
policy phase *starts* as well as when the section is printed, so an outcome
from an earlier phase of the same process can never supply a line for a later
phase that wrote nothing.

Two in-process sequences make this concrete, and both stay inside one
thread, which is what the record is scoped to:

- A Codex launch reads the record before the launch repair writes the home
  settings file. The launch's own policy call takes whatever the record holds,
  and the settings write that follows records its outcome afterwards, where
  the next phase would find it. The launch branch therefore clears first.
- Profile seeding records as it seeds, and a test process that seeds and then
  installs on the same thread would see the seeding outcome if the install's
  write phase did not clear before `ensure_settings_json` runs. It does.

An `amplihack update` is not one of these. `update` re-executes the new
binary out of process, so nothing it does afterwards shares a record with the
run that started it.

On a machine with a Claude home, managed profiles and a Codex home:

```text
🛡️  Configuring dangerous-command deny rules:
  ℹ️  Claude Code: deny rules written to permissions.deny in the home settings file and in every managed profile that has one.
  ℹ️  Codex: deny rules written to the execpolicy rules file in its home.
  ℹ️  Copilot CLI: approvals are saved per project and there is no user-level file for command deny rules, so amplihack supplies the same policy as session arguments on each launch.
```

The Copilot line is unconditional, because its mechanism is the launch
arguments and there is nothing on disk to succeed or fail at. The other two
lines vary with what happened.

| What happened | What is printed |
|---------------|-----------------|
| Every file for that harness holds the entries, whether this run added them or found them | the ℹ️ line above |
| There is no harness home to write into | `↩️  Codex: no Codex home on this machine, so there was nothing to write; the rules are written when one appears.` |
| Some files took the entries and some were refused | `ℹ️  Claude Code: deny rules written where they could be; each file that was refused is named in the warnings below.` |
| Every file for that harness was refused | no ℹ️ line at all, only the warnings |

Each refused file adds one stderr line naming it, in the form shown under
[How the files are written](#how-the-files-are-written). Install still exits 0.

The lines are printed on every explicit `amplihack install` and `amplihack
update`, including a run where every file was already current: they describe
what the files hold, not what changed. The launch-time repair prints nothing
unless it has a warning, which it writes to stderr prefixed `amplihack:
warning:`.

No line carries a path or a count. A profile is named, because a warning that
does not say which profile cannot be acted on.

## Uninstall

`amplihack uninstall` removes the Codex rules file, and only that file: the
file's bytes must begin with the whole 41-character marker
`# amplihack: dangerous-command deny rules`, so a symlink, an unmarked regular
file, or a file whose first line merely begins `# amplihack:` and then says
something else, is left in place. It is removed whenever it is there, on the
same unconditional terms as the rest of the Codex cleanup.

The check is a prefix test on those 41 characters rather than a comparison of
the whole first line, so a first line that starts with the marker and then
carries further text of your own still counts as amplihack's file and is
removed.

The Claude entries are not removed. They sit in an array among your own
entries, and amplihack does not edit that array on the way out. Removing them
is a manual edit of `permissions.deny`. The asymmetry is deliberate: leaving a
deny entry behind is restrictive rather than permissive, so it cannot open
anything up.

## Checking what was written

```console
$ jq '.permissions.deny' ~/.claude/settings.json
[
  "Bash(pkill:*)",
  "Bash(killall:*)",
  "Bash(pgrep:*)",
  "Bash(tmux kill-server:*)",
  "Bash(tmux kill-session -a:*)",
  "Bash(git push --force:*)",
  "Bash(git push -f:*)",
  "Bash(git push --force-with-lease:*)",
  "Bash(git push --delete:*)",
  "Bash(git push -d:*)"
]

$ codex execpolicy check --rules ~/.codex/rules/amplihack-dangerous-commands.rules git push --force
{"matchedRules":[{"prefixRuleMatch":{"matchedPrefix":["git","push","--force"],"decision":"forbidden","justification":"..."}}],"decision":"forbidden"}

$ codex execpolicy check --rules ~/.codex/rules/amplihack-dangerous-commands.rules git push -u origin main
{"matchedRules":[]}
```

Check after a warning, and after any hand edit of a settings file. A file that
has been given a `permissions` value amplihack cannot merge keeps whatever
entries it already had and takes no new ones, so the list above is the only
thing that says which rules a harness is running.

For Copilot, read the command line of a running launch. The deny flags are
arguments, so they are in the process's own `cmdline`:

```console
$ tr '\0' '\n' < /proc/4713/cmdline | grep -c -- --deny-tool
10
```

## What the policy does not stop

Every harness matches a command prefix. A prefix matcher prevents mistakes; it
is not a boundary, and nothing here should be relied on as one.

| Escape | Example | Also caught by a hook? |
|--------|---------|------------------------|
| A nested shell | `sh -c 'pkill -f vitest'` | no |
| Interposition | `env pkill -f vitest`, `xargs pkill`, `command pkill` | tmux only |
| A path invocation | `/usr/bin/pkill -f vitest`, `./pkill -f vitest`, `busybox pkill` | tmux only |
| An environment-variable prefix | `FOO=1 pkill -f vitest` | tmux only |
| Flag reordering | `git push origin --force` | no |
| Refspec deletion | `git push origin :topic` | no |
| `git -c` prefixes | `git -c core.pager=cat push --force` | no |
| Grouped tmux flags, Codex only | `tmux kill-session -at x`, `-aC` | yes |
| An attached value, Codex only | `git push --force-with-lease=origin/main` | no |
| Grouped push flags, Codex only | `git push -fu origin main` | no |
| tmux flags after the target | `tmux kill-session -t foo -a` | yes |
| A socket-qualified tmux command | `tmux -S /tmp/s kill-server` | allowed on purpose |
| A shell alias or function | `alias k=pkill` | no |
| Copilot started by hand | `copilot` run outside `amplihack` | hooks still run |

"tmux only" means the `tmux_guard` pre-tool-use hook closes that form for
tmux commands, on Claude Code, Copilot and Codex alike, but nothing closes it
for `pkill`, `killall`, `pgrep` or `git push`. See
[Hook Specifications](./hook-specifications.md#tmux-server-guard).

"Codex only" marks the three rows where the harnesses disagree. Claude and
Copilot match the command *text*, so anything that starts with the prefix is
denied; Codex compares whole argv *tokens*, so a flag that carries its value
(`--force-with-lease=origin/main`) or that is grouped with another letter
(`-fu`, `-at`) is a different token and matches nothing. This matters most
for `--force-with-lease=<ref>`, which is the spelling git's own
documentation steers people towards. Checked against codex-cli 0.166.0: both
push forms above return `{"matchedRules":[]}`, while `git push --force`,
`git push -f origin main` and a bare `git push --force-with-lease` are
`forbidden`. It cannot be closed in the rules file, because the set of
attached values and flag groupings is open-ended. Only the two `git push` rows
are left open by it: the grouped tmux flags are denied by the tmux guard
regardless of how they are spelled.

The path-invocation row deserves a note, because `/` and `.` are accepted
characters in a policy entry and a reader may expect a prefix to follow the
binary rather than the text. It does not. `pkill` matches the word `pkill`;
`/usr/bin/pkill`, `./pkill` and `busybox pkill` are different prefixes and are
not denied. Writing the path forms as extra entries does not close the gap,
since the set of paths that reach a binary is open.

The path-invocation and nested-shell rows were measured, not assumed:
`codex execpolicy check` against the shipped file returns `{"matchedRules":[]}`
for both `/usr/bin/pkill -f x` and `bash -lc "pkill -f x"` on codex-cli
0.166.0. Issue #2589 records an earlier report that Codex caught those two
forms; on this version it does not, and the table above is what holds.

Two further costs:

- `pgrep` is denied for read-only listing as well. Use `ps -ef` and read the
  output. amplihack's own how-tos were rewritten to `ps -ef` when this policy
  landed.
- The socket-qualified tmux forms are deliberately outside the prefix, because
  tests and sandboxes need `tmux -S <socket> kill-server`. The same fact is an
  escape.

The agent guidance stays what it was: stop only a PID this run started and
recorded, wait for it with `wait $pid`, and never select a process by matching
its command line. See
[Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md).

## Verified behaviour and open checks

Each harness's matcher is third-party behaviour that can change between
releases, so the claims this page rests on are listed here with the version
they were checked against. The two Claude Code rows name the session rather
than a version, because the version that session ran was not recorded; treat
them as weaker than the Codex rows. A claim still open is one the
implementation must settle before the sentence that depends on it is final.

| Claim | Status |
|-------|--------|
| `codex execpolicy check` prints JSON; a forbidden match carries the matched prefix and justification, an unmatched command gives `{"matchedRules":[]}` | checked, codex-cli 0.166.0 |
| A `prefix_rule` with alternatives in `pattern` parses | checked, codex-cli 0.166.0 |
| A `not_match` example that does match makes the whole file fail to parse | checked, codex-cli 0.166.0 |
| The whole shipped rules file parses, and each entry's `match` and `not_match` commands decide as the table above says | checked, codex-cli 0.166.0 |
| Codex loads `$CODEX_HOME/rules/*.rules` without a `--rules` argument | checked, codex-cli 0.166.0 |
| Codex applies a rule to the absolute-path form (`/usr/bin/pkill`) | **no**, codex-cli 0.166.0: unmatched |
| Codex applies a rule to the `bash -lc "..."` form | **no**, codex-cli 0.166.0: unmatched |
| Codex matches a flag that carries its value (`--force-with-lease=<ref>`) or is grouped with another letter (`-fu`) | **no**, codex-cli 0.166.0: unmatched, and it cannot be closed in the rules file |
| Claude Code applies `permissions.deny` under `defaultMode: bypassPermissions` | checked, Claude Code session with `Bash(pgrep:*)` denied and `defaultMode: bypassPermissions` set: `pgrep --version` was denied. First reported in [#2674](https://github.com/rysweet/amplihack-rs/issues/2674) |
| Claude Code matches a denied prefix inside `&&`, `;`, `\|` and `$(...)` chains | checked for `&&` in the same session: `true && pgrep --version` was denied. The other three separators are still [#2674](https://github.com/rysweet/amplihack-rs/issues/2674) only |
| Copilot CLI treats `shell(<prefix>:*)` as a prefix rule rather than a literal | documented by GitHub for shell rules, not executed here; amplihack sends the suffixed form because it is the one both GitHub pages agree on |
| Copilot CLI matches a bare `shell(<prefix>)` against invocations with arguments | **not settled**: GitHub's CLI reference implies yes, its `permissions-config.json` reference documents literal matching, and [github/copilot-cli#1736](https://github.com/github/copilot-cli/issues/1736) reports the narrower behaviour. amplihack does not rely on it |
| Copilot CLI resolves `--deny-tool` over a saved project approval | open |
| Copilot CLI accepts a multi-token prefix in `shell(...)` | open |

The bypass-mode row carries more weight than the others. amplihack writes
`defaultMode: bypassPermissions` into the same file it writes the deny entries
into, and four of the clients it manages (`claude`, `rusty`, `rustyclawd` and
`amplifier`) read that file. If Claude Code did not consult `permissions.deny`
in that mode, the Claude rendering would cover nothing on any of them. It was
checked directly: in a session with `defaultMode: bypassPermissions` set and
`Bash(pgrep:*)` denied, `pgrep --version` was refused, as was
`true && pgrep --version`. That agrees with the live testing reported in
[#2674](https://github.com/rysweet/amplihack-rs/issues/2674). It is one
session on one Claude Code version, not a property of every version you may
run, so if you depend on the Claude rendering, repeat the check with one
denied command in a bypass-mode session and record the version here.

The discovery check was made by putting a deliberately unparseable `.rules`
file in `$CODEX_HOME/rules/` and starting `codex exec`: it stopped with
`Error loading rules:` naming that file, which it had not been pointed at.
That has a consequence beyond discovery: a malformed rules file in that
directory stops Codex from starting at all, which is why the renderer refuses
any character that is syntax in the file rather than escaping it.

The two absolute-path and nested-shell results are negative, and correct the
claim in the original issue that Codex catches those forms. They are listed
under [What the policy does not stop](#what-the-policy-does-not-stop).

If a check comes back negative, the sentence it supports is amended rather than
left standing: an open check is not an assumption this page is entitled to
make. The Copilot checks need a `copilot` binary, which is not a dependency of
the build.

## Related

- [Why Dangerous Commands Are Denied in the Harness](../concepts/dangerous-command-policy.md): the incidents behind the list and why it is not a new hook
- [Add Dangerous-Command Deny Rules](../howto/add-dangerous-command-deny-rules.md): adding your own entries and checking they arrived
- [User Configuration](./user-config.md#policydeny_commands): the `policy.deny_commands` key
- [Hook Specifications](./hook-specifications.md#tmux-server-guard): the `tmux_guard` hook the tmux entries are aligned with
- [Launch Flag Injection](./launch-flag-injection.md#-deny-tool): the `--deny-tool` arguments, and the Claude-compatible tool list
- [Wait Without Leaving Orphan Processes](../howto/wait-without-orphan-processes.md): what to do instead of a pattern kill or a wait loop
- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md): the related rule about what appears in a process's command line
