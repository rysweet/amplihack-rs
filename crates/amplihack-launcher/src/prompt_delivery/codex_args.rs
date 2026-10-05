//! Recognize command positions without rewriting caller arguments.
use std::io::{self, ErrorKind};

pub(super) struct Mode {
    pub exec: bool,
    pub resume: bool,
    pub last: bool,
    pub session: bool,
    pub add_dir: bool,
    pub prompt_option: bool,
}

pub(super) fn classify(args: &[String]) -> io::Result<Mode> {
    let mut mode = Mode {
        exec: false,
        resume: false,
        last: false,
        session: false,
        add_dir: false,
        prompt_option: false,
    };
    let mut command_seen = false;
    let mut positional = false;
    let mut exec_command_pending = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if positional {
            if mode.resume {
                mode.session = true;
            }
        } else if arg == "--" {
            positional = true;
        } else if arg.starts_with('-') {
            let name = arg.split('=').next().unwrap();
            mode.last |= name == "--last";
            mode.add_dir |= name == "--add-dir";
            mode.prompt_option |= name == "--prompt";
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
                positional = true;
            }
        } else if exec_command_pending && arg == "resume" {
            mode.resume = true;
            exec_command_pending = false;
        } else {
            if mode.resume {
                mode.session = true;
            }
            exec_command_pending = false;
        }
        index += 1;
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
        for args in [
            vec!["--model", "exec"],
            vec!["--", "exec"],
            vec!["-mresume"],
        ] {
            let parsed = mode(&args);
            assert!(!parsed.exec && !parsed.resume);
        }
        let parsed = mode(&["--config", "--last", "resume", "session"]);
        assert!(parsed.resume && parsed.session && !parsed.last);
        let parsed = mode(&["exec", "resume", "--", "--last"]);
        assert!(parsed.resume && parsed.session && !parsed.last);
        let parsed = mode(&["--ask-for-approval", "exec", "-mresume", "e"]);
        assert!(parsed.exec && !parsed.resume);
    }
    #[test]
    fn resume_options_can_follow_session_and_missing_values_fail() {
        let parsed = mode(&["resume", "session", "--last"]);
        assert!(parsed.resume && parsed.last && parsed.session);
        assert!(classify(&["--model".into()]).is_err());
        assert!(classify(&["--unknown".into(), "exec".into()]).is_err());
    }
}
