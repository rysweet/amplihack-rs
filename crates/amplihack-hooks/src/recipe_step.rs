//! Whether this session is a recipe step (issues #1328, #2533 and #3199).
//!
//! The recipe runner sets [`RECIPE_RUN_ID_ENV`] on every agent it spawns and
//! [`STEP_ID_ENV`] on every agent launch, at any depth. A variable counts when
//! it is set and not empty. The routing hook and the prompt-event hook both
//! call these functions, so the two cannot disagree about a session.

/// Environment marker set by `recipe::run::execute` on every agent it spawns.
pub(crate) const RECIPE_RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";

/// Set by the recipe runner on every agent launch, at any depth.
pub(crate) const STEP_ID_ENV: &str = "AMPLIHACK_STEP_ID";

fn set_and_not_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// [`RECIPE_RUN_ID_ENV`] is set and not empty.
pub(crate) fn run_id_set() -> bool {
    set_and_not_empty(RECIPE_RUN_ID_ENV)
}

/// [`STEP_ID_ENV`] is set and not empty.
pub(crate) fn step_id_set() -> bool {
    set_and_not_empty(STEP_ID_ENV)
}

/// Either variable is set and not empty.
pub(crate) fn is_recipe_step() -> bool {
    run_id_set() || step_id_set()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvVarGuard, env_lock};

    #[test]
    fn either_variable_makes_a_recipe_step_and_empty_counts_as_unset() {
        let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let cases = [
            (None, None, false),
            (Some(""), Some(""), false),
            (Some("run-1"), None, true),
            (None, Some("step-1"), true),
            (Some(""), Some("step-1"), true),
        ];
        for (run, step, expected) in cases {
            let _run = match run {
                Some(v) => EnvVarGuard::set(RECIPE_RUN_ID_ENV, v),
                None => EnvVarGuard::unset(RECIPE_RUN_ID_ENV),
            };
            let _step = match step {
                Some(v) => EnvVarGuard::set(STEP_ID_ENV, v),
                None => EnvVarGuard::unset(STEP_ID_ENV),
            };
            assert_eq!(is_recipe_step(), expected, "run {run:?}, step {step:?}");
        }
    }
}
