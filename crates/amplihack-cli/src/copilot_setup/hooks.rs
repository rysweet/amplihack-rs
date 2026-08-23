//! Hook wiring, instructions generation, and related helpers.

use amplihack_types::hook_io::normalize_executable_script_line_endings;
use anyhow::{Context, Result};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{
    COPILOT_HOOK_TIMEOUT_SEC, COPILOT_HOOK_WRAPPERS, HookWrapperSpec, INSTRUCTIONS_MARKER_END,
    INSTRUCTIONS_MARKER_START, fs_helpers, jsonc,
};

/// Build the Copilot hooks manifest as JSON. Hook entries reference absolute
/// paths to the staged bash wrappers under `<repo>/.github/hooks/`.
pub(super) fn build_copilot_hooks_manifest(hooks_dir: &Path) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for spec in COPILOT_HOOK_WRAPPERS {
        let bash_path = hooks_dir.join(spec.hook_name);
        let entry = serde_json::json!({
            "type": "command",
            "bash": bash_path.to_string_lossy(),
            "timeoutSec": COPILOT_HOOK_TIMEOUT_SEC,
        });
        hooks.insert(spec.copilot_event.to_string(), serde_json::json!([entry]));
    }
    serde_json::json!({
        "version": 1,
        "hooks": hooks,
    })
}

pub(super) fn stage_repo_hooks(repo_root: &Path) -> Result<usize> {
    // Defensive guard (issue #536): never write `.github/hooks/` into the
    // amplihack-rs workspace itself. The combination of `Cargo.toml` +
    // `amplifier-bundle/` + `crates/amplihack-cli/` is the unambiguous
    // workspace marker; user repos that happen to ship a Cargo.toml will not
    // also have those two amplihack-specific paths. This protects against
    // any test or subprocess that inadvertently calls into us with cwd
    // pointing at the amplihack-rs checkout (the `github_hooks_scope_creep_is_absent`
    // contract). Production users staging hooks into their own checkouts are
    // unaffected.
    if is_amplihack_workspace_root(repo_root) {
        tracing::debug!(
            ?repo_root,
            "stage_repo_hooks: refusing to write into amplihack-rs workspace root"
        );
        return Ok(0);
    }

    let hooks_dir = repo_root.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    let manifest = build_copilot_hooks_manifest(&hooks_dir);
    let manifest_dest = hooks_dir.join("amplihack-hooks.json");
    fs::write(
        &manifest_dest,
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;

    let mut count = 1; // manifest itself

    for spec in COPILOT_HOOK_WRAPPERS {
        let dest = hooks_dir.join(spec.hook_name);
        if should_preserve_user_hook(&dest)? {
            continue;
        }
        let script = build_wrapper_script(spec);
        write_executable_script(&dest, &script)?;
        set_executable(&dest)?;
        count += 1;
    }

    let error_dest = hooks_dir.join("_error_handler");
    write_executable_script(&error_dest, error_wrapper_script())?;
    set_executable(&error_dest)?;
    count += 1;

    Ok(count)
}

/// Detect the amplihack-rs workspace root or any path inside it.
///
/// Returns true when `path` or one of its ancestors has **all three** marker
/// paths: `Cargo.toml`, `amplifier-bundle/`, and `crates/amplihack-cli/`. User
/// project repos that happen to ship a `Cargo.toml` will not also have those
/// amplihack-specific directories, so this guard never blocks legitimate user
/// staging.
///
/// Used by `stage_repo_hooks` to refuse writing into the amplihack-rs
/// workspace tree, satisfying the `github_hooks_scope_creep_is_absent`
/// contract (issue #536).
fn is_amplihack_workspace_root(path: &Path) -> bool {
    path.ancestors().any(|candidate| {
        candidate.join("Cargo.toml").is_file()
            && candidate.join("amplifier-bundle").is_dir()
            && candidate.join("crates").join("amplihack-cli").is_dir()
    })
}

/// Merge an amplihack hooks block into `~/.copilot/config.json` so that hooks
/// fire regardless of which directory Copilot is launched from. Existing
/// non-amplihack hook entries are preserved; amplihack-owned entries (those
/// whose `bash` path resolves under `~/.amplihack` or whose `_amplihack`
/// marker is set) are replaced atomically.
pub(super) fn write_user_level_hooks(copilot_home: &Path) -> Result<()> {
    // We point the user-level hooks at the staged framework directory so the
    // hook wrappers work irrespective of which repo Copilot opens. The
    // wrappers themselves resolve `amplihack-hooks` from PATH or the standard
    // install locations, so they degrade gracefully if the binary is missing.
    let hooks_dir = copilot_home.join(".github").join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    // Materialize the wrappers under ~/.copilot/.github/hooks/ so their bash
    // paths resolve absolutely on disk.
    for spec in COPILOT_HOOK_WRAPPERS {
        let dest = hooks_dir.join(spec.hook_name);
        let script = build_wrapper_script(spec);
        write_executable_script(&dest, &script)?;
        set_executable(&dest)?;
    }

    let config_path = copilot_home.join("config.json");
    let (mut root, prefix): (serde_json::Value, String) = if config_path.is_file() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?;
        let prefix = jsonc::leading_comment_prefix(&raw).to_string();
        let stripped = jsonc::strip_jsonc_comments(&raw);
        let value = if stripped.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&stripped).with_context(|| {
                format!(
                    "parse {} as JSON before merging hooks",
                    config_path.display()
                )
            })?
        };
        (value, prefix)
    } else {
        (serde_json::json!({}), String::new())
    };

    let manifest = build_copilot_hooks_manifest(&hooks_dir);
    let amplihack_hooks = match manifest {
        serde_json::Value::Object(mut m) => match m.remove("hooks") {
            Some(serde_json::Value::Object(h)) => h,
            _ => serde_json::Map::new(),
        },
        _ => serde_json::Map::new(),
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json root is not a JSON object"))?;
    let hooks_entry = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let hooks_obj = hooks_entry
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json `hooks` is not a JSON object"))?;

    // Replace amplihack-owned entries; keep any user-defined non-amplihack
    // hooks the user has registered for the same event by appending after.
    for (event, new_arr_value) in amplihack_hooks {
        let mut merged = match new_arr_value {
            serde_json::Value::Array(a) => a,
            _ => Vec::new(),
        };
        if let Some(serde_json::Value::Array(existing)) = hooks_obj.remove(&event) {
            merged.extend(
                existing
                    .into_iter()
                    .filter(|entry| !is_amplihack_owned(entry)),
            );
        }
        hooks_obj.insert(event, serde_json::Value::Array(merged));
    }

    let body = serde_json::to_string_pretty(&root)? + "\n";
    fs::write(&config_path, jsonc::apply_prefix(&prefix, body))
        .with_context(|| format!("write {}", config_path.display()))?;
    Ok(())
}

