use super::*;

fn publication(staged: &Path) -> Publication {
    Publication {
        staged: staged.to_path_buf(),
        tools: ["Claude Code", "Codex", "Copilot"]
            .map(|tool| {
                (
                    tool,
                    Removals {
                        skills: BTreeSet::from(["extra".into()]),
                        agents: BTreeSet::from(["core/guide".into()]),
                    },
                )
            })
            .into(),
    }
}

#[test]
fn unchanged_publication_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let publication = Publication {
        staged: dir.path().to_path_buf(),
        ..Default::default()
    };
    let mut output = Vec::new();
    write_notice(&publication, &mut output, true).unwrap();
    assert!(output.is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn one_block_is_saved_and_printed_only_by_its_writer() {
    let dir = tempfile::tempdir().unwrap();
    let publication = publication(dir.path());
    let mut first = Vec::new();
    write_notice(&publication, &mut first, true).unwrap();
    let saved = fs::read(dir.path().join("install").join(NOTICE)).unwrap();
    assert!(!saved.is_empty());
    assert_eq!(first, saved);
    assert_eq!(fs::read_dir(dir.path().join("install")).unwrap().count(), 1);
    let mut later = Vec::new();
    write_notice(&publication, &mut later, true).unwrap();
    assert!(later.is_empty());
    assert_eq!(
        fs::read(dir.path().join("install").join(NOTICE)).unwrap(),
        saved
    );
}

#[test]
fn ineligible_writer_saves_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let publication = publication(dir.path());
    let mut output = Vec::new();
    write_notice(&publication, &mut output, false).unwrap();
    assert!(output.is_empty());
    assert!(dir.path().join("install").join(NOTICE).is_file());
    write_notice(&publication, &mut output, true).unwrap();
    assert!(output.is_empty());
}

#[test]
fn terminal_and_recipe_eligibility_are_independent() {
    assert!(eligible_in(true, false));
    for (terminal, recipe) in [(false, false), (true, true), (false, true)] {
        assert!(!eligible_in(terminal, recipe));
    }
}

#[test]
fn failed_print_keeps_the_file_without_replay() {
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
    let publication = publication(dir.path());
    assert!(write_notice(&publication, &mut Broken, true).is_err());
    let saved = fs::read(dir.path().join("install").join(NOTICE)).unwrap();
    assert!(!saved.is_empty());
    let mut later = Vec::new();
    write_notice(&publication, &mut later, true).unwrap();
    assert!(later.is_empty());
}

#[cfg(unix)]
#[test]
fn unwritable_notice_folder_still_prints_and_reports_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let install = dir.path().join("install");
    fs::create_dir(&install).unwrap();
    fs::set_permissions(&install, fs::Permissions::from_mode(0o500)).unwrap();
    let mut output = Vec::new();
    let result = write_notice(&publication(dir.path()), &mut output, true);
    // Restore permissions before any assertion, so temporary cleanup succeeds.
    fs::set_permissions(&install, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    warn(result);
    assert!(!output.is_empty());
    assert!(!install.join(NOTICE).exists());
}

#[test]
fn counts_include_only_removed_bundled_entries_outside_selection() {
    let dir = tempfile::tempdir().unwrap();
    for relative in [
        "skills/pdf/SKILL.md",
        "skills/category/extra/SKILL.md",
        "agents/core/guide.md",
        "agents/core/reviewer.md",
    ] {
        let file = dir.path().join(relative);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "fixture").unwrap();
    }
    let mut report = PluginRegistration {
        removed_skills: ["pdf", "extra", "obsolete", "amplihack-command-help"]
            .map(str::to_owned)
            .into(),
        removed_agents: ["core/guide", "core/reviewer", "old/agent"]
            .map(str::to_owned)
            .into(),
        ..Default::default()
    };
    let selection = ListSelection::default();
    let counts = bundled_removals(dir.path(), "Claude Code", &report, &selection).unwrap();
    assert_eq!(counts.skills, BTreeSet::from(["extra".into()]));
    assert_eq!(counts.agents, BTreeSet::from(["core/guide".into()]));
    report.removed_skills = BTreeSet::from(["category".into()]);
    report.removed_agents = BTreeSet::from(["guide".into(), "reviewer".into()]);
    let counts = bundled_removals(dir.path(), "Copilot", &report, &selection).unwrap();
    assert_eq!(counts.skills, BTreeSet::from(["extra".into()]));
    assert_eq!(counts.agents, BTreeSet::from(["core/guide".into()]));
    report.removed_agents = BTreeSet::from(["core-guide".into(), "core-reviewer".into()]);
    let counts = bundled_removals(dir.path(), "Codex", &report, &selection).unwrap();
    assert!(counts.skills.is_empty());
    assert_eq!(counts.agents, BTreeSet::from(["core/guide".into()]));
}

#[test]
fn publications_outside_installation_do_not_create_notice_files() {
    let dir = tempfile::tempdir().unwrap();
    record(
        dir.path(),
        "Claude Code",
        &PluginRegistration {
            removed_skills: BTreeSet::from(["extra".into()]),
            ..Default::default()
        },
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn nested_installation_collects_all_tools_before_writing() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(dir.path());
    let staged = dir.path().join(".amplihack/.claude");
    let skill = staged.join("skills/extra");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "extra").unwrap();
    let outer = begin();
    let nested = begin();
    let report = PluginRegistration {
        removed_skills: BTreeSet::from(["extra".into()]),
        ..Default::default()
    };
    for tool in ["Claude Code", "Codex", "Copilot"] {
        record(&staged, tool, &report);
    }
    drop(nested);
    assert!(!staged.join("install").join(NOTICE).exists());
    PUBLICATION.with(|publication| {
        let publication = publication.borrow();
        let tools = &publication.as_ref().unwrap().tools;
        assert_eq!(tools.len(), 3);
        assert!(
            tools
                .values()
                .all(|counts| counts.skills == BTreeSet::from(["extra".into()]))
        );
    });
    drop(outer);
    assert!(staged.join("install").join(NOTICE).is_file());
}
