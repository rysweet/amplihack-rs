//! Supporting YAML is parsed as data; skill bodies and identities use the production loader.
use serde::Deserialize;
use std::path::Path;

fn collect(root: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).expect("read corpus directory") {
        let entry = entry.expect("read corpus entry");
        let kind = entry.file_type().expect("read corpus type");
        if kind.is_dir() {
            collect(&entry.path(), files);
        } else if kind.is_file()
            && matches!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml")
            )
        {
            files.push(entry.path());
        }
    }
}

#[test]
fn supporting_yaml_documents_parse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/skills");
    let mut files = Vec::new();
    collect(&root, &mut files);
    assert_eq!(files.len(), 24, "fixed supporting YAML inventory");
    let mut failures = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read supporting YAML");
        for document in serde_yaml::Deserializer::from_str(&source) {
            if let Err(error) = serde_yaml::Value::deserialize(document) {
                failures.push(format!("{}: {error}", file.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("Parsed {} supporting YAML files", files.len());
}

#[test]
fn yaml_validation_checks_every_document_and_reports_invalid_examples() {
    let source = "name: first\n---\nname: second\n";
    let documents: Vec<_> = serde_yaml::Deserializer::from_str(source)
        .map(serde_yaml::Value::deserialize)
        .collect();
    assert_eq!(documents.len(), 2);
    assert!(documents.iter().all(Result::is_ok));
    let invalid = "name: valid\n---\ninvalid: [unterminated\n";
    assert!(
        serde_yaml::Deserializer::from_str(invalid)
            .map(serde_yaml::Value::deserialize)
            .any(|document| document.is_err())
    );
}