fn is_amplihack_owned(entry: &serde_json::Value) -> bool {
    let bash = entry
        .get("bash")
        .and_then(|v| v.as_str())
        .or_else(|| entry.get("command").and_then(|v| v.as_str()))
        .unwrap_or("");
    bash.contains(".amplihack")
        || bash.contains("/.copilot/.github/hooks/")
        || bash.contains("/.github/hooks/session-start")
        || bash.contains("/.github/hooks/user-prompt-submit")
        || bash.contains("/.github/hooks/pre-tool-use")
        || bash.contains("/.github/hooks/post-tool-use")
        || bash.contains("/.github/hooks/pre-compact")
        || bash.contains("/.github/hooks/stop")
        || bash.contains("/.github/hooks/session-end")
}

pub(super) fn generate_copilot_instructions(copilot_home: &Path) -> Result<()> {
    let agents_dir = copilot_home.join("agents").join("amplihack");
    if !agents_dir.is_dir() {
        return Ok(());
    }

    let instructions_path = copilot_home.join("instructions.md");
    let existing = if instructions_path.exists() {
        fs::read_to_string(&instructions_path)
            .with_context(|| format!("read {}", instructions_path.display()))?
    } else {
        String::new()
    };

    let mut section = String::new();
    section.push_str(INSTRUCTIONS_MARKER_START);
    section.push('\n');

    let mut agent_files = fs_helpers::walk_files(&agents_dir)?;
    agent_files.sort();
    for file in agent_files {
        if file.extension().is_some_and(|ext| ext == "md") {
            let content = fs::read_to_string(&file)
                .with_context(|| format!("read agent instructions {}", file.display()))?;
            section.push_str(&content);
            section.push('\n');
        }
    }

    section.push_str(INSTRUCTIONS_MARKER_END);
    section.push('\n');

    let updated = replace_or_append_section(&existing, &section);
    fs::write(&instructions_path, updated)
        .with_context(|| format!("write {}", instructions_path.display()))?;

    Ok(())
}

pub(super) fn replace_or_append_section(existing: &str, section: &str) -> String {
    if let Some(start) = existing.find(INSTRUCTIONS_MARKER_START)
        && let Some(end) = existing.find(INSTRUCTIONS_MARKER_END)
    {
        let before = &existing[..start];
        let after = &existing[end + INSTRUCTIONS_MARKER_END.len()..];
        return format!("{before}{section}{after}");
    }

    if existing.is_empty() {
        return section.to_string();
    }

    format!("{existing}\n{section}")
}

fn should_preserve_user_hook(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("read existing hook {}", path.display()))?;
    Ok(!content.contains("amplihack") && !content.contains("AMPLIHACK"))
}

