//! Reapply list settings through each installed tool's existing publisher.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use super::{binary, clone, codex_plugin, paths};

/// Apply the desired configuration without registering an additional tool.
/// Keep the configuration on failure, so the same command can complete it.
pub(crate) fn apply() -> Result<()> {
    let home = paths::home_dir()?;
    let staged = paths::staging_claude_dir()?;
    let copilot = home.join(".copilot");
    let tools = [
        (
            "Claude Code",
            staged
                .join("install/claude-skills-manifest.json")
                .try_exists()?
                || home.join(".claude/skills/amplihack").try_exists()?,
        ),
        (
            "Codex",
            home.join(".amplihack/codex/ownership.json").try_exists()?,
        ),
        (
            "Copilot",
            crate::copilot_setup::user_config_registers_amplihack(&copilot)?,
        ),
    ];
    if tools.iter().any(|(_, installed)| *installed) {
        refresh_staged_assets(&staged)
            .context("list settings were saved; could not refresh framework definitions; repeat the command to finish publication")?;
    }
    apply_installed(&tools, |tool| {
        ensure!(
            staged.is_dir(),
            "run `amplihack install` to restore framework assets"
        );
        match tool {
            "Claude Code" => {
                let report = crate::claude_plugin::ensure_claude_plugin_installed()?;
                ensure!(
                    report.skipped.is_empty() && report.wrapper_error.is_none(),
                    "{}",
                    report.warning().unwrap_or_default()
                );
                if let Some(warning) = report.warning() {
                    eprintln!("{warning}");
                }
                Ok(())
            }
            "Copilot" => crate::copilot_setup::ensure_copilot_home_staged_with(
                None,
                crate::copilot_setup::UserConfig::KeepExisting,
            ),
            "Codex" => apply_codex(&staged),
            _ => unreachable!("installed tool"),
        }
    })
}

/// List changes need current definitions, even though discovery skips startup
/// repair. Reuse source validation and transactional directory publication,
/// without rewriting the index or registering another tool. The full installer
/// still owns the version stamp, binaries and release switch.
fn refresh_staged_assets(staged: &Path) -> Result<()> {
    let _lock = crate::install_lock::acquire()?;
    let installed = super::version_stamp::read_installed_version()?;
    if installed.as_deref().is_some_and(|version| {
        super::version_stamp::relation(crate::VERSION, version)
            != super::version_stamp::VersionRelation::Newer
    }) {
        return Ok(());
    }
    let policy = super::framework_source::SourcePolicy {
        local: None,
        force_refresh: false,
        mode: super::InstallMode::Implicit,
        fetch_backoff: super::version_stamp::installed_version_path()
            .ok()
            .and_then(|path| path.parent().map(super::fetch_backoff::marker_path)),
    };
    let source = super::framework_source::acquire_framework_source(
        &policy,
        &super::clone::FrameworkRootQuery::from_environment(),
        &super::framework_source::BinaryIdentity::current(),
        &super::tag_fetch::ReleaseTagFetcher::production(),
        &|candidate| {
            super::bundle_compat::validate_framework_bundle_compatibility(candidate)
                .map_err(|error| format!("{error:#}"))
        },
    )?;
    let (root, layout) = super::directories::find_source_root(&source.root)?;
    super::directories::copytree_manifest(&root, layout, staged)?;
    super::write_layout_marker(staged, layout)
}

fn apply_codex(staged: &Path) -> Result<()> {
    ensure!(
        staged.is_dir(),
        "run `amplihack install` to restore framework assets"
    );
    let source = clone::find_bundled_framework_root()
        .context("framework source is unavailable; run `amplihack install` and retry")?;
    let hooks = binary::stable_hooks_binary()?;
    crate::freshness::with_provider("codex", || {
        ensure!(
            codex_plugin::install(&source.root, &hooks)?,
            "the installed Codex package could not be updated"
        );
        Ok(())
    })
}

fn apply_installed(
    tools: &[(&str, bool)],
    mut publish: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    for &(tool, installed) in tools {
        if installed {
            match publish(tool) {
                Ok(()) => completed.push(tool),
                Err(error) => failed.push(format!("{tool}: {error:#}")),
            }
        }
    }
    if !failed.is_empty() {
        bail!(
            "list settings were saved; publication failed for {}. Completed applications: {}. Repeat the command to finish publication",
            failed.join("; "),
            if completed.is_empty() {
                "none".into()
            } else {
                completed.join(", ")
            }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_and_newer_installs_keep_their_staged_definitions() {
        let _lock = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let _home = crate::test_support::HomeGuard::set(home.path());
        let staged = home.path().join(".amplihack/.claude");
        std::fs::create_dir_all(&staged).unwrap();
        let definition = staged.join("definition.md");
        std::fs::write(&definition, "installed definition").unwrap();
        for version in [crate::VERSION, "9999.0.0"] {
            super::super::version_stamp::write_installed_version(version).unwrap();
            refresh_staged_assets(&staged).unwrap();
            assert_eq!(std::fs::read(&definition).unwrap(), b"installed definition");
            assert_eq!(
                super::super::version_stamp::read_installed_version().unwrap(),
                Some(version.into())
            );
            assert!(!staged.join("skills").exists());
        }
    }

    #[test]
    fn only_installed_tools_are_published() {
        let mut called = Vec::new();
        apply_installed(
            &[("Claude Code", true), ("Codex", false), ("Copilot", true)],
            |tool| {
                called.push(tool.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(called, ["Claude Code", "Copilot"]);
    }

    #[test]
    fn failure_does_not_prevent_other_installed_tools_from_converging() {
        let tools = [("Claude Code", true), ("Codex", true), ("Copilot", true)];
        let mut called = Vec::new();
        let error = apply_installed(&tools, |tool| {
            called.push(tool.to_owned());
            if tool == "Codex" {
                bail!("test publication error");
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(called, ["Claude Code", "Codex", "Copilot"]);
        let diagnostic = error.to_string();
        for tool in ["Claude Code", "Codex", "Copilot"] {
            assert!(diagnostic.contains(tool));
        }
        apply_installed(&tools, |_| Ok(())).unwrap();
    }
}
