use super::*;

#[test]
fn step_type_inference() {
    let fields: HashSet<String> = ["shell".into()].into_iter().collect();
    assert_eq!(StepType::infer(&fields), StepType::Shell);

    let fields: HashSet<String> = ["recipe".into()].into_iter().collect();
    assert_eq!(StepType::infer(&fields), StepType::SubRecipe);

    let fields: HashSet<String> = ["prompt".into()].into_iter().collect();
    assert_eq!(StepType::infer(&fields), StepType::Agent);

    let empty: HashSet<String> = HashSet::new();
    assert_eq!(StepType::infer(&empty), StepType::Prompt);
}

#[test]
fn step_type_display() {
    assert_eq!(StepType::Agent.to_string(), "agent");
    assert_eq!(StepType::Shell.to_string(), "shell");
    assert_eq!(StepType::SubRecipe.to_string(), "sub_recipe");
}

#[test]
fn step_status_display() {
    assert_eq!(StepStatus::Succeeded.to_string(), "succeeded");
    assert_eq!(StepStatus::Failed.to_string(), "failed");
    assert_eq!(StepStatus::Skipped.to_string(), "skipped");
}

#[test]
fn step_construction() {
    let step = Step::new("s1", "First step", StepType::Shell);
    assert_eq!(step.id, "s1");
    assert_eq!(step.name, "First step");
    assert_eq!(step.step_type, StepType::Shell);
    assert!(!step.allow_failure);
}

#[test]
fn recipe_construction() {
    let steps = vec![
        Step::new("s1", "init", StepType::Shell),
        Step::new("s2", "build", StepType::Agent),
    ];
    let recipe = Recipe::new("test-recipe", steps);
    assert_eq!(recipe.name, "test-recipe");
    assert_eq!(recipe.step_count(), 2);
    assert_eq!(recipe.version, "1.0.0");
    assert!(recipe.get_step("s1").is_some());
    assert!(recipe.get_step("s3").is_none());
}

#[test]
fn recipe_step_ids() {
    let steps = vec![
        Step::new("a", "first", StepType::Shell),
        Step::new("b", "second", StepType::Agent),
    ];
    let recipe = Recipe::new("ids", steps);
    assert_eq!(recipe.step_ids(), vec!["a", "b"]);
}

#[test]
fn step_result_success() {
    let result = StepResult::success("s1", "done");
    assert!(result.is_success());
    assert_eq!(result.output.as_deref(), Some("done"));
    assert!(result.error.is_none());
}

#[test]
fn step_result_failure() {
    let result = StepResult::failure("s1", "boom");
    assert!(!result.is_success());
    assert_eq!(result.error.as_deref(), Some("boom"));
}

#[test]
fn step_result_truncated_output() {
    let result = StepResult::success("s1", "a".repeat(1000));
    let truncated = result.truncated_output(10).unwrap();
    assert!(truncated.starts_with("aaaaaaaaaa"));
    assert!(truncated.ends_with('…'));
    assert!(truncated.len() > 10);
}

#[test]
fn recipe_result_aggregation() {
    let mut result = RecipeResult::new("test");
    assert!(result.success);

    let mut s1 = StepResult::success("s1", "ok");
    s1.duration_seconds = 1.5;
    result.add_step(s1);

    let mut s2 = StepResult::failure("s2", "err");
    s2.duration_seconds = 0.5;
    result.add_step(s2);

    result.add_step(StepResult::skipped("s3"));

    assert!(!result.success);
    assert_eq!(result.step_count(), 3);
    assert_eq!(result.succeeded_count(), 1);
    assert_eq!(result.failed_count(), 1);
    assert_eq!(result.skipped_count(), 1);
    assert!((result.total_duration_seconds - 2.0).abs() < f64::EPSILON);
}

#[test]
fn recipe_result_serde_roundtrip() {
    let mut result = RecipeResult::new("serde-test");
    result.add_step(StepResult::success("s1", "output"));
    let json = serde_json::to_string(&result).unwrap();
    let restored: RecipeResult = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.recipe_name, "serde-test");
    assert_eq!(restored.step_count(), 1);
    assert!(restored.success);
}

#[test]
fn step_serde_roundtrip() {
    let mut step = Step::new("s1", "test", StepType::Agent);
    step.prompt = Some("Do something".into());
    step.timeout_seconds = Some(60);
    let json = serde_json::to_string(&step).unwrap();
    let restored: Step = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, "s1");
    assert_eq!(restored.step_type, StepType::Agent);
    assert_eq!(restored.timeout_seconds, Some(60));
}

#[test]
fn step_execution_error_messages() {
    let e = StepExecutionError::Timeout {
        step_id: "s1".into(),
        timeout_secs: 30,
    };
    assert!(e.to_string().contains("timed out"));
    assert!(e.to_string().contains("30"));

    let e = StepExecutionError::ExecutionFailed {
        step_id: "s2".into(),
        message: "exit code 1".into(),
    };
    assert!(e.to_string().contains("exit code 1"));
}
