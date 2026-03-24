//! Copilot home staging to match Python launcher behavior.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const INSTRUCTIONS_WORKFLOW_DESC: &str = "Standard development workflow";

const INSTRUCTIONS_MARKER_START: &str = "<!-- AMPLIHACK_INSTRUCTIONS_START -->";
const INSTRUCTIONS_MARKER_END: &str = "<!-- AMPLIHACK_INSTRUCTIONS_END -->";
const COPILOT_HOOKS_MANIFEST: &str = r#"{
  "version": 1,
  "hooks": {
    "sessionStart": [
      {
        "type": "command",
        "bash": ".github/hooks/session-start",
        "timeoutSec": 30
      }
    ],
    "sessionEnd": [
      {
        "type": "command",
        "bash": ".github/hooks/session-stop",
        "timeoutSec": 30
      }
    ],
    "userPromptSubmitted": [
      {
        "type": "command",
        "bash": ".github/hooks/user-prompt-submit",
        "timeoutSec": 10
      }
    ],
    "preToolUse": [
      {
        "type": "command",
        "bash": ".github/hooks/pre-tool-use",
        "timeoutSec": 15
      }
    ],
    "postToolUse": [
      {
        "type": "command",
        "bash": ".github/hooks/post-tool-use",
        "timeoutSec": 10
      }
    ],
    "errorOccurred": [
      {
        "type": "command",
        "bash": ".github/hooks/error-occurred",
        "timeoutSec": 10
      }
    ]
  }
}
"#;

struct HookWrapperSpec {
    hook_name: &'static str,
    subcommands: &'static [&'static str],
}

const COPILOT_HOOK_WRAPPERS: &[HookWrapperSpec] = &[
    HookWrapperSpec {
        hook_name: "session-start",
        subcommands: &["session-start"],
    },
    HookWrapperSpec {
        hook_name: "session-stop",
        subcommands: &["stop", "session-stop"],
    },
    HookWrapperSpec {
        hook_name: "pre-tool-use",
        subcommands: &["pre-tool-use"],
    },
    HookWrapperSpec {
        hook_name: "post-tool-use",
        subcommands: &["post-tool-use"],
    },
    HookWrapperSpec {
        hook_name: "user-prompt-submit",
        subcommands: &["workflow-classification-reminder", "user-prompt-submit"],
    },
];

pub fn ensure_copilot_home_staged() -> Result<()> {
    let source_root = staged_framework_dir()?;
    let copilot_home = copilot_home()?;
    fs::create_dir_all(&copilot_home)
        .with_context(|| format!("failed to create {}", copilot_home.display()))?;

    stage_agents(&source_root.join("agents").join("amplihack"), &copilot_home)?;
    stage_skills(&source_root.join("skills"), &copilot_home)?;
    stage_directory(&source_root.join("workflow"), &copilot_home, "workflow")?;
    stage_directory(&source_root.join("context"), &copilot_home, "context")?;
    stage_command_docs(
        &source_root.join("commands").join("amplihack"),
        &copilot_home,
    )?;
    register_plugin(
        &source_root.join("commands").join("amplihack"),
        &copilot_home,
    )?;
    stage_repo_hooks(
        &std::env::current_dir().context("failed to determine current working directory")?,
    )?;
    generate_copilot_instructions(&copilot_home)?;
    Ok(())
}

pub fn disable_github_mcp_server() -> Result<bool> {
    let mcp_config_dir = copilot_home()?.join("github-copilot");
    let mcp_config_file = mcp_config_dir.join("mcp.json");
    fs::create_dir_all(&mcp_config_dir)
        .with_context(|| format!("failed to create {}", mcp_config_dir.display()))?;

    let mut config = if mcp_config_file.exists() {
        fs::read_to_string(&mcp_config_file)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let Some(root) = config.as_object_mut() else {
        return Err(anyhow!("mcp config root must be a JSON object"));
    };
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Default::default()));
    if !servers.is_object() {
        *servers = Value::Object(Default::default());
    }
    let servers = servers
        .as_object_mut()
        .expect("mcpServers converted to object");
    let github = servers
        .entry("github-mcp-server")
        .or_insert_with(|| Value::Object(Default::default()));
    if !github.is_object() {
        *github = Value::Object(Default::default());
    }
    github
        .as_object_mut()
        .expect("github-mcp-server converted to object")
        .insert("disabled".to_string(), Value::Bool(true));

    fs::write(
        &mcp_config_file,
        serde_json::to_string_pretty(&config)? + "\n",
    )
    .with_context(|| format!("failed to write {}", mcp_config_file.display()))?;
    Ok(true)
}

