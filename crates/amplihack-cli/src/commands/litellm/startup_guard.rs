use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupIntent {
    Ordinary,
    LiteLlmHelp,
    VerifyLive,
    LiteLlmParseOnly,
}

pub fn classify_startup(argv: &[OsString]) -> StartupIntent {
    let Some(command) = argv.get(1) else {
        return StartupIntent::Ordinary;
    };
    if command == "--" {
        return StartupIntent::Ordinary;
    }
    let Some(command) = command.to_str() else {
        return StartupIntent::LiteLlmParseOnly;
    };
    if command != "litellm" {
        return StartupIntent::Ordinary;
    }

    let words = argv
        .iter()
        .skip(2)
        .take_while(|word| word.as_os_str() != "--")
        .collect::<Vec<_>>();
    if words
        .iter()
        .any(|word| word.as_os_str() == "-h" || word.as_os_str() == "--help")
    {
        return StartupIntent::LiteLlmHelp;
    }

    match words.first().and_then(|word| word.to_str()) {
        Some("verify-live") => StartupIntent::VerifyLive,
        _ => StartupIntent::LiteLlmParseOnly,
    }
}

pub fn ci_environment_present() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "TF_BUILD",
        "GITLAB_CI",
        "BUILDKITE",
        "CIRCLECI",
        "JENKINS_URL",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
}

pub fn print_ci_refusal() {
    eprintln!(
        "AH-LIVE-CI-001 stage=host-eligibility: host-only LiteLLM verification cannot run in CI.\n\
         Run it from a trusted host against the exact PR head.\n\
         No credentials, network endpoints, client binaries, or evidence paths were accessed."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn classifies_exact_live_command() {
        assert_eq!(
            classify_startup(&args(&["amplihack", "litellm", "verify-live", "--pr", "1"])),
            StartupIntent::VerifyLive
        );
        assert_eq!(
            classify_startup(&args(&["amplihack", "litellm", "verify-live-extra"])),
            StartupIntent::LiteLlmParseOnly
        );
    }

    #[test]
    fn help_and_separator_never_enter_live_mode() {
        assert_eq!(
            classify_startup(&args(&["amplihack", "litellm", "verify-live", "--help"])),
            StartupIntent::LiteLlmHelp
        );
        assert_eq!(
            classify_startup(&args(&["amplihack", "--", "litellm", "verify-live"])),
            StartupIntent::Ordinary
        );
        assert_eq!(
            classify_startup(&args(&[
                "amplihack",
                "litellm",
                "verify-live",
                "--",
                "--help"
            ])),
            StartupIntent::VerifyLive
        );
    }
}
