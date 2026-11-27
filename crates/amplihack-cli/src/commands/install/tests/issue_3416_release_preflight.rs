//! Source inspection uses only disposable assets and never mutates HOME.

use super::{SourceLayout, helpers, local_install, release_preflight};
use std::fs;
use std::path::Path;

#[cfg(unix)]
#[test]
fn rejected_install_preserves_live_links_and_staged_files() {
    use std::os::unix::fs::symlink;
    if crate::test_support::isolated_install_test() {
        return;
    }

    let home = super::home_dir().unwrap();
    let source = tempfile::tempdir().unwrap();
    helpers::create_bundle_only_source_repo(source.path());
    let bundle = source.path().join("amplifier-bundle");
    // Start with no installation at all: even the lock file must not appear.
    fs::remove_dir_all(bundle.join("recipes")).unwrap();
    let empty = snapshot(&home);
    assert!(local_install(source.path(), None).is_err());
    assert_eq!(snapshot(&home), empty);

    for relative in [
        ".amplihack/releases/previous/.complete",
        ".amplihack/releases/previous/amplifier-bundle/asset",
        ".amplihack/releases/previous/bin/amplihack",
        ".amplihack/amplifier-bundle.staging/asset",
        ".amplihack/amplifier-bundle.old/asset",
        ".amplihack/.claude/context.staging/asset",
        ".amplihack/.claude/context/asset",
        ".amplihack/.installed-version",
        ".amplihack/CLAUDE.md",
        ".claude/settings.json",
    ] {
        let path = home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, relative).unwrap();
    }
    symlink(
        "releases/previous/amplifier-bundle",
        home.join(".amplihack/amplifier-bundle"),
    )
    .unwrap();
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    symlink(
        "../../.amplihack/releases/previous/bin/amplihack",
        home.join(".local/bin/amplihack"),
    )
    .unwrap();
    let before = snapshot(&home);
    assert!(local_install(source.path(), None).is_err());
    assert_eq!(snapshot(&home), before);

    // Restore fixture recipes, leaving only markers in place of skill entrypoints.
    helpers::create_bundle_only_source_repo(source.path());
    fs::remove_dir_all(bundle.join("skills")).unwrap();
    fs::create_dir(bundle.join("skills")).unwrap();
    fs::write(bundle.join("skills/marker.txt"), "fixture\n").unwrap();
    assert!(local_install(source.path(), None).is_err());
    assert_eq!(snapshot(&home), before);
}