fn stage_agents(source_agents: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_agents.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join("agents").join("amplihack");
    reset_markdown_dir(&dest)?;
    flatten_markdown_tree(source_agents, &dest)
}

fn stage_directory(source_dir: &Path, copilot_home: &Path, dest_name: &str) -> Result<usize> {
    if !source_dir.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join(dest_name).join("amplihack");
    reset_markdown_dir(&dest)?;
    flatten_markdown_tree(source_dir, &dest)
}

fn stage_skills(source_skills: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_skills.exists() {
        return Ok(0);
    }

    let skills_dest = copilot_home.join("skills");
    fs::create_dir_all(&skills_dest)
        .with_context(|| format!("failed to create {}", skills_dest.display()))?;

    let mut copied = 0usize;
    for entry in fs::read_dir(source_skills)
        .with_context(|| format!("failed to read {}", source_skills.display()))?
    {
        let entry = entry?;
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let dest_skill = skills_dest.join(entry.file_name());
        let is_new = !dest_skill.exists();
        copy_dir_recursive(&skill_dir, &dest_skill)?;
        if is_new {
            copied += 1;
        }
    }
    Ok(copied)
}

fn stage_command_docs(source_commands: &Path, copilot_home: &Path) -> Result<usize> {
    if !source_commands.exists() {
        return Ok(0);
    }

    let dest = copilot_home.join("commands").join("amplihack");
    fs::create_dir_all(&dest).with_context(|| format!("failed to create {}", dest.display()))?;

    let mut copied = 0usize;
    for entry in fs::read_dir(source_commands)
        .with_context(|| format!("failed to read {}", source_commands.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        fs::copy(&path, dest.join(entry.file_name())).with_context(|| {
            format!("failed to copy {} into {}", path.display(), dest.display())
        })?;
        copied += 1;
    }

    Ok(copied)
}

fn register_plugin(source_commands: &Path, copilot_home: &Path) -> Result<bool> {
    if !source_commands.exists() {
        return Ok(false);
    }

    let plugin_cache = copilot_home
        .join("installed-plugins")
        .join("amplihack@local");
    let plugin_commands = plugin_cache.join("commands");
    fs::create_dir_all(&plugin_commands)
        .with_context(|| format!("failed to create {}", plugin_commands.display()))?;

    if source_commands.join("plugin.json").exists() {
        fs::copy(
            source_commands.join("plugin.json"),
            plugin_cache.join("plugin.json"),
        )
        .with_context(|| format!("failed to copy plugin.json into {}", plugin_cache.display()))?;
    }

    let mut copied = 0usize;
    for entry in fs::read_dir(source_commands)
        .with_context(|| format!("failed to read {}", source_commands.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        fs::copy(&path, plugin_commands.join(entry.file_name())).with_context(|| {
            format!(
                "failed to copy {} into {}",
                path.display(),
                plugin_commands.display()
            )
        })?;
        copied += 1;
    }

    if copied == 0 {
        return Ok(false);
    }

    let config_path = copilot_home.join("config.json");
    let mut config = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let Some(root) = config.as_object_mut() else {
        return Err(anyhow!("copilot config root must be a JSON object"));
    };
    let installed = root
        .entry("installed_plugins")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !installed.is_array() {
        *installed = Value::Array(Vec::new());
    }
    let installed = installed
        .as_array_mut()
        .expect("installed_plugins converted to array");
    installed.retain(|entry| entry.get("name").and_then(Value::as_str) != Some("amplihack"));
    installed.push(json!({
        "name": "amplihack",
        "marketplace": "local",
        "version": "1.0.0",
        "enabled": true,
        "cache_path": plugin_cache.to_string_lossy(),
        "source": "local"
    }));

    fs::write(&config_path, serde_json::to_string_pretty(&config)? + "\n")
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(true)
}

fn stage_repo_hooks(repo_root: &Path) -> Result<usize> {
    let hooks_dir = repo_root.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;
    let manifest_path = hooks_dir.join("amplihack-hooks.json");
    fs::write(&manifest_path, COPILOT_HOOKS_MANIFEST)
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    let hook_engine = hook_engine()?;
    let rust_binary = if hook_engine == "rust" {
        Some(find_rust_hook_binary()?.ok_or_else(|| {
            anyhow!(
                "AMPLIHACK_HOOK_ENGINE=rust but amplihack-hooks binary not found. Install it or set AMPLIHACK_HOOK_ENGINE=python."
            )
        })?)
    } else {
        None
    };

    let mut staged = 0usize;
    for spec in COPILOT_HOOK_WRAPPERS {
        let wrapper_path = hooks_dir.join(spec.hook_name);
        if should_preserve_user_hook(&wrapper_path)? {
            continue;
        }
        fs::write(
            &wrapper_path,
            build_wrapper_script(spec, hook_engine, rust_binary.as_deref()),
        )
        .with_context(|| format!("failed to write {}", wrapper_path.display()))?;
        set_executable(&wrapper_path)?;
        staged += 1;
    }

    let error_wrapper = hooks_dir.join("error-occurred");
    if !should_preserve_user_hook(&error_wrapper)? {
        fs::write(&error_wrapper, error_wrapper_script())
            .with_context(|| format!("failed to write {}", error_wrapper.display()))?;
        set_executable(&error_wrapper)?;
        staged += 1;
    }

    Ok(staged)
}

fn generate_copilot_instructions(copilot_home: &Path) -> Result<()> {
    let workflow_desc = workflow_description(copilot_home);
    let instructions_path = copilot_home.join("copilot-instructions.md");
    let section = format!(
        r#"{INSTRUCTIONS_MARKER_START}
# Amplihack Framework Integration

You have access to the amplihack agentic coding framework. Use these resources:

## Workflows
Read workflow files from `{workflow}` to follow structured processes:
- `DEFAULT_WORKFLOW.md` — {workflow_desc}
- `INVESTIGATION_WORKFLOW.md` — Research and exploration (6 phases)
- `CASCADE_WORKFLOW.md`, `DEBATE_WORKFLOW.md`, `N_VERSION_WORKFLOW.md` — Fault tolerance patterns

    For any non-trivial development or investigation task, use `/dev` (or `Skill(skill="dev-orchestrator")`)
    so the smart-orchestrator recipe executes the workflow instead of handling it manually.

## Context
Read context files from `{context}` for project philosophy and patterns:
- `PHILOSOPHY.md` — Core principles (ruthless simplicity, zero-BS, modular design)
- `PATTERNS.md` — Reusable solution patterns
- `TRUST.md` — Anti-sycophancy and direct communication guidelines
- `USER_PREFERENCES.md` — User-specific preferences (MANDATORY)

    ## Commands
    Read command definitions from `{commands}` for available capabilities:
    - `dev.md` — Primary dev-orchestrator entry point
    - `ultrathink.md` — Deprecated alias to `/dev`
    - `analyze.md` — Comprehensive code review
    - `improve.md` — Self-improvement and learning capture

## Agents
Custom agents are available at `{agents}`. Use them via the task tool.

## Skills
Skills are available at `{skills}`. They auto-activate based on context.
{INSTRUCTIONS_MARKER_END}"#,
        workflow = format!(
            "{}/",
            copilot_home.join("workflow").join("amplihack").display()
        ),
        context = format!(
            "{}/",
            copilot_home.join("context").join("amplihack").display()
        ),
        commands = format!(
            "{}/",
            copilot_home.join("commands").join("amplihack").display()
        ),
        agents = format!(
            "{}/",
            copilot_home.join("agents").join("amplihack").display()
        ),
        skills = format!("{}/", copilot_home.join("skills").display()),
        workflow_desc = workflow_desc,
    );

    let updated = if instructions_path.exists() {
        let existing = fs::read_to_string(&instructions_path)
            .with_context(|| format!("failed to read {}", instructions_path.display()))?;
        replace_or_append_section(&existing, &section)
    } else {
        format!("{section}\n")
    };

    fs::write(&instructions_path, updated)
        .with_context(|| format!("failed to write {}", instructions_path.display()))
}

fn workflow_description(copilot_home: &Path) -> String {
    let default_workflow = copilot_home
        .join("workflow")
        .join("amplihack")
        .join("DEFAULT_WORKFLOW.md");
    let Ok(content) = fs::read_to_string(default_workflow) else {
        return INSTRUCTIONS_WORKFLOW_DESC.to_string();
    };
    let count = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("### Step ")
                && trimmed
                    .split_once(':')
                    .map(|(prefix, _)| {
                        prefix["### Step ".len()..]
                            .chars()
                            .all(|c| c.is_ascii_digit() || c == '.')
                    })
                    .unwrap_or(false)
        })
        .count();
    if count == 0 {
        INSTRUCTIONS_WORKFLOW_DESC.to_string()
    } else {
        format!("{INSTRUCTIONS_WORKFLOW_DESC} ({count} steps)")
    }
}

