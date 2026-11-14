//! Completion-first dispatch for Crusty. Agent judgement chooses the action;
//! the driver validates its evidence before granting completion authority.
mod decision;
mod dispatch;
mod evaluator;
mod evidence;
mod private_state;
pub(super) mod secure_fs;

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct Prepare {
    #[arg(long)]
    state_dir: PathBuf,
    #[arg(long)]
    round_label: String,
    #[arg(long)]
    record: PathBuf,
    #[arg(long)]
    repo: PathBuf,
    #[arg(long)]
    review_dir: PathBuf,
    #[arg(long)]
    launch_tools: PathBuf,
    #[arg(long)]
    evaluator_context_file: Option<PathBuf>,
    /// Declared evaluator data, copied into trusted state before the child.
    #[arg(long = "context-from-file")]
    context_files: Vec<String>,
}

#[derive(Args, Debug)]
pub struct Write {
    #[arg(long)]
    private_state: PathBuf,
    #[arg(long, value_enum)]
    action: decision::Action,
    #[arg(long)]
    reason: String,
}

#[derive(Args, Debug)]
pub struct Dispatch {
    #[arg(long)]
    private_state: PathBuf,
    #[arg(long, allow_hyphen_values = true)]
    child_exit_code: i32,
}

pub fn prepare(args: Prepare) -> Result<()> {
    private_state::prepare(args)
}
pub fn write(args: Write) -> Result<()> {
    decision::write(args)
}
pub fn seal(path: PathBuf) -> Result<()> {
    private_state::seal(&path)
}
pub fn dispatch(args: Dispatch) -> Result<()> {
    let code = dispatch::run(args)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn health_verdict(log: &str) -> &'static str {
    evaluator::verdict(log)
}
