//! Power-Steering re-enable prompt shown at launch time.

use amplihack_types::ProjectDirs;
use anyhow::Result;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::util::read_user_input_with_timeout;

use super::POWER_STEERING_PROMPT_TIMEOUT;

pub(crate) fn maybe_prompt_re_enable_power_steering(project_path: &Path) -> Result<()> {
    maybe_prompt_re_enable_power_steering_with(project_path, read_user_input_with_timeout)
}

pub(super) fn maybe_prompt_re_enable_power_steering_with<F>(
    project_path: &Path,
    prompt_reader: F,
) -> Result<()>
where
    F: FnOnce(&str, Duration) -> Result<Option<String>>,
{
    if std::env::var_os("AMPLIHACK_SKIP_POWER_STEERING").is_some() {
        return Ok(());
    }

    let dirs = ProjectDirs::from_root(project_path);
    let disabled_file = dirs.power_steering.join(".disabled");
    if !disabled_file.exists() {
        return Ok(());
    }

    println!("\nPower-Steering is currently disabled.");
    let prompt = "Would you like to re-enable it? [Y/n] (30s timeout, defaults to YES): ";
    let response = match prompt_reader(prompt, POWER_STEERING_PROMPT_TIMEOUT) {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                project = %project_path.display(),
                "power-steering re-enable prompt failed: {error}; defaulting to YES"
            );
            None
        }
    };

    let normalized = response
        .as_deref()
        .unwrap_or("y")
        .trim()
        .to_ascii_lowercase();
    if normalized == "n" || normalized == "no" {
        println!(
            "\nPower-Steering remains disabled. You can re-enable it by removing:\n{}\n",
            disabled_file.display()
        );
        return Ok(());
    }
    if !normalized.is_empty() && normalized != "y" && normalized != "yes" {
        tracing::warn!(
            project = %project_path.display(),
            input = %response.as_deref().unwrap_or_default(),
            "invalid power-steering re-enable response; defaulting to YES"
        );
    }

    remove_disabled_file_with_warning(&disabled_file);
    Ok(())
}

fn remove_disabled_file_with_warning(disabled_file: &Path) {
    match fs::remove_file(disabled_file) {
        Ok(()) => {
            println!("\nPower-Steering re-enabled.\n");
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                path = %disabled_file.display(),
                "failed to re-enable power-steering by removing .disabled: {error}"
            );
        }
    }
}
