//! Add caller command hooks to the Codex session-flags layer.
//! Codex discovers the other hooks from their original layers in order.

use super::hook_settings::{ExtraHooks, commands_in, unique_groups};
use serde_json::Value;
use std::collections::HashSet;
use toml_edit::{Array, InlineTable, Value as TomlValue};

type Version = (u32, u32, u32);

// Codex 0.140 ignores inline hooks. 0.141 loads them from the session flags.
const INLINE_HOOKS: Version = (0, 141, 0);
const HOOK_TRUST_FLAG: Version = (0, 131, 0);

fn event_supported(event: &str, version: Version) -> bool {
    match event {
        "PreToolUse" | "PermissionRequest" | "PostToolUse" | "PreCompact" | "PostCompact"
        | "SessionStart" | "UserPromptSubmit" | "Stop" | "SubagentStart" | "SubagentStop" => {
            version >= INLINE_HOOKS
        }
        "SessionEnd" => version >= (0, 145, 0),
        _ => false,
    }
}

pub(super) struct LaunchHooks<'a> {
    index: usize,
    overrides: Vec<String>,
    omitted_events: Vec<&'a str>,
    disabled: Option<&'static str>,
    omit_trust_flag: bool,
}

pub(super) fn prepare<'a>(
    extra: &'a ExtraHooks,
    version: Option<&str>,
    args: &[String],
    local_model: bool,
) -> Option<LaunchHooks<'a>> {
    let mut index = super::codex_agent_step::exec_subcommand_index(args)? + 1;
    let version = version.and_then(super::contract_file::parse_claude_version);
    let omit_trust_flag = !local_model && version.is_some_and(|version| version < HOOK_TRUST_FLAG);
    let version = version.unwrap_or_default();
    let reads_user_config =
        !local_model && !super::codex_agent_step::has_option(args, "--ignore-user-config", None);
    let disabled = if version < INLINE_HOOKS {
        Some("per-launch configuration unavailable for this Codex version")
    } else if super::codex_local_model::caller_disables_hooks(args)
        || (reads_user_config
            && super::codex_hook_check::home_disables_hooks()
            && !enables_hooks(args))
    {
        Some("hooks are disabled in Codex configuration")
    } else {
        None
    };
    // Codex replaces arrays within the session flags layer. Start with the
    // effective caller array and put the combined override after the caller's
    // hooks flags, including an assignment to the entire hooks table.
    let caller_hooks: Vec<_> = super::codex_agent_step::config_overrides(args)
        .filter_map(|(end, pair)| {
            let (key, value) = pair.split_once('=')?;
            let key = key.trim();
            if key != "hooks" && !key.starts_with("hooks.") {
                return None;
            }
            index = index.max(end);
            Some((key, value.trim().parse::<TomlValue>().ok()))
        })
        .collect();
    let home_hooks = if disabled.is_none() && reads_user_config {
        super::codex_hook_check::home_hooks()
    } else {
        None
    };
    let mut overrides = Vec::new();
    let mut omitted_events = Vec::new();
    for (event, groups) in extra.events() {
        if groups
            .as_array()
            .expect("validated matcher groups")
            .iter()
            .all(|group| {
                group["hooks"]
                    .as_array()
                    .expect("validated command array")
                    .is_empty()
            })
        {
            continue;
        }
        if disabled.is_some() || !event_supported(event, version) {
            omitted_events.push(event.as_str());
            continue;
        }
        let Ok(mut converted) = caller_event(&caller_hooks, event) else {
            omitted_events.push(event.as_str());
            continue;
        };
        let caller_len = converted.len();
        let mut seen = caller_commands(&converted);
        if let Some(existing) = home_hooks
            .as_ref()
            .and_then(|hooks| hooks[event].as_array())
        {
            seen.extend(commands_in(existing));
        }
        let mut omitted = false;
        for group in unique_groups(groups, &mut seen) {
            if let Some(group) = convert_group(&group, &mut omitted) {
                converted.push(group);
            }
        }
        if converted.len() > caller_len {
            overrides.extend(["-c".into(), format!("hooks.{event}={converted}")]);
        }
        if omitted {
            omitted_events.push(event.as_str());
        }
    }
    Some(LaunchHooks {
        index,
        overrides,
        omitted_events,
        disabled,
        omit_trust_flag,
    })
}

