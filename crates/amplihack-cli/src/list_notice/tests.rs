use super::*;
use crate::claude_plugin::SkippedSkill;
use std::collections::BTreeSet;

fn report() -> PluginRegistration {
    PluginRegistration {
        removed_skills: BTreeSet::from(["extra".into(), "another".into()]),
        removed_agents: BTreeSet::from(["core/guide".into()]),
        preserved: vec![SkippedSkill {
            name: "edited".into(),
            reason: "changed locally".into(),
        }],
        ..Default::default()
    }
}

#[test]
fn fresh_or_unchanged_publication_creates_no_notice_files() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "Claude Code", &PluginRegistration::default()).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn removed_entries_are_saved_and_deferred_then_displayed_once() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "Claude Code", &report()).unwrap();
    let saved = fs::read(dir.path().join("install").join(HISTORY)).unwrap();
    let mut out = Vec::new();
    show_pending_at(dir.path(), &mut out, false).unwrap();
    assert!(out.is_empty());
    assert!(!dir.path().join("install").join(SHOWN).exists());
    show_pending_at(dir.path(), &mut out, true).unwrap();
    assert_eq!(out, saved);
    show_pending_at(dir.path(), &mut out, true).unwrap();
    assert_eq!(out, saved);
    assert_eq!(
        fs::read(dir.path().join("install").join(HISTORY)).unwrap(),
        saved
    );
}

#[test]
fn later_tool_migrations_preserve_history_and_show_only_new_changes() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "Claude Code", &report()).unwrap();
    let mut first = Vec::new();
    show_pending_at(dir.path(), &mut first, true).unwrap();
    record(dir.path(), "Codex", &report()).unwrap();
    record(dir.path(), "Copilot", &report()).unwrap();
    let mut later = Vec::new();
    show_pending_at(dir.path(), &mut later, true).unwrap();
    first.extend(later);
    assert_eq!(
        first,
        fs::read(dir.path().join("install").join(HISTORY)).unwrap()
    );
    let mut again = Vec::new();
    show_pending_at(dir.path(), &mut again, true).unwrap();
    assert!(again.is_empty());
}

#[test]
fn terminal_and_recipe_eligibility_are_independent() {
    assert!(eligible_in(true, false));
    for (terminal, recipe) in [(false, false), (true, true), (false, true)] {
        assert!(!eligible_in(terminal, recipe));
    }
}

#[test]
fn failed_display_remains_pending() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "Copilot", &report()).unwrap();
    assert!(show_pending_at(dir.path(), &mut Broken, true).is_err());
    assert!(!dir.path().join("install").join(SHOWN).exists());
    let mut out = Vec::new();
    show_pending_at(dir.path(), &mut out, true).unwrap();
    assert_eq!(
        out,
        fs::read(dir.path().join("install").join(HISTORY)).unwrap()
    );
}

#[test]
fn notice_is_not_classified_as_a_recipe_failure() {
    let dir = tempfile::tempdir().unwrap();
    for tool in ["Claude Code", "Codex", "Copilot"] {
        record(dir.path(), tool, &report()).unwrap();
    }
    let text = fs::read_to_string(dir.path().join("install").join(HISTORY)).unwrap();
    use crate::commands::recipe::run::{FailureClass, classify_failure_text};
    assert_eq!(
        classify_failure_text(&text),
        (FailureClass::Indeterminate, None)
    );
}

#[test]
fn concurrent_publishers_do_not_lose_notices() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = tempfile::tempdir().unwrap();
    record(baseline.path(), "Copilot", &report()).unwrap();
    let one = fs::read(baseline.path().join("install").join(HISTORY)).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..3 {
            let staged = dir.path();
            scope.spawn(move || record(staged, "Copilot", &report()).unwrap());
        }
    });
    let mut out = Vec::new();
    show_pending_at(dir.path(), &mut out, true).unwrap();
    assert_eq!(
        out,
        fs::read(dir.path().join("install").join(HISTORY)).unwrap()
    );
    assert_eq!(out, one.repeat(3));
    let shown: usize =
        serde_json::from_slice(&fs::read(dir.path().join("install").join(SHOWN)).unwrap()).unwrap();
    assert_eq!(shown, out.len());
}
