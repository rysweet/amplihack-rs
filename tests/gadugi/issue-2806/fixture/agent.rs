//! Deterministic actions and observation-backed model receipts.
use super::environment::{IsolatedCase, confined_path, observe_git, private_dir};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contract {
    pub name: String,
    pub fields: Vec<String>,
}
impl Contract {
    pub fn json_object(name: &str, fields: &[&str]) -> Self {
        Self {
            name: name.into(),
            fields: fields.iter().map(|x| (*x).into()).collect(),
        }
    }
    pub fn validate_response(&self, response: &str) -> Result<Value> {
        let value: Value = serde_json::from_str(response)?;
        ensure!(value.is_object(), "{} requires an object", self.name);
        for field in &self.fields {
            ensure!(
                value.get(field).is_some_and(Value::is_string),
                "{} requires string field {field}",
                self.name
            );
        }
        Ok(value)
    }
}

pub fn select_contract<'a>(task: &str, contracts: &'a [Contract]) -> Result<&'a Contract> {
    // A quoted JSON key followed by a colon is the contract's declaration.
    // Word occurrences in surrounding prose are insufficient.
    let mut matches = contracts.iter().filter(|contract| {
        contract.fields.iter().all(|field| {
            let key = format!("\"{field}\"");
            task.match_indices(&key)
                .any(|(index, _)| task[index + key.len()..].trim_start().starts_with(':'))
        })
    });
    let result = matches
        .next()
        .ok_or_else(|| anyhow::anyhow!("no declared response contract"))?;
    ensure!(
        matches.next().is_none(),
        "ambiguous declared response contracts"
    );
    Ok(result)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub files: Vec<FileAction>,
    pub checks: Vec<Check>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileAction {
    pub path: PathBuf,
    pub content: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub executable: PathBuf,
    pub args: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    files: Vec<(PathBuf, Vec<u8>)>,
    checks: Vec<(PathBuf, Vec<String>)>,
    review: Option<(String, String)>,
}
impl Receipt {
    pub fn clean_review(before: String, reviewed: String) -> Self {
        Self {
            files: vec![],
            checks: vec![],
            review: Some((before, reviewed)),
        }
    }
}

pub fn execute_plan(case: &IsolatedCase, cwd: &Path, plan: &Plan) -> Result<Receipt> {
    ensure!(
        !plan.files.is_empty() && !plan.checks.is_empty(),
        "implementation requires files and checks"
    );
    ensure!(
        cwd.canonicalize()?.starts_with(&case.root),
        "action cwd escapes case"
    );
    let mut receipt = Receipt {
        files: vec![],
        checks: vec![],
        review: None,
    };
    for file in &plan.files {
        let path = confined_path(cwd, &file.path)?;
        if let Some(parent) = path.parent() {
            private_dir(parent)?;
        }
        fs::write(&path, &file.content)?;
        receipt.files.push((file.path.clone(), fs::read(&path)?));
    }
    for check in &plan.checks {
        let output = case
            .command(&check.executable)?
            .args(&check.args)
            .current_dir(cwd)
            .output()?;
        ensure!(
            output.status.success(),
            "verification {} failed: {}",
            check.executable.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        receipt
            .checks
            .push((check.executable.clone(), check.args.clone()));
    }
    validate_receipt(cwd, &receipt)?;
    Ok(receipt)
}

pub fn validate_receipt(cwd: &Path, receipt: &Receipt) -> Result<()> {
    if let Some((before, reviewed)) = &receipt.review {
        let current = observe_git(cwd, &["rev-parse", "HEAD"])?;
        ensure!(
            before == reviewed && reviewed == &current,
            "clean review changed or did not review the observed head"
        );
    } else {
        ensure!(
            !receipt.files.is_empty() && !receipt.checks.is_empty(),
            "receipt has no implementation or checks"
        );
        for (path, content) in &receipt.files {
            ensure!(
                fs::read(confined_path(cwd, path)?)? == *content,
                "receipt content changed: {}",
                path.display()
            );
        }
    }
    Ok(())
}

pub fn write_codex_final(case: &IsolatedCase, path: &Path, response: &str) -> Result<()> {
    let relative = path
        .strip_prefix(&case.root)
        .map_err(|_| anyhow::anyhow!("Codex output escapes private root"))?;
    let target = confined_path(&case.root, relative)?;
    if !target.parent().is_some_and(Path::is_dir) {
        bail!("Codex output parent does not exist");
    }
    fs::write(target, response)?;
    Ok(())
}

/// Read only a declared, rendered prompt field. Agent steps do not receive
/// the shell-step RECIPE_VAR aliases from the supported runner.
pub fn task_field<'a>(task: &'a str, prefix: &str, suffix: &str) -> Result<&'a str> {
    let (_, tail) = task
        .split_once(prefix)
        .with_context(|| format!("rendered task field absent: {prefix}"))?;
    let (value, _) = tail
        .split_once(suffix)
        .with_context(|| format!("rendered task field unterminated: {prefix}"))?;
    Ok(value)
}
pub fn task_json(task: &str, prefix: &str) -> Result<Value> {
    let (_, tail) = task
        .split_once(prefix)
        .with_context(|| format!("rendered task JSON absent: {prefix}"))?;
    serde_json::Deserializer::from_str(tail)
        .into_iter::<Value>()
        .next()
        .context("rendered task JSON empty")?
        .map_err(Into::into)
}
