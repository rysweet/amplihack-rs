use super::*;
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// compute_content_hash
// ---------------------------------------------------------------------------

#[test]
fn hash_is_deterministic() {
    let h1 = compute_content_hash("hello world");
    let h2 = compute_content_hash("hello world");
    assert_eq!(h1, h2);
}

#[test]
fn hash_normalizes_trailing_spaces() {
    let h1 = compute_content_hash("hello\nworld\n");
    let h2 = compute_content_hash("hello  \nworld  \n\n");
    assert_eq!(h1, h2);
}

#[test]
fn hash_strips_leading_trailing_blanks() {
    let h1 = compute_content_hash("\n\nhello\n\n\n");
    let h2 = compute_content_hash("hello");
    assert_eq!(h1, h2);
}

#[test]
fn hash_differs_for_different_content() {
    let h1 = compute_content_hash("hello");
    let h2 = compute_content_hash("world");
    assert_ne!(h1, h2);
}

#[test]
fn hash_is_hex_sha256() {
    let h = compute_content_hash("test");
    assert_eq!(h.len(), 64, "SHA-256 hex digest should be 64 chars");
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}

// ---------------------------------------------------------------------------
// detect_claude_state
// ---------------------------------------------------------------------------

#[test]
fn detect_missing() {
    let tmp = TempDir::new().expect("tempdir");
    assert_eq!(detect_claude_state(tmp.path()), ClaudeState::Missing);
}

#[test]
fn detect_current_version() {
    let tmp = TempDir::new().expect("tempdir");
    let content =
        format!("# CLAUDE.md\n{CLAUDE_VERSION_MARKER} {CURRENT_VERSION} -->\nContent here.\n");
    fs::write(tmp.path().join("CLAUDE.md"), &content).expect("write");
    assert_eq!(detect_claude_state(tmp.path()), ClaudeState::Default);
}

#[test]
fn detect_outdated_version() {
    let tmp = TempDir::new().expect("tempdir");
    let content = format!("# CLAUDE.md\n{CLAUDE_VERSION_MARKER} 0.1.0 -->\nOld content.\n");
    fs::write(tmp.path().join("CLAUDE.md"), &content).expect("write");
    assert_eq!(detect_claude_state(tmp.path()), ClaudeState::CustomDirty);
}

#[test]
fn detect_custom_content_no_marker() {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("CLAUDE.md"),
        "# My custom CLAUDE.md\n\nDo things this way.\n",
    )
    .expect("write");
    assert_eq!(detect_claude_state(tmp.path()), ClaudeState::CustomClean);
}

#[cfg(unix)]
#[test]
fn detect_symlink_as_custom() {
    let tmp = TempDir::new().expect("tempdir");
    let real_file = tmp.path().join("real_claude.md");
    fs::write(&real_file, "content").expect("write");
    std::os::unix::fs::symlink(&real_file, tmp.path().join("CLAUDE.md")).expect("symlink");
    assert_eq!(detect_claude_state(tmp.path()), ClaudeState::CustomClean);
}

// ---------------------------------------------------------------------------
// parse_claude_state (internal, tested via detect)
// ---------------------------------------------------------------------------

#[test]
fn parse_no_marker() {
    assert_eq!(
        parse_claude_state("No markers here"),
        ClaudeState::CustomClean
    );
}

#[test]
fn parse_malformed_marker() {
    let content = format!("{CLAUDE_VERSION_MARKER} no closing tag");
    assert_eq!(parse_claude_state(&content), ClaudeState::CustomClean);
}

// ---------------------------------------------------------------------------
// handle_claude_md — Overwrite mode
// ---------------------------------------------------------------------------

#[test]
fn overwrite_deploys_to_empty_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source_claude.md");
    fs::write(&source, "# Source CLAUDE.md\nContent.\n").expect("write");

    let target = TempDir::new().expect("target tempdir");
    let result = handle_claude_md(&source, target.path(), HandleMode::Overwrite).expect("handle");

    assert_eq!(result.action, HandleMode::Overwrite);
    assert!(result.content_hash.is_some());
    assert!(target.path().join("CLAUDE.md").is_file());
}