fn enables_hooks(args: &[String]) -> bool {
    super::codex_agent_step::has_option(args, "--enable", Some("hooks"))
        || super::codex_agent_step::config_overrides(args).any(|(_, pair)| {
            let Some((key, value)) = pair.split_once('=') else {
                return false;
            };
            match key.trim() {
                "features.hooks" => value.trim() == "true",
                "features" => {
                    value
                        .trim()
                        .parse::<TomlValue>()
                        .ok()
                        .and_then(|table| table.as_inline_table()?.get("hooks")?.as_bool())
                        == Some(true)
                }
                _ => false,
            }
        })
}

fn caller_event(overrides: &[(&str, Option<TomlValue>)], event: &str) -> Result<Array, ()> {
    for (key, value) in overrides.iter().rev() {
        if *key == "hooks" {
            let table = value
                .as_ref()
                .and_then(TomlValue::as_inline_table)
                .ok_or(())?;
            return match table.get(event) {
                Some(value) => value.as_array().cloned().ok_or(()),
                None => Ok(Array::new()),
            };
        }
        if let Some((_, path)) = key.split_once('.') {
            if path == event {
                return value
                    .as_ref()
                    .and_then(TomlValue::as_array)
                    .cloned()
                    .ok_or(());
            }
            if path.split_once('.').is_some_and(|(root, _)| root == event) {
                return Err(());
            }
        }
    }
    Ok(Array::new())
}

fn caller_commands(groups: &Array) -> HashSet<String> {
    groups
        .iter()
        .filter_map(TomlValue::as_inline_table)
        .filter_map(|group| group.get("hooks")?.as_array())
        .flat_map(Array::iter)
        .filter_map(TomlValue::as_inline_table)
        .filter(|handler| handler.get("type").and_then(TomlValue::as_str) == Some("command"))
        .filter_map(|handler| handler.get("command")?.as_str())
        .map(str::to_owned)
        .collect()
}

impl LaunchHooks<'_> {
    pub(super) fn omit_trust_flag(&self) -> bool {
        self.omit_trust_flag && !self.omitted_events.is_empty()
    }

    pub(super) fn arguments(&self, args: &[String]) -> Option<Vec<String>> {
        if self.overrides.is_empty() {
            return None;
        }
        let mut args = args.to_vec();
        args.splice(self.index..self.index, self.overrides.iter().cloned());
        Some(args)
    }

    pub(super) fn report_omissions(self, step: &str) {
        if self.omitted_events.is_empty() {
            return;
        }
        let events = self.omitted_events.join(",");
        let reason = self
            .disabled
            .unwrap_or("events or command options unavailable in Codex");
        let line = format!(
            "amplihack: Codex step {step} launched with extra hooks omitted (events={events}; {reason})"
        );
        eprintln!("{line}");
        crate::commands::profile::limit_wait::append_recipe_log(&line);
    }
}

fn convert_group(group: &Value, omitted: &mut bool) -> Option<InlineTable> {
    let mut handlers = Array::new();
    for handler in group["hooks"].as_array().expect("validated command array") {
        if let Some(handler) = convert_command(handler) {
            handlers.push(handler);
        } else {
            *omitted = true;
        }
    }
    if handlers.is_empty() {
        return None;
    }
    let mut table = InlineTable::new();
    if let Some(matcher) = group["matcher"].as_str() {
        table.insert("matcher", matcher.into());
    }
    table.insert("hooks", handlers.into());
    Some(table)
}

fn convert_command(handler: &Value) -> Option<InlineTable> {
    let mut table = InlineTable::new();
    for (key, value) in handler.as_object().expect("validated hook command") {
        let value = match key.as_str() {
            "type" | "command" | "statusMessage" => value.as_str()?.into(),
            "timeout" => value.as_i64()?.into(),
            "async" => value.as_bool()?.into(),
            // Claude ignores once in settings files.
            "once" => continue,
            // A different shell, filter, or failure policy could run the
            // command under conditions the caller did not choose.
            _ => return None,
        };
        table.insert(key, value);
    }
    Some(table)
}