#[cfg(unix)]
#[test]
fn implicit_rejection_preserves_installation_state() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    let home = super::home_dir().unwrap();
    let _lock = crate::test_support::env_lock().lock().unwrap();
    let source = tempfile::tempdir().unwrap();
    let binary = super::super::framework_source::BinaryIdentity::current();
    let fetcher = super::super::tag_fetch::ReleaseTagFetcher::production();
    for seeded in [false, true] {
        if seeded {
            let release = home.join(".amplihack/releases/previous");
            fs::create_dir_all(&release).unwrap();
            fs::write(release.join(".complete"), "previous release\n").unwrap();
            std::os::unix::fs::symlink(
                "releases/previous",
                home.join(".amplihack/amplifier-bundle"),
            )
            .unwrap();
            let staging = home.join(".amplihack/.claude/context.staging");
            fs::create_dir_all(&staging).unwrap();
            fs::write(staging.join("asset"), "staged content\n").unwrap();
            fs::write(home.join(".amplihack/.installed-version"), "0.0.0\n").unwrap();
        }
        for missing in ["recipes", "skills"] {
            helpers::create_bundle_only_source_repo(source.path());
            let removed = source.path().join("amplifier-bundle").join(missing);
            fs::remove_dir_all(&removed).unwrap();
            if missing == "skills" {
                fs::create_dir(&removed).unwrap();
                fs::write(removed.join("marker.txt"), "fixture\n").unwrap();
            }
            let before = snapshot(&home);
            for startup in [false, true] {
                let prepare = || {
                    let acquired = super::super::acquire_install_source(
                        Some(source.path().to_path_buf()),
                        false,
                        super::InstallMode::Implicit,
                        &binary,
                        &fetcher,
                    )?;
                    Ok(|| {
                        super::super::install_acquired_source(
                            acquired,
                            false,
                            false,
                            super::InstallMode::Implicit,
                            &binary,
                        )
                    })
                };
                let result = if startup {
                    let args = ["amplihack".into(), "launch".into()];
                    crate::self_heal::ensure_assets_match_binary_version_prepared(
                        &args,
                        &mut Vec::new(),
                        prepare,
                    )
                } else {
                    super::super::ensure_framework_installed_prepared(
                        &mut Vec::new(),
                        || Ok(true),
                        prepare,
                    )
                };
                assert!(result.is_err());
                assert_eq!(snapshot(&home), before);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn fetched_incomplete_source_preserves_failure_marker() {
    if crate::test_support::isolated_install_test() {
        return;
    }
    let home = super::home_dir().unwrap();
    let _lock = crate::test_support::env_lock().lock().unwrap();
    let marker = home.join(".amplihack/.tag-fetch-failed");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "previous fetch failure\n").unwrap();
    let binary = super::super::framework_source::BinaryIdentity {
        version: crate::VERSION.to_string(),
        may_fetch: true,
    };
    for missing in ["recipes", "skills"] {
        let fetcher = super::issue_1955_support::FakeFetcher::new(move |destination| {
            helpers::create_bundle_only_source_repo(destination);
            super::issue_1955_support::write_workspace_toml(destination, crate::VERSION);
            let removed = destination.join("amplifier-bundle").join(missing);
            fs::remove_dir_all(&removed).unwrap();
            if missing == "skills" {
                fs::create_dir(&removed).unwrap();
                fs::write(removed.join("marker.txt"), "fixture\n").unwrap();
            }
            Ok(destination.to_path_buf())
        });
        let before = snapshot(&home);
        assert!(
            super::super::acquire_install_source(
                None,
                true,
                super::InstallMode::Explicit,
                &binary,
                &fetcher,
            )
            .is_err()
        );
        assert_eq!(fetcher.calls.borrow().len(), 1);
        assert_eq!(snapshot(&home), before);
    }
}

#[cfg(unix)]
pub(super) fn snapshot(
    root: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, (u8, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    let mut entries = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let value = if metadata.file_type().is_symlink() {
                (
                    b'l',
                    fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_bytes()
                        .to_vec(),
                )
            } else if metadata.is_dir() {
                pending.push(path.clone());
                (b'd', Vec::new())
            } else {
                (b'f', fs::read(&path).unwrap())
            };
            entries.insert(path.strip_prefix(root).unwrap().to_path_buf(), value);
        }
    }
    entries
}

#[test]
fn complete_fixture_and_checkout_are_accepted() {
    let fixture = tempfile::tempdir().unwrap();
    helpers::create_bundle_only_source_repo(fixture.path());
    let (source, layout) = release_preflight::inspect(fixture.path()).unwrap();
    assert_eq!(source, fixture.path().join("amplifier-bundle"));
    assert_eq!(layout, SourceLayout::Bundle);
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(release_preflight::inspect(&checkout).is_ok());
}

#[test]
fn missing_recipes_and_recipe_only_stub_are_rejected() {
    for missing in ["recipes", "skills"] {
        let fixture = tempfile::tempdir().unwrap();
        helpers::create_bundle_only_source_repo(fixture.path());
        let removed = fixture.path().join("amplifier-bundle").join(missing);
        fs::remove_dir_all(&removed).unwrap();
        // Reproduce the former stub's marker-only skills directory.
        if missing == "skills" {
            fs::create_dir(&removed).unwrap();
            fs::write(removed.join("marker.txt"), "fixture\n").unwrap();
        }
        assert!(release_preflight::inspect(fixture.path()).is_err());
    }
}

#[test]
fn incomplete_execution_assets_are_rejected() {
    for (relative, content) in [
        (
            "recipes/default-workflow.yaml",
            "name: default-workflow\nsteps: []\n",
        ),
        (
            "recipes/investigation-workflow.yaml",
            "name: investigation-workflow\nsteps: [fixture]\n",
        ),
        ("skills/dev-orchestrator/SKILL.md", ""),
        ("tools/statusline.sh", ""),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        helpers::create_bundle_only_source_repo(fixture.path());
        fs::write(
            fixture.path().join("amplifier-bundle").join(relative),
            content,
        )
        .unwrap();
        assert!(release_preflight::inspect(fixture.path()).is_err());
    }
}

#[test]
fn instruction_source_precedence_and_legacy_locations_are_preserved() {
    let fixture = tempfile::tempdir().unwrap();
    helpers::create_bundle_only_source_repo(fixture.path());
    let bundle = fixture.path().join("amplifier-bundle");
    let candidates = [
        bundle.join("context/AMPLIHACK_HOME_CLAUDE.md"),
        bundle.join("CLAUDE.md"),
        fixture.path().join("CLAUDE.md"),
    ];
    for path in &candidates {
        fs::write(path, "home instructions\n").unwrap();
    }
    for path in &candidates {
        assert_eq!(
            release_preflight::home_instructions_source(&bundle).unwrap(),
            *path
        );
        assert!(release_preflight::inspect(fixture.path()).is_ok());
        fs::remove_file(path).unwrap();
    }
    assert!(release_preflight::inspect(fixture.path()).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_execution_assets_and_dangling_optional_directories_are_rejected() {
    use std::os::unix::fs::symlink;
    for relative in [
        "skills/default-workflow",
        "recipes/default-workflow.yaml",
        "commands",
    ] {
        let fixture = tempfile::tempdir().unwrap();
        helpers::create_bundle_only_source_repo(fixture.path());
        let target = fixture.path().join("amplifier-bundle").join(relative);
        if target.is_dir() {
            fs::remove_dir_all(&target).unwrap();
        } else if target.is_file() {
            fs::remove_file(&target).unwrap();
        }
        symlink(fixture.path().join("absent"), &target).unwrap();
        assert!(release_preflight::inspect(fixture.path()).is_err());
    }
}