fn write_executable_script(path: &Path, content: &str) -> Result<()> {
    let normalized = normalize_executable_script_line_endings(content);
    fs::write(path, normalized)
        .with_context(|| format!("write executable script {}", path.display()))
}

pub(super) fn build_wrapper_script(spec: &HookWrapperSpec) -> String {
    let mut script = String::from("#!/usr/bin/env bash\nset -euo pipefail\n\n");

    script.push_str("HOOKS_BIN=\"\"\n");
    script.push_str("for candidate in \\\n");
    script.push_str("    \"$(command -v amplihack-hooks 2>/dev/null)\" \\\n");
    script.push_str("    \"${HOME}/.amplihack/target/release/amplihack-hooks\" \\\n");
    script.push_str("    \"${HOME}/.amplihack/target/debug/amplihack-hooks\"; do\n");
    script.push_str("  if [ -n \"$candidate\" ] && [ -x \"$candidate\" ]; then\n");
    script.push_str("    HOOKS_BIN=\"$candidate\"\n");
    script.push_str("    break\n");
    script.push_str("  fi\n");
    script.push_str("done\n\n");

    script.push_str("if [ -z \"$HOOKS_BIN\" ]; then\n");
    script.push_str("  echo \"[amplihack] hooks binary not found, skipping\" >&2\n");
    script.push_str("  exit 0\n");
    script.push_str("fi\n\n");

    if spec.subcommands.len() == 1 {
        script.push_str(&format!(
            "exec \"$HOOKS_BIN\" {} \"$@\"\n",
            spec.subcommands[0]
        ));
    } else {
        for sub in spec.subcommands {
            script.push_str(&format!("\"$HOOKS_BIN\" {sub} \"$@\" || true\n"));
        }
    }

    normalize_executable_script_line_endings(&script)
}

pub(super) fn error_wrapper_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
LOG="${HOME}/.amplihack/logs/errors.log"
[ -f "$LOG" ] || exit 0
COUNT=$(wc -l < "$LOG" 2>/dev/null || echo 0)
if [ "$COUNT" -gt 0 ]; then
  echo "[amplihack] $COUNT error(s) in $LOG"
  sed -n '1,5p' "$LOG"
fi
"#
}

