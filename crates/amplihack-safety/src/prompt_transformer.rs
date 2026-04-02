//! Prompt transformation for directory change instructions.
//!
//! When files are staged to a temp directory, the auto-mode prompt must
//! be transformed to include a `cd` instruction so the agent works in
//! the correct directory.

use std::path::Path;

/// Transform auto mode prompts to include directory change.
pub struct PromptTransformer;

impl PromptTransformer {
    /// Transform prompt to include directory change instruction.
    ///
    /// If `used_temp` is false, returns the original prompt unchanged.
    /// Otherwise, prepends a "Change your working directory to …" instruction,
    /// preserving any leading slash commands.
    pub fn transform(
        original_prompt: &str,
        target_directory: impl AsRef<Path>,
        used_temp: bool,
    ) -> String {
        if !used_temp {
            return original_prompt.to_string();
        }

        let target = target_directory.as_ref().to_path_buf();
        let (slash_commands, remaining) = Self::extract_slash_commands(original_prompt);
        let dir_instruction = format!("Change your working directory to {}. ", target.display());

        if slash_commands.is_empty() {
            format!("{dir_instruction}{remaining}")
        } else {
            format!("{slash_commands} {dir_instruction}{remaining}")
        }
    }

    /// Extract leading slash commands from a prompt.
    ///
    /// Returns `(slash_commands_joined, remaining_text)`.
    fn extract_slash_commands(prompt: &str) -> (String, String) {
        let trimmed = prompt.trim();
        let mut commands = Vec::new();
        let mut rest = trimmed;

        loop {
            if !rest.starts_with('/') {
                break;
            }
            // Find the end of the slash command (word boundary)
            let cmd_end = rest[1..]
                .find(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(rest.len());

            let cmd = &rest[..cmd_end];
            // Validate: slash commands contain only word chars, colons, hyphens
            if cmd[1..]
                .chars()
                .all(|c| c.is_alphanumeric() || c == ':' || c == '-' || c == '_')
            {
                commands.push(cmd.to_string());
                rest = rest[cmd_end..].trim_start();
            } else {
                break;
            }
        }

        if commands.is_empty() {
            (String::new(), trimmed.to_string())
        } else {
            (commands.join(" "), rest.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_temp_returns_original() {
        let result = PromptTransformer::transform("fix the bug", "/tmp/work", false);
        assert_eq!(result, "fix the bug");
    }

    #[test]
    fn temp_prepends_cd_instruction() {
        let result = PromptTransformer::transform("fix the bug", "/tmp/work", true);
        assert!(result.starts_with("Change your working directory to /tmp/work."));
        assert!(result.ends_with("fix the bug"));
    }

    #[test]
    fn preserves_slash_commands() {
        let result = PromptTransformer::transform("/dev fix the bug", "/tmp/work", true);
        assert!(result.starts_with("/dev Change your working directory"));
        assert!(result.ends_with("fix the bug"));
    }

    #[test]
    fn multiple_slash_commands() {
        let result =
            PromptTransformer::transform("/dev /ultrathink fix the bug", "/tmp/work", true);
        assert!(result.starts_with("/dev /ultrathink Change your working directory"));
    }

    #[test]
    fn empty_prompt_with_temp() {
        let result = PromptTransformer::transform("", "/tmp/work", true);
        assert!(result.contains("Change your working directory"));
    }

    #[test]
    fn extract_no_slash_commands() {
        let (cmds, rest) = PromptTransformer::extract_slash_commands("fix the bug");
        assert!(cmds.is_empty());
        assert_eq!(rest, "fix the bug");
    }

    #[test]
    fn extract_one_slash_command() {
        let (cmds, rest) = PromptTransformer::extract_slash_commands("/dev fix the bug");
        assert_eq!(cmds, "/dev");
        assert_eq!(rest, "fix the bug");
    }

    #[test]
    fn extract_slash_with_colon() {
        let (cmds, rest) = PromptTransformer::extract_slash_commands("/amplihack:customize show");
        assert_eq!(cmds, "/amplihack:customize");
        assert_eq!(rest, "show");
    }

    #[test]
    fn slash_command_only() {
        let (cmds, rest) = PromptTransformer::extract_slash_commands("/dev");
        assert_eq!(cmds, "/dev");
        assert_eq!(rest, "");
    }
}
