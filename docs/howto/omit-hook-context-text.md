# Omit the text hooks add to model context

Use `AMPLIHACK_HOOK_TEXT=none` when a session needs less hook context:

```sh
AMPLIHACK_HOOK_TEXT=none amplihack codex
```

The same setting works with `amplihack claude` and `amplihack copilot`.
The launcher prints a notice on stderr. It adds no notice to model context.

SessionStart, UserPromptSubmit and the workflow classification reminder still
run their normal bookkeeping. Their `additionalContext` fields, top-level and
under `hookSpecificOutput`, are removed from the response. A top-level
`systemMessage` is kept: Claude Code shows it to the person and does not add
it to the model's context, so in Claude Code capacity notices stay visible
while their model context is left out. Under Codex no capacity notice is
shown, with or without the setting. Stop approves without running lock
mode, power steering or reflection. Use this setting only when those Stop
functions are unnecessary for the session.

PreToolUse command enforcement and the StopFailure limit-resume hook behave
as usual. Other hooks, control decisions, diagnostics and failure responses
keep their existing behaviour.

Only the exact value `none` enables suppression. Leaving the variable unset
keeps existing behaviour silently. Any other explicit value, including an
empty value, keeps existing behaviour and produces an escaped diagnostic on
stderr when a participating hook runs.

Child sessions inherit the variable. A Claude, Copilot or ordinary Codex
launch inside a session with this setting also skips those Stop functions
and reports that on stderr. To restore the default for a child launch:

```sh
env -u AMPLIHACK_HOOK_TEXT amplihack claude
```

The setting changes hook output. It does not remove project instructions,
skills, plugins or the agent's own instructions.

See the [hook reference](../reference/hook-specifications.md#optional-hook-text-suppression)
for the response fields and participating hooks.