#[test]
fn overwrite_replaces_existing() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source_claude.md");
    fs::write(&source, "# New content\n").expect("write source");

    let target = TempDir::new().expect("target tempdir");
    fs::write(target.path().join("CLAUDE.md"), "# Old content\n").expect("write old");

    let result = handle_claude_md(&source, target.path(), HandleMode::Overwrite).expect("handle");
    assert_eq!(result.action, HandleMode::Overwrite);

    let deployed = fs::read_to_string(target.path().join("CLAUDE.md")).expect("read");
    assert!(deployed.contains("New content"));
}

// ---------------------------------------------------------------------------
// handle_claude_md — Preserve mode
// ---------------------------------------------------------------------------

#[test]
fn preserve_deploys_when_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source.md");
    fs::write(&source, "# Source\n").expect("write");

    let target = TempDir::new().expect("target");
    let result = handle_claude_md(&source, target.path(), HandleMode::Preserve).expect("handle");
    assert_eq!(result.action, HandleMode::Preserve);
    assert!(target.path().join("CLAUDE.md").is_file());
}

#[test]
fn preserve_backs_up_custom_content() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source.md");
    fs::write(&source, "# Amplihack source\n").expect("write source");

    let target = TempDir::new().expect("target");
    fs::write(
        target.path().join("CLAUDE.md"),
        "# My custom content\nDo not lose this.\n",
    )
    .expect("write custom");

    let result = handle_claude_md(&source, target.path(), HandleMode::Preserve).expect("handle");
    assert_eq!(result.action, HandleMode::Preserve);

    // Backup files should exist.
    let preserved = target.path().join(".claude/context/CLAUDE.md.preserved");
    assert!(preserved.is_file(), "preserved backup should exist");

    let preserved_content = fs::read_to_string(preserved).expect("read preserved");
    assert!(preserved_content.contains("custom content"));

    let project_md = target.path().join(".claude/context/PROJECT.md");
    assert!(project_md.is_file(), "PROJECT.md backup should exist");
}

#[test]
fn preserve_skips_when_current() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source.md");
    fs::write(&source, "# Source\n").expect("write source");

    let target = TempDir::new().expect("target");
    let versioned = format!("{CLAUDE_VERSION_MARKER} {CURRENT_VERSION} -->\n# Content\n");
    fs::write(target.path().join("CLAUDE.md"), &versioned).expect("write versioned");

    let result = handle_claude_md(&source, target.path(), HandleMode::Preserve).expect("handle");
    assert_eq!(result.action, HandleMode::Preserve);
    assert!(result.content_hash.is_some());
}

// ---------------------------------------------------------------------------
// handle_claude_md — Merge mode
// ---------------------------------------------------------------------------

#[test]
fn merge_combines_content() {
    let tmp = TempDir::new().expect("tempdir");
    let source = tmp.path().join("source.md");
    fs::write(&source, "# Amplihack section\n").expect("write source");

    let target = TempDir::new().expect("target");
    fs::write(target.path().join("CLAUDE.md"), "# Existing content\n").expect("write existing");

    let result = handle_claude_md(&source, target.path(), HandleMode::Merge).expect("handle");
    assert_eq!(result.action, HandleMode::Merge);

    let content = fs::read_to_string(target.path().join("CLAUDE.md")).expect("read");
    assert!(content.contains("Existing content"));
    assert!(content.contains("Amplihack section"));
    assert!(content.contains("---"), "separator should be present");
}

// ---------------------------------------------------------------------------
// handle_claude_md — error cases
// ---------------------------------------------------------------------------

#[test]
fn handle_missing_source_errors() {
    let target = TempDir::new().expect("target");
    let result = handle_claude_md(
        Path::new("/nonexistent/source.md"),
        target.path(),
        HandleMode::Overwrite,
    );
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// backup_to_project_md idempotency
// ---------------------------------------------------------------------------

#[test]
fn backup_to_project_md_is_idempotent() {
    let tmp = TempDir::new().expect("tempdir");
    backup_to_project_md(tmp.path(), "first backup").expect("first");
    backup_to_project_md(tmp.path(), "second backup").expect("second");

    let project_md = tmp.path().join(".claude/context/PROJECT.md");
    let content = fs::read_to_string(project_md).expect("read");

    // Should only contain one preserved section.
    let count = content.matches(BEGIN_MARKER).count();
    assert_eq!(count, 1, "should not duplicate preserved sections");
}
