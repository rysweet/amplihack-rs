//! Workstream identity and priority class (D7, D9). The environment is
//! passed in as a closure, so nothing here reads the process environment.

use super::model::Class;

pub const WORKSTREAM_ENV: &str = "AMPLIHACK_WORKSTREAM";
pub const WORKSTREAM_ISSUE_ENV: &str = "AMPLIHACK_WORKSTREAM_ISSUE";
pub const RECIPE_RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";
pub const CLASS_ENV: &str = "AMPLIHACK_GOVERNOR_CLASS";
/// The workstream of a start that names none.
pub const DEFAULT_WORKSTREAM: &str = "default";
const MAX_WORKSTREAM: usize = 64;

/// Review recipes shipped in amplifier-bundle/recipes. A fixed list, so a
/// recipe cannot claim a class by its name alone.
const REVIEW_RECIPES: &[&str] = &[
    "consensus-pr-review",
    "workflow-pr-review",
    "workflow-refactor-review",
];
/// Recipe names starting with this are recovery work.
const RECOVERY_PREFIX: &str = "recovery";
/// The coordination marker, counted only as the first word of the task.
const COORD_MARKER: &str = "[coord]";

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// The clap value parser for `--workstream`: `[A-Za-z0-9._-]{1,64}`, but
/// not `.` or `..`.
pub fn parse_workstream(raw: &str) -> Result<String, String> {
    let ok = !raw.is_empty()
        && raw.len() <= MAX_WORKSTREAM
        && raw.chars().all(allowed)
        && raw != "."
        && raw != "..";
    if ok {
        Ok(raw.to_string())
    } else {
        Err(
            "a workstream is 1 to 64 of A-Z, a-z, 0-9, '.', '_' and '-', and not '.' or '..'"
                .into(),
        )
    }
}

/// A run id with every other character replaced by `-`, cut to 64.
fn clean_run_id(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if allowed(c) { c } else { '-' })
        .take(MAX_WORKSTREAM)
        .collect();
    parse_workstream(&cleaned).ok()
}

/// The workstream: the flag, [`WORKSTREAM_ENV`], [`WORKSTREAM_ISSUE_ENV`]
/// (decimal only, as `ws-<n>`), the cleaned [`RECIPE_RUN_ID_ENV`], then
/// `default`. An invalid [`WORKSTREAM_ENV`] gives `default`; its value is
/// never logged, only its length.
pub fn workstream(flag: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> String {
    if let Some(flag) = flag {
        return flag.to_string();
    }
    if let Some(raw) = env(WORKSTREAM_ENV) {
        return match parse_workstream(&raw) {
            Ok(ws) => ws,
            Err(_) => {
                tracing::warn!(
                    len = raw.len(),
                    "{WORKSTREAM_ENV} is not a valid workstream; using default"
                );
                DEFAULT_WORKSTREAM.to_string()
            }
        };
    }
    if let Some(issue) = env(WORKSTREAM_ISSUE_ENV)
        && !issue.is_empty()
        && issue.len() <= 20
        && issue.bytes().all(|b| b.is_ascii_digit())
    {
        return format!("ws-{issue}");
    }
    if let Some(id) = env(RECIPE_RUN_ID_ENV).and_then(|raw| clean_run_id(&raw)) {
        return id;
    }
    DEFAULT_WORKSTREAM.to_string()
}

/// What the work itself says: `[coord]` as the first word of the task, or
/// a recovery or review recipe.
fn infer(recipe: Option<&str>, task: Option<&str>) -> Option<Class> {
    if task.and_then(|t| t.split_whitespace().next()) == Some(COORD_MARKER) {
        return Some(Class::Coordination);
    }
    let recipe = recipe?;
    if recipe.starts_with(RECOVERY_PREFIX) {
        return Some(Class::Recovery);
    }
    REVIEW_RECIPES.contains(&recipe).then_some(Class::Review)
}

/// The class: the flag, [`CLASS_ENV`] when it names a class, inference,
/// then `ordinary`.
pub fn class(
    flag: Option<Class>,
    env: &dyn Fn(&str) -> Option<String>,
    recipe: Option<&str>,
    task: Option<&str>,
) -> Class {
    flag.or_else(|| env(CLASS_ENV).and_then(|raw| Class::parse(raw.trim())))
        .or_else(|| infer(recipe, task))
        .unwrap_or(Class::Ordinary)
}
