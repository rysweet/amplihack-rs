use super::{MODEL_ENV, ProxyError, nonempty_env, proxy_requested};

/// Child launcher integration target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliTarget {
    Claude,
    CopilotCli,
    RustyClawd,
}

/// Reject launch modes that can bypass a locally configured proxy.
pub fn validate_launch_args(target: CliTarget, args: &[String]) -> Result<(), ProxyError> {
    let option_args = launch_option_arguments(target, args);
    let remote_requested = option_args.iter().copied().any(|arg| match target {
        CliTarget::CopilotCli => {
            arg == "--cloud"
                || arg.starts_with("--cloud=")
                || arg == "--remote"
                || arg.starts_with("--remote=")
                || arg == "--remote-export"
                || arg.starts_with("--remote-export=")
                || arg == "--share-gist"
                || arg.starts_with("--share-gist=")
                || arg == "--share"
                || arg.starts_with("--share=")
                || arg == "--connect"
                || arg.starts_with("--connect=")
                || arg == "--continue"
                || arg.starts_with("--continue=")
                || arg == "-c"
                || arg == "--resume"
                || arg.starts_with("--resume=")
                || arg == "-r"
                || arg.starts_with("-r=")
                || (arg.starts_with("-r") && !arg.starts_with("--"))
                || arg == "--session-id"
                || arg.starts_with("--session-id=")
        }
        CliTarget::Claude | CliTarget::RustyClawd => {
            arg == "--cloud"
                || arg.starts_with("--cloud=")
                || arg == "--teleport"
                || arg.starts_with("--teleport=")
                || arg == "--remote-control"
                || arg.starts_with("--remote-control=")
                || arg == "--environment"
                || arg.starts_with("--environment=")
                || arg == "--settings"
                || arg.starts_with("--settings=")
                || arg == "--setting-sources"
                || arg.starts_with("--setting-sources=")
                || arg == "--from-pr"
                || arg.starts_with("--from-pr=")
                || arg == "--continue"
                || arg.starts_with("--continue=")
                || arg == "--resume"
                || arg.starts_with("--resume=")
                || arg == "--session-id"
                || arg.starts_with("--session-id=")
                || arg == "-c"
                || arg == "-r"
                || arg.starts_with("-r=")
                || (arg.starts_with("-r") && !arg.starts_with("--"))
                || arg == "ultrareview"
        }
    });
    if proxy_requested() && remote_requested {
        return Err(ProxyError::InvalidConfig(
            "remote sessions or session export cannot use the configured LiteLLM gateway; remove the remote/export option or unset AMPLIHACK_LITELLM_ENDPOINT"
                .to_string(),
        ));
    }
    if proxy_requested() {
        let custom_agent_requested = option_args.iter().any(|arg| match target {
            CliTarget::Claude | CliTarget::RustyClawd => {
                matches!(*arg, "--agent" | "--agents")
                    || arg.starts_with("--agent=")
                    || arg.starts_with("--agents=")
                    || *arg == "--plugin-dir"
                    || arg.starts_with("--plugin-dir=")
                    || *arg == "--plugin-url"
                    || arg.starts_with("--plugin-url=")
                    || *arg == "--safe-mode"
                    || arg.starts_with("--safe-mode=")
            }
            CliTarget::CopilotCli => {
                *arg == "--agent"
                    || arg.starts_with("--agent=")
                    || *arg == "-C"
                    || (arg.starts_with("-C") && !arg.starts_with("--"))
                    || *arg == "--add-dir"
                    || arg.starts_with("--add-dir=")
                    || *arg == "--plugin-dir"
                    || arg.starts_with("--plugin-dir=")
            }
        });
        if custom_agent_requested {
            return Err(ProxyError::InvalidConfig(
                "custom agents, plugins, and safe-mode overrides cannot be used while LiteLLM routing is enabled because they can override the configured model"
                    .to_string(),
            ));
        }
        if target == CliTarget::RustyClawd
            && requested_providers(&option_args)?
                .iter()
                .any(|provider| *provider != "anthropic")
        {
            return Err(ProxyError::InvalidConfig(
                "RustyClawd --provider must be anthropic while LiteLLM routing is enabled; amplihack selects its explicit Anthropic-compatible gateway transport"
                    .to_string(),
            ));
        }
        if target == CliTarget::CopilotCli
            && option_args
                .iter()
                .any(|arg| *arg == "--secret-env-vars" || arg.starts_with("--secret-env-vars="))
        {
            return Err(ProxyError::InvalidConfig(
                "custom --secret-env-vars cannot be used while LiteLLM routing is enabled"
                    .to_string(),
            ));
        }
        let configured_model = nonempty_env(MODEL_ENV);
        for requested_model in requested_models(&option_args)? {
            if configured_model.as_deref() != Some(requested_model) {
                return Err(ProxyError::InvalidConfig(format!(
                    "requested and fallback models must match {MODEL_ENV} while LiteLLM routing is enabled"
                )));
            }
        }
    }
    Ok(())
}

