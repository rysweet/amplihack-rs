//! Recognize command positions without rewriting caller arguments.
use std::io::{self, ErrorKind};

pub(super) struct Mode {
    pub exec: bool,
    pub resume: bool,
    pub last: bool,
    pub session: bool,
    pub delimiter: bool,
    pub stdin_marker: bool,
    pub trailing_images: bool,
    pub add_dir: bool,
    pub prompt_option: bool,
}

pub(super) fn classify(args: &[String]) -> io::Result<Mode> {
    let mut mode = Mode {
        exec: false,
        resume: false,
        last: false,
        session: false,
        delimiter: false,
        stdin_marker: false,
        trailing_images: false,
        add_dir: false,
        prompt_option: false,
    };
    let mut command_seen = false;
    let mut positional = false;
    let mut exec_command_pending = false;
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        mode.trailing_images = false;
        if positional || arg == "-" {
            positionals.push(arg.as_str());
            exec_command_pending = false;
        } else if arg == "--" {
            positional = true;
            mode.delimiter = true;
        } else if arg.starts_with('-') {
            let name = arg.split('=').next().unwrap();
            mode.last |= name == "--last";
            mode.add_dir |= name == "--add-dir";
            mode.prompt_option |= name == "--prompt";
            // Shared root/exec and interactive-resume images are variadic.
            // Exec resume has its own unary image option. Inline/attached
            // values close the occurrence, as clap's native parser does.
            let image = name == "--image" || arg.starts_with("-i");
            if image && !arg.contains('=') && (name == "--image" || arg == "-i") {
                index += 1;
                if index == args.len() || args[index].starts_with('-') {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "Codex image requires a value",
                    ));
                }
                if !(mode.exec && mode.resume) {
                    while index + 1 < args.len() && !args[index + 1].starts_with('-') {
                        index += 1;
                    }
                    mode.trailing_images = index + 1 == args.len();
                }
                index += 1;
                continue;
            }
            let long_value = [
                "--ask-for-approval",
                "--model",
                "--profile",
                "--config",
                "--sandbox",
                "--cd",
                "--image",
                "--add-dir",
                "--enable",
                "--disable",
                "--local-provider",
                "--output-last-message",
                "--output-schema",
                "--color",
                "--thread-source",
                "--remote",
                "--remote-auth-token-env",
            ]
            .contains(&name);
            let short_value = ["-a", "-m", "-p", "-c", "-s", "-C", "-i", "-o"]
                .iter()
                .any(|prefix| arg.starts_with(prefix));
            if (long_value && !arg.contains('=')) || (short_value && arg.len() == 2) {
                index += 1;
                if index == args.len() {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "Codex option requires a value",
                    ));
                }
            } else if !long_value
                && !short_value
                && ![
                    "--strict-config",
                    "--approve-for-me",
                    "--dangerously-bypass-hook-trust",
                    "--worktree",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--last",
                    "--all",
                    "--full-auto",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--yolo",
                    "--oss",
                    "--search",
                    "--no-alt-screen",
                    "--no-daemon",
                    "--include-non-interactive",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--json",
                    "--help",
                    "-h",
                    "--version",
                    "-V",
                    "--prompt",
                ]
                .contains(&name)
            {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "unsupported Codex option arity",
                ));
            }
        } else if !command_seen {
            command_seen = true;
            mode.exec = arg == "exec" || arg == "e";
            mode.resume = arg == "resume";
            exec_command_pending = mode.exec;
            if !mode.exec && !mode.resume {
                positionals.push(arg.as_str());
                positional = true;
            }
        } else if exec_command_pending && arg == "resume" {
            mode.resume = true;
            exec_command_pending = false;
        } else {
            positionals.push(arg.as_str());
            exec_command_pending = false;
        }
        index += 1;
    }
    // A supplied envelope owns the prompt slot. Preserve one explicit exec
    // stdin marker, but never append a second prompt to competing caller text.
    let prompt_positions = if mode.resume && !mode.last && !positionals.is_empty() {
        mode.session = true;
        &positionals[1..]
    } else {
        &positionals[..]
    };
    mode.stdin_marker = mode.exec && prompt_positions == ["-"];
    if !prompt_positions.is_empty() && !mode.stdin_marker {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Codex caller positional prompt conflicts with supplied prompt",
        ));
    }
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mode(args: &[&str]) -> Mode {
        classify(&args.iter().map(|a| (*a).to_string()).collect::<Vec<_>>()).unwrap()
    }
    #[test]
    fn values_and_delimiters_cannot_select_commands_or_flags() {
        for args in [vec!["--model", "exec"], vec!["-mresume"]] {
            let parsed = mode(&args);
            assert!(!parsed.exec && !parsed.resume);
        }
        let parsed = mode(&["--config", "--last", "resume", "session"]);
        assert!(parsed.resume && parsed.session && !parsed.last);
        let parsed = mode(&["exec", "resume", "--", "--last"]);
        assert!(parsed.resume && parsed.session && !parsed.last);
        assert!(classify(&["--".into(), "exec".into()]).is_err());
        let parsed = mode(&["--ask-for-approval", "exec", "-mresume", "e"]);
        assert!(parsed.exec && !parsed.resume);
    }
    #[test]
    fn resume_options_can_follow_session_and_missing_values_fail() {
        assert!(classify(&["resume".into(), "session".into(), "--last".into()]).is_err());
        assert!(classify(&["--model".into()]).is_err());
        assert!(classify(&["--unknown".into(), "exec".into()]).is_err());
    }
}