fn replace_or_append_section(existing: &str, section: &str) -> String {
    if let Some(start) = existing.find(INSTRUCTIONS_MARKER_START)
        && let Some(end_rel) = existing[start..].find(INSTRUCTIONS_MARKER_END)
    {
        let end = start + end_rel + INSTRUCTIONS_MARKER_END.len();
        let mut updated = String::new();
        updated.push_str(&existing[..start]);
        updated.push_str(section);
        updated.push_str(&existing[end..]);
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        return updated;
    }

    if existing.trim().is_empty() {
        format!("{section}\n")
    } else {
        format!("{}\n\n{section}\n", existing.trim_end())
    }
}

fn reset_markdown_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
        }
    }
    Ok(())
}

fn flatten_markdown_tree(source: &Path, dest: &Path) -> Result<usize> {
    let mut copied = 0usize;
    for path in walk_files(source)? {
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let file_name = path
            .file_name()
            .context("source markdown file missing name")?;
        fs::copy(&path, dest.join(file_name)).with_context(|| {
            format!("failed to copy {} into {}", path.display(), dest.display())
        })?;
        copied += 1;
    }
    Ok(copied)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            paths.extend(walk_files(&path)?);
        } else {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target).with_context(|| {
                format!(
                    "failed to copy {} into {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

fn should_preserve_user_hook(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(!existing.contains("amplihack"))
}

fn build_wrapper_script(
    spec: &HookWrapperSpec,
    hook_engine: &str,
    rust_binary: Option<&Path>,
) -> String {
    if spec.hook_name == "pre-tool-use" {
        return build_pre_tool_use_wrapper(hook_engine, rust_binary);
    }

    if hook_engine == "rust" {
        return build_rust_wrapper(spec, rust_binary);
    }

    build_python_wrapper(spec)
}

fn error_wrapper_script() -> &'static str {
    r#"#!/usr/bin/env bash
# Copilot hook: error-occurred
# Logs error to runtime log. No dedicated Python hook exists for this event;
# error_protocol.py is a utility module, not a hook entry point.

AMPLIHACK_HOOKS="$HOME/.amplihack/.claude/tools/amplihack/hooks"
LOG_DIR="$HOME/.amplihack/.claude/runtime/logs"

# If a dedicated error_occurred.py hook exists, use it
if [[ -f "${AMPLIHACK_HOOKS}/error_occurred.py" ]]; then
    python3 "${AMPLIHACK_HOOKS}/error_occurred.py" "$@"
elif REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" && [[ -f "${REPO_ROOT}/.claude/tools/amplihack/hooks/error_occurred.py" ]]; then
    python3 "${REPO_ROOT}/.claude/tools/amplihack/hooks/error_occurred.py" "$@"
else
    # Fallback: log the error from stdin
    mkdir -p "$LOG_DIR"
    INPUT=$(cat)
    ERROR_MSG=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('error',{}).get('message','unknown'))" 2>/dev/null || echo "unknown")
    echo "$(date -Iseconds): ERROR - $ERROR_MSG" >> "${LOG_DIR}/errors.log"
    echo "{}"
fi
"#
}

fn build_rust_wrapper(spec: &HookWrapperSpec, rust_binary: Option<&Path>) -> String {
    let quoted_binary = rust_binary
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "amplihack-hooks".to_string());

    match spec.hook_name {
        "session-start" | "post-tool-use" => format!(
            "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack (rust engine)\n\
exec {quoted_binary} {} \"$@\"\n",
            spec.subcommands[0]
        ),
        "session-stop" => format!(
            "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack (rust engine)\n\
# Runs multiple hook scripts for this event\n\
INPUT=$(cat)\n\n\
echo \"$INPUT\" | {quoted_binary} stop \"$@\" || true\n\n\
echo \"$INPUT\" | {quoted_binary} session-stop \"$@\" || true\n"
        ),
        "user-prompt-submit" => format!(
            r#"#!/usr/bin/env bash
# Copilot hook wrapper - generated by amplihack (rust engine)
# Runs multiple hook scripts for this event
INPUT=$(cat)

echo "$INPUT" | {quoted_binary} user-prompt-submit "$@" || true

AMPLIHACK_HOOKS="$HOME/.amplihack/.claude/tools/amplihack/hooks"
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || REPO_ROOT=""
if [[ -f "${{AMPLIHACK_HOOKS}}/workflow_classification_reminder.py" ]]; then
    echo "$INPUT" | python3 "${{AMPLIHACK_HOOKS}}/workflow_classification_reminder.py" "$@" 2>/dev/null || true
elif [[ -n "$REPO_ROOT" ]] && [[ -f "${{REPO_ROOT}}/.claude/tools/amplihack/hooks/workflow_classification_reminder.py" ]]; then
    echo "$INPUT" | python3 "${{REPO_ROOT}}/.claude/tools/amplihack/hooks/workflow_classification_reminder.py" "$@" 2>/dev/null || true
fi
"#
        ),
        _ => build_python_wrapper(spec),
    }
}

