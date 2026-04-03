use amplihack_agent_generator::{Complexity, PromptAnalyzer};

fn analyzer() -> PromptAnalyzer {
    PromptAnalyzer::new()
}

// ---------------------------------------------------------------------------
// PromptAnalyzer — behavioral tests
// ---------------------------------------------------------------------------

#[test]
fn simple_prompt_analysis() {
    let goal = analyzer().analyze("Build a data pipeline").unwrap();
    assert_eq!(goal.domain, "data-processing");
    assert_eq!(goal.goal, "Build a data pipeline");
    assert!(!goal.raw_prompt.is_empty());
}

#[test]
fn complex_prompt_with_constraints() {
    let goal = analyzer()
        .analyze(
            "Create a security scanner that checks for CVEs. \
             Must run in under 10 seconds and support offline mode.",
        )
        .unwrap();
    assert_eq!(goal.domain, "security");
    assert!(!goal.constraints.is_empty());
}

#[test]
fn domain_detection_data_processing() {
    let goal = analyzer()
        .analyze("Parse CSV files and aggregate monthly sales data")
        .unwrap();
    assert_eq!(goal.domain, "data-processing");
}

#[test]
fn domain_detection_security() {
    let goal = analyzer()
        .analyze("Scan container images for known vulnerabilities")
        .unwrap();
    assert_eq!(goal.domain, "security");
}

#[test]
fn empty_prompt_rejection() {
    assert!(analyzer().analyze("").is_err());
}

#[test]
fn complexity_classification_simple() {
    let goal = analyzer().analyze("List files in a directory").unwrap();
    assert_eq!(goal.complexity, Complexity::Simple);
}

#[test]
fn complexity_classification_complex() {
    let goal = analyzer()
        .analyze(
            "Design a distributed system for real-time fraud detection \
             with sub-100ms latency, multi-region failover, and GDPR compliance",
        )
        .unwrap();
    assert_eq!(goal.complexity, Complexity::Complex);
}

#[test]
fn prompt_with_special_characters() {
    let goal = analyzer()
        .analyze("Handle input with <html>, \"quotes\", & symbols © ®")
        .unwrap();
    assert!(!goal.goal.is_empty());
    assert!(!goal.domain.is_empty());
}

#[test]
fn multiline_prompt_analysis() {
    let goal = analyzer()
        .analyze(
            "Goal: build a log analyzer\n\
             Constraints:\n\
             - Must handle gzip files\n\
             - Support regex patterns",
        )
        .unwrap();
    assert!(!goal.constraints.is_empty());
    assert_eq!(goal.domain, "log-analysis");
}
