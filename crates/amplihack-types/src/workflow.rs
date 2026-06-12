//! Canonical workflow labels used by generated context.

/// Canonical selected/default workflow target shown to users and agents.
pub const DEFAULT_WORKFLOW_SELECTION: &str = "`default-workflow` skill/recipe";

/// Canonical top-level development orchestrator skill.
pub const DEV_ORCHESTRATOR_SKILL: &str = "`dev-orchestrator`";

/// Canonical recipe-runner entry point used by the orchestrator.
pub const SMART_ORCHESTRATOR_RECIPE_COMMAND: &str = "`amplihack recipe run smart-orchestrator`";

/// Session-start workflow guidance for top-level agent sessions.
pub const DEFAULT_WORKFLOW_SESSION_CONTEXT: &str = "\
## Default Workflow

The canonical default workflow is the `default-workflow` skill/recipe.

For development and investigation tasks, invoke the `dev-orchestrator` skill; it routes through `amplihack recipe run smart-orchestrator`.

Do not treat legacy markdown workflow files as the source of truth; they are compatibility and migration artifacts only.";
