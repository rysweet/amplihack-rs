use amplihack_agent_core::{Intent, IntentDetector};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn detector() -> IntentDetector {
    IntentDetector::new()
}

// ---------------------------------------------------------------------------
// Question classification
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_what_question() {
    let d = detector();
    assert_eq!(d.classify("what is Rust?"), Intent::AnswerQuestion);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_who_question() {
    let d = detector();
    assert_eq!(d.classify("who created Linux?"), Intent::AnswerQuestion);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_how_question() {
    let d = detector();
    assert_eq!(d.classify("how does TDD work?"), Intent::AnswerQuestion);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_trailing_question_mark() {
    let d = detector();
    assert_eq!(d.classify("is this a test?"), Intent::AnswerQuestion);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_why_question() {
    let d = detector();
    assert_eq!(d.classify("why use OODA loops?"), Intent::AnswerQuestion);
}

// ---------------------------------------------------------------------------
// Content (store) classification
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_factual_statement() {
    let d = detector();
    assert_eq!(
        d.classify("Rust was first released in 2015."),
        Intent::StoreContent,
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_declarative_content() {
    let d = detector();
    assert_eq!(
        d.classify("The agent uses an OODA loop for decision making."),
        Intent::StoreContent,
    );
}

// ---------------------------------------------------------------------------
// Task / command classification
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_run_command() {
    let d = detector();
    assert_eq!(d.classify("run cargo test"), Intent::ExecuteTask);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_create_command() {
    let d = detector();
    assert_eq!(d.classify("create a new module"), Intent::ExecuteTask);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_build_command() {
    let d = detector();
    assert_eq!(d.classify("build the project"), Intent::ExecuteTask);
}

// ---------------------------------------------------------------------------
// Unknown / edge cases
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_empty_input() {
    let d = detector();
    assert_eq!(d.classify(""), Intent::Unknown);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_single_word() {
    let d = detector();
    // A single word with no clear intent.
    let intent = d.classify("hello");
    assert!(
        intent == Intent::Unknown || intent == Intent::StoreContent,
        "single word should be Unknown or StoreContent, got {intent}",
    );
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_whitespace_only() {
    let d = detector();
    assert_eq!(d.classify("   "), Intent::Unknown);
}

#[test]
#[should_panic(expected = "not yet implemented")]
fn classify_mixed_case_question() {
    let d = detector();
    assert_eq!(d.classify("WHAT is going on?"), Intent::AnswerQuestion);
}

// ---------------------------------------------------------------------------
// Intent enum properties
// ---------------------------------------------------------------------------

#[test]
fn intent_display_values() {
    assert_eq!(Intent::StoreContent.to_string(), "store_content");
    assert_eq!(Intent::AnswerQuestion.to_string(), "answer_question");
    assert_eq!(Intent::ExecuteTask.to_string(), "execute_task");
    assert_eq!(Intent::Unknown.to_string(), "unknown");
}

#[test]
fn intent_needs_memory() {
    assert!(Intent::StoreContent.needs_memory());
    assert!(Intent::AnswerQuestion.needs_memory());
    assert!(!Intent::ExecuteTask.needs_memory());
    assert!(!Intent::Unknown.needs_memory());
}

#[test]
fn intent_is_actionable() {
    assert!(Intent::StoreContent.is_actionable());
    assert!(!Intent::Unknown.is_actionable());
}

#[test]
fn intent_serde_roundtrip() {
    for intent in [
        Intent::StoreContent,
        Intent::AnswerQuestion,
        Intent::ExecuteTask,
        Intent::Unknown,
    ] {
        let json = serde_json::to_string(&intent).unwrap();
        let parsed: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, intent);
    }
}
