use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target/debug/amplihack");
    path
}

fn require_binary(bin: &std::path::Path) {
    assert!(
        bin.exists(),
        "amplihack binary not found at {}; run `cargo build --bin amplihack` first.",
        bin.display()
    );
}

fn write_sample_blarify_json(path: &std::path::Path) {
    fs::write(
        path,
        r#"{
  "files": [
    {
      "path": "src/example/module.py",
      "language": "python",
      "lines_of_code": 10
    }
  ],
  "classes": [
    {
      "id": "class:Example",
      "name": "Example",
      "file_path": "src/example/module.py",
      "line_number": 1
    }
  ],
  "functions": [
    {
      "id": "func:Example.process",
      "name": "process",
      "file_path": "src/example/module.py",
      "line_number": 2,
      "class_id": "class:Example"
    }
  ],
  "imports": [],
  "relationships": []
}"#,
    )
    .unwrap();
}

#[test]
fn index_code_surfaces_legacy_kuzu_path_notice() {
    let bin = amplihack_bin();
    require_binary(&bin);

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("blarify.json");
    let db_path = dir.path().join("graph_db");
    write_sample_blarify_json(&input);

    let output = Command::new(&bin)
        .args([
            "index-code",
            input.to_str().unwrap(),
            "--kuzu-path",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "index-code failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CLI flag `--kuzu-path` is a legacy compatibility alias"));
    assert!(
        db_path.exists(),
        "expected graph db to be created at {}",
        db_path.display()
    );
}

#[test]
fn query_code_json_keeps_stdout_clean_when_using_legacy_kuzu_path_flag() {
    let bin = amplihack_bin();
    require_binary(&bin);

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("blarify.json");
    let db_path = dir.path().join("graph_db");
    write_sample_blarify_json(&input);

    let index = Command::new(&bin)
        .args([
            "index-code",
            input.to_str().unwrap(),
            "--db-path",
            db_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "index-code setup failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&index.stdout),
        String::from_utf8_lossy(&index.stderr)
    );

    let output = Command::new(&bin)
        .current_dir(dir.path())
        .args([
            "query-code",
            "--json",
            "--kuzu-path",
            db_path.to_str().unwrap(),
            "stats",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "query-code failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.trim_start().starts_with('{'),
        "expected JSON stdout, got:\n{stdout}"
    );
    assert!(!stdout.contains("Compatibility mode"));
    assert!(stderr.contains("CLI flag `--kuzu-path` is a legacy compatibility alias"));
}
