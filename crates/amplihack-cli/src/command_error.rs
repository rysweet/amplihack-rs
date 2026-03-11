use anyhow::Error;
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub struct CliExitError {
    code: i32,
}

impl CliExitError {
    pub fn new(code: i32) -> Self {
        Self { code }
    }

    pub fn code(&self) -> i32 {
        self.code
    }
}

impl fmt::Display for CliExitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cli exited with status {}", self.code)
    }
}

impl StdError for CliExitError {}

pub fn exit_error(code: i32) -> Error {
    CliExitError::new(code).into()
}

pub fn exit_code(error: &Error) -> Option<i32> {
    error.downcast_ref::<CliExitError>().map(CliExitError::code)
}
