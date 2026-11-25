//! Reapply list settings through each installed tool's existing publisher.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use super::{binary, clone, codex_plugin, paths};

/// List publication uses installed definitions and never refreshes the framework.
/// Before the first installation, settings may be saved without publishing.
pub(crate) fn ensure_current_installation() -> Result<()> {
    let installed = super::version_stamp::read_installed_version()
        .context("cannot check installed framework version; run `amplihack install`")?;
    ensure!(
        (installed.is_none() && !paths::staging_claude_dir()?.exists())
            || installed.as_deref() == Some(crate::VERSION),
        "installed framework does not match this binary; run `amplihack install` before changing lists"
    );
    Ok(())
}

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
                .try_exists()
                .and_then(|found| Ok(found || home.join(".claude/skills/amplihack").try_exists()?))
                .map_err(Into::into),
        ),
        (
            "Codex",
            home.join(".amplihack/codex/ownership.json")
                .try_exists()
                .map_err(Into::into),
        ),
        (
            "Copilot",
            crate::copilot_setup::user_config_registers_amplihack(&copilot),
        ),
    ];
    apply_installed(&tools, |tool| {
        ensure!(
            staged.is_dir(),
            "run `amplihack install` to restore framework assets"
        );
        match tool {
            "Claude Code" => {
                let report = crate::claude_plugin::ensure_claude_plugin_installed()?;
                ensure!(
                    report.wrapper_error.is_none(),
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

fn apply_codex(staged: &Path) -> Result<()> {
    ensure!(
        staged.is_dir(),
        "run `amplihack install` to restore framework assets"
    );
    let source = clone::find_bundled_framework_root()
        .context("framework source is unavailable; run `amplihack install` and retry")?;
    let hooks = binary::stable_hooks_binary()?;
    // As the installer does, Codex is optional here: when it is not
    // available it is skipped, and the other tools decide the exit status.
    if !codex_plugin::install(&source.root, &hooks)? {
        eprintln!("amplihack: warning: Codex is not available; its list was not updated.");
    }
    Ok(())
}

fn apply_installed(
    tools: &[(&str, Result<bool>)],
    mut publish: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    for &(tool, ref installed) in tools {
        match installed {
            Ok(true) => match publish(tool) {
                Ok(()) => completed.push(tool),
                Err(error) => failed.push(format!("{tool}: {error:#}")),
            },
            Ok(false) => {}
            Err(error) => failed.push(format!("{tool}: {error:#}")),
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
    fn only_installed_tools_are_published() {
        let mut called = Vec::new();
        apply_installed(
            &[
                ("Claude Code", Ok(true)),
                ("Codex", Ok(false)),
                ("Copilot", Ok(true)),
            ],
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
        let tools = [
            ("Claude Code", Ok(true)),
            ("Codex", Ok(true)),
            ("Copilot", Ok(true)),
        ];
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

    #[cfg(unix)]
    #[test]
    fn unreadable_installation_path_is_a_failure_of_its_own_entry() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let locked = home.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let probe = locked.join("ownership.json").try_exists();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if probe.is_ok() {
            // Running with privileges that read any folder; nothing to test.
            return;
        }
        let tools = [
            ("Claude Code", Ok(true)),
            ("Codex", probe.map_err(Into::into)),
            ("Copilot", Ok(true)),
        ];
        let mut published = Vec::new();
        let error = apply_installed(&tools, |tool| {
            published.push(tool.to_owned());
            Ok(())
        })
        .unwrap_err();
        assert_eq!(published, ["Claude Code", "Copilot"]);
        assert!(error.to_string().contains("Codex"));
    }

    #[test]
    fn malformed_copilot_configuration_does_not_prevent_other_publications() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.json");
        std::fs::write(&config, "not JSON").unwrap();
        let tools = [
            ("Claude Code", Ok(true)),
            (
                "Copilot",
                crate::copilot_setup::user_config_registers_amplihack(home.path()),
            ),
            ("Codex", Ok(true)),
        ];
        let mut published = Vec::new();
        let error = apply_installed(&tools, |tool| {
            published.push(tool.to_owned());
            Ok(())
        })
        .unwrap_err();
        assert_eq!(published, ["Claude Code", "Codex"]);
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("publication failed for Copilot:"));
        assert!(diagnostic.contains("Completed applications: Claude Code, Codex"));
        assert_eq!(std::fs::read(config).unwrap(), b"not JSON");
    }
}
