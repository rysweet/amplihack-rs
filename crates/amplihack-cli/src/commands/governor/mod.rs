//! The capacity governor (issue #1811): decides whether a recipe run or an
//! agent launch may start now, from the usage readings of every login
//! profile, a concurrency limit that follows the burn rate, protected
//! reserves for coordination, recovery and review work, and a fair queue
//! between workstreams. It never moves an account, never spawns a process
//! and writes only inside `<profile root>/.governor.d`.
//!
//! See docs/reference/capacity-governor.md.

mod admit;
mod admit_state;
mod budget;
mod config;
mod files;
mod guard;
mod identity;
mod inputs;
mod liveness;
mod manifest;
mod model;
mod policy;
mod queue;
mod rate;
mod status;
mod status_table;
mod store;

#[cfg(all(test, unix))]
mod tests_admit;
#[cfg(test)]
mod tests_budget;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_identity;
#[cfg(all(test, unix))]
mod tests_liveness;
#[cfg(test)]
mod tests_policy;
#[cfg(test)]
mod tests_policy_flow;
#[cfg(test)]
mod tests_queue;
#[cfg(test)]
mod tests_rate;
#[cfg(all(test, unix))]
mod tests_seed;
#[cfg(test)]
mod tests_source_scan;
#[cfg(all(test, unix))]
mod tests_status;
#[cfg(all(test, unix))]
mod tests_store;

pub use guard::{Denied, SlotGuard};
pub use model::Class;

use admit::AdmitContext;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The exit code of a start the governor did not allow.
pub const EXIT_GOVERNOR_DENIED: i32 = 80;
/// `governor status` only: the workstream usage manifest for `token_share`.
pub const GOVERNOR_MANIFEST_ENV: &str = "AMPLIHACK_GOVERNOR_MANIFEST";

/// The governor's flags on `launch`, `claude` and `recipe run`.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct GovernorArgs {
    /// Workstream this start belongs to, for the fair queue
    #[arg(long = "workstream", value_name = "ID", value_parser = identity::parse_workstream)]
    pub workstream: Option<String>,
    /// Priority class: coordination, recovery, review or ordinary
    #[arg(long = "governor-class", value_name = "CLASS", value_enum)]
    pub governor_class: Option<Class>,
    /// How long to wait for capacity: digits with an optional s, m or h; 0
    /// does not wait
    #[arg(long = "governor-wait", value_name = "DURATION", value_parser = config::parse_wait)]
    pub governor_wait: Option<u64>,
}

#[derive(Debug, clap::Subcommand)]
pub enum GovernorCommands {
    /// Show the concurrency limit, budgets, slots, queue and workstreams
    Status {
        /// Print the JSON document instead of the table
        #[arg(long)]
        json: bool,
    },
}

/// One start to decide.
pub struct AdmitRequest {
    args: Option<GovernorArgs>,
    recipe: Option<String>,
    task: Option<String>,
}

impl AdmitRequest {
    /// `amplihack launch` or `amplihack claude`; the task is the text after
    /// `-p` or `--print`, read only for the `[coord]` marker.
    pub fn launch(args: &GovernorArgs, claude_args: &[String]) -> Self {
        let task = claude_args
            .iter()
            .position(|a| a == "-p" || a == "--print")
            .and_then(|i| claude_args.get(i + 1))
            .cloned();
        Self {
            args: Some(args.clone()),
            recipe: None,
            task,
        }
    }

    /// `amplihack recipe run`. `None` arguments (a dry run, or a call from
    /// inside amplihack) admit without asking.
    pub fn recipe(args: Option<&GovernorArgs>, recipe: &str, task: Option<&str>) -> Self {
        Self {
            args: args.cloned(),
            recipe: Some(recipe.to_string()),
            task: task.map(str::to_string),
        }
    }
}

static SETTLED_TREE_ID: OnceLock<String> = OnceLock::new();

/// The tree id the admission of this process recorded with its slot, when
/// `AMPLIHACK_TREE_ID` was not inherited. The environment builder and the
/// recipe runner hand it to children, so their launches find the slot.
pub fn settled_tree_id() -> Option<String> {
    SETTLED_TREE_ID.get().cloned()
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn settings() -> config::Settings {
    let user = crate::user_config::load_user_only();
    config::resolve(&env, &|key| user.file_value(key).cloned())
}

/// The profile root, when it is an existing directory.
fn root() -> Option<PathBuf> {
    crate::commands::profile::governor_view::root_from_env().filter(|r| r.is_dir())
}

/// Decides one start, waiting for capacity when told to; see "Waiting,
/// denial and exit code 80" in docs/reference/capacity-governor.md. Keep the
/// guard until the work ends.
pub fn admit(request: AdmitRequest) -> Result<SlotGuard, Denied> {
    let Some(args) = request.args else {
        return Ok(SlotGuard::inert(None));
    };
    let settings = settings();
    if !settings.enabled {
        return Ok(SlotGuard::inert(None));
    }
    let root = root();
    let ctx = AdmitContext {
        root: root.as_deref(),
        settings,
        class: identity::class(
            args.governor_class,
            &env,
            request.recipe.as_deref(),
            request.task.as_deref(),
        ),
        workstream: identity::workstream(args.workstream.as_deref(), &env),
        wait_secs: config::wait_secs(args.governor_wait, &env, &settings),
        session_depth: env("AMPLIHACK_SESSION_DEPTH"),
        tree_id: env("AMPLIHACK_TREE_ID"),
        task_text: request.task,
    };
    let guard = admit::admit_with(&ctx)?;
    if ctx.tree_id.is_none()
        && let Some(id) = guard.tree_id()
    {
        let _ = SETTLED_TREE_ID.set(id.to_string());
    }
    Ok(guard)
}

/// `amplihack governor ...`.
pub fn run(cmd: GovernorCommands) -> Result<()> {
    match cmd {
        GovernorCommands::Status { json } => {
            let manifest = std::env::var_os(GOVERNOR_MANIFEST_ENV).map(PathBuf::from);
            let v = status::view(
                root().as_deref(),
                &settings(),
                manifest.as_deref(),
                store::now(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                print!("{}", status::render_table(&v));
            }
            Ok(())
        }
    }
}
