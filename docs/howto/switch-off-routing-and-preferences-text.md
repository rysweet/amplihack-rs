# Switch off the routing text and the repeated preferences table

amplihack adds two kinds of text to a session that you can switch off on
their own: the routing text, and the preferences table repeated on every
prompt event. Each has a key in `amplihack config` and an environment
variable for one launch. With nothing set, a session receives the same text
as before.

To omit every hook's context text at once, use `AMPLIHACK_HOOK_TEXT=none`
instead. See [Omit the text hooks add to model context](omit-hook-context-text.md).

## For one launch

```sh
AMPLIHACK_ROUTING_TEXT=off amplihack claude
AMPLIHACK_PROMPT_PREFERENCES=off amplihack claude
```

The same variables work with `amplihack copilot` and `amplihack codex`. The
session's hooks inherit the variable, so it applies to the whole session and
to child sessions started from it. A child launch without the variable takes
the value from the configuration files, or the default `on` when they do not
set the key:

```sh
env -u AMPLIHACK_ROUTING_TEXT amplihack claude
```

## For good

```sh
amplihack config set text.routing off
amplihack config set text.prompt_preferences off
```

This writes the user file. To set a key for one project only, write it in
the project's configuration file instead; see
[User configuration](../reference/user-config.md#textrouting-and-textprompt_preferences).
The variable takes precedence over the project file, and the project file
over the user file.

Check the value that the hooks will use:

```sh
amplihack config get text.routing
```

## What each setting leaves out

`text.routing = off` leaves out:

- the routing reminder on prompt events;
- the session-start section that sends development and investigation work
  to the orchestrator;
- the router lines of the launch contract appended to the system prompt.
  The contract keeps its rules on waiting and on stopping processes.

The text that tells a nested recipe session not to start a workflow stays.
`AMPLIHACK_NO_SYSTEM_PROMPT_APPEND` still drops the whole launch contract.

`text.prompt_preferences = off` leaves out the preferences table and the
learned-patterns line that the prompt-event hook adds. The preferences
inserted at session start stay. A recipe step never receives this table on
a prompt event, whatever the setting says.

## Values

Only `on` and `off` are values. An empty variable counts as unset. Any other
value, in the variable or in a file, leaves `on` in force. The hook writes
one escaped line about it on stderr and adds nothing to the model's context.

The launcher itself does not report an unknown value. A launch with
`AMPLIHACK_ROUTING_TEXT=OFF`, for example, appends the full launch contract
and prints nothing about the variable. Plain `amplihack config get
text.routing` prints only the value in force. Add `--show-origin` to see
where it came from, including a value in the variable that is not valid:

```sh
amplihack config get text.routing --show-origin
```

Run it when a setting seems to have no effect.

## Seeing why text was left out

The hooks log two further lines, at levels that the default log filter
drops:

- `text switch off`, at `info`, naming the key and where its value came
  from, each time a setting makes a hook leave text out. Leaving the
  preferences table out of a recipe step writes no line, because no
  setting decides it;
- `amplihack config file skipped`, at `warn`, when a configuration file
  cannot be read or parsed and its settings are not used.

The hook program writes its log to standard error and filters it with
`RUST_LOG`. With `RUST_LOG` unset, only errors are written, so neither
line appears in a normal session. To see them, set the variable for the
launch; the session's hooks inherit it:

```sh
RUST_LOG=amplihack_hooks=info amplihack claude
```

## Related

- [Hook reference: optional hook text suppression](../reference/hook-specifications.md#optional-hook-text-suppression)
- [System prompt append](../SYSTEM_PROMPT_APPEND.md#two-parts)
