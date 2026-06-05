use anyhow::{Result, bail};

const XPIA_SHELL_HOOKS: &[&str] = &["session_start.sh", "post_tool_use.sh", "pre_tool_use.sh"];
const NOISY_SUBSTRINGS: &[&str] = &["profile_management", "Skipping symlink"];

pub fn assert_no_noisy_install_update_regressions(output: &str) -> Result<()> {
    for line in output.lines() {
        for hook in XPIA_SHELL_HOOKS {
            if line.contains('❌') && line.contains(hook) {
                bail!(
                    "noisy install/update regression output contains missing XPIA hook line for {hook}: {line}"
                );
            }
        }
    }

    for needle in NOISY_SUBSTRINGS {
        if let Some(line) = output.lines().find(|line| line.contains(needle)) {
            bail!(
                "noisy install/update regression output contains `{}`",
                line.trim()
            );
        }
    }

    Ok(())
}
