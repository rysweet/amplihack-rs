# Recipe Executor Environment

Reference for environment variables and context injected by the recipe
executor into shell and agent steps.

## Shell Step Environment Variables

Every shell recipe step receives these environment variables:

| Variable          | Value                  | Behavior            |
|-------------------|------------------------|---------------------|
| `HOME`            | Caller's `$HOME`       | Preserved from caller; falls back to `/root` |
| `PATH`            | Caller's `$PATH`       | Preserved from caller; falls back to standard paths |
| `NONINTERACTIVE`  | `1`                    | Overrides unconditionally |
| `DEBIAN_FRONTEND` | `noninteractive`       | Overrides unconditionally |
| `CI`              | `true`                 | Overrides unconditionally |

**Design rationale**: `HOME` and `PATH` preserve inherited values so that
user-installed tools and configuration remain accessible. The non-interactive
flags override unconditionally because recipe steps must never prompt for
input.

### Implementation

See `amplihack_recipe::executor::shell_step_env()` in
`crates/amplihack-recipe/src/executor.rs`.

## Agent Step Context Augmentation

Agent recipe steps receive a `Working Directory Context` block prepended to
their prompt:

```
## Working Directory Context
- **Path**: /absolute/path/to/working/directory
- **Files**: Cargo.toml, src, tests, README.md

Write all output files to this directory unless the task specifies otherwise.
```

If the agent prompt already contains the working directory path (or the
string `working_directory`), the context block is not injected to avoid
duplication.

### Implementation

See `amplihack_recipe::executor::AgentContext` in
`crates/amplihack-recipe/src/executor.rs`.

## Shell Prerequisite Validation

Before executing a shell step, the executor parses the command for references
to known tools and checks their availability via `which`:

**Known tools**: `python3`, `python`, `pip3`, `pip`, `node`, `npm`, `npx`,
`cargo`, `rustc`, `go`, `java`, `dotnet`, `ruby`, `gem`

When a tool is referenced but not found on `PATH`, execution fails with a
clear error message:

```
Shell step requires tool(s) not found on PATH: python3.
Install the missing tool(s) or adjust the recipe step.
```

### Implementation

See `amplihack_recipe::executor::validate_shell_prerequisites()` in
`crates/amplihack-recipe/src/executor.rs`.

## Interaction with NONINTERACTIVE

The `NONINTERACTIVE=1` variable is a convention understood by many tools:

- **apt-get**: Suppresses interactive prompts (combined with `DEBIAN_FRONTEND`)
- **npm**: Skips interactive setup wizards
- **Homebrew**: Suppresses interactive confirmation
- **amplihack hooks**: Skip interactive classification prompts

See also: [How to run in non-interactive mode](../howto/run-in-noninteractive-mode.md)
