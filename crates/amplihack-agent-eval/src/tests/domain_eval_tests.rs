use super::*;

struct MockAgent {
    name: String,
    domain: String,
    response: String,
}

impl MockAgent {
    fn new(response: &str) -> Self {
        Self {
            name: "mock-agent".into(),
            domain: "testing".into(),
            response: response.into(),
        }
    }
}

impl DomainEvalAgent for MockAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn domain(&self) -> &str {
        &self.domain
    }
    fn execute(&self, _input: &str) -> Result<String, EvalError> {
        Ok(self.response.clone())
    }
    fn reset(&mut self) -> Result<(), EvalError> {
        Ok(())
    }
}

fn scenario(id: &str, level: TestLevel, expected: &str) -> EvalScenario {
    EvalScenario {
        id: id.into(),
        level,
        input: "test input".into(),
        expected_output: expected.into(),
        rubric: vec![],
        tags: vec![],
    }
}

fn rubric_scenario(id: &str, level: TestLevel, mentions: Vec<&str>) -> EvalScenario {
    EvalScenario {
        id: id.into(),
        level,
        input: "test input".into(),
        expected_output: String::new(),
        rubric: vec![RubricItem {
            criterion: "completeness".into(),
            weight: 1.0,
            must_mention: mentions.into_iter().map(String::from).collect(),
        }],
        tags: vec![],
    }
}

#[test]
fn scenario_result_pass_threshold() {
    let result = ScenarioResult::new("s1", TestLevel::L1Recall, 0.95, "out", "exp");
    assert!(result.passed); // L1 threshold is 0.9
}

#[test]
fn level_report_aggregation() {
    let results = vec![
        ScenarioResult::new("s1", TestLevel::L1Recall, 0.9, "a", "a"),
        ScenarioResult::new("s2", TestLevel::L1Recall, 0.5, "b", "b"),
    ];
    let report = LevelReport::from_results(TestLevel::L1Recall, results);
    assert_eq!(report.scenarios_run, 2);
    assert_eq!(report.scenarios_passed, 1);
    assert!((report.average_score - 0.7).abs() < f64::EPSILON);
}

#[test]
fn eval_report_overall() {
    let reports = vec![
        LevelReport::from_results(
            TestLevel::L1Recall,
            vec![ScenarioResult::new(
                "s1",
                TestLevel::L1Recall,
                0.9,
                "a",
                "a",
            )],
        ),
        LevelReport::from_results(
            TestLevel::L2MultiSourceSynthesis,
            vec![ScenarioResult::new(
                "s2",
                TestLevel::L2MultiSourceSynthesis,
                0.3,
                "b",
                "b",
            )],
        ),
    ];
    let report = EvalReport::from_levels("agent", "domain", reports);
    assert_eq!(report.levels_passed, 1);
    assert_eq!(report.levels_failed, 1);
    assert!(!report.passed());
}

#[test]
fn harness_run_level() {
    let scenarios = vec![
        scenario("s1", TestLevel::L1Recall, "the answer"),
        scenario("s2", TestLevel::L1Recall, "the answer"),
        scenario("s3", TestLevel::L2MultiSourceSynthesis, "other"),
    ];
    let harness = DomainEvalHarness::new(scenarios);
    let mut agent = MockAgent::new("the answer");
    let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
    assert_eq!(report.scenarios_run, 2);
    assert!(report.average_score > 0.0);
}

#[test]
fn harness_run_all() {
    let scenarios = vec![
        scenario("s1", TestLevel::L1Recall, "hello world"),
        scenario("s2", TestLevel::L2MultiSourceSynthesis, "hello world"),
    ];
    let harness = DomainEvalHarness::new(scenarios);
    let mut agent = MockAgent::new("hello world");
    let report = harness.run_all(&mut agent).unwrap();
    assert_eq!(report.level_reports.len(), 2);
    assert!(report.overall_score > 0.0);
}

#[test]
fn rubric_grading() {
    let scenarios = vec![rubric_scenario(
        "s1",
        TestLevel::L1Recall,
        vec!["security", "encryption"],
    )];
    let harness = DomainEvalHarness::new(scenarios);
    let mut agent = MockAgent::new("We use AES encryption for security");
    let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
    assert_eq!(report.scenario_results[0].score, 1.0);
}

#[test]
fn rubric_partial_match() {
    let scenarios = vec![rubric_scenario(
        "s1",
        TestLevel::L1Recall,
        vec!["security", "encryption", "hashing"],
    )];
    let harness = DomainEvalHarness::new(scenarios);
    let mut agent = MockAgent::new("We use security measures");
    let report = harness.run_level(&mut agent, TestLevel::L1Recall).unwrap();
    let score = report.scenario_results[0].score;
    assert!((score - 1.0 / 3.0).abs() < 0.01);
}

#[test]
fn similarity_grading_identical() {
    let harness = DomainEvalHarness::new(vec![]);
    assert!((harness.grade_by_similarity("hello world", "hello world") - 1.0).abs() < 0.01);
}

#[test]
fn similarity_grading_empty() {
    let harness = DomainEvalHarness::new(vec![]);
    assert_eq!(harness.grade_by_similarity("", "expected"), 0.0);
    assert_eq!(harness.grade_by_similarity("output", ""), 0.0);
    assert_eq!(harness.grade_by_similarity("", ""), 1.0);
}

#[test]
fn harness_levels() {
    let scenarios = vec![
        scenario("s1", TestLevel::L1Recall, "a"),
        scenario("s2", TestLevel::L3TemporalReasoning, "b"),
        scenario("s3", TestLevel::L1Recall, "c"),
    ];
    let harness = DomainEvalHarness::new(scenarios);
    assert_eq!(harness.scenario_count(), 3);
    let levels = harness.levels();
    assert_eq!(levels.len(), 2);
    assert_eq!(levels[0], TestLevel::L1Recall);
}

#[test]
fn eval_report_serde() {
    let report = EvalReport::from_levels("test", "domain", vec![]);
    let json = serde_json::to_string(&report).unwrap();
    let restored: EvalReport = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.agent_name, "test");
}