fn build_pre_tool_use_wrapper(hook_engine: &str, rust_binary: Option<&Path>) -> String {
    let amplihack_capture = if hook_engine == "rust" {
        let binary = rust_binary
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "amplihack-hooks".to_string());
        format!(
            "AMPLIHACK_OUTPUT=$(echo \"$INPUT\" | {binary} pre-tool-use \"$@\" 2>/dev/null || printf '{{}}')"
        )
    } else {
        "AMPLIHACK_OUTPUT=\"{}\"\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\
if [[ -f \"${AMPLIHACK_HOOKS}/pre_tool_use.py\" ]]; then\n\
    AMPLIHACK_OUTPUT=$(echo \"$INPUT\" | python3 \"${AMPLIHACK_HOOKS}/pre_tool_use.py\" \"$@\" 2>/dev/null || printf '{}')\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/pre_tool_use.py\" ]]; then\n\
    AMPLIHACK_OUTPUT=$(echo \"$INPUT\" | python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/pre_tool_use.py\" \"$@\" 2>/dev/null || printf '{}')\n\
fi"
            .to_string()
    };

    format!(
        r#"#!/usr/bin/env bash
# Copilot hook wrapper - generated by amplihack ({hook_engine} engine)
# Aggregates amplihack and XPIA pre-tool validation into one JSON response
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || REPO_ROOT=""
INPUT=$(cat)

{amplihack_capture}

XPIA_OUTPUT="{{}}"
XPIA_HOOKS="$HOME/.amplihack/.claude/tools/xpia/hooks"
if [[ -f "${{XPIA_HOOKS}}/pre_tool_use.py" ]]; then
    XPIA_OUTPUT=$(echo "$INPUT" | python3 "${{XPIA_HOOKS}}/pre_tool_use.py" "$@" 2>/dev/null || printf '{{}}')
elif [[ -n "$REPO_ROOT" ]] && [[ -f "${{REPO_ROOT}}/.claude/tools/xpia/hooks/pre_tool_use.py" ]]; then
    XPIA_OUTPUT=$(echo "$INPUT" | python3 "${{REPO_ROOT}}/.claude/tools/xpia/hooks/pre_tool_use.py" "$@" 2>/dev/null || printf '{{}}')
fi

python3 - "$AMPLIHACK_OUTPUT" "$XPIA_OUTPUT" <<'PY'
import json
import sys


def parse_payload(raw: str) -> dict:
    raw = raw.strip()
    if not raw:
        return {{}}
    for line in reversed(raw.splitlines()):
        line = line.strip()
        if not line:
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return value
    return {{}}


amplihack = parse_payload(sys.argv[1])
xpia = parse_payload(sys.argv[2])

permission = xpia.get("permissionDecision")
if permission in {{"allow", "deny", "ask"}}:
    print(json.dumps(xpia))
    raise SystemExit(0)

permission = amplihack.get("permissionDecision")
if permission in {{"allow", "deny", "ask"}}:
    print(json.dumps(amplihack))
    raise SystemExit(0)

if amplihack.get("block"):
    print(
        json.dumps(
            {{
                "permissionDecision": "deny",
                "message": amplihack.get(
                    "message",
                    "Blocked by amplihack pre-tool-use hook.",
                ),
            }}
        )
    )
    raise SystemExit(0)

print("{{}}")
PY
"#
    )
}

