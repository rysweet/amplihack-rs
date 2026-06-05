//! Delivery-aware command builders for launcher subprocesses.

use std::ffi::OsStr;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::process::Command;

use amplihack_utils::prompt_delivery::{
    DeliveryCaps, DeliveryHandle, DeliveryMode, PromptDelivery, deliver, from_env,
};

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
    validate_prompt_delivery_request(binary, requested)?;

    let mut command = Command::new(binary.env_value());
    command.current_dir(project_path);
    command.env("AMPLIHACK_AGENT_BINARY", binary.env_value());

    add_prompt_prefix_args(&mut command, binary, extra_args);

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
    let stdin_payload = (selected_mode == DeliveryMode::Stdin).then(|| prompt.as_bytes().to_vec());
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
            command.arg("--dangerously-skip-permissions");
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
