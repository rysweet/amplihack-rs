//! #2342: native CLI and production QA consumer contracts.
//! Prompt checks verify the agents' contract, not a live agent wait.
#![cfg(unix)]

use super::{field, recipe_yaml, step, steps};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod workflow;

fn diagnostic(out: &Output) -> String {
    format!(
        "exit={}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