fn build_python_wrapper(spec: &HookWrapperSpec) -> String {
    match spec.hook_name {
        "session-start" => "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
HOOK=\"session_start.py\"\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/${HOOK}\" ]]; then\n\
    exec python3 \"${AMPLIHACK_HOOKS}/${HOOK}\" \"$@\"\n\
elif REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/${HOOK}\" ]]; then\n\
    exec python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/${HOOK}\" \"$@\"\n\
else\n\
    echo \"{}\"\n\
fi\n"
            .to_string(),
        "session-stop" => "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
# Runs multiple hook scripts for this event\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\
REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" || REPO_ROOT=\"\"\n\
INPUT=$(cat)\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/stop.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${AMPLIHACK_HOOKS}/stop.py\" \"$@\" 2>/dev/null || true\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/stop.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/stop.py\" \"$@\" 2>/dev/null || true\n\
fi\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/session_stop.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${AMPLIHACK_HOOKS}/session_stop.py\" \"$@\" 2>/dev/null || true\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/session_stop.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/session_stop.py\" \"$@\" 2>/dev/null || true\n\
fi\n"
            .to_string(),
        "post-tool-use" => "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
HOOK=\"post_tool_use.py\"\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/${HOOK}\" ]]; then\n\
    exec python3 \"${AMPLIHACK_HOOKS}/${HOOK}\" \"$@\"\n\
elif REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/${HOOK}\" ]]; then\n\
    exec python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/${HOOK}\" \"$@\"\n\
else\n\
    echo \"{}\"\n\
fi\n"
            .to_string(),
        "user-prompt-submit" => "#!/usr/bin/env bash\n\
# Copilot hook wrapper - generated by amplihack\n\
# Runs multiple hook scripts for this event\n\
AMPLIHACK_HOOKS=\"$HOME/.amplihack/.claude/tools/amplihack/hooks\"\n\
REPO_ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" || REPO_ROOT=\"\"\n\
INPUT=$(cat)\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/user_prompt_submit.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${AMPLIHACK_HOOKS}/user_prompt_submit.py\" \"$@\" 2>/dev/null || true\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/user_prompt_submit.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/user_prompt_submit.py\" \"$@\" 2>/dev/null || true\n\
fi\n\n\
if [[ -f \"${AMPLIHACK_HOOKS}/workflow_classification_reminder.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${AMPLIHACK_HOOKS}/workflow_classification_reminder.py\" \"$@\" 2>/dev/null || true\n\
elif [[ -n \"$REPO_ROOT\" ]] && [[ -f \"${REPO_ROOT}/.claude/tools/amplihack/hooks/workflow_classification_reminder.py\" ]]; then\n\
    echo \"$INPUT\" | python3 \"${REPO_ROOT}/.claude/tools/amplihack/hooks/workflow_classification_reminder.py\" \"$@\" 2>/dev/null || true\n\
fi\n"
            .to_string(),
        _ => String::new(),
    }
}

