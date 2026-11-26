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
to child sessions started from it. To restore the default for a child launch:

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

## Related

- [Hook reference: optional hook text suppression](../reference/hook-specifications.md#optional-hook-text-suppression)
- [System prompt append](../SYSTEM_PROMPT_APPEND.md#two-parts)
