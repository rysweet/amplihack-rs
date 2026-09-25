//! Delivery-aware command builders for launcher subprocesses.

use std::ffi::OsStr;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::process::Command;

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryHandle, DeliveryMode, PromptDelivery, deliver, from_env,
    sanitize_prompt_nul,
};
use amplihack_utils::root_sandbox;

use crate::flag_matrix::{
    AgentBinary, delivery_mode_name, prompt_delivery_caps_for, prompt_delivery_name,
};

#[derive(Debug)]
pub struct DeliveredCommand {
    pub command: Command,
    pub delivery_handle: DeliveryHandle,
    pub requested_mode: PromptDelivery,
    pub selected_mode: DeliveryMode,
    pub warnings: Vec<DeliveryWarning>,
    pub stdin_payload: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryWarning {
    UnsupportedMode {
        requested: PromptDelivery,
        effective: DeliveryMode,
        message: String,
    },
}

impl DeliveryWarning {
    pub fn message(&self) -> &str {
        match self {
            Self::UnsupportedMode { message, .. } => message,
        }
    }
}

pub fn validate_prompt_delivery_request(
    binary: AgentBinary,
    requested: PromptDelivery,
) -> io::Result<()> {
    if binary == AgentBinary::Amplifier
        && matches!(requested, PromptDelivery::Tempfile | PromptDelivery::Stdin)
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Amplifier prompt delivery mode '{}' is unsupported: upstream documents only `amplifier run [OPTIONS] [PROMPT]`; no stable prompt-file or stdin prompt contract is available",
                prompt_delivery_name(requested)
            ),
        ));
    }
    Ok(())
}

pub fn validate_prompt_delivery_env_for(binary: AgentBinary) -> io::Result<()> {
    validate_prompt_delivery_request(binary, from_env())
}

pub fn build_command_with_prompt_delivery<I, S>(
    program: &OsStr,
    args: I,
    prompt: &str,
    requested: PromptDelivery,
    caps: DeliveryCaps,
) -> io::Result<DeliveredCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    finish_prompt_delivery(command, prompt, requested, caps)
}

pub fn build_tool_command_with_prompt_delivery(
    binary: AgentBinary,
    project_path: &Path,
    extra_args: &[String],
    prompt: &str,
    requested: PromptDelivery,
) -> io::Result<DeliveredCommand> {
    build_tool_command_with_root_sandbox(
        binary,
        project_path,
        extra_args,
        prompt,
        requested,
        root_sandbox::detect,
    )
}

/// [`build_tool_command_with_prompt_delivery`] with the issue #1482
/// root-sandbox decision injected; `detect` is called only for Claude.
fn build_tool_command_with_root_sandbox(
    binary: AgentBinary,
    project_path: &Path,
    extra_args: &[String],
    prompt: &str,
    requested: PromptDelivery,
    detect: impl FnOnce() -> root_sandbox::SkipPermissionsEnv,
) -> io::Result<DeliveredCommand> {
    validate_prompt_delivery_request(binary, requested)?;

    let mut command = Command::new(binary.env_value());
    command.current_dir(project_path);
    command.env("AMPLIHACK_AGENT_BINARY", binary.env_value());

    add_prompt_prefix_args(&mut command, binary, extra_args);
    if binary == AgentBinary::Claude {
        // Issue #1482: `--dangerously-skip-permissions` as root needs
        // `IS_SANDBOX=1` on this child, or a clear error before the spawn.
        detect().apply(&mut command)?;
    }

    finish_prompt_delivery(command, prompt, requested, prompt_delivery_caps_for(binary))
}

pub fn build_tool_command_from_env(
    binary: AgentBinary,
    project_path: &Path,
    extra_args: &[String],
    prompt: &str,
) -> io::Result<DeliveredCommand> {
    build_tool_command_with_prompt_delivery(binary, project_path, extra_args, prompt, from_env())
}