pub(super) fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let perms = fs::metadata(path)?.permissions();
        let mut new_perms = perms;
        new_perms.set_mode(new_perms.mode() | 0o111);
        fs::set_permissions(path, new_perms)?;
    }
    let _ = path; // suppress unused warning on non-Unix
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::run_output_with_timeout;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn wrapper_script_generation_uses_lf_only_line_endings() {
        let script = build_wrapper_script(&HookWrapperSpec {
            hook_name: "pre-tool-use",
            copilot_event: "preToolUse",
            subcommands: &["pre-tool-use", "post-tool-use"],
        });

        assert!(
            !script.as_bytes().contains(&b'\r'),
            "generated bash wrapper must be LF-only before it is written"
        );
        assert!(script.contains("\"$HOOKS_BIN\" pre-tool-use \"$@\" || true\n"));
        assert!(script.contains("\"$HOOKS_BIN\" post-tool-use \"$@\" || true\n"));
    }

    #[test]
    fn repo_hook_staging_writes_lf_only_executable_scripts() {
        let repo = tempfile::tempdir().unwrap();

        stage_repo_hooks(repo.path()).unwrap();

        assert_generated_hook_scripts_are_lf_only(repo.path().join(".github/hooks").as_path());
    }

    #[test]
    fn user_level_hook_staging_writes_lf_only_executable_scripts() {
        let copilot_home = tempfile::tempdir().unwrap();

        write_user_level_hooks(copilot_home.path()).unwrap();

        assert_generated_hook_scripts_are_lf_only(
            copilot_home.path().join(".github/hooks").as_path(),
        );
    }

    #[test]
    fn executable_script_writer_normalizes_crlf_and_lone_cr_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let crlf_script = dir.path().join("crlf-hook");
        let lone_cr_script = dir.path().join("lone-cr-hook");

        write_executable_script(
            &crlf_script,
            "#!/usr/bin/env bash\r\nset -euo pipefail\r\necho crlf\r\n",
        )
        .unwrap();
        write_executable_script(
            &lone_cr_script,
            "#!/usr/bin/env bash\rset -euo pipefail\recho lone-cr\r",
        )
        .unwrap();

        for script in [&crlf_script, &lone_cr_script] {
            let bytes = fs::read(script).unwrap();
            assert!(
                !bytes.contains(&b'\r'),
                "{} contains carriage returns and will fail under bash",
                script.display()
            );
            assert_bash_accepts_script(script);
        }
    }

    fn assert_generated_hook_scripts_are_lf_only(hooks_dir: &Path) {
        let mut scripts = fs::read_dir(hooks_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name != "amplihack-hooks.json")
            })
            .collect::<Vec<_>>();
        scripts.sort();

        assert!(
            !scripts.is_empty(),
            "expected generated hook scripts in {}",
            hooks_dir.display()
        );
        for script in scripts {
            let bytes = fs::read(&script).unwrap();
            assert!(
                !bytes.contains(&b'\r'),
                "{} contains carriage returns and will fail under bash",
                script.display()
            );
            assert_bash_accepts_script(&script);
        }
    }

    fn assert_bash_accepts_script(script: &Path) {
        // `bash` is spawned by bare name, so this resolves through the
        // process-global $PATH. Sibling tests in this crate (install::binary,
        // install::hook_staging, recipe_runner) legitimately point $PATH at a
        // temp dir to exercise install-location logic; they serialise those
        // mutations on `env_lock`. This spawn is a *reader* of the same global
        // and must join the same lock, or it intermittently fails to spawn
        // ("failed to spawn subprocess") whenever libtest overlaps it with a
        // mutator on another thread.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut command = Command::new("bash");
        command.arg("-n").arg(script);
        let output = run_output_with_timeout(command, Duration::from_secs(2))
            .unwrap_or_else(|err| panic!("failed to run bash -n for {}: {err}", script.display()));

        assert!(
            output.status.success(),
            "bash -n rejected {}:\nstdout:\n{}\nstderr:\n{}",
            script.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // ----------------------------------------------------------------------
    // R5 (issue #1002): wire Copilot's `sessionEnd` event so whole-session
    // teardown fires and subscribers/groups do not leak on Copilot session end.
    //
    // Copilot DOES emit `sessionEnd`, but amplihack never wired it, so the
    // Signal channel was only ever torn down (incorrectly) by the per-turn
    // `agentStop` path. These tests pin the installer contract:
    //   * a `sessionEnd` wrapper exists, dispatching the `session-stop-event`
    //     subcommand (which runs SessionStopHook → teardown),
    //   * the generated user-level `~/.copilot/config.json` registers it,
    //   * `is_amplihack_owned` recognizes the new wrapper so re-running setup
    //     replaces rather than duplicates it.
    //
    // RED: these fail until the `sessionEnd` wrapper + ownership matcher land.
    // ----------------------------------------------------------------------

    #[test]
    fn copilot_wrappers_include_session_end_to_session_stop_event() {
        let spec = COPILOT_HOOK_WRAPPERS
            .iter()
            .find(|s| s.copilot_event == "sessionEnd")
            .expect("a sessionEnd wrapper must be registered so teardown fires");
        assert!(
            spec.subcommands.contains(&"session-stop-event"),
            "sessionEnd must dispatch the session-stop-event subcommand (teardown), got {:?}",
            spec.subcommands
        );
    }

    #[test]
    fn per_turn_agent_stop_wrapper_does_not_teardown_the_channel() {
        // The per-turn `agentStop` wrapper must keep dispatching only `stop`
        // (outbound relay). If it dispatched a teardown subcommand the group
        // would be destroyed after every turn.
        let spec = COPILOT_HOOK_WRAPPERS
            .iter()
            .find(|s| s.copilot_event == "agentStop")
            .expect("agentStop wrapper present");
        assert!(
            !spec.subcommands.contains(&"session-stop-event"),
            "per-turn agentStop must NOT run teardown"
        );
        assert!(spec.subcommands.contains(&"stop"));
    }

    #[test]
    fn user_level_config_registers_session_end_hook() {
        let copilot_home = tempfile::tempdir().unwrap();
        write_user_level_hooks(copilot_home.path()).unwrap();

        let raw = fs::read_to_string(copilot_home.path().join("config.json")).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = cfg["hooks"].as_object().expect("hooks object");
        let arr = hooks
            .get("sessionEnd")
            .and_then(|v| v.as_array())
            .expect("sessionEnd must be registered in ~/.copilot/config.json");
        assert!(!arr.is_empty(), "sessionEnd must have a wrapper entry");
        let bash = arr[0]["bash"]
            .as_str()
            .or_else(|| arr[0]["command"].as_str())
            .unwrap_or("");
        assert!(
            bash.contains("session-end") || bash.contains("session-stop-event"),
            "sessionEnd wrapper must point at the session-end teardown hook; got {bash:?}"
        );
    }

    #[test]
    fn is_amplihack_owned_recognizes_session_end_wrapper() {
        let entry = serde_json::json!({
            "type": "command",
            "bash": "/home/user/.copilot/.github/hooks/session-end \"$@\""
        });
        assert!(
            is_amplihack_owned(&entry),
            "the session-end wrapper must be recognized as amplihack-owned so \
             re-running setup replaces it instead of leaving a duplicate"
        );
    }
}