fn launch_option_arguments(target: CliTarget, args: &[String]) -> Vec<&str> {
    let mut options = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument == "--" {
            break;
        }
        if target == CliTarget::CopilotCli
            && matches!(argument, "-p" | "--prompt" | "-i" | "--interactive")
        {
            index = index.saturating_add(2);
            continue;
        }
        if target == CliTarget::CopilotCli
            && (argument.starts_with("-p=")
                || argument.starts_with("--prompt=")
                || argument.starts_with("-i=")
                || argument.starts_with("--interactive="))
        {
            index += 1;
            continue;
        }
        options.push(argument);
        index += 1;
    }
    options
}

fn requested_models<'a>(args: &[&'a str]) -> Result<Vec<&'a str>, ProxyError> {
    let mut models = Vec::new();
    let mut primary_count = 0;
    let mut fallback_count = 0;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index];
        let (kind, value) = if argument == "--model" || argument == "--fallback-model" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| ProxyError::InvalidConfig(format!("{argument} requires a value")))?;
            (argument, *value)
        } else if let Some(value) = argument.strip_prefix("--model=") {
            ("--model", value)
        } else if let Some(value) = argument.strip_prefix("--fallback-model=") {
            ("--fallback-model", value)
        } else {
            index += 1;
            continue;
        };
        if kind == "--model" {
            primary_count += 1;
        } else {
            fallback_count += 1;
        }

        models.push(value);
        index += 1;
    }
    if primary_count > 1 || fallback_count > 1 {
        return Err(ProxyError::InvalidConfig(
            "duplicate model options are not allowed with LiteLLM routing".to_string(),
        ));
    }
    Ok(models)
}