fn finish_prompt_delivery(
    mut command: Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: DeliveryCaps,
) -> io::Result<DeliveredCommand> {
    let delivery_handle = deliver(&mut command, prompt, requested, &caps)?;
    let selected_mode = delivery_handle.mode();
    let warnings = warnings_for(requested, selected_mode, &caps);
    let stdin_payload = (selected_mode == DeliveryMode::Stdin)
        .then(|| sanitize_prompt_nul(prompt).as_bytes().to_vec());
    Ok(DeliveredCommand {
        command,
        delivery_handle,
        requested_mode: requested,
        selected_mode,
        warnings,
        stdin_payload,
    })
}

fn warnings_for(
    requested: PromptDelivery,
    effective: DeliveryMode,
    caps: &DeliveryCaps,
) -> Vec<DeliveryWarning> {
    let unsupported = match requested {
        PromptDelivery::Auto => false,
        PromptDelivery::Argv => !caps.supports_argv,
        PromptDelivery::Tempfile => !caps.supports_tempfile,
        PromptDelivery::Stdin => !caps.supports_stdin,
    };
    if !unsupported {
        return Vec::new();
    }
    vec![DeliveryWarning::UnsupportedMode {
        requested,
        effective,
        message: format!(
            "requested {} is unsupported; degrading to {}",
            prompt_delivery_name(requested),
            delivery_mode_name(effective)
        ),
    }]
}

fn add_prompt_prefix_args(command: &mut Command, binary: AgentBinary, extra_args: &[String]) {
    match binary {
        AgentBinary::Claude => {
            command.arg(root_sandbox::SKIP_PERMISSIONS_FLAG);
            command.args(extra_args);
            command.arg("-p");
        }
        AgentBinary::Copilot => {
            command.args(extra_args);
            command.arg("-p");
        }
        AgentBinary::Codex => {
            command.args(extra_args);
            command.arg("--prompt");
        }
        AgentBinary::Amplifier => {
            command.arg("run");
            command.args(extra_args);
        }
    }
}

#[cfg(test)]
mod root_sandbox_tests {
    //! Issue #1482: the Claude builder's IS_SANDBOX wiring, with the decision
    //! injected so every branch runs whatever uid the tests run as.

    use super::*;
    use root_sandbox::{IS_SANDBOX_ENV, SkipPermissionsEnv};

    fn build(binary: AgentBinary, decision: SkipPermissionsEnv) -> io::Result<DeliveredCommand> {
        build_tool_command_with_root_sandbox(
            binary,
            Path::new("."),
            &[],
            "hello",
            PromptDelivery::Argv,
            || decision,
        )
    }

    fn is_sandbox(delivered: &DeliveredCommand) -> Option<String> {
        delivered
            .command
            .get_envs()
            .find(|(key, _)| *key == IS_SANDBOX_ENV)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn claude_gets_is_sandbox_exactly_when_amplihack_enables_it() {
        for (decision, expected) in [
            (
                SkipPermissionsEnv::SetSandbox {
                    signal: "/.dockerenv",
                },
                Some("1"),
            ),
            (
                SkipPermissionsEnv::NormalizeExplicit {
                    value: "yes".to_string(),
                },
                Some("1"),
            ),
            (SkipPermissionsEnv::NotRoot, None),
            (SkipPermissionsEnv::AlreadySandboxed, None),
        ] {
            let delivered = build(AgentBinary::Claude, decision.clone()).unwrap();
            assert_eq!(is_sandbox(&delivered).as_deref(), expected, "{decision:?}");
        }
    }

    #[test]
    fn claude_fails_before_spawning_when_claude_code_would_refuse() {
        for decision in [
            SkipPermissionsEnv::RootOutsideSandbox,
            SkipPermissionsEnv::ExplicitlyNotSandboxed {
                value: "0".to_string(),
            },
        ] {
            let error = build(AgentBinary::Claude, decision).expect_err("refused up front");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
        }
    }

    #[test]
    fn other_binaries_are_never_probed() {
        for binary in [AgentBinary::Copilot, AgentBinary::Codex] {
            let delivered = build_tool_command_with_root_sandbox(
                binary,
                Path::new("."),
                &[],
                "hello",
                PromptDelivery::Argv,
                || panic!("only claude is probed"),
            )
            .unwrap();
            assert_eq!(is_sandbox(&delivered), None);
        }
    }
}
