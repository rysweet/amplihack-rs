mod evidence;
mod preflight;
pub mod startup_guard;
mod types;

use crate::{VerifyLiveArgs, command_error};
use anyhow::Result;

pub fn run_verify_live(args: VerifyLiveArgs) -> Result<()> {
    match preflight::run(&args) {
        Ok(summary) => {
            println!("{}", serde_json::to_string(&summary)?);
            Ok(())
        }
        Err(failure) => {
            failure.emit();
            if let Some(summary) = failure.summary {
                println!("{}", serde_json::to_string(&summary)?);
            }
            Err(command_error::exit_error(failure.exit.into()))
        }
    }
}
