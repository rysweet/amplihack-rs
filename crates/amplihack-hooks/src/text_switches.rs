//! The two text switches as the hook program reads them (issue #3247).
//!
//! `text.routing` and `text.prompt_preferences` come from
//! [`amplihack_utils::user_config::text_switch`], the same code
//! `amplihack config get` uses: the key's environment variable, then the
//! project file, then the user file under `HOME`, then the default `on`.
//! Nothing here starts a process or writes a file.
//!
//! A hook that reads a switch reports what it found:
//!
//! - an unknown value: one line straight on standard error, quoted, cut to 64
//!   characters and escaped, as `run_hook` does for `AMPLIHACK_HOOK_TEXT`;
//! - a file that cannot be read or parsed: one `warn` line in the hook's log;
//! - a switch that is off: one `info` line in the hook's log naming the key and
//!   where its value came from.
//!
//! None of these is added to the model's context, and none fails the hook.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use amplihack_utils::user_config::text_switch::{
    self, Layers, PROMPT_PREFERENCES, ROUTING, Setting, Source, TextSwitch, Unknown,
};

/// Characters of an unknown value quoted in its report.
const RECORD_CHARS: usize = 64;

/// Whether `text.routing` is on for this hook run.
pub(crate) fn routing_on() -> bool {
    is_on(ROUTING)
}

/// Whether `text.prompt_preferences` is on for this hook run.
pub(crate) fn prompt_preferences_on() -> bool {
    is_on(PROMPT_PREFERENCES)
}

/// The switch's value from this process's environment, working directory and
/// `HOME`.
fn read(switch: TextSwitch) -> Setting {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env = |name: &str| std::env::var(name).ok();
    text_switch::read(
        switch,
        &Layers {
            home: home.as_deref(),
            cwd: &cwd,
            read_project: true,
            env: &env,
        },
    )
}

fn is_on(switch: TextSwitch) -> bool {
    let setting = read(switch);
    for problem in &setting.problems {
        if first_time(format!("file {}", problem.path.display())) {
            tracing::warn!(
                path = %problem.path.display(),
                reason = %problem.reason,
                "amplihack config file skipped; its settings are not used"
            );
        }
    }
    if let Some(unknown) = &setting.unknown
        && first_time(format!("unknown {}", switch.key))
    {
        let _ = writeln!(
            std::io::stderr(),
            "{}",
            unknown_value_record(switch, unknown)
        );
    }
    if !setting.on {
        tracing::info!(
            key = switch.key,
            source = setting.source.label(),
            path = source_path(&setting.source),
            "text switch off"
        );
    }
    setting.on
}

/// `true` the first time `what` is seen in this process.
fn first_time(what: String) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(what)
}

fn source_path(source: &Source) -> String {
    match source {
        Source::Env(var) => (*var).to_string(),
        Source::Project(path) | Source::User(path) => path.display().to_string(),
        Source::Default => String::new(),
    }
}

/// One line naming the setting, where the value came from and the value,
/// cut to [`RECORD_CHARS`] characters (never bytes) and Debug-escaped, so a
/// line break or an escape sequence can neither split the line nor reach a
/// terminal raw.
pub(crate) fn unknown_value_record(switch: TextSwitch, unknown: &Unknown) -> String {
    let cut: String = unknown.raw.chars().take(RECORD_CHARS).collect();
    let place = match &unknown.source {
        Source::Env(var) => (*var).to_string(),
        Source::Project(_) => format!("{} in the project file", switch.key),
        Source::User(_) => format!("{} in the user file", switch.key),
        Source::Default => switch.key.to_string(),
    };
    format!(
        "amplihack-hooks: unknown {place}={cut:?}; only on and off are supported; keeping the default on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_value_is_cut_and_escaped_on_one_line() {
        let raw = format!("x\n\u{1b}[31m{}", "y".repeat(200));
        let record = unknown_value_record(
            ROUTING,
            &Unknown {
                raw,
                source: Source::Env(ROUTING.env),
            },
        );
        assert!(!record.contains('\n'));
        assert!(!record.contains('\u{1b}'));
        assert!(!record.contains(&"y".repeat(RECORD_CHARS)));
        assert!(record.contains(&"y".repeat(10)));
    }
}
