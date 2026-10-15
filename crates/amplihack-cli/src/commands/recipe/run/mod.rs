use super::*;

mod binary;
mod correlation;
pub(crate) mod execute;
/// Issue #1267 — transient vs terminal classification of a failed run.
mod failure_class;
mod format;
/// Issue #1484 — `gh` compatibility layer for hosts that block GraphQL.
mod gh_compat;
/// Issue #1267 — bounded mechanical retry for transient transport faults.
mod retry;

use execute::execute_recipe_via_rust;
use format::format_recipe_run_result;

#[cfg(test)]
fn execute_recipe_via_rust_for_test(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    verbose: bool,
    working_dir: &Path,
    search_dirs: &[PathBuf],
    step_timeout: Option<u64>,
) -> Result<RecipeRunResult> {
    struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    let inherited_identity = [
        (
            "AMPLIHACK_TREE_ID",
            option_env!("AMPLIHACK_TREE_ID").map(std::ffi::OsStr::new),
        ),
        (
            "AMPLIHACK_SESSION_DEPTH",
            option_env!("AMPLIHACK_SESSION_DEPTH").map(std::ffi::OsStr::new),
        ),
    ];
    let is_outer_orchestration = inherited_identity
        .iter()
        .all(|(key, inherited)| std::env::var_os(key).as_deref() == *inherited);
    let _restore = is_outer_orchestration.then(|| {
        EnvRestore(
            [
                "AMPLIHACK_SESSION_TREE_DIR",
                "AMPLIHACK_TREE_ID",
                "AMPLIHACK_SESSION_DEPTH",
                "AMPLIHACK_MAX_DEPTH",
                "AMPLIHACK_RECIPE_RUN_ID",
            ]
            .into_iter()
            .map(|key| {
                let previous = std::env::var_os(key);
                unsafe { std::env::remove_var(key) };
                (key, previous)
            })
            .collect(),
        )
    });

    execute_recipe_via_rust(
        recipe_path,
        context,
        dry_run,
        verbose,
        working_dir,
        search_dirs,
        step_timeout,
    )
}

/// Build the ordered, deduplicated list of sub-recipe search dirs to forward
/// to recipe-runner-rs as `-R` flags (issue #494).
///
/// Order:
///   1. The recipe's own parent directory (so co-located sub-recipes win).
///   2. Canonical `recipe_search_dirs(None, working_dir)` output (anchored,
///      env, and home-based dirs; same list amplihack uses to resolve
///      top-level recipes).
///
/// Duplicates are removed using `push_unique_path` (path equality only — no
/// canonicalisation here; upstream is responsible for resolving symlinks).
fn build_search_dirs(recipe_path: &Path, working_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(parent) = recipe_path.parent()
        && !parent.as_os_str().is_empty()
    {
        push_unique_path(&mut dirs, parent.to_path_buf());
    }
    for dir in recipe_search_dirs(None, working_dir)? {
        push_unique_path(&mut dirs, dir);
    }
    Ok(dirs)
}

pub fn run_recipe(
    recipe_path: &str,
    context_args: &[String],
    dry_run: bool,
    verbose: bool,
    format: &str,
    working_dir: Option<&str>,
    step_timeout: Option<u64>,
) -> Result<()> {
    run_recipe_with(
        recipe_path,
        context_args,
        dry_run,
        verbose,
        format,
        working_dir,
        step_timeout,
        &RootSandboxPreflight::live(),
        &mut io::stderr(),
    )
}

/// Where the issue #1482 pre-flight gets the agent binary and the
/// root-sandbox decision; injected so tests reach the call in
/// [`run_recipe_with`] whatever uid and configuration they run under.
pub(crate) struct RootSandboxPreflight {
    pub(crate) agent_binary: fn() -> String,
    pub(crate) decision: fn() -> amplihack_utils::root_sandbox::SkipPermissionsEnv,
}

impl RootSandboxPreflight {
    fn live() -> Self {
        Self {
            agent_binary: crate::env_builder::active_agent_binary,
            decision: amplihack_utils::root_sandbox::detect,
        }
    }
}

