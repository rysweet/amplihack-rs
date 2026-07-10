# Skill-to-Agent Redirect API Reference

Developer reference for the Skill-to-Agent Redirect guardrail in the `amplihack-hooks` crate. It documents the `known_agents` registry and the `PreToolUse` redirect helper that intercepts `Skill` calls naming an agent-only target.

> [Home](../index.md) > [Reference](../index.md) > Skill-to-Agent Redirect API

See the [feature overview](../features/skill-to-agent-redirect.md) for the conceptual model and behavior matrix.

## Module Layout

| File                                            | Role                                                                                  |
| ----------------------------------------------- | ------------------------------------------------------------------------------------- |
| `crates/amplihack-hooks/src/known_agents.rs`    | Compile-time registry of amplihack agent names + membership API.                      |
| `amplifier-bundle/skills/**/SKILL.md`           | Runtime source of truth for skill identity — scanned by `bundled_skill_names()` for precedence (issue #863). No hardcoded skill registry. |
| `crates/amplihack-hooks/src/pre_tool_use/mod.rs`| `check_skill_redirect()` helper wired into the `PreToolUse` `process()` flow.          |
| `crates/amplihack-hooks/src/lib.rs`             | Registers `pub mod known_agents;`.                                                     |

---

## `known_agents` Module

A filesystem-free, compile-time registry of amplihack agent names.

### Data

```rust
/// All built-in amplihack agent names. Keep sorted for `binary_search`.
static AMPLIHACK_AGENTS: &[&str] = &[
    "ambiguity",
    "amplifier-cli-architect",
    "amplihack-improvement-workflow",
    "analyzer",
    "api-designer",
    "architect",
    // ... 41 unique names total, sorted ...
    "worktree-manager",
    "xpia-defense",
];
```

The registry contains the **41 unique** agent names sourced from `amplifier-bundle/agents/**/*.md`. The duplicate `guide.md` basename is deduplicated so the slice stays unique and sorted.

### Functions

```rust
/// Returns true if `name` is a known amplihack agent (case-sensitive, exact).
pub fn is_amplihack_agent(name: &str) -> bool;

/// Returns the number of registered agent names.
pub fn agent_count() -> usize;
```

- `is_amplihack_agent` is a `binary_search` over the static slice — O(log n), no allocation, no regex, no path or shell use.
- Matching is **exact and case-sensitive**. `"prompt-writer"` matches; `"PROMPT-WRITER"` and `"specialized/prompt-writer"` do not.

#### Example

```rust
use amplihack_hooks::known_agents::{agent_count, is_amplihack_agent};

assert!(is_amplihack_agent("prompt-writer"));
assert!(is_amplihack_agent("architect"));
assert!(!is_amplihack_agent("default-workflow")); // that's a skill
assert!(!is_amplihack_agent(""));
assert_eq!(agent_count(), 41);
```

### Registry Consistency Test

A `#[cfg(test)]` consistency test globs `amplifier-bundle/agents/**/*.md`, collects the deduplicated basenames, and asserts they exactly match `AMPLIHACK_AGENTS`. If an agent is added, renamed, or removed without updating the registry, the build fails. This is the guard against registry drift.

---

## `PreToolUse` Redirect

### Constant

```rust
/// Static guidance template for an agent-only Skill invocation.
const SKILL_IS_AGENT_REDIRECT: &str = "\
\"{name}\" is an amplihack agent, not a skill. The Skill tool cannot run it. \
Invoke it as an agent instead (use the Task/agent tool with agent type \
\"{name}\"), or reference it from a recipe step as `agent: \"amplihack:{name}\"`. \
Do not retry this as a Skill call.";
```

Only the sanitized target name is substituted into the template. The full tool input is never echoed.

### Helper

```rust
/// Returns `Some(block_response)` when `tool_name == "Skill"` and the named
/// target is an agent but not a skill; otherwise `None` (pass through).
fn check_skill_redirect(tool_name: &str, input: &serde_json::Value) -> Option<serde_json::Value>;
```

Semantics:

1. Return `None` immediately unless `tool_name == "Skill"`.
2. Extract the target name with total, panic-free accessors:
   ```rust
   let name = input
       .get("skill")
       .or_else(|| input.get("name"))
       .and_then(serde_json::Value::as_str)?;
   ```
   The copilot `Skill` payload uses the `skill` key; `name` is accepted as a fallback. A missing, non-string, or null value yields `None` (pass through).
3. Apply the predicate:
   ```rust
   // Skill precedence: only redirect agent-only names. The skill set is
   // scanned from the on-disk skills directory at runtime (issue #863),
   // so the directory is the single source of truth, not a hardcoded list.
   if is_amplihack_agent(name) && !bundled_skill_names().contains(name) {
       // build redirect
   }
   ```
4. On match, return:
   ```json
   { "block": true, "message": "<SKILL_IS_AGENT_REDIRECT with name substituted>" }
   ```
   where `name` is sanitized to `[A-Za-z0-9-]` before substitution.

### Wiring

`check_skill_redirect()` is called inside `PreToolUseHook::process()` **after** the XPIA security check and **before** the `if tool_name != "Bash"` early return:

```rust
fn process(&self, input: HookInput) -> anyhow::Result<Value> {
    let (tool_name, tool_input) = /* ... */;

    // Launcher context injection (side effect only).
    // XPIA security validation.
    if let Some(block) = xpia::check_xpia(&tool_name, &tool_input) {
        return Ok(block);
    }

    // Skill-to-agent redirect (this feature).
    if let Some(block) = check_skill_redirect(&tool_name, &tool_input) {
        return Ok(block);
    }

    if tool_name != "Bash" {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    // ... Bash-only checks ...
}
```

Placing the check before the `Bash` gate is required because `Skill` is not `Bash`; the existing early return (`Value::Object(serde_json::Map::new())`, which serializes to `{}`) would otherwise pass every non-Bash tool through untouched.

### Failure Policy

`PreToolUseHook` returns `FailurePolicy::Open`. The redirect is advisory UX: any parse or lookup failure passes the original tool call through. The hook never fails closed and never aborts a run.

---

## Output Contract

A matched redirect produces a standard `PreToolUse` block response:

```json
{
  "block": true,
  "message": "\"prompt-writer\" is an amplihack agent, not a skill. ..."
}
```

A non-match produces an empty object (`{}`), identical to any other allowed non-Bash tool call.

---

## Security Considerations

- **Total parsing.** Name extraction uses `Option` accessors only — no `unwrap`, `expect`, or indexing. Missing/non-string/null inputs pass through.
- **Pure lookup.** `is_amplihack_agent` is a `binary_search` over a static `&[&str]`. No regex, path traversal, format-string, or shell evaluation — no injection vector.
- **Bounded, fail-open filesystem scan.** Agent names are compile-time constants. The skill set is derived at runtime by scanning the bundled skills directory (issue #863): the walk does **not** follow symlinks, is depth-bounded (`MAX_SKILL_SCAN_DEPTH`), reads only `SKILL.md` frontmatter as opaque text, and fails open (unreadable roots or files are skipped). The parsed skill name is used solely for set membership — never to build a filesystem path, command, or URL — so a hostile `name` stays inert data.
- **Non-reflective message.** The redirect emits a static template plus the bare sanitized name (`[A-Za-z0-9-]`), never the full tool input, to avoid leaking surrounding prompt content into logs or transcripts.
- **No persistence.** The registry holds public agent names only; the hook performs no disk or database writes and adds no new log sinks beyond existing `tracing`.

---

## Tests

| Location                                  | Coverage                                                                                                    |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `known_agents.rs` (`#[cfg(test)]`)        | Membership (`prompt-writer` → true, unknown → false), slice-is-sorted assertion, `agent_count` value, and the filesystem consistency test against `amplifier-bundle/agents/**/*.md`. |
| `pre_tool_use/mod.rs` (`#[cfg(test)]`)    | `Skill(prompt-writer)` → blocked + redirect; `gherkin-expert` / `tla-plus-expert` / `default-workflow` → pass (skill precedence); `totally-unknown` → pass; malformed inputs (`{}`, `{"skill":123}`, `{"skill":null}`, nested junk) → pass-through with no panic. |

Run them with:

```bash
cargo test -p amplihack-hooks known_agents
cargo test -p amplihack-hooks skill_redirect
```

---

## Related

- [Skill-to-Agent Redirect feature overview](../features/skill-to-agent-redirect.md)
- [Hook specifications reference](hook-specifications.md)
- [UserPromptSubmit hook API reference](user-prompt-submit-hook-api.md)
