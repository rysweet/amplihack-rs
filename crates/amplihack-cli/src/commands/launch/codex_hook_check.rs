//! Issue #3435: whether a Codex step has amplihack's hooks, which give a
//! running step the guidance appended after it started.
//!
//! A Codex step has them when the effective Codex home (`CODEX_HOME`, else
//! `~/.codex`) holds a `hooks.json` with amplihack's PostToolUse and
//! UserPromptSubmit handlers, its `config.toml` does not set
//! `[features] hooks = false`, and the caller's arguments do not switch
//! hooks off. A local-model launch uses a Codex home of amplihack's own
//! with only the PreToolUse handler, so it has none.
//!
//! This is a judgement read from files, not proof that a hook runs: a
//! command that names `amplihack-hooks` or a `bin/hook` wrapper may still
//! fail to start, and a Codex build may not fire the event. A wrong "yes"
//! does not silence the sender. The launch records the step as
//! `guidance_step_hooked`, and until one of its hooks reaches `amplihack`
//! and records `guidance_step_hook_seen`, `recipe guide` names the running
//! step as not yet confirmed (`crate::run_guidance::step`). A wrong "no"
//! records `guidance_step_unhooked`; a hook that does fire still delivers.

use std::path::{Path, PathBuf};

/// The largest `hooks.json` or `config.toml` read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Whether this Codex launch has amplihack's guidance hooks.
pub(super) fn codex_step_has_hook(local_model: bool, args: &[String]) -> bool {
    if local_model || super::codex_local_model::caller_disables_hooks(args) {
        return false;
    }
    let Some(home) = codex_home() else {
        return false;
    };
    let Some(hooks) = read_small(&home.join("hooks.json")) else {
        return false;
    };
    hooks_ready(&hooks, read_small(&home.join("config.toml")).as_deref())
}

fn codex_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".codex"))
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Whether `hooks_json` holds amplihack's PostToolUse and UserPromptSubmit
/// handlers and `config_toml` leaves hooks on.
pub(super) fn hooks_ready(hooks_json: &str, config_toml: Option<&str>) -> bool {
    if config_toml.is_some_and(hooks_switched_off) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(hooks_json) else {
        return false;
    };
    has_handler(&value, "PostToolUse", &["post-tool-use"])
        && has_handler(
            &value,
            "UserPromptSubmit",
            &["user-prompt", "user-prompt-submit"],
        )
}

fn hooks_switched_off(config: &str) -> bool {
    toml::from_str::<toml::Table>(config)
        .ok()
        .and_then(|table| table.get("features")?.get("hooks")?.as_bool())
        == Some(false)
}

fn has_handler(value: &serde_json::Value, event: &str, subcommands: &[&str]) -> bool {
    let Some(groups) = value
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    groups.iter().any(|group| {
        group
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|handlers| {
                handlers.iter().any(|handler| {
                    handler
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|command| runs_amplihack_hook(command, subcommands))
                })
            })
    })
}

/// Whether `command` runs amplihack's hooks binary, or the Codex plugin's
/// `bin/hook` wrapper, with one of `subcommands` as its last word.
fn runs_amplihack_hook(command: &str, subcommands: &[&str]) -> bool {
    let Ok(words) = shell_words::split(command) else {
        return false;
    };
    let Some((last, rest)) = words.split_last() else {
        return false;
    };
    subcommands.contains(&last.as_str())
        && rest.iter().any(|word| {
            let path = Path::new(word);
            match path.file_name().and_then(|n| n.to_str()) {
                Some("amplihack-hooks") => true,
                Some("hook") => {
                    path.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("bin"))
                }
                _ => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(post: &str, prompt: &str) -> String {
        serde_json::json!({"hooks": {
            "PostToolUse": [{"hooks": [{"type": "command", "command": post}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": prompt}]}],
        }})
        .to_string()
    }

    #[test]
    fn the_plugin_wrapper_and_the_hooks_binary_both_count() {
        assert!(hooks_ready(
            &hooks("'/p/bin/hook' post-tool-use", "'/p/bin/hook' user-prompt"),
            None
        ));
        assert!(hooks_ready(
            &hooks(
                "/x/amplihack-hooks post-tool-use",
                "env AMPLIHACK_HOOK_PROTOCOL=codex /x/amplihack-hooks user-prompt-submit"
            ),
            None
        ));
    }

    #[test]
    fn both_events_are_needed() {
        let only_post = serde_json::json!({"hooks": {
            "PostToolUse": [{"hooks": [{"command": "/p/bin/hook post-tool-use"}]}],
        }})
        .to_string();
        assert!(!hooks_ready(&only_post, None));
        assert!(!hooks_ready(
            &hooks("/p/bin/hook pre-tool-use", "/p/bin/hook user-prompt"),
            None
        ));
        assert!(!hooks_ready(
            &hooks("/usr/bin/other post-tool-use", "/p/bin/hook user-prompt"),
            None
        ));
        assert!(!hooks_ready("not json", None));
    }

    #[test]
    fn hooks_switched_off_in_config_count_as_none() {
        let ready = hooks("/p/bin/hook post-tool-use", "/p/bin/hook user-prompt");
        assert!(!hooks_ready(&ready, Some("[features]\nhooks = false\n")));
        assert!(hooks_ready(&ready, Some("[features]\nhooks = true\n")));
        assert!(hooks_ready(&ready, Some("model = \"m\"\n")));
    }

    #[test]
    fn a_local_model_or_a_caller_switching_hooks_off_has_none() {
        assert!(!codex_step_has_hook(true, &[]));
        let off = ["-c".to_string(), "features.hooks=false".to_string()];
        assert!(!codex_step_has_hook(false, &off));
        assert!(!codex_step_has_hook(
            false,
            &["--disable".to_string(), "hooks".to_string()]
        ));
    }
}