/// [`run_recipe`] with the root-sandbox pre-flight's inputs injected and its
/// output (the notice, or the refusal) written to `preflight_out`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_recipe_with(
    recipe_path: &str,
    context_args: &[String],
    dry_run: bool,
    verbose: bool,
    format: &str,
    working_dir: Option<&str>,
    step_timeout: Option<u64>,
    preflight: &RootSandboxPreflight,
    preflight_out: &mut dyn Write,
) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let (context, errors) = parse_context_args(context_args);
    if !errors.is_empty() {
        for error in errors {
            writeln!(io::stderr(), "Error: {error}")?;
        }
        return Err(exit_error(1));
    }

    let working_dir = working_dir.unwrap_or(".");
    let abs_working_dir = validate_path(working_dir, false)?;
    let validated_path = resolve_recipe_path(recipe_path, &abs_working_dir)?;
    let recipe = parse_recipe_from_path(&validated_path)?;
    let (merged_context, inferred) = infer_missing_context(&recipe.context, &context);
    if verbose {
        writeln!(io::stderr(), "Executing recipe: {}", recipe.name)?;
        if dry_run {
            writeln!(io::stderr(), "DRY RUN MODE - No actual execution")?;
        }
        if !inferred.is_empty() {
            writeln!(
                io::stderr(),
                "[context] Inferred {} variable(s): {}",
                inferred.len(),
                inferred.join(", ")
            )?;
        }
    }
    if !dry_run
        && let Err(error) = preflight_root_sandbox(
            &recipe,
            &(preflight.agent_binary)(),
            &(preflight.decision)(),
            preflight_out,
        )
    {
        writeln!(preflight_out, "Error: {error}")?;
        return Err(exit_error(1));
    }
    let search_dirs = build_search_dirs(&validated_path, &abs_working_dir)?;
    let result = match execute_recipe_via_rust(
        &validated_path,
        &merged_context,
        dry_run,
        verbose,
        &abs_working_dir,
        &search_dirs,
        step_timeout,
    ) {
        Ok(result) => result,
        Err(error) => {
            // Issue #1326: preserve a distinguishing exit status when the failure
            // carries one (e.g. EXIT_ORCHESTRATION_UNAVAILABLE = 79 for a policy
            // refusal). Collapsing everything to 1 is what made a terminal refusal
            // indistinguishable from a transient fault, which is what agents then
            // retried around. Such errors have already reported themselves, so do
            // not print them a second time.
            if let Some(code) = crate::command_error::exit_code(&error) {
                return Err(exit_error(code));
            }
            writeln!(io::stderr(), "Error: {error}")?;
            return Err(exit_error(1));
        }
    };

    println!("{}", format_recipe_run_result(&result, format, false)?);

    if result.success {
        Ok(())
    } else {
        Err(exit_error(1))
    }
}

/// Issue #1482: fail before the runner starts, rather than inside the first
/// agent step, when agent steps would launch `claude
/// --dangerously-skip-permissions` as root and Claude Code would refuse it.
///
/// Sub-recipe steps count as agent steps: what they run is not known here.
///
/// When amplihack sets `IS_SANDBOX=1` by itself, the notice is written to
/// `notices` (stderr) once, here. The nested `amplihack claude` processes
/// print it too, but the recipe runner keeps their stderr in temp files and
/// shows it only when a step fails, so this is the copy the user sees.
fn preflight_root_sandbox(
    recipe: &RecipeDoc,
    agent_binary: &str,
    decision: &amplihack_utils::root_sandbox::SkipPermissionsEnv,
    notices: &mut dyn Write,
) -> Result<()> {
    if agent_binary != "claude" {
        return Ok(());
    }
    let launches_agents = recipe.steps.iter().any(|step| {
        matches!(
            super::show_validate::infer_step_type(step),
            "agent" | "recipe"
        )
    });
    if !launches_agents {
        return Ok(());
    }
    decision.check().map_err(|error| {
        anyhow::anyhow!("recipe pre-flight failed for '{}': {error}", recipe.name)
    })?;
    if let Some(notice) = decision.notice() {
        writeln!(notices, "{notice}")?;
    }
    Ok(())
}

fn parse_context_args(context_args: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut context = BTreeMap::new();
    let mut errors = Vec::new();

    for arg in context_args {
        if let Some((key, value)) = arg.split_once('=') {
            context.insert(key.to_string(), value.to_string());
        } else {
            errors.push(format!(
                "Invalid context format '{arg}'. Use key=value format (e.g., -c 'question=What is X?' -c 'var=value')"
            ));
        }
    }

    (context, errors)
}

fn infer_missing_context(
    recipe_defaults: &BTreeMap<String, Value>,
    user_context: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut merged = recipe_defaults
        .iter()
        .map(|(key, value)| (key.clone(), scalar_to_context_value(value)))
        .collect::<BTreeMap<_, _>>();

    for (key, value) in user_context {
        merged.insert(key.clone(), value.clone());
    }

    let mut inferred = Vec::new();
    let keys = merged.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if merged.get(&key).is_some_and(|value| !value.is_empty()) {
            continue;
        }

        let env_key = format!("AMPLIHACK_CONTEXT_{}", key.to_uppercase());
        if let Ok(value) = std::env::var(&env_key)
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from ${env_key})"));
            continue;
        }

        if key == "task_description"
            && let Ok(value) = std::env::var("AMPLIHACK_TASK_DESCRIPTION")
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from $AMPLIHACK_TASK_DESCRIPTION)"));
        } else if key == "repo_path" {
            let value = std::env::var("AMPLIHACK_REPO_PATH").unwrap_or_else(|_| ".".to_string());
            if value != "." {
                inferred.push(format!("{key} (from $AMPLIHACK_REPO_PATH)"));
            }
            merged.insert(key.clone(), value);
        }
    }

    (merged, inferred)
}

fn scalar_to_context_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(v) => {
            if *v {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

#[cfg(test)]
mod tests_context;
#[cfg(test)]
mod tests_execute;
#[cfg(test)]
mod tests_failure_class;
#[cfg(test)]
mod tests_format;
#[cfg(test)]
mod tests_root_sandbox;
#[cfg(test)]
mod tests_teardown;
#[cfg(test)]
mod tests_terminal_record;