fn requested_providers<'a>(args: &[&'a str]) -> Result<Vec<&'a str>, ProxyError> {
    let mut providers = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index];
        if argument == "--provider" {
            index += 1;
            providers.push(args.get(index).copied().ok_or_else(|| {
                ProxyError::InvalidConfig("--provider requires a value".to_string())
            })?);
        } else if let Some(provider) = argument.strip_prefix("--provider=") {
            providers.push(provider);
        }
        index += 1;
    }
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::litellm_proxy::ENDPOINT_ENV;

    #[test]
    fn rejects_all_launcher_bypass_controls() {
        let _guard = crate::test_serial::acquire();
        let previous = std::env::var_os(ENDPOINT_ENV);
        unsafe { std::env::set_var(ENDPOINT_ENV, "https://gateway.example.test") };

        for (target, controls) in [
            (
                CliTarget::CopilotCli,
                &[
                    "--cloud",
                    "--remote",
                    "--remote-export",
                    "--share-gist",
                    "--share",
                    "--connect",
                    "--agent",
                    "-C",
                    "-C=/tmp/project",
                    "-C/tmp/project",
                    "--add-dir",
                    "--secret-env-vars",
                    "--continue",
                    "-c",
                    "--resume",
                    "-rsession",
                    "--session-id",
                ][..],
            ),
            (
                CliTarget::Claude,
                &[
                    "--cloud",
                    "--teleport",
                    "--remote-control",
                    "--environment",
                    "--settings",
                    "--setting-sources",
                    "--agent",
                    "--agents",
                    "--plugin-dir",
                    "--safe-mode",
                    "--from-pr",
                    "--continue",
                    "-c",
                    "--resume",
                    "-rsession",
                    "--session-id",
                ],
            ),
            (
                CliTarget::RustyClawd,
                &[
                    "--settings",
                    "--setting-sources",
                    "--agent",
                    "--agents",
                    "--continue",
                    "-c",
                    "--resume",
                    "-rsession",
                    "--session-id",
                ],
            ),
        ] {
            for control in controls {
                assert!(
                    validate_launch_args(target, &[control.to_string()]).is_err(),
                    "{target:?} accepted {control}"
                );
            }
        }

        assert!(
            validate_launch_args(CliTarget::Claude, &["--safe-mode=false".to_string()]).is_err(),
            "Claude safe mode must remain launcher-owned during routed launches"
        );

        match previous {
            Some(value) => unsafe { std::env::set_var(ENDPOINT_ENV, value) },
            None => unsafe { std::env::remove_var(ENDPOINT_ENV) },
        }
    }

    #[test]
    fn rejects_every_supported_plugin_loading_option() {
        let _guard = crate::test_serial::acquire();
        let previous = std::env::var_os(ENDPOINT_ENV);
        unsafe { std::env::set_var(ENDPOINT_ENV, "https://gateway.example.test") };

        for (target, options) in [
            (
                CliTarget::Claude,
                &[
                    "--plugin-dir",
                    "--plugin-dir=/tmp/plugin",
                    "--plugin-url",
                    "--plugin-url=https://plugins.example.test/plugin",
                ][..],
            ),
            (
                CliTarget::RustyClawd,
                &[
                    "--plugin-dir",
                    "--plugin-dir=/tmp/plugin",
                    "--plugin-url",
                    "--plugin-url=https://plugins.example.test/plugin",
                ],
            ),
            (
                CliTarget::CopilotCli,
                &[
                    "--add-dir",
                    "--add-dir=/tmp/project",
                    "--plugin-dir",
                    "--plugin-dir=/tmp/plugin",
                ],
            ),
        ] {
            for option in options {
                assert!(
                    validate_launch_args(target, &[option.to_string()]).is_err(),
                    "{target:?} accepted plugin loading option {option}"
                );
            }
        }

        match previous {
            Some(value) => unsafe { std::env::set_var(ENDPOINT_ENV, value) },
            None => unsafe { std::env::remove_var(ENDPOINT_ENV) },
        }
    }

    #[test]
    fn rustyclawd_route_requires_the_native_anthropic_gateway_backend() {
        let _guard = crate::test_serial::acquire();
        let previous = std::env::var_os(ENDPOINT_ENV);
        unsafe { std::env::set_var(ENDPOINT_ENV, "https://gateway.example.test") };

        assert!(
            validate_launch_args(
                CliTarget::RustyClawd,
                &["--provider".to_string(), "anthropic".to_string()]
            )
            .is_ok()
        );
        assert!(
            validate_launch_args(
                CliTarget::RustyClawd,
                &["--provider".to_string(), "copilot".to_string()]
            )
            .is_err(),
            "the Copilot backend can bypass RustyClawd's native Anthropic-compatible gateway route"
        );

        match previous {
            Some(value) => unsafe { std::env::set_var(ENDPOINT_ENV, value) },
            None => unsafe { std::env::remove_var(ENDPOINT_ENV) },
        }
    }
}
