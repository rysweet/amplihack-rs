use crate::error::Result;
use crate::models::{ExecutionPlan, GoalDefinition};

/// Converts a [`GoalDefinition`] into a phased [`ExecutionPlan`].
pub struct ObjectivePlanner;

impl ObjectivePlanner {
    pub fn new() -> Self {
        Self
    }

    /// Produce an execution plan with ordered phases, dependency graph, and
    /// parallel-opportunity annotations.
    pub fn plan(&self, _goal: &GoalDefinition) -> Result<ExecutionPlan> {
        todo!("ObjectivePlanner::plan not yet implemented")
    }
}

impl Default for ObjectivePlanner {
    fn default() -> Self {
        Self::new()
    }
}