fn find_rust_hook_binary() -> Result<Option<PathBuf>> {
    let mut candidates = Vec::new();
    if let Some(path) = find_executable_on_path("amplihack-hooks") {
        candidates.push(path);
    }
    if let Ok(home) = home_dir() {
        candidates.push(
            home.join(".amplihack")
                .join(".claude")
                .join("bin")
                .join("amplihack-hooks"),
        );
        candidates.push(home.join(".amplihack").join("bin").join("amplihack-hooks"));
        candidates.push(home.join(".cargo").join("bin").join("amplihack-hooks"));
    }
    Ok(candidates.into_iter().find(|candidate| candidate.is_file()))
}

fn hook_engine() -> Result<&'static str> {
    match std::env::var("AMPLIHACK_HOOK_ENGINE") {
        Ok(value) if value.eq_ignore_ascii_case("python") => Ok("python"),
        Ok(value) if value.eq_ignore_ascii_case("rust") => Ok("rust"),
        Ok(_) => Ok("python"),
        Err(_) => {
            if find_rust_hook_binary()?.is_some() {
                Ok("rust")
            } else {
                Ok("python")
            }
        }
    }
}

fn find_executable_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn staged_framework_dir() -> Result<PathBuf> {
    home_dir().map(|home| home.join(".amplihack").join(".claude"))
}

