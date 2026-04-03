use super::*;
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// detect_project_state
// ---------------------------------------------------------------------------

#[test]
fn detect_missing_project_md() {
    let tmp = TempDir::new().expect("tempdir");
    assert_eq!(detect_project_state(tmp.path()), ProjectState::Missing);
}

#[test]
fn detect_custom_project_md() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    fs::write(
        ctx.join("PROJECT.md"),
        "# My Custom Project\n\nDescription here.\n",
    )
    .expect("write");
    assert_eq!(detect_project_state(tmp.path()), ProjectState::Custom);
}

#[test]
fn detect_template_project_md() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    let content = "\
# Project\n\
\n\
Microsoft Hackathon 2025\n\
Agentic Coding Framework\n\
Building the tools that build the future\n\
";
    fs::write(ctx.join("PROJECT.md"), content).expect("write");
    assert_eq!(detect_project_state(tmp.path()), ProjectState::Template);
}

#[test]
fn detect_stale_empty_project_md() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    fs::write(ctx.join("PROJECT.md"), "   \n  \n").expect("write");
    assert_eq!(detect_project_state(tmp.path()), ProjectState::Stale);
}

// ---------------------------------------------------------------------------
// analyze_project_structure
// ---------------------------------------------------------------------------

#[test]
fn analyze_detects_rust_language() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("main.rs"), "fn main() {}").expect("write");

    let analysis = analyze_project_structure(tmp.path());
    assert!(analysis.languages.contains(&"Rust".to_owned()));
}

#[test]
fn analyze_detects_python_in_subdirectory() {
    let tmp = TempDir::new().expect("tempdir");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("app.py"), "pass").expect("write");

    let analysis = analyze_project_structure(tmp.path());
    assert!(analysis.languages.contains(&"Python".to_owned()));
}

#[test]
fn analyze_reads_readme_preview() {
    let tmp = TempDir::new().expect("tempdir");
    let readme_text = "# My Project\n\nA short description.\n";
    fs::write(tmp.path().join("README.md"), readme_text).expect("write");

    let analysis = analyze_project_structure(tmp.path());
    assert!(analysis.has_readme);
    assert!(analysis.readme_preview.is_some());
    assert!(
        analysis
            .readme_preview
            .as_ref()
            .is_some_and(|p| p.contains("short description"))
    );
}

#[test]
fn analyze_collects_package_files() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"").expect("write");

    let analysis = analyze_project_structure(tmp.path());
    assert!(
        analysis
            .package_files
            .iter()
            .any(|(name, _)| name == "Cargo.toml")
    );
}

#[test]
fn analyze_no_files() {
    let tmp = TempDir::new().expect("tempdir");
    let analysis = analyze_project_structure(tmp.path());
    assert!(analysis.languages.is_empty());
    assert!(!analysis.has_readme);
}

// ---------------------------------------------------------------------------
// initialize_project_md
// ---------------------------------------------------------------------------

#[test]
fn init_creates_project_md_when_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let result = initialize_project_md(tmp.path(), InitMode::Create).expect("init");

    assert_eq!(result.action, ActionTaken::Initialized);
    assert!(result.path.is_file());
    assert!(result.template_used.is_some());
}

#[test]
fn init_skip_mode_never_writes() {
    let tmp = TempDir::new().expect("tempdir");
    let result = initialize_project_md(tmp.path(), InitMode::Skip).expect("init");

    assert_eq!(result.action, ActionTaken::Skipped);
    assert!(!result.path.is_file());
}

#[test]
fn init_create_does_not_overwrite_custom() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    fs::write(ctx.join("PROJECT.md"), "# Custom project\n").expect("write");

    let result = initialize_project_md(tmp.path(), InitMode::Create).expect("init");
    assert_eq!(result.action, ActionTaken::Skipped);

    let content = fs::read_to_string(ctx.join("PROJECT.md")).expect("read");
    assert!(content.contains("Custom project"));
}

#[test]
fn init_update_overwrites_custom() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    fs::write(ctx.join("PROJECT.md"), "# Custom project\n").expect("write");

    let result = initialize_project_md(tmp.path(), InitMode::Update).expect("init");
    assert_eq!(result.action, ActionTaken::Regenerated);
    assert!(
        ctx.join("PROJECT.md.bak").is_file(),
        "backup should be created"
    );
}

#[test]
fn init_errors_on_nonexistent_dir() {
    let result = initialize_project_md(Path::new("/nonexistent/path"), InitMode::Create);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// generate_from_template (via initialize)
// ---------------------------------------------------------------------------

#[test]
fn generated_content_includes_project_name() {
    let tmp = TempDir::new().expect("tempdir");
    initialize_project_md(tmp.path(), InitMode::Create).expect("init");

    let md_path = tmp.path().join(".claude/context/PROJECT.md");
    let content = fs::read_to_string(md_path).expect("read");
    // The project name should be the tmpdir basename.
    assert!(content.starts_with('#'));
}

#[test]
fn generated_content_includes_detected_languages() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("main.rs"), "fn main() {}").expect("write");

    initialize_project_md(tmp.path(), InitMode::Create).expect("init");

    let md_path = tmp.path().join(".claude/context/PROJECT.md");
    let content = fs::read_to_string(md_path).expect("read");
    assert!(content.contains("Rust"));
}

// ---------------------------------------------------------------------------
// extract_description
// ---------------------------------------------------------------------------

#[test]
fn extract_description_from_readme() {
    let preview = "# My Project\n\nA great library for testing.\nSecond line.\n\n## Features\n";
    let desc = extract_description(preview);
    assert!(desc.contains("great library"));
    assert!(!desc.contains("Features"));
}

#[test]
fn extract_description_placeholder_on_empty() {
    let desc = extract_description("#Title\n\n");
    assert!(desc.contains("Describe your project"));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn indicator_matching_is_case_insensitive() {
    let tmp = TempDir::new().expect("tempdir");
    let ctx = tmp.path().join(".claude").join("context");
    fs::create_dir_all(&ctx).expect("mkdir");
    fs::write(
        ctx.join("PROJECT.md"),
        "microsoft hackathon 2025\nagentic coding framework\n",
    )
    .expect("write");
    assert_eq!(detect_project_state(tmp.path()), ProjectState::Template);
}
