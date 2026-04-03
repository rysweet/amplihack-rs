use amplihack_delegation::evidence_collector::{
    EvidenceCollector, evidence_patterns, language_for_extension,
};
use amplihack_delegation::models::EvidenceType;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_dir_with_files(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("create tempdir");
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, content).expect("write file");
    }
    dir
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn collect_discovers_code_files() {
    let dir = make_dir_with_files(&[("main.rs", "fn main() {}"), ("lib.py", "# lib")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    let evidence = c
        .collect(None, Some(&[EvidenceType::CodeFile]), None)
        .expect("collect");
    assert!(
        evidence.len() >= 2,
        "expected at least 2 code files, got {}",
        evidence.len()
    );
}

#[test]
fn collect_discovers_test_files() {
    let dir = make_dir_with_files(&[("test_main.py", "pass"), ("foo_test.go", "package foo")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    let evidence = c
        .collect(None, Some(&[EvidenceType::TestFile]), None)
        .expect("collect");
    assert!(
        evidence.len() >= 2,
        "expected at least 2 test files, got {}",
        evidence.len()
    );
}

#[test]
fn collect_injects_execution_log() {
    let dir = make_dir_with_files(&[]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    let evidence = c
        .collect(
            Some("all tests passed"),
            Some(&[EvidenceType::ExecutionLog]),
            None,
        )
        .expect("collect");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].evidence_type, EvidenceType::ExecutionLog);
    assert_eq!(evidence[0].path, "<execution_log>");
    assert!(evidence[0].content.contains("all tests passed"));
}

#[test]
fn collect_respects_exclude_patterns() {
    let dir = make_dir_with_files(&[("src/main.rs", "fn main(){}"), ("build/out.rs", "// out")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    let evidence = c
        .collect(
            None,
            Some(&[EvidenceType::CodeFile]),
            Some(&["build/**"]),
        )
        .expect("collect");
    assert!(
        evidence.iter().all(|e| !e.path.starts_with("build")),
        "excluded files should not appear"
    );
}

#[test]
fn get_by_type_filters_correctly() {
    let dir = make_dir_with_files(&[("main.rs", "fn main(){}"), ("README.md", "# Hello")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    c.collect(None, None, None).expect("collect");
    let code = c.get_by_type(&EvidenceType::CodeFile);
    let docs = c.get_by_type(&EvidenceType::Documentation);
    assert!(code.iter().all(|e| e.evidence_type == EvidenceType::CodeFile));
    assert!(docs.iter().all(|e| e.evidence_type == EvidenceType::Documentation));
}

#[test]
fn get_by_path_pattern_works() {
    let dir = make_dir_with_files(&[("src/lib.rs", "// lib"), ("src/main.rs", "fn main(){}")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    c.collect(None, Some(&[EvidenceType::CodeFile]), None)
        .expect("collect");
    let matches = c.get_by_path_pattern("src/*.rs").expect("pattern");
    assert!(matches.len() >= 2);
}

#[test]
fn metadata_includes_language_and_line_count() {
    let dir = make_dir_with_files(&[("hello.py", "print('hello')\nprint('world')")]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    c.collect(None, Some(&[EvidenceType::CodeFile]), None)
        .expect("collect");
    let items = c.get_by_type(&EvidenceType::CodeFile);
    let py = items.iter().find(|e| e.path.ends_with(".py")).expect("py file");
    assert_eq!(py.metadata.get("language").map(|s| s.as_str()), Some("python"));
    assert!(py.metadata.contains_key("line_count"));
}

#[test]
fn excerpt_is_truncated_for_large_content() {
    let large = "x".repeat(500);
    let dir = make_dir_with_files(&[("big.py", &large)]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    c.collect(None, Some(&[EvidenceType::CodeFile]), None)
        .expect("collect");
    let items = c.get_by_type(&EvidenceType::CodeFile);
    assert!(items.iter().all(|e| e.excerpt.len() <= 200));
}

#[test]
fn evidence_patterns_covers_all_types() {
    let pats = evidence_patterns();
    assert!(pats.contains_key(&EvidenceType::CodeFile));
    assert!(pats.contains_key(&EvidenceType::Diagram));
    assert!(pats.len() >= 10);
}

#[test]
fn language_for_extension_known_and_unknown() {
    assert_eq!(language_for_extension("rs"), Some("rust"));
    assert_eq!(language_for_extension("kt"), Some("kotlin"));
    assert_eq!(language_for_extension("xyz"), None);
}

#[test]
fn collect_skips_binary_files() {
    let dir = make_dir_with_files(&[]);
    // Write a binary file.
    std::fs::write(dir.path().join("img.rs"), &[0xFF, 0xD8, 0xFF]).expect("write");
    let mut c = EvidenceCollector::new(dir.path(), None);
    // Should not panic on binary content.
    let _evidence = c.collect(None, Some(&[EvidenceType::CodeFile]), None);
}

#[test]
fn empty_directory_collects_nothing() {
    let dir = make_dir_with_files(&[]);
    let mut c = EvidenceCollector::new(dir.path(), None);
    let evidence = c
        .collect(None, Some(&[EvidenceType::CodeFile]), None)
        .expect("collect");
    assert!(evidence.is_empty());
}
