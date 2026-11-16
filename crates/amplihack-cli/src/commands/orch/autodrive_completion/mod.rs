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
    /// The PR number, so the dispatch reads this PR's head (#2333). Older
    /// state files have none; the dispatch then resolves it from the branch.
    #[arg(long, value_parser = parse_pr)]
    pr: Option<u64>,
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

/// The PR number, as `prepare` accepts it (#2333 S7): typed, positive, with
/// no leading zero and bounded, so it can only ever have come from the loop's
/// own `AD_PR`. Never read from the round record or from any file the round's
/// agent writes.
fn parse_pr(s: &str) -> Result<u64, String> {
    let digits = !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits || s.starts_with('0') {
        return Err("the PR number must be 1-9999999999".to_string());
    }
    s.parse()
        .map_err(|_| "the PR number must be 1-9999999999".to_string())
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