fn copilot_home() -> Result<PathBuf> {
    home_dir().map(|home| home.join(".copilot"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("HOME is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_copilot_home_stages_assets_and_plugin() {
        let _home_guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _cwd_guard = crate::test_support::cwd_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&repo_root).unwrap();
        let previous_cwd = crate::test_support::set_cwd(&repo_root).unwrap();

        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::create_dir_all(staged.join("skills/dev-orchestrator")).unwrap();
        fs::create_dir_all(staged.join("skills/quality-audit")).unwrap();
        fs::create_dir_all(staged.join("workflow")).unwrap();
        fs::create_dir_all(staged.join("context")).unwrap();
        fs::create_dir_all(staged.join("commands/amplihack")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent").unwrap();
        fs::write(staged.join("skills/dev-orchestrator/SKILL.md"), "skill-a").unwrap();
        fs::write(staged.join("skills/quality-audit/SKILL.md"), "skill-b").unwrap();
        fs::write(staged.join("workflow/DEFAULT_WORKFLOW.md"), "workflow").unwrap();
        fs::write(staged.join("context/USER_PREFERENCES.md"), "prefs").unwrap();
        fs::write(staged.join("commands/amplihack/dev.md"), "command").unwrap();
        fs::write(
            staged.join("commands/amplihack/plugin.json"),
            "{\"name\":\"amplihack\"}",
        )
        .unwrap();

        ensure_copilot_home_staged().unwrap();

        assert!(
            temp.path()
                .join(".copilot/agents/amplihack/architect.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/dev-orchestrator/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/skills/quality-audit/SKILL.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/workflow/amplihack/DEFAULT_WORKFLOW.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/commands/amplihack/dev.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".copilot/installed-plugins/amplihack@local/commands/dev.md")
                .exists()
        );
        assert!(repo_root.join(".github/hooks/session-start").exists());
        assert!(
            repo_root
                .join(".github/hooks/amplihack-hooks.json")
                .exists()
        );

        let config = fs::read_to_string(temp.path().join(".copilot/config.json")).unwrap();
        assert!(config.contains("\"name\": \"amplihack\""));

        crate::test_support::restore_cwd(&previous_cwd).unwrap();
        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn replace_or_append_section_updates_existing_block() {
        let existing =
            format!("before\n{INSTRUCTIONS_MARKER_START}\nold\n{INSTRUCTIONS_MARKER_END}\nafter\n");
        let updated = replace_or_append_section(&existing, "NEW");
        assert!(updated.contains("before"));
        assert!(updated.contains("after"));
        assert!(updated.contains("NEW"));
        assert!(!updated.contains("old"));
    }

    #[test]
    fn build_wrapper_script_uses_binary_subcommand_for_single_hook() {
        let binary = Path::new("/tmp/amplihack-hooks");
        let script = build_wrapper_script(
            &HookWrapperSpec {
                hook_name: "session-start",
                subcommands: &["session-start"],
            },
            "rust",
            Some(binary),
        );

        assert!(script.contains("/tmp/amplihack-hooks"));
        assert!(script.contains("exec /tmp/amplihack-hooks session-start \"$@\""));
        assert!(!script.contains("python3"));
    }

    #[test]
    fn build_wrapper_script_uses_multiple_binary_subcommands() {
        let binary = Path::new("/tmp/amplihack-hooks");
        let script = build_wrapper_script(
            &HookWrapperSpec {
                hook_name: "user-prompt-submit",
                subcommands: &["user-prompt-submit"],
            },
            "rust",
            Some(binary),
        );

        assert!(
            script.contains(
                "echo \"$INPUT\" | /tmp/amplihack-hooks user-prompt-submit \"$@\" || true"
            )
        );
        assert!(script.contains("workflow_classification_reminder.py"));
        assert!(script.contains("python3"));
    }

    #[test]
    fn error_wrapper_script_matches_python_fallback_shape() {
        let script = error_wrapper_script();
        assert!(script.contains("python3"));
        assert!(script.contains("error_occurred.py"));
        assert!(script.contains("errors.log"));
    }
}
