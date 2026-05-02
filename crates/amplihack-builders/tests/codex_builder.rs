// crates/amplihack-builders/tests/codex_builder.rs
//
// TDD: failing tests for CodexTranscriptsBuilder (port of
// amplifier-bundle/tools/amplihack/builders/codex_transcripts_builder.py).

use amplihack_builders::codex::CodexTranscriptsBuilder;
use tempfile::TempDir;

fn fixture_dir() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let proposer = tmp.path().join("proposer");
    let reviewer = tmp.path().join("reviewer");
    std::fs::create_dir_all(&proposer).unwrap();
    std::fs::create_dir_all(&reviewer).unwrap();
    std::fs::write(
        proposer.join("session1.json"),
        r#"{"session_id":"session1","messages":[{"role":"assistant","content":"draft"}]}"#,
    )
    .unwrap();
    std::fs::write(
        reviewer.join("session1.json"),
        r#"{"session_id":"session1","messages":[{"role":"assistant","content":"review"}]}"#,
    )
    .unwrap();
    tmp
}

#[test]
fn build_comprehensive_codex_returns_non_empty_output() {
    let tmp = fixture_dir();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().to_path_buf()));
    let out = b.build_comprehensive_codex(None).unwrap();
    assert!(out.contains("session1"));
    assert!(out.contains("draft") || out.contains("review"));
}

#[test]
fn build_focused_codex_filters_by_focus_area() {
    let tmp = fixture_dir();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().to_path_buf()));
    let out = b.build_focused_codex("review", None).unwrap();
    assert!(out.to_lowercase().contains("review"));
}

#[test]
fn extract_learning_corpus_includes_assistant_messages() {
    let tmp = fixture_dir();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().to_path_buf()));
    let corpus = b.extract_learning_corpus(None).unwrap();
    assert!(corpus.contains("draft") || corpus.contains("review"));
}

#[test]
fn generate_insights_report_is_markdown_formatted() {
    let tmp = fixture_dir();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().to_path_buf()));
    let report = b.generate_insights_report(None).unwrap();
    assert!(report.starts_with('#') || report.contains("\n#"));
}

#[test]
fn missing_output_dir_yields_empty_but_no_panic() {
    let tmp = TempDir::new().unwrap();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().join("nonexistent")));
    let out = b.build_comprehensive_codex(None).unwrap();
    assert!(out.is_empty() || out.contains("no sessions"));
}

#[test]
fn session_id_filter_only_returns_requested_sessions() {
    let tmp = fixture_dir();
    let b = CodexTranscriptsBuilder::new(Some(tmp.path().to_path_buf()));
    let out = b
        .build_comprehensive_codex(Some(vec!["session1".into()]))
        .unwrap();
    assert!(out.contains("session1"));
}
