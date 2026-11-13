//! Recovery records: one JSON object per line in `<dir>/<sanitised id>.jsonl`.
//!
//! Records are parsed with unknown fields refused and their fields checked
//! against each other, so a hand-edited or forged line is an error, never a
//! silently accepted decision.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::SCHEMA_VERSION;
use super::secure_fs;

/// Largest record file `report` and `run` will read.
pub const RECORD_FILE_LIMIT: usize = 4 * 1024 * 1024;
/// Longest text field (reason, repair, needs, owner).
pub const TEXT_LIMIT: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    RetryAfterRepair,
    AlternativeStep,
    ContinueRecorded,
    Handoff,
}

impl Choice {
    pub const ALL: [Choice; 4] = [
        Choice::RetryAfterRepair,
        Choice::AlternativeStep,
        Choice::ContinueRecorded,
        Choice::Handoff,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Choice::RetryAfterRepair => "retry_after_repair",
            Choice::AlternativeStep => "alternative_step",
            Choice::ContinueRecorded => "continue_recorded",
            Choice::Handoff => "handoff",
        }
    }

    pub fn parse(text: &str) -> Option<Choice> {
        Choice::ALL.into_iter().find(|c| c.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Agent,
    Validator,
    Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    pub signature: String,
    pub output_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema_version: u32,
    pub step_id: String,
    pub failure: Failure,
    pub choice: Choice,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternative: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    pub recurrence: bool,
    pub source: Source,
    pub recorded_at: String,
}

/// Checks a text field: 1..=1024 bytes and no control characters.
pub fn check_text(name: &str, value: &str) -> Result<()> {
    ensure!(
        (1..=TEXT_LIMIT).contains(&value.len()),
        "{name} must be 1-{TEXT_LIMIT} bytes"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "{name} must not contain control characters"
    );
    Ok(())
}

impl Record {
    /// The field invariants documented in the reference page.
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA_VERSION,
            "unsupported schema_version {}",
            self.schema_version
        );
        validate_record_step_id(&self.step_id)?;
        check_text("reason", &self.reason)?;
        let pairs = [
            (
                "repair",
                &self.repair,
                self.choice == Choice::RetryAfterRepair,
            ),
            (
                "alternative",
                &self.alternative,
                self.choice == Choice::AlternativeStep,
            ),
            ("owner", &self.owner, self.choice == Choice::Handoff),
            ("needs", &self.needs, self.choice == Choice::Handoff),
        ];
        for (name, value, wanted) in pairs {
            match (value, wanted) {
                (Some(v), true) => check_text(name, v)?,
                (None, false) => {}
                (Some(_), false) => bail!("{name} is not allowed with {}", self.choice.as_str()),
                (None, true) => bail!("{name} is required with {}", self.choice.as_str()),
            }
        }
        let f = &self.failure;
        let explained = [
            f.exit_code.is_some(),
            f.signal.is_some(),
            f.unavailable_reason.is_some(),
        ];
        ensure!(
            explained.iter().filter(|b| **b).count() == 1,
            "exactly one of exit_code, signal and unavailable_reason must explain the failure"
        );
        if let Some(reason) = &f.unavailable_reason {
            check_text("unavailable_reason", reason)?;
        }
        ensure!(
            f.signature.starts_with("exit=") && f.signature.contains(";tail="),
            "malformed failure signature"
        );
        ensure!(
            !f.output_excerpt
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
            "output_excerpt must not contain control characters"
        );
        ensure!(self.recorded_at.ends_with('Z'), "recorded_at must be UTC");
        Ok(())
    }
}

fn is_id_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')
}

/// A caller's `--step-id`: `^[A-Za-z0-9._-]{1,128}$`, not `.` or `..`.
pub fn validate_caller_step_id(id: &str) -> Result<()> {
    ensure!(
        (1..=128).contains(&id.len()) && id.bytes().all(is_id_char),
        "step id must match ^[A-Za-z0-9._-]{{1,128}}$: {id:?}"
    );
    ensure!(id != "." && id != "..", "step id must not be . or ..");
    Ok(())
}

/// An alternative's name: `^[A-Za-z0-9_-]{1,64}$`.
pub fn validate_alternative_name(name: &str) -> Result<()> {
    ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "alternative name must match ^[A-Za-z0-9_-]{{1,64}}$: {name:?}"
    );
    Ok(())
}

/// A recorded id: a caller id, or `<caller id>:alt:<name>` built by the driver.
pub fn validate_record_step_id(id: &str) -> Result<()> {
    match id.split_once(":alt:") {
        Some((base, name)) => {
            validate_caller_step_id(base)?;
            validate_alternative_name(name)
        }
        None => validate_caller_step_id(id),
    }
}

/// `:alt:` becomes `__alt__`; the name is never shortened.
pub fn sanitise_step_id(id: &str) -> String {
    id.replace(":alt:", "__alt__")
}

pub fn record_path(dir: &Path, step_id: &str) -> PathBuf {
    dir.join(format!("{}.jsonl", sanitise_step_id(step_id)))
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parses every line of a record file; any invalid line is an error.
pub fn parse_lines(path: &Path, bytes: &[u8]) -> Result<Vec<Record>> {
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", path.display()))?;
    let mut records = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let record: Record = serde_json::from_str(line)
            .with_context(|| format!("{} line {}: invalid record", path.display(), n + 1))?;
        record
            .check()
            .with_context(|| format!("{} line {}: invalid record", path.display(), n + 1))?;
        records.push(record);
    }
    Ok(records)
}

/// Records of one step, oldest first. A missing file has none.
pub fn read_step(dir: &Path, step_id: &str) -> Result<Vec<Record>> {
    let path = record_path(dir, step_id);
    match secure_fs::read_optional(&path, RECORD_FILE_LIMIT)? {
        Some(bytes) => parse_lines(&path, &bytes),
        None => Ok(Vec::new()),
    }
}

/// Every record file in the directory, by file name, each with its records.
pub fn read_all(dir: &Path) -> Result<Vec<(PathBuf, Vec<Record>)>> {
    secure_fs::check_dir(dir)?;
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot list {}", dir.display()))? {
        let name = entry?.file_name();
        let name = name
            .to_str()
            .with_context(|| format!("non UTF-8 file name in {}", dir.display()))?;
        if name.ends_with(".jsonl") {
            names.push(name.to_string());
        }
    }
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let bytes = secure_fs::read(&path, RECORD_FILE_LIMIT)
            .with_context(|| format!("unsafe or unreadable record file {}", path.display()))?;
        let records = parse_lines(&path, &bytes)?;
        out.push((path, records));
    }
    Ok(out)
}

/// Appends one checked record as a single line.
pub fn append(dir: &Path, record: &Record) -> Result<PathBuf> {
    record
        .check()
        .context("refusing to append an invalid record")?;
    let path = record_path(dir, &record.step_id);
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    secure_fs::append(&path, line.as_bytes())
        .with_context(|| format!("could not append a record to {}", path.display()))?;
    Ok(path)
}
